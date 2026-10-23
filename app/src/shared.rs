//! Platform-agnostic helpers shared by `app_linux.rs` and `app_windows.rs`.
//!
//! The Linux and Windows backends are composed from parallel platform
//! modules (see `docs/cross-platform-development.md`), while this module
//! owns logic verified to have the same semantics on both platforms. It
//! also contains shared orchestration that reaches platform-specific leaf
//! functions through `platform.rs`.
//!
//! When extending this module, only add a function here if it does not
//! call into anything that has (or could reasonably grow) a
//! platform-specific implementation — otherwise leave it in the
//! platform-specific file so the two backends don't silently diverge
//! through a shared function that assumes one platform's semantics.
//!
//! Some functions below are genuinely shared *orchestration* that call
//! down into platform-specific leaf implementations through `platform.rs`.
//! The active implementation is selected at compile time, so no runtime
//! dispatch is needed.

pub(crate) mod diagnostics;
mod events;
mod gpu;
pub(crate) mod install_info;
pub(crate) mod lora_browser;
pub(crate) mod manager;
pub(crate) mod snapshots;
pub(crate) mod tray;
pub(crate) mod workflow_browser;
pub(crate) mod workflow_check;

pub use events::{
    emit_comfyui_runtime_event, emit_comfyui_runtime_log_event, emit_install_event,
    DownloadProgressEvent,
};
pub use gpu::{amd_gpu_details_cache, intel_gpu_details_cache, AmdGpuDetails, IntelGpuDetails};

use crate::contracts::PreflightItem;
use crate::platform::{
    comfy_extra_model_config, detect_amd_gpu_details, detect_intel_gpu_details,
    detect_nvidia_gpu_details, effective_download_root, git_latest_release_tag, normalize_path,
    resolve_root_path, run_command_capture, spawn_comfyui_start_monitor, start_comfyui_root_impl,
    stop_comfyui_root_impl, update_tray_comfy_status, write_extra_model_paths_yaml,
};

use crate::host::{AppHandle, State};
use arctic_downloader::app::AppContext;
use arctic_downloader::catalog::CatalogRefreshStatus;
use arctic_downloader::config::{AppSettings, DEFAULT_COMFYUI_PORT};
use arctic_downloader::download::{remaining_download_bytes, DownloadSignal, DownloadStatus};
use arctic_downloader::model::{ModelArtifact, ModelCatalog, ResolvedModel, WorkflowDefinition};
use arctic_downloader::ram::RamTier;
use arctic_downloader::vram::VramTier;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// Recovers from a poisoned `Mutex`/`RwLock` by taking the guard anyway,
/// instead of propagating an error or panicking. A panic while holding one
/// of `AppState`'s locks doesn't leave the (simple, owned) data behind it
/// torn -- Rust's poisoning is a conservative "might be inconsistent"
/// signal here, not proof of actual corruption -- so recovering keeps the
/// app functional after a single panic instead of every future access to
/// that lock failing, or silently no-op'ing, for the rest of the process's
/// life. Used uniformly in place of the previous mix of some call sites
/// hard-erroring on poison and others silently skipping their body.
pub fn recover_lock<T>(result: Result<T, std::sync::PoisonError<T>>) -> T {
    result.unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Runs `probe` (the platform-specific "how do I query this vendor's GPU"
/// query -- `nvidia-smi`, a PowerShell `Get-CimInstance` script, `lspci`,
/// etc.) on a background thread and returns the cached result immediately:
/// callers get `T::default()` until the first successful probe completes,
/// then the cached value on every call after. A failed/empty probe result
/// is not cached and does not stick permanently -- `probe_started` resets
/// so a later call retries instead of reporting "no GPU" forever (e.g. if
/// the vendor's tool wasn't on PATH yet at the first check, right after a
/// driver install or before a session PATH refresh).
///
/// `app_linux.rs` and `app_windows.rs` each had their own copy of this
/// caching/retry pattern, three times per platform (NVIDIA/AMD/Intel) --
/// identical except for the query function and (on Linux) an extra
/// "probe complete" flag that, unlike this version, gave up retrying
/// permanently after the very first attempt, success or failure.
pub fn detect_gpu_details_cached<T>(
    cache: &'static Mutex<Option<T>>,
    probe_started: &'static AtomicBool,
    has_result: impl Fn(&T) -> bool + Send + 'static,
    probe: impl FnOnce() -> T + Send + 'static,
) -> T
where
    T: Clone + Default + Send + 'static,
{
    if let Ok(guard) = cache.lock() {
        if let Some(details) = guard.clone() {
            return details;
        }
    }

    if !probe_started.swap(true, Ordering::SeqCst) {
        std::thread::spawn(move || {
            let details = probe();
            if let Ok(mut guard) = cache.lock() {
                if has_result(&details) {
                    *guard = Some(details);
                } else {
                    *guard = None;
                    probe_started.store(false, Ordering::SeqCst);
                }
            }
        });
    }

    T::default()
}

/// True if `path`'s final component is `ComfyUI` or `ComfyUI-<suffix>`
/// (case-insensitive), the naming convention used for install directories
/// (see README: "Install New" creates `ComfyUI`, `ComfyUI-01`, ...).
pub fn path_name_is_comfyui(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|name| {
            let lower = name.to_ascii_lowercase();
            lower == "comfyui" || lower.starts_with("comfyui-")
        })
        .unwrap_or(false)
}

/// True if `path` exists as a directory with no entries in it.
pub fn is_empty_dir(path: &Path) -> bool {
    std::fs::read_dir(path)
        .ok()
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(false)
}

/// Quotes `input` as a single-quoted YAML scalar, doubling any embedded
/// single quotes per the YAML spec. Used when writing `extra_model_paths.yaml`.
pub fn yaml_single_quote(input: &str) -> String {
    format!("'{}'", input.replace('\'', "''"))
}

/// Unquotes a single- or double-quoted YAML scalar, or returns the trimmed
/// value unchanged if it isn't quoted.
pub fn parse_yaml_scalar(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2 {
        let single = trimmed.starts_with('\'') && trimmed.ends_with('\'');
        let double = trimmed.starts_with('"') && trimmed.ends_with('"');
        if single || double {
            let inner = &trimmed[1..trimmed.len() - 1];
            if single {
                return inner.replace("''", "'");
            }
            return inner.replace("\\\"", "\"");
        }
    }
    trimmed.to_string()
}

/// Parses common YAML boolean spellings (`true`/`yes`/`on`/`1` and their
/// negatives), case-insensitively. Returns `None` for anything else.
pub fn parse_yaml_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// Builds a stable identity key for a model artifact, used to match
/// frontend-selected artifacts back to catalog entries in batch download
/// requests.
pub fn model_artifact_selection_key(artifact: &ModelArtifact) -> String {
    [
        artifact.target_category.slug(),
        artifact.repo.trim(),
        artifact.path.trim(),
        artifact
            .direct_url
            .as_deref()
            .map(str::trim)
            .unwrap_or_default(),
        artifact.file_name(),
    ]
    .join("::")
}

/// Filters `artifacts` down to the ones matching `selected_keys` (as
/// produced by [`model_artifact_selection_key`]), or returns all of them
/// unchanged when `selected_keys` is `None` (no explicit selection made).
pub fn filter_selected_model_artifacts(
    artifacts: Vec<ModelArtifact>,
    selected_keys: Option<&Vec<String>>,
) -> Vec<ModelArtifact> {
    let Some(selected_keys) = selected_keys else {
        return artifacts;
    };
    let selected: HashSet<&str> = selected_keys.iter().map(String::as_str).collect();
    artifacts
        .into_iter()
        .filter(|artifact| selected.contains(model_artifact_selection_key(artifact).as_str()))
        .collect()
}

/// On-disk marker written into a ComfyUI install directory
/// (`.arctic_install_state.json`) so an interrupted install/update can be
/// detected and resumed on the next run.
#[derive(Debug, Serialize, Deserialize)]
pub struct InstallState {
    pub status: String, // in_progress | completed
    pub step: String,
}

/// Writes (or overwrites) the install-state marker for `install_root`.
/// Best-effort: a failure to serialize or write is silently ignored, since
/// this is a resume hint, not a correctness-critical record.
pub fn write_install_state(install_root: &Path, status: &str, step: &str) {
    let path = install_root.join(".arctic_install_state.json");
    let payload = InstallState {
        status: status.to_string(),
        step: step.to_string(),
    };
    if let Ok(data) = serde_json::to_vec_pretty(&payload) {
        let _ = std::fs::write(path, data);
    }
}

/// Scans immediate children of `base_root` for a `ComfyUI`/`ComfyUI-NN`
/// directory whose install-state marker says `in_progress`, returning its
/// path and parsed state if found.
pub fn find_in_progress_install(base_root: &Path) -> Option<(PathBuf, InstallState)> {
    if let Ok(entries) = std::fs::read_dir(base_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !(name == "ComfyUI"
                || (name.starts_with("ComfyUI-") && name.len() == "ComfyUI-00".len()))
            {
                continue;
            }
            let state_path = path.join(".arctic_install_state.json");
            if !state_path.exists() {
                continue;
            }
            let data = match std::fs::read(&state_path) {
                Ok(d) => d,
                Err(_) => continue,
            };
            let parsed: InstallState = match serde_json::from_slice(&data) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if parsed.status == "in_progress" {
                return Some((path, parsed));
            }
        }
    }
    None
}

/// Picks the destination folder for a new ComfyUI install under
/// `base_root`: resumes an in-progress install if one exists (unless
/// `force_fresh`), otherwise the first unused `ComfyUI-NN` (01-99), falling
/// back to a timestamp-suffixed name in the unlikely case all 99 are taken.
pub fn choose_install_folder(base_root: &Path, force_fresh: bool) -> PathBuf {
    if !force_fresh {
        if let Some((existing, _)) = find_in_progress_install(base_root) {
            return existing;
        }
    }

    for index in 1..=99u32 {
        let candidate = base_root.join(format!("ComfyUI-{index:02}"));
        if !candidate.exists() {
            return candidate;
        }
    }

    // Extremely unlikely fallback if 01..99 are occupied.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    base_root.join(format!("ComfyUI-{ts}"))
}

