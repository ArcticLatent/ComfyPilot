//! Torch/Python environment utilities for the Linux backend: uv/pip
//! plumbing, attention-backend package cleanup, torch-profile
//! detection/enforcement (which CUDA/ROCm/XPU torch+triton wheel set is
//! installed or should be installed), CUDA runtime library-path discovery,
//! and the launch-arg/env assembly consumed when starting ComfyUI. This is
//! genuinely shared infrastructure -- `addons.rs`, `custom_nodes.rs`,
//! `install.rs`, `install_state.rs`, and `runtime.rs` all depend on pieces
//! of it via `use super::{...}`, which keeps working unchanged now that
//! `app_linux.rs` re-exports this module's public surface the same way it
//! re-exports every other slice's.
//!
//! Windows counterpart: `app_windows/torch_env.rs`. The two have
//! historically diverged more than most slices (different profile tables,
//! different DLL/library-path handling, different launch-arg sets) --
//! divergence notes live alongside each function below where relevant, and
//! in the roadmap doc.

use crate::shared::{
    append_attention_launch_arg, append_torch_overrides, apply_torch_allocator_env_compat,
    custom_node_exists, emit_install_event, existing_torch_overrides, remove_torch_overrides,
    same_base_version, sha256_file, torch_build, uv_release_asset, uv_release_url,
    write_torch_overrides, TORCH_STACK_PINS_PY, UV_VERSION,
};
use arctic_downloader::config::AppSettings;
// General command/env utilities, `pip_has_package`, and the
// `ComfyInstallRequest`/`ComfyInstallRecommendation` request/response types
// defined in the parent `app_linux` module itself (not `shared.rs`).
use super::{
    apply_background_command_flags, apply_python_tls_environment, command_available,
    download_http_file, get_comfyui_install_recommendation, pip_has_package, python_exe_for_root,
    python_for_root, run_command, run_command_capture, run_command_env, ComfyInstallRequest,
};
use crate::host::AppHandle;
use std::path::{Path, PathBuf};

pub(crate) fn pip_uninstall_best_effort(root: &Path, py_path: &str, packages: &[&str]) {
    let uv_bin = discover_uv_binary();
    for package in packages {
        if let Some(uv) = uv_bin.as_deref() {
            let _ = run_uv_pip_strict(uv, py_path, &["uninstall", package], Some(root), &[]);
        } else {
            let _ = run_command_capture(
                py_path,
                &["-m", "pip", "uninstall", "-y", package],
                Some(root),
            );
        }
    }
}