/// True if `path` looks like an existing ComfyUI checkout (has `main.py`
/// directly, or one directory level down under a `comfyui*`-named child —
/// covers picking either the checkout itself or its parent folder).
pub fn detect_existing_comfyui_root(path: &Path) -> Option<PathBuf> {
    if path.join("main.py").is_file() {
        return Some(path.to_path_buf());
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    let entries = std::fs::read_dir(path).ok()?;
    for entry in entries.flatten() {
        let child = entry.path();
        if !child.is_dir() {
            continue;
        }
        let name = child
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !name.starts_with("comfyui") {
            continue;
        }
        if child.join("main.py").is_file() {
            candidates.push(child);
        }
    }

    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by(|a, b| {
        let an = a.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let bn = b.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        an.cmp(bn)
    });
    candidates.into_iter().next()
}

/// True if `path`'s directory contains nothing but leftovers that are safe
/// to clone/extract over (shared runtime caches, our own state/log files) —
/// used to decide whether a pre-existing directory blocks a fresh install.
pub fn is_recoverable_preclone_dir(path: &Path) -> bool {
    let allowed = [
        ".venv",
        ".python",
        ".tools",
        ".arctic_install_state.json",
        "install.log",
        "install-summary.json",
    ];
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    entries.flatten().all(|entry| {
        entry
            .file_name()
            .to_str()
            .map(|name| allowed.iter().any(|item| item.eq_ignore_ascii_case(name)))
            .unwrap_or(false)
    })
}

/// Removes everything in `path` except the shared `.tools`/`.python`
/// runtime caches, used when clearing a directory for a fresh install
/// without re-downloading the shared runtime.
pub fn clear_directory_contents(path: &Path) -> Result<(), String> {
    let entries = std::fs::read_dir(path).map_err(|err| err.to_string())?;
    for entry in entries.flatten() {
        let p = entry.path();
        let keep = p
            .file_name()
            .and_then(|n| n.to_str())
            .map(|name| name.eq_ignore_ascii_case(".tools") || name.eq_ignore_ascii_case(".python"))
            .unwrap_or(false);
        if keep {
            continue;
        }
        if p.is_dir() {
            std::fs::remove_dir_all(&p).map_err(|err| err.to_string())?;
        } else {
            std::fs::remove_file(&p).map_err(|err| err.to_string())?;
        }
    }
    Ok(())
}

/// True if `root/custom_nodes/<name>` exists as a directory.
pub fn custom_node_exists(root: &Path, name: &str) -> bool {
    root.join("custom_nodes").join(name).is_dir()
}

/// Deletes `root/custom_nodes/<name>` for each name in `names`, ignoring
/// missing entries and individual removal failures (best-effort cleanup).
pub fn remove_custom_node_dirs(root: &Path, names: &[&str]) {
    let custom_nodes = root.join("custom_nodes");
    for name in names {
        let path = custom_nodes.join(name);
        if path.exists() {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

/// Repo URL, install folder name, and every folder name a built-in custom
/// node has ever been installed under by any version of this app (used for
/// detection/removal, so a toggle or addon-state check never misses an
/// install made under an older name).
///
/// `app_linux.rs` and `app_windows.rs` each define their own `CustomNodeSpec`
/// table (data genuinely differs by platform in at least one case -- see
/// that table's doc comment) and reference it by `flag_key` from the
/// install/toggle/addon-state flows, so a URL/folder-name update only has
/// to be made in one place per platform instead of three near-identical
/// hand-copied literals.
#[derive(Debug, Clone, Copy)]
pub struct CustomNodeSpec {
    pub flag_key: &'static str,
    pub display_name: &'static str,
    pub repo_url: &'static str,
    pub install_folder_name: &'static str,
    pub known_folder_names: &'static [&'static str],
}

/// True if any of `spec`'s known folder-name variants exists under
/// `root/custom_nodes/`.
pub fn custom_node_installed(root: &Path, spec: &CustomNodeSpec) -> bool {
    spec.known_folder_names
        .iter()
        .any(|name| custom_node_exists(root, name))
}

// --- Torch stack pins ---------------------------------------------------
//
// Once a profile's torch stack is installed and verified, its exact versions
// (e.g. `torch==2.14.1+cu130`) go into an override file inside the Python
// environment, and every `uv pip install` the app runs there passes it with
// `--overrides`. ComfyUI's and custom nodes' requirements then can't replace
// torch with another build (such as PyPI's), so the stack is installed once
// instead of being forcibly reinstalled after each requirements step.
// Installing the stack itself removes the file first.

/// Python that prints `name==version` for each installed torch stack package.
pub const TORCH_STACK_PINS_PY: &str = concat!(
    "import importlib.metadata as m\n",
    "for n in ('torch', 'torchvision', 'torchaudio', 'triton', 'triton-windows', 'triton-xpu',\n",
    "          'pytorch-triton-xpu', 'triton-rocm', 'pytorch-triton-rocm'):\n",
    "    try:\n",
    "        print(f'{n}=={m.version(n)}')\n",
    "    except m.PackageNotFoundError:\n",
    "        pass\n",
);

/// Where the override file for `py_exe`'s environment lives: the venv root
/// (`.venv/bin/python`, `.venv\\Scripts\\python.exe`) or, for an embedded
/// Python, the folder holding `python.exe`.
pub fn torch_overrides_path(py_exe: &Path) -> Option<PathBuf> {
    let bin_dir = py_exe.parent()?;
    let env_dir = match bin_dir.file_name().and_then(|name| name.to_str()) {
        Some("bin" | "Scripts") => bin_dir.parent()?,
        _ => bin_dir,
    };
    Some(env_dir.join("arctic-torch-overrides.txt"))
}

/// The override file for `python_target`, if the stack there is pinned.
pub fn existing_torch_overrides(python_target: &str) -> Option<PathBuf> {
    torch_overrides_path(Path::new(python_target)).filter(|path| path.is_file())
}

/// Writes the override file from [`TORCH_STACK_PINS_PY`]'s output.
pub fn write_torch_overrides(py_exe: &Path, pins_output: &str) -> Result<(), String> {
    let pins: Vec<&str> = pins_output
        .lines()
        .map(str::trim)
        .filter(|line| line.contains("=="))
        .collect();
    if !pins.iter().any(|pin| pin.starts_with("torch==")) {
        return Err("Cannot pin the torch stack: torch is not installed.".to_string());
    }
    let path = torch_overrides_path(py_exe)
        .ok_or_else(|| format!("No environment folder for {}", py_exe.display()))?;
    let contents = format!(
        "# Torch stack pinned by ComfyPilot; passed to uv as --overrides.\n{}\n",
        pins.join("\n")
    );
    std::fs::write(&path, contents)
        .map_err(|err| format!("Failed to write {}: {err}", path.display()))
}

/// Removes the override file so the torch stack itself can be (re)installed.
pub fn remove_torch_overrides(py_exe: &Path) {
    if let Some(path) = torch_overrides_path(py_exe) {
        let _ = std::fs::remove_file(path);
    }
}

/// Appends `--overrides <file>` to a `uv pip install` argument list when the
/// target environment's torch stack is pinned.
pub fn append_torch_overrides(uv_args: &mut Vec<String>, python_target: &str) {
    let is_install = uv_args.first().is_some_and(|arg| arg == "install");
    let has_overrides = uv_args.iter().any(|arg| arg == "--overrides");
    if let Some(path) =
        existing_torch_overrides(python_target).filter(|_| is_install && !has_overrides)
    {
        uv_args.push("--overrides".to_string());
        uv_args.push(path.to_string_lossy().to_string());
    }
}

/// Whether installed versions (`2.14.1+cu130`) match a profile's (`2.14.1`
/// or `2.14.1+cu130`), ignoring the local build tag.
pub fn same_base_version(installed: &str, wanted: &str) -> bool {
    let base = |version: &str| {
        version
            .trim()
            .split('+')
            .next()
            .unwrap_or_default()
            .to_string()
    };
    !installed.trim().is_empty() && base(installed) == base(wanted)
}

// --- ComfyUI-Manager ----------------------------------------------------
//
// Current ComfyUI ships ComfyUI-Manager as a pip package (`comfyui_manager`),
// pinned in its `manager_requirements.txt` and switched on with
// `--enable-manager`. With that flag ComfyUI skips any custom_nodes folder
// whose name contains "comfyui-manager", so the built-in manager and a legacy
// git clone never run together. Older ComfyUI checkouts have neither and
// still use the clone.

/// ComfyUI's pinned ComfyUI-Manager requirements file, if `comfy_dir` is a
/// checkout new enough to ship the built-in manager.
pub fn builtin_manager_requirements(comfy_dir: &Path) -> Option<PathBuf> {
    let path = comfy_dir.join("manager_requirements.txt");
    path.is_file().then_some(path)
}

/// The uv release the app downloads when uv isn't on PATH.
pub(crate) const UV_VERSION: &str = "0.13.5";

/// The uv release archive for this OS/architecture and its SHA-256 (from
/// the release's `.sha256` files, pinned here so a tampered download can't
/// bring its own checksum).
pub(crate) fn uv_release_asset() -> Option<(&'static str, &'static str)> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some((
            "uv-x86_64-unknown-linux-gnu.tar.gz",
            "f68d291c75783122dad58ab77def0e13f3233f358731db5d6d7e8715603b928f",
        )),
        ("linux", "aarch64") => Some((
            "uv-aarch64-unknown-linux-gnu.tar.gz",
            "4bb69cf95ed13f23713bb7bc09f241896ea0c75b7c557e615a20604b82f9c403",
        )),
        ("windows", "x86_64") => Some((
            "uv-x86_64-pc-windows-msvc.zip",
            "a38b8d1902b7cfbc76b423ba1bb456f623f42064cdc72628b6075b89afd1019a",
        )),
        _ => None,
    }
}

pub(crate) fn uv_release_url(asset: &str) -> String {
    format!("https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/{asset}")
}

/// Lowercase hex SHA-256 of a file.
pub(crate) fn sha256_file(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path)
        .map_err(|err| format!("Failed to open {}: {err}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|err| format!("Failed to read {}: {err}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// The site-packages folders of the Python environment `py_exe` belongs to:
/// a venv (`bin/python` → `lib/python3.X/site-packages`, `Scripts\python.exe`
/// → `Lib\site-packages`) or a Windows embedded Python (`Lib\site-packages`
/// beside it).
fn site_packages_dirs(py_exe: &Path) -> Vec<PathBuf> {
    let Some(bin_dir) = py_exe.parent() else {
        return Vec::new();
    };
    let mut dirs = vec![bin_dir.join("Lib").join("site-packages")];
    if let Some(env_dir) = bin_dir.parent() {
        dirs.push(env_dir.join("Lib").join("site-packages"));
        if let Ok(entries) = std::fs::read_dir(env_dir.join("lib")) {
            dirs.extend(
                entries
                    .flatten()
                    .filter(|entry| entry.file_name().to_string_lossy().starts_with("python"))
                    .map(|entry| entry.path().join("site-packages")),
            );
        }
    }
    dirs.retain(|dir| dir.is_dir());
    dirs
}

/// PEP 503 name normalization (`Flash_Attn` -> `flash-attn`), for
/// comparing distribution names.
fn normalize_dist_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_sep = false;
    for c in name.chars() {
        if matches!(c, '-' | '_' | '.') {
            if !last_sep {
                out.push('-');
            }
            last_sep = true;
        } else {
            out.push(c.to_ascii_lowercase());
            last_sep = false;
        }
    }
    out
}

/// Whether the distribution `name` is installed in `py_exe`'s environment,
/// read from its `<name>-<version>.dist-info` folder. Checking the disk
/// instead of importing keeps install-state checks instant (importing an
/// add-on loads torch first: seconds per package).
pub fn python_dist_installed(py_exe: &Path, name: &str) -> bool {
    let wanted = normalize_dist_name(name);
    site_packages_dirs(py_exe).iter().any(|dir| {
        std::fs::read_dir(dir).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                let file = entry.file_name().to_string_lossy().to_string();
                file.strip_suffix(".dist-info")
                    .and_then(|stem| stem.rsplit_once('-'))
                    .is_some_and(|(dist, _version)| normalize_dist_name(dist) == wanted)
            })
        })
    })
}

/// torch's build as recorded in its `torch/version.py`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TorchBuild {
    pub version: String,
    pub cuda: Option<String>,
    pub hip: Option<String>,
    pub xpu: Option<String>,
}

/// Reads torch's build from `torch/version.py` in `py_exe`'s environment
/// without starting Python.
pub fn torch_build(py_exe: &Path) -> Option<TorchBuild> {
    let text = site_packages_dirs(py_exe)
        .iter()
        .find_map(|dir| std::fs::read_to_string(dir.join("torch").join("version.py")).ok())?;
    parse_torch_version_py(&text)
}

fn parse_torch_version_py(text: &str) -> Option<TorchBuild> {
    let value = |key: &str| {
        text.lines().find_map(|line| {
            let (name, value) = line.split_once('=')?;
            let name = name.split(':').next()?.trim();
            (name == key).then(|| {
                value
                    .trim()
                    .trim_matches(|c| c == '\'' || c == '"')
                    .to_string()
            })
        })
    };
    let optional = |key: &str| value(key).filter(|v| !v.is_empty() && v != "None");
    Some(TorchBuild {
        version: value("__version__").filter(|v| !v.is_empty())?,
        cuda: optional("cuda"),
        hip: optional("hip"),
        xpu: optional("xpu"),
    })
}

/// Whether the built-in ComfyUI-Manager package is installed in `py_exe`'s
/// environment. Checked on disk, so it is cheap enough for every launch.
pub fn builtin_manager_installed(py_exe: &Path) -> bool {
    site_packages_dirs(py_exe)
        .iter()
        .any(|dir| dir.join("comfyui_manager").join("__init__.py").is_file())
}

/// Renames legacy ComfyUI-Manager clones in `custom_nodes_dir` to
/// `<name>.disabled` (which ComfyUI skips), keeping their files. Returns the
/// renamed folder names.
pub fn disable_legacy_manager_clones(custom_nodes_dir: &Path) -> Result<Vec<String>, String> {
    let Ok(entries) = std::fs::read_dir(custom_nodes_dir) else {
        return Ok(Vec::new());
    };
    let mut renamed = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let lower = name.to_ascii_lowercase();
        if !entry.path().is_dir()
            || !lower.contains("comfyui-manager")
            || lower.ends_with(".disabled")
        {
            continue;
        }
        let target = custom_nodes_dir.join(format!("{name}.disabled"));
        if target.exists() {
            std::fs::remove_dir_all(&target)
                .map_err(|err| format!("failed to replace {}: {err}", target.display()))?;
        }
        std::fs::rename(entry.path(), &target)
            .map_err(|err| format!("failed to disable legacy {name}: {err}"))?;
        renamed.push(name);
    }
    Ok(renamed)
}

/// Adds `--enable-manager` when the built-in ComfyUI-Manager is installed,
/// unless the launch arguments already choose a manager mode.
pub fn append_manager_launch_arg(args: &mut Vec<String>, builtin_manager_installed: bool) {
    let already_chosen = args
        .iter()
        .any(|arg| arg == "--enable-manager" || arg == "--enable-manager-legacy-ui");
    if builtin_manager_installed && !already_chosen {
        args.push("--enable-manager".to_string());
    }
}

/// Runs `install` (the actual clone/dependency-install for `spec`, which
/// stays platform-specific -- different retry/venv-resolution plumbing --
/// and is passed in rather than called directly from here) and records the
/// outcome into `summary` the same way for every built-in custom node:
/// mark the install-state step, push an `ok`/`failed` summary row, and emit
/// a warning event on failure.
pub fn install_custom_node_and_record(
    app: &AppHandle,
    install_root: &Path,
    spec: &CustomNodeSpec,
    summary: &mut Vec<InstallSummaryItem>,
    install: impl FnOnce() -> Result<(), String>,
) {
    write_install_state(install_root, "in_progress", spec.flag_key);
    match install() {
        Ok(()) => summary.push(InstallSummaryItem {
            name: spec.display_name.to_string(),
            status: "ok".to_string(),
            detail: "Installed successfully.".to_string(),
        }),
        Err(err) => {
            summary.push(InstallSummaryItem {
                name: spec.display_name.to_string(),
                status: "failed".to_string(),
                detail: err.clone(),
            });
            emit_install_event(app, "warn", &format!("{} failed: {err}", spec.display_name));
        }
    }
}

/// Reads `__version__` out of `root/comfyui_version.py`, if present.
pub fn read_comfyui_installed_version(root: &Path) -> Option<String> {
    let path = root.join("comfyui_version.py");
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("__version__") {
            continue;
        }
        let (_, rhs) = trimmed.split_once('=')?;
        let value = rhs.trim().trim_matches('"').trim_matches('\'').trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

/// True if `host:port` resolves and accepts a connection attempt at the DNS
/// level (used for connectivity preflight checks, e.g. before a guided
/// install step that needs network access).
pub fn has_dns(host: &str, port: u16) -> bool {
    (host, port)
        .to_socket_addrs()
        .map(|mut it| it.next().is_some())
        .unwrap_or(false)
}

/// A timeout budget generous enough for a slow `git clone`/`fetch` of a
/// ComfyUI-sized repo over a poor connection, but bounded so an
/// unreachable host or a stalled server doesn't hang the calling command
/// forever -- previously the only way out was killing the whole app.
pub const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(300);

/// Uses a bounded timeout for Git network operations and the ordinary
/// `Command::status` path for local tools.
pub(crate) fn status_with_optional_timeout(
    program: &str,
    mut command: std::process::Command,
) -> std::io::Result<std::process::ExitStatus> {
    if program == "git" {
        run_with_timeout(command, GIT_COMMAND_TIMEOUT)
    } else {
        command.status()
    }
}

/// `Command::output` equivalent of [`status_with_optional_timeout`].
pub(crate) fn output_with_optional_timeout(
    program: &str,
    mut command: std::process::Command,
) -> std::io::Result<std::process::Output> {
    if program == "git" {
        run_with_timeout_capturing_output(command, GIT_COMMAND_TIMEOUT)
    } else {
        command.output()
    }
}

/// Runs `cmd` (already fully configured -- program, args, cwd, env, and
/// any `Stdio` the caller wants for stdin) to completion like
/// `Command::status()`, but kills it and returns an error if it hasn't
/// exited within `timeout`. Safe to use with inherited stdout/stderr
/// (the common case for this codebase's `run_command`-style helpers)
/// since nothing here pipes or reads them.
pub fn run_with_timeout(
    mut cmd: std::process::Command,
    timeout: Duration,
) -> std::io::Result<std::process::ExitStatus> {
    let mut child = cmd.spawn()?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("command timed out after {timeout:?}"),
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Runs `cmd` to completion like `Command::output()` (stdout/stderr
/// captured, not inherited), but kills it and returns an error if it
/// hasn't exited within `timeout`. Drains stdout/stderr on background
/// threads while polling for exit, the same way `Command::output()`
/// itself does internally -- a naive "poll `try_wait`, read pipes only
/// after exit" implementation would let a command that produces more
/// output than one OS pipe buffer (~64KB) deadlock against its own
/// unread pipe and get killed by this timeout for a reason that has
/// nothing to do with actually being stuck.
pub fn run_with_timeout_capturing_output(
    mut cmd: std::process::Command,
    timeout: Duration,
) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn()?;
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();

    let stdout_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(pipe) = stdout_pipe.as_mut() {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(pipe) = stderr_pipe.as_mut() {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    // Dropping the child closes its ends of the pipes, so
                    // the reader threads see EOF and these joins return
                    // promptly rather than blocking further.
                    let stdout = stdout_thread.join().unwrap_or_default();
                    let stderr = stderr_thread.join().unwrap_or_default();
                    let _ = (stdout, stderr);
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!("command timed out after {timeout:?}"),
                    ));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    };

    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr = stderr_thread.join().unwrap_or_default();
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

/// Derives a human-facing instance name from an install path's final
/// component (falls back to `"ComfyUI"` if the path has none).
pub fn comfyui_instance_name_from_path(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or("ComfyUI")
        .to_string()
}

/// Parses a `major.minor.patch` version, tolerating a leading `v`/`V` and a
/// trailing pre-release/build suffix after a `-` (e.g. `v2.1.0-rc1`).
/// Rejects anything with more or fewer than three numeric components.
pub fn parse_semver_triplet(input: &str) -> Option<(u64, u64, u64)> {
    let trimmed = input.trim().trim_start_matches('v').trim_start_matches('V');
    let core = trimmed.split('-').next().unwrap_or(trimmed);
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Normalizes a release version string to plain `major.minor.patch`,
/// discarding any `v` prefix or pre-release/build suffix.
pub fn normalize_release_version(input: &str) -> Option<String> {
    let (major, minor, patch) = parse_semver_triplet(input)?;
    Some(format!("{major}.{minor}.{patch}"))
}

/// Splits a custom launch-args string into argv-style tokens, honoring
/// single/double quotes. Backslash is only treated as an escape character
/// *inside* a quoted string (so `\"`/`\\` can embed a literal quote or
/// backslash there); outside quotes it's preserved literally. Errors on an
/// unclosed quote.
///
/// Backslash is deliberately not an escape character outside quotes: these
/// args frequently contain unquoted Windows paths (e.g.
/// `--extra-model-paths-config C:\Users\name\file.yaml`, the natural way to
/// paste one), and treating `\` as an escape there silently ate every
/// backslash, turning that into `C:Usersnamefile.yaml` with no error raised.
pub fn parse_custom_launch_args(input: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut quote: Option<char> = None;

    while let Some(ch) = chars.next() {
        match quote {
            Some(active_quote) => match ch {
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                }
                _ if ch == active_quote => quote = None,
                _ => current.push(ch),
            },
            None => match ch {
                '"' | '\'' => quote = Some(ch),
                c if c.is_whitespace() => {
                    if !current.is_empty() {
                        args.push(std::mem::take(&mut current));
                    }
                }
                _ => current.push(ch),
            },
        }
    }

    if quote.is_some() {
        return Err("Custom launch args contain an unclosed quote.".to_string());
    }
    if !current.is_empty() {
        args.push(current);
    }

    Ok(args)
}

/// Finds the value of a `- key: value` line (as emitted by `uvx hf env`) in
/// `text`.
pub fn parse_hf_env_value(text: &str, key: &str) -> Option<String> {
    let prefix = format!("- {key}:");
    text.lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix(&prefix).map(str::trim))
        .map(str::to_string)
}

/// Parses a RAM tier identifier (see [`RamTier::from_identifier`]).
pub fn parse_ram_tier(value: &str) -> Option<RamTier> {
    RamTier::from_identifier(value)
}

/// Parses a VRAM tier identifier (see [`VramTier::from_identifier`]).
pub fn parse_vram_tier(value: &str) -> Option<VramTier> {
    VramTier::from_identifier(value)
}

/// Appends the launch flag matching `backend` (`"flash"`, `"sage"`/`"sage3"`)
/// to `args`, or does nothing for `None`/an unrecognized/empty value.
pub fn append_attention_launch_arg(args: &mut Vec<String>, backend: Option<&str>) {
    match backend
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("flash") => args.push("--use-flash-attention".to_string()),
        Some("sage") | Some("sage3") => args.push("--use-sage-attention".to_string()),
        _ => {}
    }
}

/// Renames the legacy `PYTORCH_CUDA_ALLOC_CONF` env var to
/// `PYTORCH_ALLOC_CONF` (newer PyTorch releases warn on the old name) before
/// a subprocess launch, without clobbering an explicitly-set new-style value.
pub fn apply_torch_allocator_env_compat(cmd: &mut std::process::Command) {
    if let Ok(value) = std::env::var("PYTORCH_CUDA_ALLOC_CONF") {
        if std::env::var_os("PYTORCH_ALLOC_CONF").is_none() {
            cmd.env("PYTORCH_ALLOC_CONF", value);
        }
        cmd.env_remove("PYTORCH_CUDA_ALLOC_CONF");
    }
}

/// True if the guided-setup UI-testing escape hatch for AMD is active
/// (`ARCTIC_FAKE_AMD=1`) — lets the AMD/ROCm UI flow be exercised on a
/// machine without a real AMD GPU.
pub fn fake_amd_enabled() -> bool {
    std::env::var("ARCTIC_FAKE_AMD")
        .map(|value| value == "1")
        .unwrap_or(false)
}

/// True if the guided-setup UI-testing escape hatch for Intel is active
/// (`ARCTIC_FAKE_INTEL=1`).
pub fn fake_intel_enabled() -> bool {
    std::env::var("ARCTIC_FAKE_INTEL")
        .map(|value| value == "1")
        .unwrap_or(false)
}

/// True if verbose diagnostic UI/logging is enabled (`ARCTIC_NERDSTATS=1`).
pub fn nerdstats_enabled() -> bool {
    std::env::var("ARCTIC_NERDSTATS")
        .map(|value| value == "1")
        .unwrap_or(false)
}

/// Appends a preflight result row to `items`.
pub fn push_preflight(
    items: &mut Vec<PreflightItem>,
    status: &str,
    title: &str,
    detail: impl Into<String>,
) {
    items.push(PreflightItem {
        status: status.to_string(),
        title: title.to_string(),
        detail: detail.into(),
    });
}

/// One row of an install/add-on-change summary (`install-summary.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallSummaryItem {
    pub name: String,
    pub status: String, // ok | failed | skipped
    pub detail: String,
}

/// Writes `install-summary.json` for `install_root`. Best-effort, like
/// [`write_install_state`].
pub fn write_install_summary(install_root: &Path, items: &[InstallSummaryItem]) {
    let path = install_root.join("install-summary.json");
    if let Ok(data) = serde_json::to_vec_pretty(items) {
        let _ = std::fs::write(path, data);
    }
}

// --- GPU detail caches -----------------------------------------------------
//
// The detection logic that *populates* these caches (`detect_amd_gpu_details`,
// `detect_intel_gpu_details`) differs by platform (different system queries
// entirely) and stays in `app_linux.rs`/`app_windows.rs`. Only the AMD/Intel
// memoization plumbing — the struct shapes and the lazily-initialized `Mutex`
// accessors — was actually identical, so only that part lives here. The
// adjacent probe-started/probe-complete `AtomicBool`s are still platform-local
// since they're read/written alongside the platform-specific detection calls.
//
// NOTE: `NvidiaGpuDetails`/`gpu_details_cache` stay OUT of this module. Their
// fields now match on both platforms (Windows gained `compute_capability` for
// CUDA 13 eligibility), but the probes behind them differ (Linux falls back to
// lspci and alone uses the capability for Hopper/SM90 detection). Always read
// the actual current field list by eye before trusting a struct to be
// identical, not just a script's verdict.

// --- Shared Tauri-managed app state -----------------------------------------
//
// `AppState` is a plain data holder with no platform-specific fields or
// methods (no `impl AppState` block exists on either side), so unlike the
// `NvidiaGpuDetails` case above, there's no hidden platform divergence to
// worry about here. Only functions that touch `AppState` and call nothing
// platform-divergent are included below — process start/stop control flow
// (`stop_comfyui_root`, `stop_comfyui_for_mutation`, and the
// `stop_comfyui_root_impl`/`update_tray_comfy_status`/`resolve_root_path`
// helpers they call) stays in `app_linux.rs`/`app_windows.rs`.

/// Tauri-managed application state: the shared [`AppContext`] plus the
/// mutable handles needed to cancel an in-flight download/install and track
/// the managed ComfyUI child process.
pub struct AppState {
    pub context: AppContext,
    pub active_cancel: Mutex<Option<CancellationToken>>,
    pub active_abort: Mutex<Option<tokio::task::AbortHandle>>,
    pub install_cancel: Mutex<Option<CancellationToken>>,
    pub comfyui_process: Mutex<Option<std::process::Child>>,
    /// The install `comfyui_process` runs (set when it starts).
    pub comfyui_process_root: Mutex<Option<PathBuf>>,
    pub quitting: Mutex<bool>,
}

/// Response payload for [`get_comfyui_runtime_status`].
#[derive(Debug, Serialize)]
pub struct ComfyRuntimeStatus {
    /// A ComfyUI is running (this app's, or one answering on its port).
    pub running: bool,
    /// The install it belongs to, when known (see [`comfyui_running_root`]).
    pub running_root: Option<String>,
    /// Where the configured ComfyUI is (or will be) served.
    pub url: String,
}

/// Returns the persisted app settings.
pub fn get_settings(state: State<'_, AppState>) -> AppSettings {
    state.context.config.settings()
}

/// How long a first launch (nothing cached yet) waits for the startup
/// catalog download before showing an empty catalog.
const FIRST_CATALOG_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// Returns a snapshot of the current model/LoRA catalog. On a first launch
/// the cache is empty, so this waits briefly for the startup refresh rather
/// than showing "Catalog unavailable" until the user presses Refresh.
pub async fn get_catalog(state: State<'_, AppState>) -> Result<ModelCatalog, String> {
    let catalog = &state.context.catalog;
    let snapshot = catalog.catalog_snapshot();
    if !snapshot.models.is_empty() {
        return Ok(snapshot);
    }
    catalog.wait_for_refresh(FIRST_CATALOG_WAIT).await;
    Ok(catalog.catalog_snapshot())
}

/// The latest catalog refresh outcome and a user-facing message for it.
#[derive(Debug, Serialize)]
pub struct CatalogStatusResponse {
    #[serde(flatten)]
    pub status: CatalogRefreshStatus,
    pub message: String,
}

/// Reports why the catalog couldn't be refreshed, if it couldn't.
pub fn get_catalog_status(state: State<'_, AppState>) -> CatalogStatusResponse {
    let status = state.context.catalog.refresh_status();
    CatalogStatusResponse {
        message: status.message(),
        status,
    }
}

/// Saves (or clears, if empty) the user's Civitai API token.
pub fn save_civitai_token(
    state: State<'_, AppState>,
    token: String,
) -> Result<AppSettings, String> {
    let trimmed = token.trim().to_string();
    state
        .context
        .config
        .update_settings(|settings| {
            settings.civitai_token = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            };
        })
        .map_err(|err| err.to_string())
}

/// Saves (or clears, when empty) the Hugging Face access token used for
/// gated and private model repos.
pub fn save_hf_token(state: State<'_, AppState>, token: String) -> Result<AppSettings, String> {
    let trimmed = token.trim().to_string();
    state
        .context
        .config
        .update_settings(|settings| {
            settings.hf_token = (!trimmed.is_empty()).then(|| trimmed.clone());
        })
        .map_err(|err| err.to_string())
}

/// Cancels an in-flight ComfyUI install, if one is running.
pub fn cancel_comfyui_install(state: State<'_, AppState>) -> Result<bool, String> {
    let mut active = recover_lock(state.install_cancel.lock());
    if let Some(token) = active.as_ref() {
        token.cancel();
        *active = None;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// True if the managed ComfyUI child process is still alive. Reaps it (sets
/// the slot back to `None`) if it has already exited or become unqueryable.
pub fn comfyui_process_running(state: &AppState) -> bool {
    let mut guard = recover_lock(state.comfyui_process.lock());
    let Some(child) = guard.as_mut() else {
        return false;
    };
    match child.try_wait() {
        Ok(Some(_)) => {
            *guard = None;
            false
        }
        Ok(None) => true,
        Err(_) => {
            *guard = None;
            false
        }
    }
}

/// What answers on ComfyUI's local port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortOccupant {
    /// Nothing is listening.
    Free,
    /// ComfyUI answered `/system_stats`.
    ComfyUi,
    /// Something accepted the connection but didn't answer in time, such as
    /// a ComfyUI busy with a workflow, or the TLS case where the app can't
    /// ask.
    Unresponsive,
    /// Another program answered.
    Other,
}

/// Where the configured ComfyUI listens on this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComfyEndpoint {
    pub port: u16,
    pub tls: bool,
}

impl ComfyEndpoint {
    /// The port from a `--port` in the custom launch args, else the
    /// `comfyui_port` setting; TLS when the custom args set `--tls-keyfile`.
    pub fn from_settings(settings: &AppSettings) -> Self {
        let custom =
            parse_custom_launch_args(&settings.comfyui_custom_launch_args).unwrap_or_default();
        let mut port = None;
        for (index, arg) in custom.iter().enumerate() {
            if let Some(value) = arg.strip_prefix("--port=") {
                port = value.parse().ok().or(port);
            } else if arg == "--port" {
                port = custom
                    .get(index + 1)
                    .and_then(|value| value.parse().ok())
                    .or(port);
            }
        }
        Self {
            port: port.unwrap_or(settings.comfyui_port),
            tls: custom.iter().any(|arg| arg.starts_with("--tls-keyfile")),
        }
    }

    pub fn url(&self) -> String {
        let scheme = if self.tls { "https" } else { "http" };
        format!("{scheme}://127.0.0.1:{}", self.port)
    }