pub(crate) fn normalize_pkg_token(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

pub(crate) fn remove_site_packages_artifacts_with_markers(
    root: &Path,
    markers: &[String],
) -> Result<(), String> {
    for venv_name in [".venv", "venv"] {
        let venv_lib = root.join(venv_name).join("lib");
        let py_entries = match std::fs::read_dir(&venv_lib) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for py in py_entries.flatten() {
            let site = py.path().join("site-packages");
            let entries = match std::fs::read_dir(&site) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                let token = normalize_pkg_token(&name);
                if !markers.iter().any(|marker| token.contains(marker)) {
                    continue;
                }
                if path.is_dir() {
                    let _ = std::fs::remove_dir_all(&path);
                } else {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }
    Ok(())
}

fn remove_attention_site_packages_artifacts(root: &Path) -> Result<(), String> {
    let markers = vec![
        normalize_pkg_token("sageattention"),
        normalize_pkg_token("sageattn3"),
        normalize_pkg_token("flash_attn"),
        normalize_pkg_token("nunchaku"),
    ];
    remove_site_packages_artifacts_with_markers(root, &markers)
}

pub(crate) fn force_cleanup_attention_backends(root: &Path, py_path: &str) -> Result<(), String> {
    let pkg_names = [
        "sageattention",
        "sageattn3",
        "flash-attn",
        "flash_attn",
        "nunchaku",
    ];
    pip_uninstall_best_effort(root, py_path, &pkg_names);
    remove_attention_site_packages_artifacts(root)?;
    for folder in ["ComfyUI-nunchaku", "nunchaku_nodes"] {
        let path = root.join("custom_nodes").join(folder);
        if path.exists() {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    let mut lingering: Vec<&str> = Vec::new();
    for pkg in pkg_names {
        if pip_has_package(root, pkg) {
            lingering.push(pkg);
        }
    }
    let mut lingering_modules: Vec<&str> = Vec::new();
    for module in ["sageattention", "sageattn3", "flash_attn", "nunchaku"] {
        if python_module_importable(root, module) {
            lingering_modules.push(module);
        }
    }
    let mut lingering_nodes: Vec<&str> = Vec::new();
    for node in ["ComfyUI-nunchaku", "nunchaku_nodes"] {
        if custom_node_exists(root, node) {
            lingering_nodes.push(node);
        }
    }
    if lingering.is_empty() && lingering_modules.is_empty() && lingering_nodes.is_empty() {
        return Ok(());
    }

    let mut parts: Vec<String> = Vec::new();
    if !lingering.is_empty() {
        parts.push(format!(
            "packages still installed: {}",
            lingering.join(", ")
        ));
    }
    if !lingering_modules.is_empty() {
        parts.push(format!(
            "modules still importable: {}",
            lingering_modules.join(", ")
        ));
    }
    if !lingering_nodes.is_empty() {
        parts.push(format!(
            "nodes still present: {}",
            lingering_nodes.join(", ")
        ));
    }
    Err(format!(
        "Failed to fully remove previous attention backends ({}). Stop ComfyUI and retry.",
        parts.join("; ")
    ))
}

pub(crate) fn clone_or_update_repo(
    root: &Path,
    target_dir: &Path,
    repo_url: &str,
) -> Result<(), String> {
    if target_dir.join(".git").exists() {
        run_command(
            "git",
            &["-C", &target_dir.to_string_lossy(), "pull", "--ff-only"],
            Some(root),
        )
    } else if target_dir.exists() {
        Err(format!(
            "Path exists and is not a git repository: {}",
            target_dir.display()
        ))
    } else {
        run_command(
            "git",
            &[
                "clone",
                "--depth=1",
                repo_url,
                &target_dir.to_string_lossy(),
            ],
            Some(root),
        )
    }
}

pub(crate) fn run_uv_pip_strict(
    uv_bin: &str,
    python_target: &str,
    pip_args: &[&str],
    working_dir: Option<&Path>,
    envs: &[(&str, &str)],
) -> Result<(), String> {
    let mut uv_compatible_args: Vec<String> = Vec::new();
    let mut index = 0usize;
    while index < pip_args.len() {
        let arg = pip_args[index];
        if arg == "--timeout" || arg == "--retries" {
            index += 2;
            continue;
        }
        if arg.starts_with("--timeout=") || arg.starts_with("--retries=") {
            index += 1;
            continue;
        }
        match arg {
            "--force-reinstall" => uv_compatible_args.push("--reinstall".to_string()),
            "--no-cache-dir" => uv_compatible_args.push("--no-cache".to_string()),
            _ => uv_compatible_args.push(arg.to_string()),
        }
        index += 1;
    }

    append_torch_overrides(&mut uv_compatible_args, python_target);

    let mut args_owned: Vec<String> = vec!["pip".to_string()];
    if let Some((first, rest)) = uv_compatible_args.split_first() {
        args_owned.push(first.clone());
        args_owned.push("--python".to_string());
        args_owned.push(python_target.to_string());
        for arg in rest {
            args_owned.push(arg.clone());
        }
    } else {
        args_owned.push("--python".to_string());
        args_owned.push(python_target.to_string());
    }

    let args: Vec<&str> = args_owned.iter().map(String::as_str).collect();
    let mut merged_envs: Vec<(&str, &str)> = Vec::with_capacity(envs.len() + 1);
    merged_envs.push(("UV_LINK_MODE", "copy"));
    merged_envs.extend_from_slice(envs);
    run_command_env(uv_bin, &args, working_dir, &merged_envs)
}
pub(crate) fn profile_from_torch_env(root: &Path) -> Result<String, String> {
    let mut cmd = python_for_root(root);
    cmd.arg("-c").arg(
        "import torch; \
         v = getattr(torch, '__version__', ''); \
         c = getattr(torch.version, 'cuda', '') or getattr(torch.version, 'hip', '') or ('xpu' if hasattr(torch, 'xpu') else ''); \
         print(v); print(c)",
    );
    cmd.current_dir(root);
    let out = cmd
        .output()
        .map_err(|err| format!("Failed to detect installed torch profile: {err}"))?;
    if !out.status.success() {
        return Err("Failed to detect installed torch profile.".to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let torch_v = lines.next().unwrap_or_default().to_ascii_lowercase();
    let cuda_v = lines.next().unwrap_or_default().to_ascii_lowercase();

    if let Some(profile) = torch_profile_from_versions(&torch_v, &cuda_v) {
        return Ok(profile);
    }

    Err(format!(
        "Unsupported installed torch runtime combo: torch={torch_v}, runtime={cuda_v}"
    ))
}

/// Where the app keeps its own uv, matching the `comfyui-runtime` folder
/// under the config store's cache directory.
fn local_uv_dir() -> Option<PathBuf> {
    arctic_downloader::config::default_cache_dir()
        .map(|cache| cache.join("comfyui-runtime").join(".tools").join("uv"))
}

fn working_uv(candidate: &Path) -> Option<String> {
    (candidate.is_file()
        && run_command_capture(&candidate.to_string_lossy(), &["--version"], None).is_ok())
    .then(|| candidate.to_string_lossy().to_string())
}

pub(crate) fn discover_uv_binary() -> Option<String> {
    if command_available("uv", &["--version"]) {
        return Some("uv".to_string());
    }

    let mut candidates: Vec<PathBuf> = local_uv_dir()
        .map(|dir| dir.join("uv"))
        .into_iter()
        .collect();
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(PathBuf::from(&home).join(".local").join("bin").join("uv"));
        candidates.push(PathBuf::from(&home).join(".cargo").join("bin").join("uv"));
    }
    candidates
        .iter()
        .find_map(|candidate| working_uv(candidate))
}

/// uv from PATH or the usual install locations; otherwise downloads the
/// pinned uv release into `<shared_runtime_root>/.tools/uv` and checks it
/// against the compiled-in SHA-256.
pub(crate) fn resolve_uv_binary(
    shared_runtime_root: &Path,
    app: &AppHandle,
) -> Result<String, String> {
    if let Some(found) = discover_uv_binary() {
        return Ok(found);
    }
    let local_root = shared_runtime_root.join(".tools").join("uv");
    if let Some(found) = working_uv(&local_root.join("uv")) {
        return Ok(found);
    }

    let (asset, expected) = uv_release_asset().ok_or_else(|| {
        format!(
            "uv was not found and no uv download is known for {}. Install uv (https://docs.astral.sh/uv/) and retry.",
            std::env::consts::ARCH
        )
    })?;
    emit_install_event(
        app,
        "step",
        &format!("uv not found. Downloading uv {UV_VERSION}..."),
    );
    std::fs::create_dir_all(&local_root).map_err(|err| err.to_string())?;
    let archive = local_root.join(asset);
    download_http_file(&uv_release_url(asset), &archive)?;
    let actual = sha256_file(&archive)?;
    if actual != expected {
        let _ = std::fs::remove_file(&archive);
        return Err(format!(
            "uv download checksum mismatch (expected {expected}, got {actual})."
        ));
    }
    // The archive holds `uv-<target>/uv` and `uv-<target>/uvx`.
    run_command(
        "tar",
        &[
            "-xzf",
            &archive.to_string_lossy(),
            "--strip-components=1",
            "-C",
            &local_root.to_string_lossy(),
        ],
        None,
    )?;
    let _ = std::fs::remove_file(&archive);
    working_uv(&local_root.join("uv")).ok_or_else(|| {
        format!(
            "uv was downloaded but {} does not run.",
            local_root.join("uv").display()
        )
    })
}

pub(crate) fn torch_profile_to_packages_linux(
    profile: &str,
) -> (&'static str, &'static str, &'static str, &'static str) {
    match profile {
        "torch271_cu128" => (
            "2.7.1",
            "0.22.1",
            "2.7.1",
            "https://download.pytorch.org/whl/cu128",
        ),
        "torch291_rocm64" => (
            "2.9.1",
            "0.24.1",
            "2.9.1",
            "https://download.pytorch.org/whl/rocm6.4",
        ),
        "torch211_rocm72" => (
            "2.11.0",
            "0.26.0",
            "2.11.0",
            "https://download.pytorch.org/whl/rocm7.2",
        ),
        // torchaudio is versioned separately from torch since 2.12.
        "torch2141_rocm72" => (
            "2.14.1",
            "0.29.1",
            "2.11.0",
            "https://download.pytorch.org/whl/rocm7.2",
        ),
        "torch2141_xpu" => (
            "2.14.1",
            "0.29.1",
            "2.11.0",
            "https://download.pytorch.org/whl/xpu",
        ),
        "torch291_xpu" => (
            "2.9.1",
            "0.24.1",
            "2.9.1",
            "https://download.pytorch.org/whl/xpu",
        ),
        "torch291_cu130" => (
            "2.9.1",
            "0.24.1",
            "2.9.1",
            "https://download.pytorch.org/whl/cu130",
        ),
        "torch2141_cu130" => (
            "2.14.1",
            "0.29.1",
            "2.11.0",
            "https://download.pytorch.org/whl/cu130",
        ),
        _ => (
            "2.8.0",
            "0.23.0",
            "2.8.0",
            "https://download.pytorch.org/whl/cu128",
        ),
    }
}

pub(crate) fn torch_profile_from_versions(torch_v: &str, cuda_v: &str) -> Option<String> {
    let t = torch_v.trim().to_ascii_lowercase();
    let c = cuda_v.trim().to_ascii_lowercase();
    if t.starts_with("2.7") && c.starts_with("12.8") {
        return Some("torch271_cu128".to_string());
    }
    if t.starts_with("2.8") && c.starts_with("12.8") {
        return Some("torch280_cu128".to_string());
    }
    if t.starts_with("2.9") && c.starts_with("6.4") {
        return Some("torch291_rocm64".to_string());
    }
    if t.starts_with("2.11") && c.starts_with("7.2") {
        return Some("torch211_rocm72".to_string());
    }
    if t.starts_with("2.14") && c.starts_with("7.2") {
        return Some("torch2141_rocm72".to_string());
    }
    if t.starts_with("2.9") && c == "xpu" {
        return Some("torch291_xpu".to_string());
    }
    if t.starts_with("2.14") && c == "xpu" {
        return Some("torch2141_xpu".to_string());
    }
    if t.starts_with("2.9") && c.starts_with("13.0") {
        return Some("torch291_cu130".to_string());
    }
    if t.starts_with("2.14") && c.starts_with("13.0") {
        return Some("torch2141_cu130".to_string());
    }
    None
}

pub(crate) fn triton_package_for_profile_linux(profile: &str) -> &'static str {
    match profile {
        "torch271_cu128" => "triton==3.3.1",
        "torch291_cu130" => "triton<3.6",
        // The version torch 2.14.1 itself requires.
        "torch2141_cu130" => "triton==3.8.0",
        _ => "triton==3.4.0",
    }
}

/// The Triton build to install over torch 2.9.1's XPU stack. Newer XPU torch
/// wheels depend on their matching `triton-xpu` themselves.
fn triton_package_spec_for_xpu_linux(profile: &str) -> Option<&'static str> {
    match profile {
        "torch291_xpu" => Some("https://download.pytorch.org/whl/triton_xpu-3.6.0-cp312-cp312-manylinux_2_27_x86_64.manylinux_2_28_x86_64.whl"),
        _ => None,
    }
}

pub(crate) fn torch_profile_is_rocm(profile: &str) -> bool {
    profile.contains("_rocm")
}

pub(crate) fn torch_profile_is_xpu(profile: &str) -> bool {
    profile.contains("_xpu")
}

/// Installs `profile`'s torch stack into `py_path`'s environment, verifies
/// it and pins it (see `shared::write_torch_overrides`). A stack that
/// already matches the profile is only pinned, so calling this after
/// requirements installs costs one check instead of a reinstall.
pub(crate) fn enforce_torch_profile_linux(
    uv_bin: &str,
    py_path: &str,
    root: &Path,
    profile: &str,
    uv_python_install_dir: &str,
) -> Result<(), String> {
    let (torch_v, tv_v, ta_v, index_url) = torch_profile_to_packages_linux(profile);
    let py_exe = Path::new(py_path);
    if let Ok(installed) = verify_torch_profile_linux(py_path, root, profile) {
        if same_base_version(&installed.torch, torch_v)
            && same_base_version(&installed.torchvision, tv_v)
            && same_base_version(&installed.torchaudio, ta_v)
        {
            // Installs made before pinning existed get pinned here.
            if existing_torch_overrides(py_path).is_none() {
                let pins = run_python_capture(py_path, root, TORCH_STACK_PINS_PY)?;
                write_torch_overrides(py_exe, &pins)?;
            }
            return Ok(());
        }
    }
    // The old pins would override the versions requested below.
    remove_torch_overrides(py_exe);
    run_uv_pip_strict(
        uv_bin,
        py_path,
        &[
            "install",
            "--upgrade",
            "--reinstall",
            &format!("torch=={torch_v}"),
            &format!("torchvision=={tv_v}"),
            &format!("torchaudio=={ta_v}"),
            "--index-url",
            index_url,
        ],
        Some(root),
        &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
    )?;
    if torch_profile_is_xpu(profile) {
        if let Some(triton) = triton_package_spec_for_xpu_linux(profile) {
            run_uv_pip_strict(
                uv_bin,
                py_path,
                &["install", "--upgrade", "--reinstall", triton],
                Some(root),
                &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
            )?;
        }
    } else if !torch_profile_is_rocm(profile) {
        run_uv_pip_strict(
            uv_bin,
            py_path,
            &[
                "install",
                "--upgrade",
                "--reinstall",
                triton_package_for_profile_linux(profile),
            ],
            Some(root),
            &[("UV_PYTHON_INSTALL_DIR", uv_python_install_dir)],
        )?;
    }
    verify_torch_profile_linux(py_path, root, profile)?;
    let pins = run_python_capture(py_path, root, TORCH_STACK_PINS_PY)?;
    write_torch_overrides(py_exe, &pins)
}

/// Installed torch stack versions.
struct InstalledTorchStack {
    torch: String,
    torchvision: String,
    torchaudio: String,
}

/// Checks that `py_path`'s torch build belongs to `profile`.
fn verify_torch_profile_linux(
    py_path: &str,
    root: &Path,
    profile: &str,
) -> Result<InstalledTorchStack, String> {
    let text = run_python_capture(
        py_path,
        root,
        "import torch, importlib.metadata as m; \
         print(getattr(torch, '__version__', '')); \
         print(getattr(torch.version, 'cuda', '') or getattr(torch.version, 'hip', '') or ('xpu' if hasattr(torch, 'xpu') else '')); \
         print(m.version('torchvision')); \
         print(m.version('torchaudio'))",
    )
    .map_err(|err| format!("Torch profile verification failed: {err}"))?;
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let installed_torch = lines.next().unwrap_or_default();
    let installed_cuda = lines.next().unwrap_or_default();
    let installed_tv = lines.next().unwrap_or_default();
    let installed_ta = lines.next().unwrap_or_default();
    let actual_profile = torch_profile_from_versions(installed_torch, installed_cuda);
    if actual_profile.as_deref() != Some(profile) {
        return Err(format!(
            "Torch profile enforce mismatch for {profile}: got torch={installed_torch}, cuda={installed_cuda}, torchvision={installed_tv}, torchaudio={installed_ta}"
        ));
    }
    Ok(InstalledTorchStack {
        torch: installed_torch.to_string(),
        torchvision: installed_tv.to_string(),
        torchaudio: installed_ta.to_string(),
    })
}

/// Runs a Python snippet with `py_path` and returns its standard output.
fn run_python_capture(py_path: &str, root: &Path, code: &str) -> Result<String, String> {
    let mut cmd = std::process::Command::new(py_path);
    apply_python_tls_environment(&mut cmd);
    cmd.arg("-c").arg(code);
    cmd.current_dir(root);
    apply_background_command_flags(&mut cmd);
    apply_torch_allocator_env_compat(&mut cmd);
    let output = cmd
        .output()
        .map_err(|err| format!("Failed to run {py_path}: {err}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn infer_torch_profile_from_installed_packages(root: &Path) -> Option<String> {
    let mut cmd = python_for_root(root);
    cmd.arg("-c").arg(
        "import torch; \
         c = getattr(torch.version, 'cuda', '') or getattr(torch.version, 'hip', '') or ('xpu' if hasattr(torch, 'xpu') else ''); \
         print(torch.__version__); print(c)",
    );
    cmd.current_dir(root);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let torch_v = lines.next().unwrap_or_default().to_ascii_lowercase();
    let cuda_v = lines.next().unwrap_or_default().to_ascii_lowercase();
    if torch_v.starts_with("2.7") && cuda_v.starts_with("12.8") {
        return Some("torch271_cu128".to_string());
    }
    if torch_v.starts_with("2.8") && cuda_v.starts_with("12.8") {
        return Some("torch280_cu128".to_string());
    }
    if torch_v.starts_with("2.9") && cuda_v.starts_with("6.4") {
        return Some("torch291_rocm64".to_string());
    }
    if torch_v.starts_with("2.11") && cuda_v.starts_with("7.2") {
        return Some("torch211_rocm72".to_string());
    }
    if torch_v.starts_with("2.14") && cuda_v.starts_with("7.2") {
        return Some("torch2141_rocm72".to_string());
    }
    if torch_v.starts_with("2.9") && cuda_v == "xpu" {
        return Some("torch291_xpu".to_string());
    }
    if torch_v.starts_with("2.14") && cuda_v == "xpu" {
        return Some("torch2141_xpu".to_string());
    }
    if torch_v.starts_with("2.9") && cuda_v.starts_with("13.0") {
        return Some("torch291_cu130".to_string());
    }
    if torch_v.starts_with("2.14") && cuda_v.starts_with("13.0") {
        return Some("torch2141_cu130".to_string());
    }
    None
}

/// The profile of the torch build installed for `root`, read from
/// `torch/version.py` when possible (no Python start), else by asking torch.
pub(crate) fn detect_torch_profile_for_root(root: &Path) -> Option<String> {
    let from_version_file = python_exe_for_root(root)
        .ok()
        .and_then(|py| torch_build(&py))
        .and_then(|build| {
            let accelerator = build
                .cuda
                .or(build.hip)
                .or_else(|| {
                    (build.xpu.is_some() || build.version.contains("+xpu"))
                        .then(|| "xpu".to_string())
                })
                .unwrap_or_default();
            torch_profile_from_versions(&build.version, &accelerator)
        });
    if from_version_file.is_some() {
        return from_version_file;
    }
    profile_from_torch_env(root)
        .or_else(|_| {
            infer_torch_profile_from_installed_packages(root)
                .ok_or_else(|| "no profile hint".to_string())
        })
        .ok()
}

pub(crate) fn resolve_desired_torch_profile(settings: &AppSettings, root: &Path) -> String {
    detect_torch_profile_for_root(root)
        .ok_or_else(|| "no profile hint".to_string())
        .or_else(|_| {
            settings
                .comfyui_torch_profile
                .clone()
                .ok_or_else(|| "no saved profile".to_string())
        })
        .unwrap_or_else(|_| get_comfyui_install_recommendation(None).torch_profile)
}

pub(crate) fn selected_attention_backend(request: &ComfyInstallRequest) -> &'static str {
    if request.include_flash_attention {
        "flash"
    } else if request.include_sage_attention3 {
        "sage3"
    } else if request.include_sage_attention {
        "sage"
    } else if request.include_nunchaku {
        "nunchaku"
    } else {
        "none"
    }
}

pub(crate) fn detect_launch_attention_backend_for_root(root: &Path) -> Option<String> {
    if python_module_importable(root, "flash_attn") {
        return Some("flash".to_string());
    }
    if python_module_importable(root, "sageattn3") {
        return Some("sage3".to_string());
    }
    if python_module_importable(root, "sageattention") {
        return Some("sage".to_string());
    }
    let has_nunchaku = python_module_importable(root, "nunchaku")
        || custom_node_exists(root, "nunchaku_nodes")
        || custom_node_exists(root, "ComfyUI-nunchaku")
        || pip_has_package(root, "nunchaku");
    if has_nunchaku {
        return Some("nunchaku".to_string());
    }
    None
}

pub(crate) fn nunchaku_backend_present(root: &Path) -> bool {
    python_module_importable(root, "nunchaku")
        || pip_has_package(root, "nunchaku")
        || custom_node_exists(root, "nunchaku_nodes")
        || custom_node_exists(root, "ComfyUI-nunchaku")
}

fn collect_cuda_runtime_library_paths(root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut push_unique = |p: PathBuf| {
        if p.exists() && !dirs.iter().any(|d| d == &p) {
            dirs.push(p);
        }
    };

    for sys in [
        // NixOS exposes the active host GPU driver here. CUDA-enabled
        // PyTorch wheels otherwise load their bundled CUDA runtime but fail
        // to find the driver's libcuda.so.1 entry point.
        "/run/opengl-driver/lib",
        "/run/opengl-driver-32/lib",
        "/opt/cuda/lib64",
        "/usr/local/cuda/lib64",
        "/usr/lib/wsl/lib",
    ] {
        push_unique(PathBuf::from(sys));
    }

    for env_key in ["CUDA_PATH", "CUDA_HOME"] {
        if let Some(base) = std::env::var_os(env_key) {
            push_unique(PathBuf::from(base).join("lib64"));
        }
    }

    for venv_name in [".venv", "venv"] {
        let venv_lib = root.join(venv_name).join("lib");
        let py_dirs = std::fs::read_dir(&venv_lib)
            .ok()
            .into_iter()
            .flat_map(|iter| iter.flatten().map(|e| e.path()).collect::<Vec<_>>());
        for py_dir in py_dirs {
            let site = py_dir.join("site-packages").join("nvidia");
            for pkg in ["cuda_runtime", "cublas", "cudnn", "cusolver", "cusparse"] {
                push_unique(site.join(pkg).join("lib"));
            }
        }
    }
    dirs
}

fn apply_cuda_runtime_env_for_root(cmd: &mut std::process::Command, root: &Path) {
    let mut paths = collect_cuda_runtime_library_paths(root);
    if paths.is_empty() {
        return;
    }
    if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
        for p in std::env::split_paths(&existing) {
            if !paths.iter().any(|d| d == &p) {
                paths.push(p);
            }
        }
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        cmd.env("LD_LIBRARY_PATH", joined);
    }
}

pub(crate) fn python_runtime_env_for_root(root: &Path) -> Vec<(String, String)> {
    let mut envs: Vec<(String, String)> = Vec::new();
    let mpl_cache = root.join(".venv").join("var").join("matplotlib");
    let _ = std::fs::create_dir_all(&mpl_cache);
    let mut paths = collect_cuda_runtime_library_paths(root);
    if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
        for p in std::env::split_paths(&existing) {
            if !paths.iter().any(|d| d == &p) {
                paths.push(p);
            }
        }
    }
    if !paths.is_empty() {
        if let Ok(joined) = std::env::join_paths(paths) {
            envs.push((
                "LD_LIBRARY_PATH".to_string(),
                joined.to_string_lossy().to_string(),
            ));
        }
    }

    if let Ok(value) = std::env::var("PYTORCH_CUDA_ALLOC_CONF") {
        if std::env::var_os("PYTORCH_ALLOC_CONF").is_none() {
            envs.push(("PYTORCH_ALLOC_CONF".to_string(), value));
        }
    }
    envs.push(("MPLBACKEND".to_string(), "Agg".to_string()));
    envs.push((
        "MPLCONFIGDIR".to_string(),
        mpl_cache.to_string_lossy().to_string(),
    ));
    envs
}

fn configure_python_runtime_env_for_root(cmd: &mut std::process::Command, root: &Path) {
    let mpl_cache = root.join(".venv").join("var").join("matplotlib");
    let _ = std::fs::create_dir_all(&mpl_cache);
    apply_torch_allocator_env_compat(cmd);
    cmd.env("MPLBACKEND", "Agg");
    cmd.env("MPLCONFIGDIR", mpl_cache.to_string_lossy().to_string());
}

pub(crate) fn python_module_importable(root: &Path, module: &str) -> bool {
    let mut cmd = python_for_root(root);
    cmd.arg("-c").arg(format!("import {module}"));
    cmd.current_dir(root);
    apply_cuda_runtime_env_for_root(&mut cmd, root);
    configure_python_runtime_env_for_root(&mut cmd, root);
    cmd.output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn comfyui_launch_args(
    listen_enabled: bool,
    pinned_memory_enabled: bool,
    attention_backend: Option<&str>,
    lowvram_enabled: bool,
    bf16_unet_enabled: bool,
    async_offload_enabled: bool,
    disable_smart_memory_enabled: bool,
    custom_launch_args: &[String],
) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if listen_enabled {
        args.push("--listen".to_string());
    }
    if lowvram_enabled {
        args.push("--lowvram".to_string());
    }
    if bf16_unet_enabled {
        args.push("--bf16-unet".to_string());
    }
    if async_offload_enabled {
        args.push("--async-offload".to_string());
    }
    if disable_smart_memory_enabled {
        args.push("--disable-smart-memory".to_string());
    }
    if !pinned_memory_enabled {
        args.push("--disable-pinned-memory".to_string());
    }
    append_attention_launch_arg(&mut args, attention_backend);
    args.extend(custom_launch_args.iter().cloned());
    args
}