    /// Asks the port for `/system_stats` (plain HTTP, so it works from any
    /// thread). With TLS only the connection is checked.
    pub fn probe(&self) -> PortOccupant {
        use std::io::{Read, Write};
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], self.port));
        let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(180)) else {
            return PortOccupant::Free;
        };
        if self.tls {
            return PortOccupant::Unresponsive;
        }
        let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
        let _ = stream.set_read_timeout(Some(Duration::from_millis(1500)));
        let request = format!(
            "GET /system_stats HTTP/1.0\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            self.port
        );
        if stream.write_all(request.as_bytes()).is_err() {
            return PortOccupant::Unresponsive;
        }
        let mut response = Vec::new();
        // A timeout keeps what was read so far.
        let _ = Read::by_ref(&mut stream)
            .take(256 * 1024)
            .read_to_end(&mut response);
        classify_system_stats_response(&response)
    }
}

/// Classifies the raw HTTP response to `GET /system_stats`.
fn classify_system_stats_response(response: &[u8]) -> PortOccupant {
    if response.is_empty() {
        return PortOccupant::Unresponsive;
    }
    let text = String::from_utf8_lossy(response);
    let ok = text.starts_with("HTTP/1.") && text.split_whitespace().nth(1) == Some("200");
    if ok && text.contains("\"system\"") && text.contains("\"devices\"") {
        PortOccupant::ComfyUi
    } else if !text.contains("\r\n\r\n") {
        // Headers not complete yet: slow, not foreign.
        PortOccupant::Unresponsive
    } else {
        PortOccupant::Other
    }
}

/// Fails when another program holds ComfyUI's port, so starting ComfyUI
/// doesn't end in a bind error.
pub fn ensure_comfyui_port_available(endpoint: ComfyEndpoint) -> Result<(), String> {
    if endpoint.probe() == PortOccupant::Other {
        return Err(format!(
            "Port {} is used by another program. Close it or choose another ComfyUI port.",
            endpoint.port
        ));
    }
    Ok(())
}

/// True if ComfyUI answers on the configured port, whether or not it's the
/// process we launched — covers ComfyUI instances started outside this app.
pub fn comfyui_external_running(state: &AppState) -> bool {
    let endpoint = ComfyEndpoint::from_settings(&state.context.config.settings());
    matches!(
        endpoint.probe(),
        PortOccupant::ComfyUi | PortOccupant::Unresponsive
    )
}

/// The launch flags the ComfyUI checkout at `comfy_dir` accepts, read from
/// its `comfy/cli_args.py`; `None` when that can't be read.
pub fn comfyui_supported_launch_flags(comfy_dir: &Path) -> Option<BTreeSet<String>> {
    let source = std::fs::read_to_string(comfy_dir.join("comfy").join("cli_args.py")).ok()?;
    let flags: BTreeSet<String> = source
        .match_indices("add_argument(")
        .filter_map(|(index, call)| {
            let rest = source[index + call.len()..].trim_start();
            let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
            let name = &rest[1..1 + rest[1..].find(quote)?];
            name.starts_with("--").then(|| name.to_string())
        })
        .collect();
    (!flags.is_empty()).then_some(flags)
}

/// Adds the app-managed memory and port flags to `args`, which already hold
/// the custom launch args (a custom `--port` wins).
pub fn append_launch_settings_args(args: &mut Vec<String>, settings: &AppSettings) {
    if settings.comfyui_disable_dynamic_vram_enabled {
        args.push("--disable-dynamic-vram".to_string());
    }
    if settings.comfyui_fast_disk_enabled {
        args.push("--fast-disk".to_string());
    }
    let has_port = args
        .iter()
        .any(|arg| arg == "--port" || arg.starts_with("--port="));
    if settings.comfyui_port != DEFAULT_COMFYUI_PORT && !has_port {
        args.push("--port".to_string());
        args.push(settings.comfyui_port.to_string());
    }
}

/// Removes app-added flags the installed ComfyUI doesn't accept (it would
/// refuse to start), keeping the user's custom arguments as written.
/// Returns the removed flags.
pub fn drop_unsupported_launch_flags(
    args: &mut Vec<String>,
    supported: &BTreeSet<String>,
    custom_args: &[String],
) -> Vec<String> {
    let mut dropped = Vec::new();
    args.retain(|arg| {
        let flag = arg.split('=').next().unwrap_or(arg);
        let keep = !arg.starts_with("--") || supported.contains(flag) || custom_args.contains(arg);
        if !keep {
            dropped.push(arg.clone());
        }
        keep
    });
    dropped
}

/// Sets the port ComfyUI listens on.
pub fn set_comfyui_port(state: State<'_, AppState>, port: u16) -> Result<AppSettings, String> {
    if port < 1024 {
        return Err("Choose a port from 1024 to 65535.".to_string());
    }
    state
        .context
        .config
        .update_settings(|settings| settings.comfyui_port = port)
        .map_err(|err| err.to_string())
}

/// True if ComfyUI is running, whether as our managed child process or an
/// externally-started instance on the default port.
pub fn comfyui_runtime_running(state: &AppState) -> bool {
    comfyui_process_running(state) || comfyui_external_running(state)
}

/// Reports whether ComfyUI is running, and which install it is.
pub fn get_comfyui_runtime_status(state: State<'_, AppState>) -> ComfyRuntimeStatus {
    let (running, running_root) = comfyui_running_root(&state);
    ComfyRuntimeStatus {
        running,
        running_root: running_root.map(|root| root.to_string_lossy().to_string()),
        url: ComfyEndpoint::from_settings(&state.context.config.settings()).url(),
    }
}

/// Whether a ComfyUI is running, and which install it is: the one this app
/// started, else (for one it didn't start, e.g. from before it opened) the
/// known install whose `main.py` the running process has (Linux). `None`
/// when that can't be told.
pub fn comfyui_running_root(state: &AppState) -> (bool, Option<PathBuf>) {
    if comfyui_process_running(state) {
        return (
            true,
            recover_lock(state.comfyui_process_root.lock()).clone(),
        );
    }
    if !comfyui_external_running(state) {
        return (false, None);
    }
    (
        true,
        crate::platform::host_comfyui_root_among(&known_install_roots(state)),
    )
}

/// The selected install and the installs in the install folder.
fn known_install_roots(state: &AppState) -> Vec<PathBuf> {
    let settings = state.context.config.settings();
    let mut roots: Vec<PathBuf> = settings.comfyui_root.into_iter().collect();
    if let Some(base) = settings.comfyui_install_base {
        if let Ok(entries) = std::fs::read_dir(&base) {
            roots.extend(entries.flatten().map(|entry| entry.path()));
        }
        roots.push(base);
    }
    roots.retain(|root| root.join("main.py").is_file());
    roots.sort();
    roots.dedup();
    roots
}

/// Polls until ComfyUI is reachable (managed process or external) or
/// `timeout` elapses. Returns an error if the managed process exits during
/// startup, becomes unqueryable, or the timeout is reached with nothing
/// listening.
pub fn wait_for_comfyui_start(state: &AppState, timeout: Duration) -> Result<(), String> {
    let started_at = Instant::now();
    loop {
        if comfyui_external_running(state) {
            return Ok(());
        }

        {
            let mut guard = recover_lock(state.comfyui_process.lock());
            if let Some(child) = guard.as_mut() {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        *guard = None;
                        return Err(format!(
                            "ComfyUI process exited during startup with status {status}."
                        ));
                    }
                    Ok(None) => {}
                    Err(err) => {
                        *guard = None;
                        return Err(format!("Failed to monitor ComfyUI startup: {err}"));
                    }
                }
            }
        }

        if started_at.elapsed() > timeout {
            // Only the port actually accepting connections counts as
            // "started" here. A merely-alive process (stuck loading a model
            // index, stuck in a Python traceback loop) must not be reported
            // as success — callers use `Ok(())` to flip the tray to
            // "running" and open the browser at ComfyUI's URL, which would
            // otherwise point at a dead port.
            if comfyui_external_running(state) {
                return Ok(());
            }
            let url = ComfyEndpoint::from_settings(&state.context.config.settings()).url();
            return Err(format!("ComfyUI did not become ready on {url} in time."));
        }
        std::thread::sleep(Duration::from_millis(220));
    }
}

/// Sets (or clears, when empty) the user's custom ComfyUI launch arguments,
/// validating that they parse before persisting.
pub fn set_comfyui_custom_launch_args(
    state: State<'_, AppState>,
    custom_launch_args: String,
) -> Result<AppSettings, String> {
    let trimmed = custom_launch_args.trim();
    let normalized = if trimmed.is_empty() {
        String::new()
    } else {
        parse_custom_launch_args(trimmed)?;
        trimmed.to_string()
    };

    state
        .context
        .config
        .update_settings(|settings| settings.comfyui_custom_launch_args = normalized.clone())
        .map_err(|err| err.to_string())
}

/// Sets the preferred GPU backend (`auto`/`nvidia`/`amd`/`intel`) used to
/// pick a torch profile on the next install/relaunch.
pub fn set_comfyui_gpu_selection(
    state: State<'_, AppState>,
    gpu_selection: String,
) -> Result<AppSettings, String> {
    let normalized = gpu_selection.trim().to_ascii_lowercase();
    if !matches!(normalized.as_str(), "auto" | "nvidia" | "amd" | "intel") {
        return Err("Unknown GPU selection.".to_string());
    }
    state
        .context
        .config
        .update_settings(|settings| {
            settings.comfyui_gpu_selection = (normalized != "auto").then(|| normalized.clone());
        })
        .map_err(|err| err.to_string())
}

/// Toggles whether ComfyUI's stdout/stderr is streamed into the app's
/// runtime log UI.
pub fn set_comfyui_show_runtime_logs(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<AppSettings, String> {
    state
        .context
        .config
        .update_settings(|settings| settings.comfyui_show_runtime_logs = enabled)
        .map_err(|err| err.to_string())
}

/// Kills the managed ComfyUI child process, if this app has a handle to one.
/// Returns `true` if a process was actually killed.
///
/// This is only the "kill the process we spawned" half of stopping ComfyUI —
/// `stop_comfyui_root_impl` in `app_linux.rs`/`app_windows.rs` calls this
/// first, then falls back to platform-specific logic for the case where
/// ComfyUI is still listening but this app has no child handle for it
/// (e.g. after an app restart, or a Flatpak host-spawned process on Linux).
/// That fallback genuinely differs by platform and stays there.
pub fn kill_managed_comfyui_child(state: &AppState) -> Result<bool, String> {
    let mut guard = recover_lock(state.comfyui_process.lock());
    let Some(child) = guard.as_mut() else {
        return Ok(false);
    };

    #[cfg(unix)]
    {
        // Give ComfyUI a chance to shut down cleanly (flush state, release
        // CUDA contexts, let an in-flight workflow reach a checkpoint)
        // before force-killing it. `Child::kill()` alone sends SIGKILL
        // immediately, with no such chance -- this mirrors the
        // SIGTERM-then-wait-then-SIGKILL pattern already used by the
        // "no managed handle" fallback paths in app_linux.rs
        // (kill_python_processes_for_root, kill_host_comfyui_for_root) so
        // the common case (app-started ComfyUI) isn't the ungraceful one.
        let pid = child.id().to_string();
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &pid])
            .status();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => {
                    *guard = None;
                    return Ok(true);
                }
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                _ => break,
            }
        }
    }

    child
        .kill()
        .map_err(|err| format!("Failed to stop ComfyUI: {err}"))?;
    let _ = child.wait();
    *guard = None;
    Ok(true)
}

/// Derives a human-facing instance name for `comfyui_root` (or the
/// configured root, if `None`) via the platform's `resolve_root_path`
/// adapter, falling back to `"ComfyUI"` if no root resolves.
pub fn resolve_comfyui_instance_name(context: &AppContext, comfyui_root: Option<String>) -> String {
    resolve_root_path(context, comfyui_root)
        .ok()
        .as_deref()
        .map(comfyui_instance_name_from_path)
        .unwrap_or_else(|| "ComfyUI".to_string())
}

/// Starts ComfyUI in the background: resolves the instance name, emits a
/// "starting" event, flips the tray to "running", then spawns a thread that
/// calls the platform's `start_comfyui_root_impl` adapter and either hands
/// off to `spawn_comfyui_start_monitor` or reports failure.
pub fn start_comfyui_root_background(app: &AppHandle, comfyui_root: Option<String>) {
    let app_handle = app.clone();
    let instance_name = {
        let state = app_handle.state::<AppState>();
        resolve_comfyui_instance_name(&state.context, comfyui_root.clone())
    };
    emit_comfyui_runtime_event(
        &app_handle,
        "starting",
        format!("Starting {instance_name}..."),
    );
    update_tray_comfy_status(&app_handle, true);
    let instance_name_for_task = instance_name.clone();
    std::thread::spawn(move || {
        let state = app_handle.state::<AppState>();
        if let Err(err) = start_comfyui_root_impl(&app_handle, &state, comfyui_root) {
            let running = comfyui_runtime_running(&state);
            update_tray_comfy_status(&app_handle, running);
            emit_comfyui_runtime_event(
                &app_handle,
                "start_failed",
                format!("{instance_name_for_task} start failed: {err}"),
            );
            return;
        }
        spawn_comfyui_start_monitor(&app_handle, instance_name_for_task);
    });
}

/// Starts ComfyUI at `comfyui_root` (or the configured root), or reports it
/// as already running if it is.
pub fn start_comfyui_root(
    app: AppHandle,
    state: State<'_, AppState>,
    comfyui_root: Option<String>,
) -> Result<(), String> {
    if comfyui_runtime_running(&state) {
        let instance_name = resolve_comfyui_instance_name(&state.context, comfyui_root.clone());
        update_tray_comfy_status(&app, true);
        emit_comfyui_runtime_event(
            &app,
            "started",
            format!("{instance_name} is already running."),
        );
        return Ok(());
    }
    start_comfyui_root_background(&app, comfyui_root);
    Ok(())
}

/// Stops ComfyUI via the platform's `stop_comfyui_root_impl` adapter,
/// updating the tray and emitting lifecycle events along the way.
pub fn stop_comfyui_root(app: AppHandle, state: State<'_, AppState>) -> Result<bool, String> {
    let instance_name = resolve_comfyui_instance_name(&state.context, None);
    emit_comfyui_runtime_event(&app, "stopping", format!("Stopping {instance_name}..."));
    let result = stop_comfyui_root_impl(&state);
    if result.is_ok() {
        let running = comfyui_runtime_running(&state);
        update_tray_comfy_status(&app, running);
        if running {
            emit_comfyui_runtime_event(
                &app,
                "stop_failed",
                format!("{instance_name} stop did not fully complete."),
            );
        } else {
            emit_comfyui_runtime_event(&app, "stopped", format!("{instance_name} stopped."));
        }
    } else if let Err(err) = &result {
        emit_comfyui_runtime_event(
            &app,
            "stop_failed",
            format!("{instance_name} stop failed: {err}"),
        );
    }
    result
}

/// Stops ComfyUI (if running) ahead of an install/add-on mutation that
/// requires it to be stopped first. Returns whether it was running
/// beforehand, so the caller can restart it afterward.
pub fn stop_comfyui_for_mutation(app: &AppHandle, state: &AppState) -> Result<bool, String> {
    let (running, running_root) = comfyui_running_root(state);
    if !running {
        return Ok(false);
    }
    // Changes go to the selected install; another install's running
    // ComfyUI isn't affected, so it keeps running (and isn't "restarted" as
    // the selected one afterwards).
    if let (Some(running_root), Some(selected)) =
        (running_root, state.context.config.settings().comfyui_root)
    {
        let canonical = |path: &Path| std::fs::canonicalize(path).unwrap_or(path.to_path_buf());
        if canonical(&running_root) != canonical(&selected) {
            return Ok(false);
        }
    }
    emit_comfyui_runtime_event(
        app,
        "stopping_for_changes",
        "Stopping ComfyUI before applying changes...",
    );
    stop_comfyui_root_impl(state)?;
    let running = comfyui_runtime_running(state);
    update_tray_comfy_status(app, running);
    if running {
        return Err("ComfyUI is still running. Stop it before applying changes.".to_string());
    }
    emit_comfyui_runtime_event(
        app,
        "stopped_for_changes",
        "ComfyUI stopped for install/remove operation.",
    );
    Ok(true)
}

/// Normalizes `raw`, if present and non-blank, via the platform's
/// `normalize_path` adapter. `None`/blank input yields `Ok(None)`.
pub fn normalize_optional_path(raw: Option<&str>) -> Result<Option<PathBuf>, String> {
    let Some(value) = raw else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    normalize_path(trimmed).map(Some)
}

/// A configured `extra_model_paths.yaml` base path, as detected by the
/// platform's `comfy_extra_model_config` adapter.
#[derive(Debug, Clone)]
pub struct ComfyExtraModelConfig {
    pub base_path: PathBuf,
    pub is_default: bool,
}

/// Frontend-facing view of [`ComfyExtraModelConfig`].
#[derive(Debug, Serialize)]
pub struct ComfyExtraModelConfigResponse {
    pub configured: bool,
    pub base_path: Option<String>,
    pub use_as_default: bool,
}

/// Reports whether `comfyui_root` (or the configured root) has a shared
/// extra-models base path configured, and whether it's the default
/// download destination.
pub fn get_comfyui_extra_model_config(
    state: State<'_, AppState>,
    comfyui_root: Option<String>,
) -> Result<ComfyExtraModelConfigResponse, String> {
    let root = resolve_root_path(&state.context, comfyui_root)?;
    let config = comfy_extra_model_config(&root);
    Ok(match config {
        Some(cfg) => ComfyExtraModelConfigResponse {
            configured: true,
            base_path: Some(cfg.base_path.to_string_lossy().to_string()),
            use_as_default: cfg.is_default,
        },
        None => ComfyExtraModelConfigResponse {
            configured: false,
            base_path: None,
            use_as_default: false,
        },
    })
}

/// Response for [`get_effective_download_destination`].
#[derive(Debug, Serialize)]
pub struct EffectiveDownloadDestinationResponse {
    pub comfyui_root: String,
    pub effective_root: String,
    pub uses_shared_default: bool,
}

/// Reports where model downloads will actually land for `comfyui_root` —
/// the configured shared extra-models path if it's set as default,
/// otherwise the ComfyUI root itself.
pub fn get_effective_download_destination(
    state: State<'_, AppState>,
    comfyui_root: Option<String>,
) -> Result<EffectiveDownloadDestinationResponse, String> {
    let root = resolve_root_path(&state.context, comfyui_root)?;
    let effective_root = effective_download_root(&root);
    Ok(EffectiveDownloadDestinationResponse {
        comfyui_root: root.to_string_lossy().to_string(),
        effective_root: effective_root.to_string_lossy().to_string(),
        uses_shared_default: effective_root != root,
    })
}

/// Sets (or clears) the shared extra-models base path for `comfyui_root`,
/// writing/removing `extra_model_paths.yaml` as needed via the platform's
/// `write_extra_model_paths_yaml` adapter.
pub fn set_comfyui_extra_model_config(
    state: State<'_, AppState>,
    comfyui_root: Option<String>,
    extra_model_root: Option<String>,
    use_as_default: bool,
) -> Result<AppSettings, String> {
    let root = resolve_root_path(&state.context, comfyui_root)?;
    let normalized_extra = normalize_optional_path(extra_model_root.as_deref())?;
    let yaml_path = root.join("extra_model_paths.yaml");
    let example_path = root.join("extra_model_paths.yaml.example");

    if let Some(extra_root) = normalized_extra.as_ref() {
        write_extra_model_paths_yaml(&root, extra_root, use_as_default)?;
    } else {
        if yaml_path.exists() {
            let _ = std::fs::remove_file(&yaml_path);
        }
        if !example_path.exists() {
            let _ = std::fs::write(
                &example_path,
                "# Rename this to extra_model_paths.yaml and ComfyUI will load it\n",
            );
        }
    }

    state
        .context
        .config
        .update_settings(|settings| {
            settings.shared_models_root = normalized_extra.clone();
            settings.shared_models_use_default = normalized_extra.is_some() && use_as_default;
        })
        .map_err(|err| err.to_string())
}

// --- GPU name detection ------------------------------------------------
//
// `detect_nvidia_gpu_details`/`detect_amd_gpu_details`/`detect_intel_gpu_details`
// stay as adapters (see re-exports above), not just because the underlying
// system query differs by platform, but because the caching/retry semantics
// around that query genuinely differ too: Linux additionally tracks a
// "probe complete" flag and gives up permanently after one failed probe,
// while Windows resets its "probe started" flag on failure so a later call
// retries. That's real behavior, not incidental duplication, so the whole
// wrapper stays platform-specific — only the thin name-lookup functions
// below (which don't touch the caching at all) are actually identical.

/// Oldest NVIDIA driver (major version) that runs CUDA 13 torch builds.
pub const CU130_MIN_DRIVER_MAJOR: u64 = 580;

/// Whether an NVIDIA GPU should get the CUDA 13 (cu130) torch builds, as
/// ComfyUI recommends for RTX 20 series and newer: driver 580 or newer and a
/// Turing (compute capability 7.5) or newer GPU, the oldest CUDA 13 supports.
/// Drivers too old to report a compute capability fall back to the name:
/// every RTX card and the GTX 16 series are Turing or newer.
pub fn nvidia_supports_cu130(
    name: Option<&str>,
    driver_version: Option<&str>,
    compute_capability: Option<&str>,
) -> bool {
    let driver_major = driver_version
        .and_then(|raw| raw.trim().split('.').next())
        .and_then(|raw| raw.parse::<u64>().ok())
        .unwrap_or_default();
    if driver_major < CU130_MIN_DRIVER_MAJOR {
        return false;
    }
    let capability = compute_capability.and_then(|raw| {
        let (major, minor) = raw.trim().split_once('.')?;
        Some((major.parse::<u32>().ok()?, minor.parse::<u32>().ok()?))
    });
    if let Some(capability) = capability {
        return capability >= (7, 5);
    }
    let name = name.unwrap_or_default().to_ascii_lowercase();
    name.contains("rtx") || name.contains("gtx 16")
}

/// The detected NVIDIA GPU's name and VRAM in MB, if any.
pub fn detect_nvidia_gpu() -> (Option<String>, Option<u64>) {
    let detailed = detect_nvidia_gpu_details();
    (detailed.name, detailed.vram_mb)
}

/// The detected AMD GPU's name, or a simulated one if `ARCTIC_FAKE_AMD=1`.
pub fn detect_amd_gpu_name() -> Option<String> {
    if fake_amd_enabled() {
        return Some("Fake AMD GPU (simulation)".to_string());
    }
    detect_amd_gpu_details().name
}

/// The detected Intel GPU's name, or a simulated one if `ARCTIC_FAKE_INTEL=1`.
pub fn detect_intel_gpu_name() -> Option<String> {
    if fake_intel_enabled() {
        return Some("Fake Intel GPU (simulation)".to_string());
    }
    detect_intel_gpu_details().name
}

/// The commit hash `git_ref` resolves to in the repo at `root`, if `git`
/// resolves it to at least a short (7+ char) hash, via the platform's
/// `run_command_capture` adapter.
pub fn git_commit_for_ref(root: &Path, git_ref: &str) -> Option<String> {
    let (stdout, _) = run_command_capture("git", &["rev-parse", git_ref], Some(root)).ok()?;
    let commit = stdout.lines().next().unwrap_or_default().trim().to_string();
    if commit.len() >= 7 {
        Some(commit)
    } else {
        None
    }
}

/// The current branch name of the repo at `root`, via the platform's
/// `run_command_capture` adapter.
pub fn git_current_branch(root: &Path) -> Option<String> {
    let (stdout, _) =
        run_command_capture("git", &["rev-parse", "--abbrev-ref", "HEAD"], Some(root)).ok()?;
    let branch = stdout.lines().next().unwrap_or_default().trim().to_string();
    if branch.is_empty() {
        None
    } else {
        Some(branch)
    }
}

/// Resolves the concrete artifact list for a download request: the
/// "always" group filtered by RAM bucket plus the selected variant's own
/// artifacts, or (when the frontend didn't send an explicit selection) the
/// model's default artifacts for `ram_tier`, then narrowed to
/// `selected_keys` if given.
pub fn model_artifacts_for_download_request(
    resolved: &ResolvedModel,
    ram_tier: Option<RamTier>,
    selected_keys: Option<&Vec<String>>,
) -> Vec<ModelArtifact> {
    let artifacts = if selected_keys.is_some() {
        let mut artifacts = Vec::new();
        for group in &resolved.master.always {
            for artifact in &group.artifacts {
                if artifact.ram_bucket.is_some() || artifact.is_supported_on_ram(ram_tier) {
                    artifacts.push(artifact.clone());
                }
            }
        }
        for artifact in &resolved.variant.artifacts {
            if artifact.is_supported_on_ram(ram_tier) {
                artifacts.push(artifact.clone());
            }
        }
        artifacts
    } else {
        resolved.artifacts_for_download(ram_tier)
    };
    filter_selected_model_artifacts(artifacts, selected_keys)
}

/// The `download-progress` event for one download signal.
fn progress_event(kind: &str, signal: DownloadSignal) -> DownloadProgressEvent {
    let kind = kind.to_string();
    match signal {
        DownloadSignal::Started {
            artifact,
            index,
            total,
            size,
        } => DownloadProgressEvent {
            kind: kind.clone(),
            phase: "started".to_string(),
            artifact: Some(artifact),
            index: Some(index + 1),
            total: Some(total),
            received: None,
            size,
            folder: None,
            message: None,
        },
        DownloadSignal::Progress {
            artifact,
            index,
            received,
            size,
        } => DownloadProgressEvent {
            kind: kind.clone(),
            phase: "progress".to_string(),
            artifact: Some(artifact),
            index: Some(index + 1),
            total: None,
            received: Some(received),
            size,
            folder: None,
            message: None,
        },
        DownloadSignal::Verifying {
            artifact,
            index,
            checked,
            size,
        } => DownloadProgressEvent {
            kind: kind.clone(),
            phase: "verifying".to_string(),
            artifact: Some(artifact),
            index: Some(index + 1),
            total: None,
            received: Some(checked),
            size,
            folder: None,
            message: None,
        },
        DownloadSignal::Finished {
            artifact,
            index,
            size,
            folder,
        } => DownloadProgressEvent {
            kind: kind.clone(),
            phase: "finished".to_string(),
            artifact: Some(artifact),
            index: Some(index + 1),
            total: None,
            received: None,
            size,
            folder,
            message: None,
        },
        DownloadSignal::Failed {
            artifact,
            index,
            error,
        } => DownloadProgressEvent {
            kind: kind.clone(),
            phase: "failed".to_string(),
            artifact: Some(artifact),
            index: Some(index + 1),
            total: None,
            received: None,
            size: None,
            folder: None,
            message: Some(error),
        },
    }
}

/// How often a file's progress is sent to the UI at most.
const PROGRESS_EVENT_INTERVAL: Duration = Duration::from_millis(150);

/// Thins out progress: the downloader reports every chunk it reads (thousands
/// a second on a fast link, from several threads for multi-part downloads).
/// Keeps each file's latest progress and sends it at most every
/// [`PROGRESS_EVENT_INTERVAL`], never going backwards; other events (started,
/// finished, failed) go out at once, after that file's pending progress.
#[derive(Default)]
struct ProgressCoalescer {
    /// The latest unsent progress/verifying event per file index.
    pending: BTreeMap<usize, DownloadProgressEvent>,
    /// The phase and byte count last sent per file index.
    sent: HashMap<usize, (String, u64)>,
}

impl ProgressCoalescer {
    fn is_progress(event: &DownloadProgressEvent) -> bool {
        event.phase == "progress" || event.phase == "verifying"
    }

    /// Takes an event; returns the events to send now.
    fn push(&mut self, event: DownloadProgressEvent) -> Vec<DownloadProgressEvent> {
        let index = event.index.unwrap_or(0);
        if Self::is_progress(&event) {
            let received = event.received.unwrap_or(0);
            let behind = |phase: &str, seen: u64| phase == event.phase && seen >= received;
            let stale = self
                .pending
                .get(&index)
                .is_some_and(|p| behind(&p.phase, p.received.unwrap_or(0)))
                || self
                    .sent
                    .get(&index)
                    .is_some_and(|(phase, seen)| behind(phase, *seen));
            if !stale {
                self.pending.insert(index, event);
            }
            return Vec::new();
        }
        let mut out: Vec<_> = self.pending.remove(&index).into_iter().collect();
        if event.phase != "started" {
            self.sent.remove(&index);
        }
        out.push(event);
        out
    }

    /// The pending progress to send now.
    fn flush(&mut self) -> Vec<DownloadProgressEvent> {
        let out: Vec<_> = std::mem::take(&mut self.pending).into_values().collect();
        for event in &out {
            if let Some(index) = event.index {
                self.sent
                    .insert(index, (event.phase.clone(), event.received.unwrap_or(0)));
            }
        }
        out
    }
}

/// Forwards download signals from `rx` to the UI as `download-progress`
/// events (thinned out by [`ProgressCoalescer`]) until the sender is dropped.
pub fn spawn_progress_emitter(
    app: AppHandle,
    kind: String,
    rx: std::sync::mpsc::Receiver<DownloadSignal>,
) {
    std::thread::spawn(move || {
        let mut coalescer = ProgressCoalescer::default();
        let mut last_flush = Instant::now();
        let send = |events: Vec<DownloadProgressEvent>| {
            for event in events {
                let _ = app.emit("download-progress", event);
            }
        };
        loop {
            let wait = PROGRESS_EVENT_INTERVAL.saturating_sub(last_flush.elapsed());
            match rx.recv_timeout(wait) {
                Ok(signal) => send(coalescer.push(progress_event(&kind, signal))),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    send(coalescer.flush());
                    break;
                }
            }
            if last_flush.elapsed() >= PROGRESS_EVENT_INTERVAL {
                send(coalescer.flush());
                last_flush = Instant::now();
            }
        }
    });
}

/// Response for [`get_comfyui_update_status`].
#[derive(Debug, Serialize)]
pub struct ComfyUiUpdateStatus {
    pub installed_version: Option<String>,
    pub latest_version: Option<String>,
    pub head_matches_latest_tag: bool,
    pub update_available: bool,
    pub checked: bool,
    pub detail: String,
}

/// Compares the installed ComfyUI version at `root` against the latest
/// release tag (via the platform's `git_latest_release_tag` adapter),
/// preferring a HEAD-matches-tag check before falling back to semver
/// comparison of version strings.
pub fn get_comfyui_update_status_blocking(root: PathBuf) -> Result<ComfyUiUpdateStatus, String> {
    let installed_version = read_comfyui_installed_version(&root);

    if !root.join(".git").exists() {
        return Ok(ComfyUiUpdateStatus {
            installed_version,
            latest_version: None,
            head_matches_latest_tag: false,
            update_available: false,
            checked: false,
            detail: "Not a git-based ComfyUI install.".to_string(),
        });
    }

    let Some((latest_tag, latest_version)) = git_latest_release_tag(&root) else {
        return Ok(ComfyUiUpdateStatus {
            installed_version,
            latest_version: None,
            head_matches_latest_tag: false,
            update_available: false,
            checked: false,
            detail: "Could not read remote ComfyUI release tags.".to_string(),
        });
    };

    let head_commit = git_commit_for_ref(&root, "HEAD");
    let tag_commit = git_commit_for_ref(&root, &latest_tag);
    if head_commit.is_some() && head_commit == tag_commit {
        return Ok(ComfyUiUpdateStatus {
            installed_version,
            latest_version: Some(latest_version.clone()),
            head_matches_latest_tag: true,
            update_available: false,
            checked: true,
            detail: format!("ComfyUI is up to date by release tags (HEAD matches {latest_tag})."),
        });
    }

    match installed_version.clone().and_then(|v| normalize_release_version(&v)) {
        Some(local_version) => {
            let local_triplet = parse_semver_triplet(&local_version);
            let latest_triplet = parse_semver_triplet(&latest_version);
            let update_available = matches!(
                (local_triplet, latest_triplet),
                (Some(local), Some(latest)) if latest > local
            );

            Ok(ComfyUiUpdateStatus {
                installed_version,
                latest_version: Some(latest_version.clone()),
                head_matches_latest_tag: false,
                update_available,
                checked: true,
                detail: if update_available {
                    format!(
                        "ComfyUI update available from release tags (local v{local_version}, latest tag {latest_tag})."
                    )
                } else {
                    format!(
                        "ComfyUI is up to date by release tags (local v{local_version}, latest tag {latest_tag})."
                    )
                },
            })
        }
        None => Ok(ComfyUiUpdateStatus {
            installed_version,
            latest_version: Some(latest_version.clone()),
            head_matches_latest_tag: false,
            update_available: false,
            checked: true,
            detail: format!(
                "Detected latest release tag {latest_tag}, but local ComfyUI version metadata is unavailable."
            ),
        }),
    }
}

/// Reports whether a ComfyUI update is available for `comfyui_root` (or the
/// configured root).
pub async fn get_comfyui_update_status(
    state: State<'_, AppState>,
    comfyui_root: Option<String>,
) -> Result<ComfyUiUpdateStatus, String> {
    let root = resolve_root_path(&state.context, comfyui_root)?;
    crate::host::async_runtime::spawn_blocking(move || get_comfyui_update_status_blocking(root))
        .await
        .map_err(|err| format!("ComfyUI update check task failed: {err}"))?
}

/// Response for [`get_comfyui_resume_state`].
#[derive(Debug, Serialize)]
pub struct ComfyResumeStateResponse {
    pub found: bool,
    pub install_dir: Option<String>,
    pub step: Option<String>,
    pub summary: String,
}

/// Reports whether an interrupted ComfyUI install was found under
/// `install_base` (or the configured install base), so the frontend can
/// offer to resume it.
pub fn get_comfyui_resume_state(
    state: State<'_, AppState>,
    install_base: Option<String>,
) -> Result<ComfyResumeStateResponse, String> {
    let base = if let Some(raw) = install_base {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            state
                .context
                .config
                .settings()
                .comfyui_install_base
                .ok_or_else(|| "ComfyUI install base folder is not set.".to_string())?
        } else {
            normalize_path(trimmed)?
        }
    } else {
        state
            .context
            .config
            .settings()
            .comfyui_install_base
            .ok_or_else(|| "ComfyUI install base folder is not set.".to_string())?
    };

    if !base.exists() {
        return Ok(ComfyResumeStateResponse {
            found: false,
            install_dir: None,
            step: None,
            summary: "No interrupted install found.".to_string(),
        });
    }

    if let Some((dir, install_state)) = find_in_progress_install(&base) {
        return Ok(ComfyResumeStateResponse {
            found: true,
            install_dir: Some(dir.to_string_lossy().to_string()),
            step: Some(install_state.step.clone()),
            summary: format!(
                "Interrupted install found in {} at step '{}'.",
                dir.display(),
                install_state.step
            ),
        });
    }

    Ok(ComfyResumeStateResponse {
        found: false,
        install_dir: None,
        step: None,
        summary: "No interrupted install found.".to_string(),
    })
}

/// Refreshes the model/LoRA catalog from the remote source and returns the
/// new snapshot.
pub async fn refresh_catalog(state: State<'_, AppState>) -> Result<ModelCatalog, String> {
    state
        .context
        .catalog
        .refresh_from_remote()
        .await
        .map_err(|err| err.to_string())?;
    Ok(state.context.catalog.catalog_snapshot())
}

/// One entry of a [`BatchModelDownloadRequest`].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchModelDownloadItem {
    pub model_id: String,
    pub variant_id: String,
    #[serde(default)]
    pub selected_artifact_keys: Option<Vec<String>>,
}

/// Request payload for [`download_model_assets_batch`].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchModelDownloadRequest {
    pub items: Vec<BatchModelDownloadItem>,
    pub ram_tier: Option<String>,
    #[serde(default)]
    pub vram_tier: Option<String>,
    pub comfyui_root: Option<String>,
}

/// Free space kept on top of what a model download needs, so filling the
/// disk exactly doesn't break ComfyUI or the system.
const DOWNLOAD_SPACE_HEADROOM: u64 = 2 * 1024 * 1024 * 1024;

/// Refuses a model download that can't fit on the disk holding `root`.
/// Unknown sizes and a failed free-space query let the download go ahead.
fn ensure_download_space(root: &Path, needed: u64) -> Result<(), String> {
    if needed == 0 {
        return Ok(());
    }
    // The models folder may not exist yet; ask about its closest ancestor.
    let Some(existing) = root
        .join("models")
        .ancestors()
        .find(|p| p.exists())
        .map(Path::to_path_buf)
    else {
        return Ok(());
    };
    let Ok(free) = fs2::available_space(&existing) else {
        return Ok(());
    };
    if free >= needed.saturating_add(DOWNLOAD_SPACE_HEADROOM) {
        return Ok(());
    }
    let gb = |bytes: u64| bytes as f64 / 1024f64.powi(3);
    Err(format!(
        "Not enough disk space: these files need {:.1} GB (plus {:.0} GB to spare), but only {:.1} GB is free on {}.",
        gb(needed),
        gb(DOWNLOAD_SPACE_HEADROOM),
        gb(free),
        existing.display()
    ))
}

/// Starts downloading the selected artifacts of one model variant in the
/// background, emitting `download-progress` events as it goes.
#[allow(clippy::too_many_arguments)]
pub async fn download_model_assets(
    app: AppHandle,
    state: State<'_, AppState>,
    model_id: String,
    variant_id: String,
    ram_tier: Option<String>,
    vram_tier: Option<String>,
    selected_artifact_keys: Option<Vec<String>>,
    comfyui_root: Option<String>,
) -> Result<(), String> {
    let root = resolve_root_path(&state.context, comfyui_root)?;
    let effective_root = effective_download_root(&root);
    let selected_vram_tier = vram_tier.as_deref().and_then(parse_vram_tier);
    let resolved = state
        .context
        .catalog
        .resolve_download_selection(&model_id, &variant_id, selected_vram_tier)
        .ok_or_else(|| "Selected model variant was not found in catalog.".to_string())?;

    let tier = ram_tier
        .as_deref()
        .and_then(parse_ram_tier)
        .or_else(|| state.context.ram_tier());
    let planned =
        model_artifacts_for_download_request(&resolved, tier, selected_artifact_keys.as_ref());
    if planned.is_empty() {
        return Err("No model files are selected for download.".to_string());
    }

    ensure_download_space(
        &effective_root,
        remaining_download_bytes(&effective_root, &resolved.master.id, &planned),
    )?;

    let cancel = CancellationToken::new();
    {
        let mut active = recover_lock(state.active_cancel.lock());
        if active.is_some() {
            return Err("A download is already active. Cancel it first.".to_string());
        }
        *active = Some(cancel.clone());
    }

    let mut resolved_for_download = resolved.clone();
    resolved_for_download.variant.artifacts = planned;

    let (tx, rx) = std::sync::mpsc::channel();
    let handle = state.context.downloads.download_variant_with_cancel(
        effective_root,
        resolved_for_download,
        tx,
        Some(cancel),
    );
    *recover_lock(state.active_abort.lock()) = Some(handle.abort_handle());
    spawn_progress_emitter(app.clone(), "model".to_string(), rx);
    let app_for_task = app.clone();
    crate::host::async_runtime::spawn(async move {
        let result = handle.await;
        let managed = app_for_task.state::<AppState>();
        *recover_lock(managed.active_cancel.lock()) = None;
        *recover_lock(managed.active_abort.lock()) = None;

        match result {
            Ok(Ok(outcomes)) => {
                let _ = app_for_task.emit(
                    "download-progress",
                    DownloadProgressEvent {
                        kind: "model".to_string(),
                        phase: "batch_finished".to_string(),
                        artifact: None,
                        index: None,
                        total: Some(outcomes.len()),
                        received: None,
                        size: None,
                        folder: None,
                        message: Some("Model download batch completed.".to_string()),
                    },
                );
            }
            Ok(Err(err)) => {
                let lower = err.to_string().to_ascii_lowercase();
                let phase = if lower.contains("cancel") {
                    "cancelled"
                } else {
                    "batch_failed"
                };
                let _ = app_for_task.emit(
                    "download-progress",
                    DownloadProgressEvent {
                        kind: "model".to_string(),
                        phase: phase.to_string(),
                        artifact: None,
                        index: None,
                        total: None,
                        received: None,
                        size: None,
                        folder: None,
                        message: Some(err.to_string()),
                    },
                );
            }
            Err(join_err) => {
                let phase = if join_err.is_cancelled() {
                    "cancelled"
                } else {
                    "batch_failed"
                };
                let _ = app_for_task.emit(
                    "download-progress",
                    DownloadProgressEvent {
                        kind: "model".to_string(),
                        phase: phase.to_string(),
                        artifact: None,
                        index: None,
                        total: None,
                        received: None,
                        size: None,
                        folder: None,
                        message: Some(join_err.to_string()),
                    },
                );
            }
        }
    });

    Ok(())
}

/// Starts downloading the selected artifacts of multiple model variants
/// sequentially in the background, emitting `download-progress` events for
/// each in turn.
pub async fn download_model_assets_batch(
    app: AppHandle,
    state: State<'_, AppState>,
    request: BatchModelDownloadRequest,
) -> Result<(), String> {
    if request.items.is_empty() {
        return Err("Select at least one model first.".to_string());
    }

    let root = resolve_root_path(&state.context, request.comfyui_root)?;
    let effective_root = effective_download_root(&root);
    let tier = request
        .ram_tier
        .as_deref()
        .and_then(parse_ram_tier)
        .or_else(|| state.context.ram_tier());
    let selected_vram_tier = request.vram_tier.as_deref().and_then(parse_vram_tier);

    let mut resolved_items = Vec::new();
    let mut needed_bytes = 0u64;
    for item in &request.items {
        let resolved = state
            .context
            .catalog
            .resolve_download_selection(&item.model_id, &item.variant_id, selected_vram_tier)
            .ok_or_else(|| {
                format!(
                    "Selected model variant was not found in catalog: {}/{}",
                    item.model_id, item.variant_id
                )
            })?;

        let planned = model_artifacts_for_download_request(
            &resolved,
            tier,
            item.selected_artifact_keys.as_ref(),
        );
        if planned.is_empty() {
            return Err(format!(
                "No model files are selected for {}.",
                resolved.master.display_name
            ));
        }

        needed_bytes += remaining_download_bytes(&effective_root, &resolved.master.id, &planned);
        let mut resolved_for_download = resolved.clone();
        resolved_for_download.variant.artifacts = planned;
        resolved_items.push(resolved_for_download);
    }
    ensure_download_space(&effective_root, needed_bytes)?;

    let cancel = CancellationToken::new();
    {
        let mut active = recover_lock(state.active_cancel.lock());
        if active.is_some() {
            return Err("A download is already active. Cancel it first.".to_string());
        }
        *active = Some(cancel.clone());
    }

    let app_for_task = app.clone();
    let downloads = state.context.downloads.clone();
    let effective_root_for_task = effective_root.clone();
    let abort = crate::host::async_runtime::spawn(async move {
        for (batch_index, resolved) in resolved_items.into_iter().enumerate() {
            let (tx, rx) = std::sync::mpsc::channel();
            spawn_progress_emitter(app_for_task.clone(), "model".to_string(), rx);
            let handle = downloads.download_variant_with_cancel(
                effective_root_for_task.clone(),
                resolved,
                tx,
                Some(cancel.clone()),
            );
            match handle.await {
                Ok(Ok(_)) => {}
                Ok(Err(err)) => {
                    let _ = app_for_task.emit(
                        "download-progress",
                        DownloadProgressEvent {
                            kind: "model".to_string(),
                            phase: if err.to_string().to_ascii_lowercase().contains("cancel") {
                                "cancelled".to_string()
                            } else {
                                "batch_failed".to_string()
                            },
                            artifact: None,
                            index: Some(batch_index + 1),
                            total: None,
                            received: None,
                            size: None,
                            folder: None,
                            message: Some(err.to_string()),
                        },
                    );
                    return;
                }
                Err(join_err) => {
                    let _ = app_for_task.emit(
                        "download-progress",
                        DownloadProgressEvent {
                            kind: "model".to_string(),
                            phase: if join_err.is_cancelled() {
                                "cancelled".to_string()
                            } else {
                                "batch_failed".to_string()
                            },
                            artifact: None,
                            index: Some(batch_index + 1),
                            total: None,
                            received: None,
                            size: None,
                            folder: None,
                            message: Some(join_err.to_string()),
                        },
                    );
                    return;
                }
            }
        }

        let _ = app_for_task.emit(
            "download-progress",
            DownloadProgressEvent {
                kind: "model".to_string(),
                phase: "batch_finished".to_string(),
                artifact: None,
                index: None,
                total: None,
                received: None,
                size: None,
                folder: None,
                message: Some("Model download batch completed.".to_string()),
            },
        );
    });

    *recover_lock(state.active_abort.lock()) = Some(abort.inner().abort_handle());

    let managed = app.clone();
    crate::host::async_runtime::spawn(async move {
        let result = abort.await;
        let state = managed.state::<AppState>();
        *recover_lock(state.active_cancel.lock()) = None;
        *recover_lock(state.active_abort.lock()) = None;
        // Cancel aborts the batch task, which then can't report it itself.
        if result.as_ref().is_err_and(|err| err.is_cancelled()) {
            let _ = managed.emit(
                "download-progress",
                DownloadProgressEvent {
                    kind: "model".to_string(),
                    phase: "cancelled".to_string(),
                    artifact: None,
                    index: None,
                    total: None,
                    received: None,
                    size: None,
                    folder: None,
                    message: Some("Download cancelled.".to_string()),
                },
            );
        }
    });

    Ok(())
}

/// Starts downloading a workflow JSON file into `<comfyui_root>/user/default/workflows`,
/// emitting `download-progress` events as it goes.
pub async fn download_workflow_asset(
    app: AppHandle,
    state: State<'_, AppState>,
    workflow_id: String,
    comfyui_root: Option<String>,
) -> Result<(), String> {
    let root = resolve_root_path(&state.context, comfyui_root)?;
    let workflow: WorkflowDefinition = state
        .context
        .catalog
        .find_workflow(&workflow_id)
        .ok_or_else(|| "Selected workflow was not found in catalog.".to_string())?;

    let workflows_dir = root.join("user").join("default").join("workflows");
    std::fs::create_dir_all(&workflows_dir).map_err(|err| {
        format!(
            "Failed to create ComfyUI workflows directory ({}): {err}",
            workflows_dir.display()
        )
    })?;

    let cancel = CancellationToken::new();
    {
        let mut active = recover_lock(state.active_cancel.lock());
        if active.is_some() {
            return Err("A download is already active. Cancel it first.".to_string());
        }
        *active = Some(cancel.clone());
    }

    let (tx, rx) = std::sync::mpsc::channel();
    let handle = state.context.downloads.download_workflow_with_cancel(
        workflows_dir,
        workflow,
        tx,
        Some(cancel),
    );
    *recover_lock(state.active_abort.lock()) = Some(handle.abort_handle());
    spawn_progress_emitter(app.clone(), "workflow".to_string(), rx);
    let app_for_task = app.clone();
    crate::host::async_runtime::spawn(async move {
        let result = handle.await;
        let managed = app_for_task.state::<AppState>();
        *recover_lock(managed.active_cancel.lock()) = None;
        *recover_lock(managed.active_abort.lock()) = None;

        match result {
            Ok(Ok(outcome)) => {
                let message = match outcome.status {
                    DownloadStatus::SkippedExisting => {
                        "Workflow already exists. Skipped download.".to_string()
                    }
                    DownloadStatus::Downloaded => "Workflow download completed.".to_string(),
                };
                let _ = app_for_task.emit(
                    "download-progress",
                    DownloadProgressEvent {
                        kind: "workflow".to_string(),
                        phase: "batch_finished".to_string(),
                        artifact: None,
                        index: None,
                        total: Some(1),
                        received: None,
                        size: None,
                        folder: None,
                        message: Some(message),
                    },
                );
            }
            Ok(Err(err)) => {
                let lower = err.to_string().to_ascii_lowercase();
                let phase = if lower.contains("cancel") {
                    "cancelled"
                } else {
                    "batch_failed"
                };
                let _ = app_for_task.emit(
                    "download-progress",
                    DownloadProgressEvent {
                        kind: "workflow".to_string(),
                        phase: phase.to_string(),
                        artifact: None,
                        index: None,
                        total: None,
                        received: None,
                        size: None,
                        folder: None,
                        message: Some(err.to_string()),
                    },
                );
            }
            Err(join_err) => {
                let phase = if join_err.is_cancelled() {
                    "cancelled"
                } else {
                    "batch_failed"
                };
                let _ = app_for_task.emit(
                    "download-progress",
                    DownloadProgressEvent {
                        kind: "workflow".to_string(),
                        phase: phase.to_string(),
                        artifact: None,
                        index: None,
                        total: None,
                        received: None,
                        size: None,
                        folder: None,
                        message: Some(join_err.to_string()),
                    },
                );
            }
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{DownloadProgressEvent, ProgressCoalescer};

    fn event(phase: &str, index: usize, received: Option<u64>) -> DownloadProgressEvent {
        DownloadProgressEvent {
            kind: "model".into(),
            phase: phase.into(),
            artifact: Some("a.safetensors".into()),
            index: Some(index),
            total: None,
            received,
            size: Some(1000),
            folder: None,
            message: None,
        }
    }

    #[test]
    fn progress_is_coalesced_and_never_goes_backwards() {
        let mut c = ProgressCoalescer::default();
        assert_eq!(c.push(event("started", 1, None)).len(), 1);
        for received in [10, 300, 200, 250] {
            assert!(c.push(event("progress", 1, Some(received))).is_empty());
        }
        let sent = c.flush();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].received, Some(300));
        // Older than what was sent: dropped.
        c.push(event("progress", 1, Some(280)));
        assert!(c.flush().is_empty());
        // Verification starts over from zero.
        c.push(event("verifying", 1, Some(5)));
        assert_eq!(c.flush()[0].received, Some(5));
        // Finished sends the pending progress first.
        c.push(event("verifying", 1, Some(900)));
        let out = c.push(event("finished", 1, None));
        assert_eq!(
            out.iter().map(|e| e.phase.as_str()).collect::<Vec<_>>(),
            ["verifying", "finished"]
        );
        assert!(c.flush().is_empty());
    }

    use super::*;

    /// A fresh, empty temporary directory for one test.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("arctic-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn builtin_manager_is_found_in_venv_and_embedded_layouts() {
        let root = temp_dir("manager-layouts");
        let linux_venv = root.join("linux/.venv");
        std::fs::create_dir_all(linux_venv.join("bin")).unwrap();
        let linux_py = linux_venv.join("bin/python");
        assert!(!builtin_manager_installed(&linux_py));
        let package = linux_venv.join("lib/python3.12/site-packages/comfyui_manager");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("__init__.py"), "").unwrap();
        assert!(builtin_manager_installed(&linux_py));

        let windows_venv = root.join("windows/.venv");
        let package = windows_venv.join("Lib/site-packages/comfyui_manager");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("__init__.py"), "").unwrap();
        assert!(builtin_manager_installed(
            &windows_venv.join("Scripts/python.exe")
        ));

        let embedded = root.join("python_embeded");
        let package = embedded.join("Lib/site-packages/comfyui_manager");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("__init__.py"), "").unwrap();
        assert!(builtin_manager_installed(&embedded.join("python.exe")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_manager_clones_are_renamed_not_deleted() {
        let custom_nodes = temp_dir("manager-legacy");
        for name in ["ComfyUI-Manager", "comfyui-manager-main", "rgthree-comfy"] {
            std::fs::create_dir_all(custom_nodes.join(name)).unwrap();
            std::fs::write(custom_nodes.join(name).join("marker"), name).unwrap();
        }
        let mut renamed = disable_legacy_manager_clones(&custom_nodes).unwrap();
        renamed.sort();
        assert_eq!(renamed, ["ComfyUI-Manager", "comfyui-manager-main"]);
        assert!(custom_nodes
            .join("ComfyUI-Manager.disabled/marker")
            .is_file());
        assert!(custom_nodes.join("rgthree-comfy/marker").is_file());
        // Already disabled: nothing more to do.
        assert!(disable_legacy_manager_clones(&custom_nodes)
            .unwrap()
            .is_empty());
        let _ = std::fs::remove_dir_all(custom_nodes);
    }

    #[test]
    fn torch_overrides_live_in_the_environment_and_feed_installs() {
        let env = temp_dir("torch-pins");
        std::fs::create_dir_all(env.join("bin")).unwrap();
        let py = env.join("bin/python");
        assert_eq!(
            torch_overrides_path(&py),
            Some(env.join("arctic-torch-overrides.txt"))
        );
        let embedded = env.join("python_embeded/python.exe");
        assert_eq!(
            torch_overrides_path(&embedded),
            Some(env.join("python_embeded/arctic-torch-overrides.txt"))
        );

        let py_s = py.to_string_lossy().to_string();
        let mut args = vec![
            "install".to_string(),
            "-r".to_string(),
            "requirements.txt".to_string(),
        ];
        append_torch_overrides(&mut args, &py_s);
        assert_eq!(args.len(), 3, "no pins written yet");

        assert!(write_torch_overrides(&py, "triton==3.8.0\n").is_err());
        write_torch_overrides(
            &py,
            "torch==2.14.1+cu130\ntorchvision==0.29.1+cu130\ntorchaudio==2.11.0+cu130\ntriton==3.8.0\n",
        )
        .unwrap();
        let contents = std::fs::read_to_string(env.join("arctic-torch-overrides.txt")).unwrap();
        assert!(contents.contains("torch==2.14.1+cu130\ntorchvision==0.29.1+cu130"));

        append_torch_overrides(&mut args, &py_s);
        assert_eq!(args[3], "--overrides");
        append_torch_overrides(&mut args, &py_s);
        assert_eq!(args.len(), 5, "added once");
        let mut uninstall = vec!["uninstall".to_string(), "torch".to_string()];
        append_torch_overrides(&mut uninstall, &py_s);
        assert_eq!(uninstall.len(), 2);

        remove_torch_overrides(&py);
        assert!(existing_torch_overrides(&py_s).is_none());
        let _ = std::fs::remove_dir_all(env);
    }

    #[test]
    fn base_versions_ignore_local_build_tags() {
        assert!(same_base_version("2.14.1+cu130", "2.14.1"));
        assert!(same_base_version(
            "0.29.0a0+rocm10.1.0",
            "0.29.0a0+rocm10.1.0"
        ));
        assert!(same_base_version("2.14.1+cu130", "2.14.1+cu130"));
        assert!(!same_base_version("2.14.0+cu130", "2.14.1"));
        assert!(!same_base_version("", "2.14.1"));
        assert!(!same_base_version("2.15.0.dev20261020+xpu", "nightly"));
    }

    fn settings_with(custom: &str, port: u16) -> AppSettings {
        AppSettings {
            comfyui_custom_launch_args: custom.to_string(),
            comfyui_port: port,
            ..AppSettings::default()
        }
    }

    #[test]
    fn endpoint_prefers_a_custom_port_and_detects_tls() {
        assert_eq!(
            ComfyEndpoint::from_settings(&settings_with("", 8188)).url(),
            "http://127.0.0.1:8188"
        );
        assert_eq!(
            ComfyEndpoint::from_settings(&settings_with("", 8190)).port,
            8190
        );
        assert_eq!(
            ComfyEndpoint::from_settings(&settings_with("--port 9000 --listen", 8190)).port,
            9000
        );
        assert_eq!(
            ComfyEndpoint::from_settings(&settings_with("--port=9001", 8190)).port,
            9001
        );
        let tls = ComfyEndpoint::from_settings(&settings_with(
            "--tls-keyfile k.pem --tls-certfile c.pem",
            8188,
        ));
        assert_eq!(tls.url(), "https://127.0.0.1:8188");
    }

    #[test]
    fn system_stats_responses_are_classified() {
        let comfy = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"system\": {\"comfyui_version\": \"0.9.1\"}, \"devices\": []}";
        assert_eq!(classify_system_stats_response(comfy), PortOccupant::ComfyUi);
        let other = b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(classify_system_stats_response(other), PortOccupant::Other);
        let html = b"HTTP/1.1 200 OK\r\n\r\n<html>dashboard</html>";
        assert_eq!(classify_system_stats_response(html), PortOccupant::Other);
        assert_eq!(
            classify_system_stats_response(b""),
            PortOccupant::Unresponsive
        );
        assert_eq!(
            classify_system_stats_response(b"HTTP/1.1 200 OK\r\nContent-"),
            PortOccupant::Unresponsive
        );
    }

    #[test]
    fn probe_tells_comfyui_from_other_programs() {
        use std::io::{Read, Write};
        let serve = |body: &'static str| {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            std::thread::spawn(move || {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut buf = [0u8; 512];
                    let _ = stream.read(&mut buf);
                    let _ = stream.write_all(body.as_bytes());
                }
            });
            ComfyEndpoint { port, tls: false }
        };
        let comfy = serve("HTTP/1.1 200 OK\r\n\r\n{\"system\": {}, \"devices\": []}");
        assert_eq!(comfy.probe(), PortOccupant::ComfyUi);
        let other = serve("HTTP/1.1 404 Not Found\r\n\r\n");
        assert_eq!(other.probe(), PortOccupant::Other);
        assert!(ensure_comfyui_port_available(serve("HTTP/1.1 404 Not Found\r\n\r\n")).is_err());
        let free = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        assert_eq!(
            ComfyEndpoint {
                port: free,
                tls: false
            }
            .probe(),
            PortOccupant::Free
        );
    }

    #[test]
    fn launch_settings_add_flags_and_port() {
        let mut settings = settings_with("", 8190);
        settings.comfyui_disable_dynamic_vram_enabled = true;
        settings.comfyui_fast_disk_enabled = true;
        let mut args = vec!["--listen".to_string()];
        append_launch_settings_args(&mut args, &settings);
        assert_eq!(
            args,
            [
                "--listen",
                "--disable-dynamic-vram",
                "--fast-disk",
                "--port",
                "8190"
            ]
        );
        let mut custom_port = vec!["--port".to_string(), "9000".to_string()];
        append_launch_settings_args(&mut custom_port, &settings_with("", 8190));
        assert_eq!(custom_port, ["--port", "9000"]);
        let mut default_port = Vec::new();
        append_launch_settings_args(&mut default_port, &settings_with("", 8188));
        assert!(default_port.is_empty());
    }

    #[test]
    fn supported_flags_come_from_cli_args_and_filter_app_flags() {
        let comfy = temp_dir("cli-args");
        std::fs::create_dir_all(comfy.join("comfy")).unwrap();
        std::fs::write(
            comfy.join("comfy/cli_args.py"),
            "parser.add_argument(\"--listen\", type=str)\nparser.add_argument('--port', type=int)\nvram_group.add_argument(\"--lowvram\", action=\"store_true\")\nparser.add_argument(\"-h\")\n",
        )
        .unwrap();
        let supported = comfyui_supported_launch_flags(&comfy).unwrap();
        assert_eq!(
            supported.iter().map(String::as_str).collect::<Vec<_>>(),
            ["--listen", "--lowvram", "--port"]
        );
        assert!(comfyui_supported_launch_flags(&comfy.join("missing")).is_none());

        let custom = vec!["--my-flag".to_string()];
        let mut args: Vec<String> = ["--listen", "--fast-disk", "--port", "8190", "--my-flag"]
            .map(String::from)
            .to_vec();
        let dropped = drop_unsupported_launch_flags(&mut args, &supported, &custom);
        assert_eq!(dropped, ["--fast-disk"]);
        assert_eq!(args, ["--listen", "--port", "8190", "--my-flag"]);
        let _ = std::fs::remove_dir_all(comfy);
    }

    #[test]
    fn distributions_and_torch_builds_are_read_from_disk() {
        let env = temp_dir("dist-info");
        let site = env.join("lib/python3.12/site-packages");
        std::fs::create_dir_all(site.join("flash_attn-2.8.3.post2.dist-info")).unwrap();
        std::fs::create_dir_all(site.join("nunchaku-1.3.0.dev20261023+cu13.0torch2.14.dist-info"))
            .unwrap();
        std::fs::create_dir_all(site.join("torch")).unwrap();
        std::fs::write(
            site.join("torch/version.py"),
            "from typing import Optional\n__version__ = '2.14.1+cu130'\ncuda: Optional[str] = '13.0'\nhip: Optional[str] = None\nxpu: Optional[str] = None\n",
        )
        .unwrap();
        std::fs::create_dir_all(env.join("bin")).unwrap();
        let py = env.join("bin/python");
        assert!(python_dist_installed(&py, "flash-attn"));
        assert!(python_dist_installed(&py, "Flash_Attn"));
        assert!(python_dist_installed(&py, "nunchaku"));
        assert!(!python_dist_installed(&py, "sageattention"));
        assert_eq!(
            torch_build(&py),
            Some(TorchBuild {
                version: "2.14.1+cu130".into(),
                cuda: Some("13.0".into()),
                hip: None,
                xpu: None
            })
        );
        let _ = std::fs::remove_dir_all(env);
    }

    #[test]
    fn manager_launch_arg_respects_an_explicit_mode() {
        let mut args = vec!["--listen".to_string()];
        append_manager_launch_arg(&mut args, false);
        assert_eq!(args, ["--listen"]);
        append_manager_launch_arg(&mut args, true);
        assert_eq!(args, ["--listen", "--enable-manager"]);
        append_manager_launch_arg(&mut args, true);
        assert_eq!(args.len(), 2);
        let mut legacy = vec!["--enable-manager-legacy-ui".to_string()];
        append_manager_launch_arg(&mut legacy, true);
        assert_eq!(legacy, ["--enable-manager-legacy-ui"]);
    }

    #[test]
    fn cu130_needs_driver_580_and_turing_or_newer() {
        let rtx5090 = Some("NVIDIA GeForce RTX 5090");
        assert!(nvidia_supports_cu130(
            rtx5090,
            Some("615.78.08"),
            Some("12.0")
        ));
        assert!(nvidia_supports_cu130(
            Some("NVIDIA GeForce RTX 2080"),
            Some("580.95.05"),
            Some("7.5")
        ));
        // Driver too old for CUDA 13.
        assert!(!nvidia_supports_cu130(
            rtx5090,
            Some("575.64.03"),
            Some("12.0")
        ));
        assert!(!nvidia_supports_cu130(rtx5090, None, Some("12.0")));
        // Pascal and Volta are not supported by CUDA 13.
        assert!(!nvidia_supports_cu130(
            Some("NVIDIA GeForce GTX 1080 Ti"),
            Some("580.95.05"),
            Some("6.1")
        ));
        assert!(!nvidia_supports_cu130(
            Some("Tesla V100-SXM2-16GB"),
            Some("580.95.05"),
            Some("7.0")
        ));
        // No compute capability reported: decide by name.
        assert!(nvidia_supports_cu130(
            Some("NVIDIA GeForce RTX 3060"),
            Some("581.57"),
            None
        ));
        assert!(nvidia_supports_cu130(
            Some("NVIDIA GeForce GTX 1660 SUPER"),
            Some("581.57"),
            None
        ));
        assert!(!nvidia_supports_cu130(
            Some("NVIDIA GeForce GTX 1080"),
            Some("581.57"),
            None
        ));
        assert!(!nvidia_supports_cu130(None, Some("581.57"), None));
    }

    /// Builds a unique per-test scratch directory path under the OS temp
    /// dir. Deliberately does *not* use `{:?}` on a `std::time::Instant` --
    /// its `Debug` output is platform-internal and unspecified, and on
    /// Windows CI it produces characters (braces, colons) that aren't legal
    /// in a path component, making every test that did this fail with
    /// `ERROR_INVALID_NAME`/`ERROR_DIRECTORY` before the directory was ever
    /// created. A nanosecond timestamp is digits-only and safe everywhere.
    fn unique_test_dir(label: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after the Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "arctic-shared-test-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn path_name_is_comfyui_matches_base_and_numbered_installs() {
        assert!(path_name_is_comfyui(Path::new("/base/ComfyUI")));
        assert!(path_name_is_comfyui(Path::new("/base/ComfyUI-01")));
        assert!(path_name_is_comfyui(Path::new("/base/comfyui-02")));
        assert!(!path_name_is_comfyui(Path::new("/base/OtherApp")));
        assert!(!path_name_is_comfyui(Path::new("/base/ComfyUIExtra")));
    }

    #[test]
    fn is_empty_dir_reports_directory_contents() {
        let dir = unique_test_dir("empty-dir");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(is_empty_dir(&dir));

        std::fs::write(dir.join("file.txt"), b"x").unwrap();
        assert!(!is_empty_dir(&dir));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn is_empty_dir_is_false_for_missing_path() {
        assert!(!is_empty_dir(Path::new(
            "/this/path/should/not/exist/arctic-test"
        )));
    }

    #[test]
    fn yaml_single_quote_escapes_embedded_quotes() {
        assert_eq!(yaml_single_quote("plain"), "'plain'");
        assert_eq!(yaml_single_quote("it's"), "'it''s'");
    }

    #[test]
    fn parse_yaml_scalar_unquotes_single_and_double_quoted_values() {
        assert_eq!(parse_yaml_scalar("'it''s fine'"), "it's fine");
        assert_eq!(parse_yaml_scalar(r#""say \"hi\"""#), "say \"hi\"");
        assert_eq!(parse_yaml_scalar("  bare value  "), "bare value");
    }

    #[test]
    fn parse_yaml_bool_accepts_common_spellings() {
        assert_eq!(parse_yaml_bool("TRUE"), Some(true));
        assert_eq!(parse_yaml_bool("yes"), Some(true));
        assert_eq!(parse_yaml_bool("On"), Some(true));
        assert_eq!(parse_yaml_bool("0"), Some(false));
        assert_eq!(parse_yaml_bool("maybe"), None);
    }

    #[test]
    fn parse_semver_triplet_tolerates_v_prefix_and_prerelease_suffix() {
        assert_eq!(parse_semver_triplet("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_semver_triplet("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_semver_triplet("V2.0.0-rc1"), Some((2, 0, 0)));
        assert_eq!(parse_semver_triplet("1.2"), None);
        assert_eq!(parse_semver_triplet("1.2.3.4"), None);
        assert_eq!(parse_semver_triplet("not-a-version"), None);
    }

    #[test]
    fn normalize_release_version_strips_prefix_and_suffix() {
        assert_eq!(
            normalize_release_version("v1.2.3-beta"),
            Some("1.2.3".to_string())
        );
        assert_eq!(normalize_release_version("garbage"), None);
    }

    #[test]
    fn parse_custom_launch_args_splits_on_whitespace_and_honors_quotes() {
        assert_eq!(
            parse_custom_launch_args(r#"--foo bar --name "hello world" --path='a b'"#).unwrap(),
            vec!["--foo", "bar", "--name", "hello world", "--path=a b"]
        );
    }

    #[test]
    fn parse_custom_launch_args_rejects_unclosed_quote() {
        assert!(parse_custom_launch_args(r#"--foo "unterminated"#).is_err());
    }

    #[test]
    fn parse_custom_launch_args_preserves_unquoted_windows_paths() {
        // Pasting a Windows path unquoted (the natural thing to do) must not
        // silently eat every backslash.
        assert_eq!(
            parse_custom_launch_args(r"--extra-model-paths-config C:\Users\name\file.yaml")
                .unwrap(),
            vec!["--extra-model-paths-config", r"C:\Users\name\file.yaml"]
        );
    }

    #[test]
    fn parse_custom_launch_args_still_honors_backslash_escapes_inside_quotes() {
        assert_eq!(
            parse_custom_launch_args(r#"--name "say \"hi\"""#).unwrap(),
            vec!["--name", r#"say "hi""#]
        );
    }

    #[test]
    fn parse_hf_env_value_finds_matching_key() {
        let text = "- endpoint: https://example.com\n- token: abc123\n";
        assert_eq!(
            parse_hf_env_value(text, "token"),
            Some("abc123".to_string())
        );
        assert_eq!(parse_hf_env_value(text, "missing"), None);
    }

    #[test]
    fn comfyui_instance_name_from_path_falls_back_to_default() {
        assert_eq!(
            comfyui_instance_name_from_path(Path::new("/base/ComfyUI-01")),
            "ComfyUI-01"
        );
        assert_eq!(comfyui_instance_name_from_path(Path::new("/")), "ComfyUI");
    }

    #[test]
    fn append_attention_launch_arg_maps_known_backends_only() {
        let mut args = Vec::new();
        append_attention_launch_arg(&mut args, Some("Flash"));
        append_attention_launch_arg(&mut args, Some("sage3"));
        append_attention_launch_arg(&mut args, Some("unknown"));
        append_attention_launch_arg(&mut args, None);
        assert_eq!(
            args,
            vec![
                "--use-flash-attention".to_string(),
                "--use-sage-attention".to_string()
            ]
        );
    }

    #[test]
    fn choose_install_folder_resumes_in_progress_install_before_scanning() {
        let dir = unique_test_dir("choose");
        std::fs::create_dir_all(&dir).unwrap();
        let existing = dir.join("ComfyUI-01");
        std::fs::create_dir_all(&existing).unwrap();
        write_install_state(&existing, "in_progress", "cloning");

        let chosen = choose_install_folder(&dir, false);
        assert_eq!(chosen, existing);

        let forced = choose_install_folder(&dir, true);
        assert_eq!(forced, dir.join("ComfyUI-02"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn custom_node_exists_checks_custom_nodes_subdir() {
        let dir = unique_test_dir("node");
        std::fs::create_dir_all(dir.join("custom_nodes").join("rgthree-comfy")).unwrap();

        assert!(custom_node_exists(&dir, "rgthree-comfy"));
        assert!(!custom_node_exists(&dir, "missing-node"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn push_preflight_appends_a_row_with_given_fields() {
        let mut items: Vec<PreflightItem> = Vec::new();
        push_preflight(&mut items, "warn", "Git", "not found in PATH");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].status, "warn");
        assert_eq!(items[0].title, "Git");
        assert_eq!(items[0].detail, "not found in PATH");
    }

    #[test]
    fn write_install_summary_round_trips_through_json() {
        let dir = unique_test_dir("summary");
        std::fs::create_dir_all(&dir).unwrap();

        let items = vec![InstallSummaryItem {
            name: "SageAttention".to_string(),
            status: "ok".to_string(),
            detail: "installed".to_string(),
        }];
        write_install_summary(&dir, &items);

        let raw = std::fs::read_to_string(dir.join("install-summary.json")).unwrap();
        let parsed: Vec<InstallSummaryItem> = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "SageAttention");
        assert_eq!(parsed[0].status, "ok");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn gpu_detail_caches_are_stable_and_independently_lockable() {
        // get_or_init should hand back the same static Mutex on every call...
        assert!(std::ptr::eq(
            amd_gpu_details_cache(),
            amd_gpu_details_cache()
        ));
        assert!(std::ptr::eq(
            intel_gpu_details_cache(),
            intel_gpu_details_cache()
        ));

        // ...and each GPU vendor's cache is independent of the others.
        {
            let mut guard = amd_gpu_details_cache().lock().unwrap();
            *guard = Some(AmdGpuDetails {
                name: Some("Test AMD GPU".to_string()),
            });
        }
        assert!(intel_gpu_details_cache().lock().unwrap().is_none());
        assert_eq!(
            amd_gpu_details_cache()
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|d| d.name.clone()),
            Some("Test AMD GPU".to_string())
        );
    }
}
