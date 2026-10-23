// Platform backend composed from focused sibling modules; see
// docs/cross-platform-development.md ("Consolidation status").
mod addons;
pub(crate) mod commands;
mod custom_nodes;
mod gpu_detection;
mod install;
mod install_state;
mod runtime;
mod torch_env;
// `start_comfyui_install`/`apply_comfyui_component_toggle` are
// `#[tauri::command]`s with no other caller in this file, so they're
// referenced by qualified path directly in `generate_handler!` in `run()`
// below -- see `install_state.rs`'s doc comment for why.
pub(crate) use runtime::{
    git_latest_release_tag, host_comfyui_root_among, kill_python_processes_for_root,
    python_exe_for_root, python_for_root, resolve_root_path, restart_comfyui_after_mutation,
    spawn_comfyui_start_monitor, start_comfyui_root_impl, stop_comfyui_root_impl,
};
// `set_comfyui_install_base`/`list_comfyui_installations`/`get_comfyui_addon_state`
// are `#[tauri::command]`s with no other caller in this file (confirmed: each
// name appears exactly once, its own definition), so they're referenced by
// qualified path directly in `generate_handler!` in `run()` below rather than
// re-exported here -- see `install_state.rs`'s doc comment for why a bare-name
// `pub(crate) use` wouldn't work for a `#[tauri::command]` anyway.
pub(crate) use crate::shared::manager::{
    comfyui_manager_installed, install_comfyui_manager, install_named_custom_node,
    uninstall_comfyui_manager,
};
pub(crate) use crate::shared::tray::update_tray_comfy_status;
pub(crate) use addons::{
    install_flashattention_linux, install_insightface, install_linux_wheel_for_profile,
    install_nunchaku_node_requirements, install_sageattention_linux, install_trellis2,
    uninstall_insightface, uninstall_trellis2,
};
pub(crate) use custom_nodes::{custom_node_spec, install_custom_node, CUSTOM_NODES};
pub(crate) use gpu_detection::{
    detect_amd_gpu_details, detect_intel_gpu_details, detect_nvidia_gpu_details,
    fake_amd_allow_rocm_setup_enabled, fake_intel_allow_xpu_setup_enabled, gpu_detection_pending,
    is_nvidia_hopper_sm90, NvidiaGpuDetails,
};
pub(crate) use torch_env::{
    clone_or_update_repo, comfyui_launch_args, detect_launch_attention_backend_for_root,
    detect_torch_profile_for_root, discover_uv_binary, enforce_torch_profile_linux,
    force_cleanup_attention_backends, normalize_pkg_token, nunchaku_backend_present,
    pip_uninstall_best_effort, profile_from_torch_env, python_module_importable,
    python_runtime_env_for_root, remove_site_packages_artifacts_with_markers,
    resolve_desired_torch_profile, resolve_uv_binary, run_uv_pip_strict,
    selected_attention_backend, torch_profile_is_rocm, torch_profile_is_xpu,
    triton_package_for_profile_linux,
};

use crate::contracts::{
    AppSnapshot, AttentionBackendChangeRequest, ComfyInstallRecommendation, ComfyInstallRequest,
    ComfyPathInspection, ComfyPreflightResponse, HfXetPreflightResponse,
    LaunchAttentionFlagRequest, PreflightItem, UpdateCheckResponse,
};
use crate::host::{AppHandle, State};
use crate::shared::{
    builtin_manager_installed, builtin_manager_requirements, custom_node_exists,
    detect_amd_gpu_name, detect_existing_comfyui_root, detect_intel_gpu_name, detect_nvidia_gpu,
    fake_amd_enabled, fake_intel_enabled, git_current_branch, has_dns, nerdstats_enabled,
    normalize_release_version, output_with_optional_timeout, parse_hf_env_value,
    parse_semver_triplet, parse_yaml_bool, parse_yaml_scalar, push_preflight,
    read_comfyui_installed_version, recover_lock, run_with_timeout_capturing_output,
    status_with_optional_timeout, stop_comfyui_for_mutation, yaml_single_quote, AppState,
    ComfyExtraModelConfig, DownloadProgressEvent,
};
use arctic_downloader::{
    app::build_context,
    config::AppSettings,
    env_flags::{auto_update_enabled, external_package_manager},
    ram::detect_ram_profile,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    io::BufRead,
    io::IsTerminal,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Mutex, OnceLock},
    time::Duration,
};

#[derive(Debug, Serialize)]
struct RocmGuidedStatus {
    distro_family: String,
    distro_label: String,
    supported: bool,
    amd_detected: bool,
    gpu_name: Option<String>,
    ready: bool,
    requires_relogin: bool,
    detail: String,
}

#[derive(Debug, Serialize)]
struct XpuGuidedStatus {
    distro_family: String,
    distro_label: String,
    supported: bool,
    intel_detected: bool,
    gpu_name: Option<String>,
    ready: bool,
    requires_relogin: bool,
    detail: String,
}

#[derive(Clone, Debug)]
struct LinuxPrereqScan {
    distro: String,
    missing_required: Vec<String>,
    missing_optional: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct LinuxOsRelease {
    id: String,
    id_like: String,
    version_id: String,
    version_codename: String,
    ubuntu_codename: String,
    pretty_name: String,
}

const UV_PYTHON_VERSION: &str = "3.12.10";

fn get_app_snapshot(state: State<'_, AppState>) -> AppSnapshot {
    let catalog = state.context.catalog.catalog_snapshot();
    let (mut nvidia_gpu_name, mut nvidia_gpu_vram_mb) = detect_nvidia_gpu();
    let mut amd_gpu_name = detect_amd_gpu_name();
    let mut intel_gpu_name = detect_intel_gpu_name();
    let gpu_detection_pending = gpu_detection_pending();
    // A fast lspci probe may finish between the calls above. Re-read completed
    // caches so one snapshot cannot preserve a stale AMD-only view.
    if !gpu_detection_pending {
        if nvidia_gpu_name.is_none() {
            (nvidia_gpu_name, nvidia_gpu_vram_mb) = detect_nvidia_gpu();
        }
        if amd_gpu_name.is_none() {
            amd_gpu_name = detect_amd_gpu_name();
        }
        if intel_gpu_name.is_none() {
            intel_gpu_name = detect_intel_gpu_name();
        }
    }
    let ram_profile = state.context.ram_profile.or_else(detect_ram_profile);
    AppSnapshot {
        version: state.context.display_version.clone(),
        total_ram_gb: ram_profile.map(|profile| profile.total_gb),
        ram_tier: ram_profile.map(|profile| profile.tier.label().to_string()),
        nvidia_gpu_name,
        nvidia_gpu_vram_mb,
        amd_gpu_name,
        intel_gpu_name,
        gpu_detection_pending,
        model_count: catalog.models.len(),
        lora_count: catalog.loras.len(),
        settings_notice: state.context.config.startup_notice(),
    }
}

const COMMAND_PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const HF_ENV_PROBE_TIMEOUT: Duration = Duration::from_secs(30);
static LINUX_PREREQ_CACHE: OnceLock<Mutex<Option<LinuxPrereqScan>>> = OnceLock::new();

fn linux_prereq_cache() -> &'static Mutex<Option<LinuxPrereqScan>> {
    LINUX_PREREQ_CACHE.get_or_init(|| Mutex::new(None))
}

fn detect_linux_distro_family() -> String {
    let os_release = detect_linux_os_release();
    let id = os_release.id;
    let id_like = os_release.id_like;
    let haystack = format!("{id} {id_like}");
    if haystack.contains("nixos") {
        "nixos".to_string()
    } else if haystack.contains("arch") {
        "arch".to_string()
    } else if haystack.contains("debian") || haystack.contains("ubuntu") {
        "debian".to_string()
    } else if haystack.contains("fedora")
        || haystack.contains("rhel")
        || haystack.contains("centos")
    {
        "fedora".to_string()
    } else {
        "unknown".to_string()
    }
}

fn detect_linux_os_release() -> LinuxOsRelease {
    #[cfg(not(target_os = "linux"))]
    {
        LinuxOsRelease::default()
    }

    #[cfg(target_os = "linux")]
    {
        let os_release = if running_in_flatpak() {
            run_command_capture("cat", &["/etc/os-release"], None)
                .map(|(stdout, _)| stdout)
                .unwrap_or_default()
        } else {
            std::fs::read_to_string("/etc/os-release").unwrap_or_default()
        };
        let mut info = LinuxOsRelease::default();
        for line in os_release.lines() {
            if let Some(value) = line.strip_prefix("ID=") {
                info.id = value.trim_matches('"').to_ascii_lowercase();
            } else if let Some(value) = line.strip_prefix("ID_LIKE=") {
                info.id_like = value.trim_matches('"').to_ascii_lowercase();
            } else if let Some(value) = line.strip_prefix("VERSION_ID=") {
                info.version_id = value.trim_matches('"').to_ascii_lowercase();
            } else if let Some(value) = line.strip_prefix("VERSION_CODENAME=") {
                info.version_codename = value.trim_matches('"').to_ascii_lowercase();
            } else if let Some(value) = line.strip_prefix("UBUNTU_CODENAME=") {
                info.ubuntu_codename = value.trim_matches('"').to_ascii_lowercase();
            } else if let Some(value) = line.strip_prefix("PRETTY_NAME=") {
                info.pretty_name = value.trim_matches('"').to_string();
            }
        }
        info
    }
}

fn linux_package_sets(distro: &str) -> (Vec<&'static str>, Vec<&'static str>) {
    match distro {
        "nixos" => (
            vec![
                "git", "curl", "wget", "python3", "gcc", "make", "cmake", "ninja",
            ],
            Vec::new(),
        ),
        "arch" => (
            vec![
                "git",
                "curl",
                "wget",
                "python",
                "base-devel",
                "cmake",
                "ninja",
            ],
            vec!["libglvnd", "mesa"],
        ),
        "debian" => (
            vec![
                "git",
                "curl",
                "wget",
                "python3",
                "build-essential",
                "cmake",
                "ninja-build",
            ],
            vec!["libgl1"],
        ),
        "fedora" => (
            vec![
                "git",
                "curl",
                "wget",
                "python3",
                "gcc",
                "gcc-c++",
                "make",
                "cmake",
                "ninja-build",
            ],
            vec!["mesa-libGL"],
        ),
        _ => (vec!["git", "curl", "wget", "python3"], Vec::new()),
    }
}

fn linux_package_installed(distro: &str, package: &str) -> bool {
    if package == "wget" && command_available("wget", &["--version"]) {
        return true;
    }
    let probe = match distro {
        "nixos" => {
            let args: &[&str] = match package {
                "git" => &["--version"],
                "curl" => &["--version"],
                "wget" => &["--version"],
                "python3" => &["--version"],
                "gcc" => &["--version"],
                "make" => &["--version"],
                "cmake" => &["--version"],
                "ninja" => &["--version"],
                _ => return false,
            };
            return command_available(package, args);
        }
        "arch" => run_command_capture("pacman", &["-Q", package], None),
        "debian" => run_command_capture("dpkg", &["-s", package], None),
        "fedora" => run_command_capture("rpm", &["-q", package], None),
        _ => {
            // Distro not recognized (openSUSE, Alpine, Void, Gentoo, ...):
            // we don't know that distro's package-query tool or package
            // names, but `linux_package_sets`'s wildcard arm only ever asks
            // about bare command names (git/curl/wget/python3) for this
            // branch, so check PATH directly instead of assuming everything
            // is already installed.
            return command_available(package, &["--version"]);
        }
    };
    probe.is_ok()
}

fn scan_linux_prereqs() -> Result<LinuxPrereqScan, String> {
    let distro = detect_linux_distro_family();
    let (required, optional) = linux_package_sets(&distro);
    let missing_required = required
        .into_iter()
        .filter(|pkg| !linux_package_installed(&distro, pkg))
        .map(str::to_string)
        .collect::<Vec<_>>();
    let missing_optional = optional
        .into_iter()
        .filter(|pkg| !linux_package_installed(&distro, pkg))
        .map(str::to_string)
        .collect::<Vec<_>>();
    Ok(LinuxPrereqScan {
        distro,
        missing_required,
        missing_optional,
    })
}

fn get_linux_prereq_cache_or_scan() -> Result<LinuxPrereqScan, String> {
    if let Ok(cache) = linux_prereq_cache().lock() {
        if let Some(cached) = cache.clone() {
            return Ok(cached);
        }
    }
    refresh_linux_prereq_cache()
}

fn refresh_linux_prereq_cache() -> Result<LinuxPrereqScan, String> {
    let scan = scan_linux_prereqs()?;
    if let Ok(mut cache) = linux_prereq_cache().lock() {
        *cache = Some(scan.clone());
    }
    Ok(scan)
}

fn warm_linux_prereq_cache_background() {
    std::thread::spawn(|| {
        let _ = refresh_linux_prereq_cache();
    });
}

fn install_missing_linux_prereqs(scan: &LinuxPrereqScan) -> Result<(), String> {
    if scan.missing_required.is_empty() {
        return Ok(());
    }
    let mut package_args: Vec<&str> = scan.missing_required.iter().map(String::as_str).collect();
    match scan.distro.as_str() {
        "arch" => {
            run_privileged_command("pacman", &["-Sy"], None)?;
            let mut args = vec!["-S", "--needed", "--noconfirm"];
            args.append(&mut package_args);
            run_privileged_command("pacman", &args, None)?;
        }
        "debian" => {
            run_privileged_command("apt", &["update"], None)?;
            let mut args = vec!["install", "-y"];
            args.append(&mut package_args);
            run_privileged_command("apt", &args, None)?;
        }
        "fedora" => {
            run_privileged_command("dnf", &["makecache"], None)?;
            let mut args = vec!["install", "-y"];
            args.append(&mut package_args);
            run_privileged_command("dnf", &args, None)?;
        }
        _ => {
            if scan.distro == "nixos" {
                return Err(
                    "This NixOS package supplies the required tools through its wrapper. Reinstall or update the ComfyPilot Nix package instead of installing system packages imperatively."
                        .to_string(),
                );
            }
            return Err(
                "Unsupported Linux distribution for automatic package install. Install required packages manually."
                    .to_string(),
            );
        }
    }
    Ok(())
}

fn user_in_group(group: &str) -> bool {
    let stdout = run_command_capture("id", &["-nG"], None)
        .ok()
        .map(|(stdout, _)| stdout)
        .or_else(|| {
            let user = std::env::var("USER").unwrap_or_default();
            if user.is_empty() {
                None
            } else {
                run_command_capture("id", &["-nG", &user], None)
                    .ok()
                    .map(|(stdout, _)| stdout)
            }
        })
        .unwrap_or_default();
    stdout
        .split_whitespace()
        .any(|value| value.trim().eq_ignore_ascii_case(group))
}

fn rocminfo_command() -> Option<String> {
    if command_available("rocminfo", &["--help"]) {
        return Some("rocminfo".to_string());
    }
    let alt = "/opt/rocm/bin/rocminfo";
    if Path::new(alt).exists() {
        return Some(alt.to_string());
    }
    None
}

fn rocm_runtime_ready() -> (bool, bool, Vec<String>) {
    let mut notes: Vec<String> = Vec::new();
    let rocminfo_cmd = rocminfo_command();
    let has_rocminfo_bin = rocminfo_cmd.is_some();
    if !has_rocminfo_bin {
        notes.push("`rocminfo` is not installed.".to_string());
    }

    let has_dev_kfd = Path::new("/dev/kfd").exists();
    if !has_dev_kfd {
        notes.push("/dev/kfd is missing.".to_string());
    }

    let render_ok = user_in_group("render");
    let video_ok = user_in_group("video");
    if !render_ok || !video_ok {
        notes.push("Current user is not yet in both `render` and `video` groups.".to_string());
    }

    let rocminfo_ok = if let Some(cmd) = rocminfo_cmd.as_deref() {
        run_command_capture(cmd, &[], None)
            .map(|(stdout, _)| stdout.to_ascii_lowercase().contains("agent"))
            .unwrap_or(false)
    } else {
        false
    };
    if has_rocminfo_bin && !rocminfo_ok {
        notes.push("`rocminfo` did not report a usable ROCm agent.".to_string());
    }

    let runtime_partially_present = has_rocminfo_bin || has_dev_kfd;
    let requires_relogin = runtime_partially_present && (!render_ok || !video_ok);

    (
        has_rocminfo_bin && has_dev_kfd && rocminfo_ok,
        requires_relogin,
        notes,
    )
}

fn dri_render_nodes() -> Vec<PathBuf> {
    let mut nodes = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/dev/dri") {
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .file_name()
                .map(|name| name.to_string_lossy().starts_with("renderD"))
                .unwrap_or(false)
            {
                nodes.push(path);
            }
        }
    }
    nodes
}

fn path_has_rw_access(path: &Path) -> bool {
    let shell_check = format!(
        "test -r {} && test -w {}",
        shell_single_quote(&path.to_string_lossy()),
        shell_single_quote(&path.to_string_lossy())
    );
    run_command_capture("sh", &["-c", &shell_check], None).is_ok()
}

fn linux_package_installed_any(distro: &str, packages: &[&str]) -> bool {
    packages
        .iter()
        .any(|package| linux_package_installed(distro, package))
}

fn xpu_runtime_ready_for_distro(distro: &str) -> (bool, bool, Vec<String>) {
    let mut notes: Vec<String> = Vec::new();
    let runtime_packages_ready = match distro {
        "arch" => {
            let compute = linux_package_installed("arch", "intel-compute-runtime");
            let level_zero =
                linux_package_installed_any("arch", &["level-zero-loader", "level-zero"]);
            if !compute {
                notes.push("`intel-compute-runtime` is not installed.".to_string());
            }
            if !level_zero {
                notes.push("Level Zero runtime is not installed.".to_string());
            }
            compute && level_zero
        }
        "fedora" => {
            let compute = linux_package_installed("fedora", "intel-compute-runtime");
            let level_zero =
                linux_package_installed_any("fedora", &["level-zero", "level-zero-loader"]);
            if !compute {
                notes.push("`intel-compute-runtime` is not installed.".to_string());
            }
            if !level_zero {
                notes.push("Level Zero runtime is not installed.".to_string());
            }
            compute && level_zero
        }
        "debian" => {
            let opencl = linux_package_installed("debian", "intel-opencl-icd");
            let level_zero = linux_package_installed_any(
                "debian",
                &["intel-level-zero-gpu", "libze-intel-gpu1", "level-zero"],
            );
            if !opencl {
                notes.push("`intel-opencl-icd` is not installed.".to_string());
            }
            if !level_zero {
                notes.push("Intel Level Zero runtime is not installed.".to_string());
            }
            opencl && level_zero
        }
        _ => false,
    };

    let render_nodes = dri_render_nodes();
    let has_render_node = !render_nodes.is_empty();
    if !has_render_node {
        notes.push("No `/dev/dri/renderD*` device was found.".to_string());
    }

    let render_ok = user_in_group("render");
    let video_ok = user_in_group("video");
    let has_render_access = render_nodes.iter().any(|path| path_has_rw_access(path));
    if !has_render_access {
        if !render_ok {
            notes.push("Current user does not yet have access to `/dev/dri/renderD*`. Log out and back in if group changes were just applied.".to_string());
        } else {
            notes.push("Current user is in the `render` group but still cannot access `/dev/dri/renderD*`.".to_string());
        }
    } else if !video_ok {
        notes.push("Current user is not in the `video` group. This is optional for Intel XPU checks, but some media features may still expect it.".to_string());
    }

    let requires_relogin = runtime_packages_ready && has_render_node && !has_render_access;
    (
        runtime_packages_ready && has_render_node && has_render_access,
        requires_relogin,
        notes,
    )
}

fn rocm_supported_for_distro(os: &LinuxOsRelease, family: &str) -> bool {
    match family {
        "arch" | "fedora" => true,
        "debian" => {
            let code = if !os.ubuntu_codename.is_empty() {
                os.ubuntu_codename.as_str()
            } else {
                os.version_codename.as_str()
            };
            matches!(code, "jammy" | "noble")
                || matches!(os.version_id.as_str(), "12" | "13" | "22.04" | "24.04")
        }
        _ => false,
    }
}

fn xpu_supported_for_distro(_os: &LinuxOsRelease, family: &str) -> bool {
    matches!(family, "arch" | "fedora" | "debian")
}

/// Emits a `comfyui-install-progress` event for the ROCm/XPU guided-setup
/// flows. (Both used to be separate, byte-for-byte identical functions.)
fn emit_guided_setup_event(app: &AppHandle, phase: &str, message: &str) {
    let _ = app.emit(
        "comfyui-install-progress",
        DownloadProgressEvent {
            kind: "comfyui_install".to_string(),
            phase: phase.to_string(),
            artifact: None,
            index: None,
            total: None,
            received: None,
            size: None,
            folder: None,
            message: Some(message.to_string()),
        },
    );
}

fn stream_command_output(
    app: &AppHandle,
    phase: &'static str,
    stream_name: &'static str,
    reader: impl std::io::Read + Send + 'static,
    tail: std::sync::Arc<Mutex<VecDeque<String>>>,
) -> std::thread::JoinHandle<()> {
    let app = app.clone();
    std::thread::spawn(move || {
        let buffered = std::io::BufReader::new(reader);
        for line in buffered.lines().map_while(Result::ok) {
            let text = line.trim_end().to_string();
            if text.is_empty() {
                continue;
            }
            if let Ok(mut lines) = tail.lock() {
                if lines.len() >= 12 {
                    lines.pop_front();
                }
                lines.push_back(format!("[{stream_name}] {text}"));
            }
            emit_guided_setup_event(&app, phase, &text);
        }
    })
}

fn run_command_streaming_with_env(
    app: &AppHandle,
    phase: &'static str,
    program: &str,
    args: &[&str],
    working_dir: Option<&Path>,
    envs: &[(&str, &str)],
) -> Result<(), String> {
    log::debug!(
        "run_command_streaming_with_env: {} {}",
        program,
        args.join(" ")
    );
    let mut cmd = build_command(program, args, working_dir, envs)?;
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|err| format!("Failed to run {program}: {err}"))?;

    let tail = std::sync::Arc::new(Mutex::new(VecDeque::<String>::new()));
    let stdout_handle = child
        .stdout
        .take()
        .map(|stdout| stream_command_output(app, phase, "stdout", stdout, tail.clone()));
    let stderr_handle = child
        .stderr
        .take()
        .map(|stderr| stream_command_output(app, phase, "stderr", stderr, tail.clone()));

    let status = child
        .wait()
        .map_err(|err| format!("Failed to wait for {program}: {err}"))?;

    if let Some(handle) = stdout_handle {
        let _ = handle.join();
    }
    if let Some(handle) = stderr_handle {
        let _ = handle.join();
    }

    if !status.success() {
        let detail = tail
            .lock()
            .ok()
            .map(|lines| lines.iter().cloned().collect::<Vec<_>>().join(" | "))
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| "no command output captured".to_string());
        return Err(format!(
            "Command failed: {} {} :: {}",
            program,
            args.join(" "),
            detail
        ));
    }
    Ok(())
}

fn install_rocm_guided_internal(app: &AppHandle) -> Result<RocmGuidedStatus, String> {
    let os = detect_linux_os_release();
    let family = detect_linux_distro_family();
    let gpu_name = detect_amd_gpu_name();
    let supported = rocm_supported_for_distro(&os, &family);
    let distro_label = if os.pretty_name.trim().is_empty() {
        family.clone()
    } else {
        os.pretty_name.clone()
    };

    if gpu_name.is_none() {
        return Ok(RocmGuidedStatus {
            distro_family: family,
            distro_label,
            supported,
            amd_detected: false,
            gpu_name: None,
            ready: false,
            requires_relogin: false,
            detail: "AMD GPU not detected on this system.".to_string(),
        });
    }

    if fake_amd_enabled() && !fake_amd_allow_rocm_setup_enabled() {
        emit_guided_setup_event(
            app,
            "warn",
            "Fake AMD mode enabled. Guided ROCm setup is disabled to avoid modifying a non-AMD system.",
        );
        return Ok(RocmGuidedStatus {
            distro_family: family,
            distro_label,
            supported: false,
            amd_detected: true,
            gpu_name,
            ready: false,
            requires_relogin: false,
            detail:
                "Fake AMD mode is active. UI testing is enabled, but guided ROCm setup is disabled."
                    .to_string(),
        });
    }

    if !supported {
        return Ok(RocmGuidedStatus {
            distro_family: family,
            distro_label,
            supported: false,
            amd_detected: true,
            gpu_name,
            ready: false,
            requires_relogin: false,
            detail: "Guided ROCm setup is currently supported only for Debian-based, Fedora, and Arch Linux families.".to_string(),
        });
    }

    match family.as_str() {
        "arch" => {
            let mut steps = vec![
                "echo Refreshing pacman package metadata for ROCm setup...".to_string(),
                "pacman -Sy".to_string(),
                "echo Installing ROCm packages with pacman...".to_string(),
                "pacman -S --needed --noconfirm rocminfo rocm-hip-sdk rocm-opencl-sdk".to_string(),
            ];
            if let Ok(user) = std::env::var("USER") {
                if !user.trim().is_empty() {
                    steps.extend(rocm_group_setup_steps(&user));
                }
            }
            run_privileged_shell_streaming(app, &steps.join(" && "), None)?;
        }
        "fedora" => {
            let mut steps = vec![
                "echo Refreshing dnf metadata for ROCm setup...".to_string(),
                "dnf makecache".to_string(),
                "echo Installing ROCm packages with dnf...".to_string(),
                "dnf install -y rocminfo rocm-hip rocm-opencl".to_string(),
            ];
            if let Ok(user) = std::env::var("USER") {
                if !user.trim().is_empty() {
                    steps.extend(rocm_group_setup_steps(&user));
                }
            }
            run_privileged_shell_streaming(app, &steps.join(" && "), None)?;
        }
        "debian" => {
            let ubuntu_code = if !os.ubuntu_codename.is_empty() {
                os.ubuntu_codename.clone()
            } else if matches!(os.version_id.as_str(), "12") {
                "jammy".to_string()
            } else if matches!(os.version_id.as_str(), "13") {
                "noble".to_string()
            } else {
                os.version_codename.clone()
            };
            if !matches!(ubuntu_code.as_str(), "jammy" | "noble") {
                return Err(format!(
                    "Guided ROCm setup currently supports Ubuntu-compatible codenames jammy/noble for Debian-family systems. Detected '{}'.",
                    ubuntu_code
                ));
            }
            let installer_url = format!(
                "https://repo.radeon.com/amdgpu-install/7.2.4/ubuntu/{ubuntu_code}/amdgpu-install_7.2.4.70204-1_all.deb"
            );
            let deb_path = "/tmp/amdgpu-install_7.2.4.70204-1_all.deb";
            emit_guided_setup_event(app, "step", "Downloading AMD amdgpu-install package...");
            run_command_with_retry("wget", &["-O", deb_path, &installer_url], None, 2)?;
            let mut steps = vec![
                "echo Refreshing apt package metadata for ROCm setup...".to_string(),
                "apt update".to_string(),
                "echo Installing amdgpu-install package...".to_string(),
                format!("apt install -y {}", shell_single_quote(deb_path)),
                "echo Running AMD ROCm guided installer...".to_string(),
                "amdgpu-install -y --usecase=rocm --no-dkms".to_string(),
            ];
            if let Ok(user) = std::env::var("USER") {
                if !user.trim().is_empty() {
                    steps.extend(rocm_group_setup_steps(&user));
                }
            }
            run_privileged_shell_streaming(app, &steps.join(" && "), None)?;
        }
        _ => {
            return Err("Unsupported distro family for guided ROCm setup.".to_string());
        }
    }

    if fake_amd_enabled() && fake_amd_allow_rocm_setup_enabled() {
        emit_guided_setup_event(
            app,
            "step",
            "Fake AMD install-test mode is active. Runtime validation is skipped because no real AMD GPU is present. If guided setup changed your groups, log out and back in before real use.",
        );
        return Ok(RocmGuidedStatus {
            distro_family: family,
            distro_label,
            supported,
            amd_detected: true,
            gpu_name,
            ready: true,
            requires_relogin: false,
            detail: "ROCm package installation finished. Runtime validation is skipped in fake AMD install-test mode. If guided setup changed your groups, log out and back in before real use.".to_string(),
        });
    }

    emit_guided_setup_event(app, "step", "Checking ROCm runtime readiness...");
    let (ready, requires_relogin, notes) = rocm_runtime_ready();
    let detail = if ready {
        "ROCm runtime looks ready for use. If guided setup changed your groups, log out and back in before launching ComfyUI.".to_string()
    } else if requires_relogin {
        "ROCm packages installed. Log out and back in, or reboot, then run the ROCm check again."
            .to_string()
    } else if notes.is_empty() {
        "ROCm guided setup finished. Log out and back in, then run the ROCm check again."
            .to_string()
    } else {
        format!(
            "ROCm guided setup finished. Log out and back in, then run the ROCm check again. {}",
            notes.join(" ")
        )
    };

    Ok(RocmGuidedStatus {
        distro_family: family,
        distro_label,
        supported,
        amd_detected: true,
        gpu_name,
        ready,
        requires_relogin,
        detail,
    })
}

fn install_xpu_guided_internal(app: &AppHandle) -> Result<XpuGuidedStatus, String> {
    let os = detect_linux_os_release();
    let family = detect_linux_distro_family();
    let gpu_name = detect_intel_gpu_name();
    let supported = xpu_supported_for_distro(&os, &family);
    let distro_label = if os.pretty_name.trim().is_empty() {
        family.clone()
    } else {
        os.pretty_name.clone()
    };

    if gpu_name.is_none() {
        return Ok(XpuGuidedStatus {
            distro_family: family,
            distro_label,
            supported,
            intel_detected: false,
            gpu_name: None,
            ready: false,
            requires_relogin: false,
            detail: "Intel GPU not detected on this system.".to_string(),
        });
    }

    if fake_intel_enabled() && !fake_intel_allow_xpu_setup_enabled() {
        emit_guided_setup_event(
            app,
            "warn",
            "Fake Intel mode enabled. Guided Intel setup is disabled to avoid modifying a non-Intel system.",
        );
        return Ok(XpuGuidedStatus {
            distro_family: family,
            distro_label,
            supported: false,
            intel_detected: true,
            gpu_name,
            ready: false,
            requires_relogin: false,
            detail: "Fake Intel mode is active. UI testing is enabled, but guided Intel setup is disabled.".to_string(),
        });
    }

    if !supported {
        return Ok(XpuGuidedStatus {
            distro_family: family,
            distro_label,
            supported: false,
            intel_detected: true,
            gpu_name,
            ready: false,
            requires_relogin: false,
            detail: "Guided Intel setup is currently supported only for Debian-based, Fedora, and Arch Linux families.".to_string(),
        });
    }

    match family.as_str() {
        "arch" => {
            let mut steps = vec![
                "echo Refreshing pacman package metadata for Intel XPU setup...".to_string(),
                "pacman -Sy".to_string(),
                "echo Installing Intel XPU runtime packages with pacman...".to_string(),
                "pacman -S --needed --noconfirm intel-compute-runtime level-zero-loader"
                    .to_string(),
            ];
            if let Ok(user) = std::env::var("USER") {
                if !user.trim().is_empty() {
                    steps.extend(rocm_group_setup_steps(&user));
                }
            }
            run_privileged_shell_streaming(app, &steps.join(" && "), None)?;
        }
        "fedora" => {
            let mut steps = vec![
                "echo Refreshing dnf metadata for Intel XPU setup...".to_string(),
                "dnf makecache".to_string(),
                "echo Installing Intel XPU runtime packages with dnf...".to_string(),
                "(dnf install -y intel-compute-runtime level-zero || dnf install -y intel-compute-runtime level-zero-loader)".to_string(),
            ];
            if let Ok(user) = std::env::var("USER") {
                if !user.trim().is_empty() {
                    steps.extend(rocm_group_setup_steps(&user));
                }
            }
            run_privileged_shell_streaming(app, &steps.join(" && "), None)?;
        }
        "debian" => {
            let mut steps = vec![
                "echo Refreshing apt package metadata for Intel XPU setup...".to_string(),
                "apt update".to_string(),
                "echo Installing Intel XPU runtime packages with apt...".to_string(),
                "apt install -y intel-opencl-icd".to_string(),
                "if apt-cache show intel-level-zero-gpu >/dev/null 2>&1; then apt install -y intel-level-zero-gpu; elif apt-cache show libze-intel-gpu1 >/dev/null 2>&1; then apt install -y libze-intel-gpu1; elif apt-cache show level-zero >/dev/null 2>&1; then apt install -y level-zero; else echo Warning: no supported Intel Level Zero package was found in apt repositories.; fi".to_string(),
            ];
            if let Ok(user) = std::env::var("USER") {
                if !user.trim().is_empty() {
                    steps.extend(rocm_group_setup_steps(&user));
                }
            }
            run_privileged_shell_streaming(app, &steps.join(" && "), None)?;
        }
        _ => return Err("Unsupported distro family for guided Intel setup.".to_string()),
    }

    if fake_intel_enabled() && fake_intel_allow_xpu_setup_enabled() {
        emit_guided_setup_event(
            app,
            "step",
            "Fake Intel install-test mode is active. Runtime validation is skipped because no real Intel GPU is present. If guided setup changed your groups, log out and back in before real use.",
        );
        return Ok(XpuGuidedStatus {
            distro_family: family,
            distro_label,
            supported,
            intel_detected: true,
            gpu_name,
            ready: true,
            requires_relogin: false,
            detail: "Intel package installation finished. Runtime validation is skipped in fake Intel install-test mode. If guided setup changed your groups, log out and back in before real use.".to_string(),
        });
    }

    emit_guided_setup_event(app, "step", "Checking Intel XPU runtime readiness...");
    let (ready, requires_relogin, notes) = xpu_runtime_ready_for_distro(&family);
    let detail = if ready {
        "Intel XPU runtime looks ready for use. If guided setup changed your groups, log out and back in before launching ComfyUI.".to_string()
    } else if requires_relogin {
        "Intel GPU packages installed. Log out and back in, or reboot, then run the Intel XPU check again.".to_string()
    } else if notes.is_empty() {
        "Intel guided setup finished. Log out and back in, then run the Intel XPU check again."
            .to_string()
    } else {
        format!(
            "Intel guided setup finished. Log out and back in, then run the Intel XPU check again. {}",
            notes.join(" ")
        )
    };

    Ok(XpuGuidedStatus {
        distro_family: family,
        distro_label,
        supported,
        intel_detected: true,
        gpu_name,
        ready,
        requires_relogin,
        detail,
    })
}

fn get_rocm_guided_status() -> RocmGuidedStatus {
    let os = detect_linux_os_release();
    let family = detect_linux_distro_family();
    let gpu_name = detect_amd_gpu_name();
    let supported = rocm_supported_for_distro(&os, &family);
    let distro_label = if os.pretty_name.trim().is_empty() {
        family.clone()
    } else {
        os.pretty_name.clone()
    };
    if fake_amd_enabled() {
        let allow_real_setup = fake_amd_allow_rocm_setup_enabled();
        return RocmGuidedStatus {
            distro_family: family,
            distro_label,
            supported: allow_real_setup,
            amd_detected: true,
            gpu_name,
            ready: true,
            requires_relogin: false,
            detail: if allow_real_setup {
                "Fake AMD install-test mode is active. Runtime validation is simulated because no real AMD GPU is present.".to_string()
            } else {
                "Fake AMD mode is active. ROCm readiness is being simulated for UI/install testing on a non-AMD system.".to_string()
            },
        };
    }
    let (ready, requires_relogin, notes) = rocm_runtime_ready();
    let detail = if gpu_name.is_none() {
        "AMD GPU not detected on this system.".to_string()
    } else if !supported {
        "Guided ROCm setup is not available for this Linux distribution family.".to_string()
    } else if ready {
        "ROCm runtime looks ready for use.".to_string()
    } else if requires_relogin {
        "ROCm install needs a logout/login or reboot, then Check ROCm again.".to_string()
    } else {
        let _ = notes;
        "ROCm not ready. Run Guided ROCm Setup, then Check ROCm again.".to_string()
    };
    RocmGuidedStatus {
        distro_family: family,
        distro_label,
        supported,
        amd_detected: gpu_name.is_some(),
        gpu_name,
        ready,
        requires_relogin,
        detail,
    }
}

fn get_xpu_guided_status() -> XpuGuidedStatus {
    let os = detect_linux_os_release();
    let family = detect_linux_distro_family();
    let gpu_name = detect_intel_gpu_name();
    let supported = xpu_supported_for_distro(&os, &family);
    let distro_label = if os.pretty_name.trim().is_empty() {
        family.clone()
    } else {
        os.pretty_name.clone()
    };
    if fake_intel_enabled() {
        let allow_real_setup = fake_intel_allow_xpu_setup_enabled();
        return XpuGuidedStatus {
            distro_family: family,
            distro_label,
            supported: allow_real_setup,
            intel_detected: true,
            gpu_name,
            ready: true,
            requires_relogin: false,
            detail: if allow_real_setup {
                "Fake Intel install-test mode is active. Runtime validation is simulated because no real Intel GPU is present.".to_string()
            } else {
                "Fake Intel mode is active. Intel XPU readiness is being simulated for UI/install testing on a non-Intel system.".to_string()
            },
        };
    }
    let (ready, requires_relogin, notes) = xpu_runtime_ready_for_distro(&family);
    let detail = if gpu_name.is_none() {
        "Intel GPU not detected on this system.".to_string()
    } else if !supported {
        "Guided Intel setup is not available for this Linux distribution family.".to_string()
    } else if ready {
        "Intel XPU runtime looks ready for use.".to_string()
    } else if requires_relogin {
        "Intel GPU setup needs a logout/login or reboot, then Check Intel XPU again.".to_string()
    } else {
        let _ = notes;
        "Intel XPU is not ready. Run Guided Intel Setup, then Check Intel XPU again.".to_string()
    };
    XpuGuidedStatus {
        distro_family: family,
        distro_label,
        supported,
        intel_detected: gpu_name.is_some(),
        gpu_name,
        ready,
        requires_relogin,
        detail,
    }
}

async fn install_rocm_guided(app: AppHandle) -> Result<RocmGuidedStatus, String> {
    tokio::task::spawn_blocking(move || install_rocm_guided_internal(&app))
        .await
        .map_err(|err| format!("ROCm guided setup task failed: {err}"))?
}

async fn install_xpu_guided(app: AppHandle) -> Result<XpuGuidedStatus, String> {
    tokio::task::spawn_blocking(move || install_xpu_guided_internal(&app))
        .await
        .map_err(|err| format!("Intel guided setup task failed: {err}"))?
}

fn get_comfyui_install_recommendation(gpu_selection: Option<String>) -> ComfyInstallRecommendation {
    let mut gpu = detect_nvidia_gpu_details();
    let mut amd_name = detect_amd_gpu_name();
    let mut intel_name = detect_intel_gpu_name();
    let detection_pending = gpu_detection_pending();
    if !detection_pending {
        if gpu.name.is_none() {
            gpu = detect_nvidia_gpu_details();
        }
        if amd_name.is_none() {
            amd_name = detect_amd_gpu_name();
        }
        if intel_name.is_none() {
            intel_name = detect_intel_gpu_name();
        }
    }
    comfy_install_recommendation_for(
        gpu,
        amd_name,
        intel_name,
        gpu_selection.as_deref(),
        detection_pending,
    )
}

fn comfy_install_recommendation_for(
    gpu: NvidiaGpuDetails,
    amd_name: Option<String>,
    intel_name: Option<String>,
    gpu_selection: Option<&str>,
    detection_pending: bool,
) -> ComfyInstallRecommendation {
    let selection = gpu_selection.unwrap_or("auto").trim().to_ascii_lowercase();
    let gpu_name = gpu.name.clone().unwrap_or_default().to_ascii_lowercase();
    if selection == "amd" || (selection == "auto" && gpu.name.is_none()) {
        if let Some(amd_name) = amd_name.clone() {
            return ComfyInstallRecommendation {
                gpu_name: Some(amd_name),
                driver_version: None,
                torch_profile: "torch2141_rocm72".to_string(),
                torch_label: "Torch 2.14.1 + ROCm 7.2".to_string(),
                reason: if selection == "amd" {
                    "Selected AMD GPU; using ROCm install profile.".to_string()
                } else {
                    "Detected AMD GPU; selecting ROCm install profile.".to_string()
                },
                detection_pending,
            };
        }
        if selection == "amd" {
            return ComfyInstallRecommendation {
                gpu_name: None,
                driver_version: None,
                torch_profile: "torch2141_rocm72".to_string(),
                torch_label: "Torch 2.14.1 + ROCm 7.2".to_string(),
                reason: "Selected AMD GPU is still being detected.".to_string(),
                detection_pending,
            };
        }
    }
    if selection == "intel" || (selection == "auto" && gpu.name.is_none() && amd_name.is_none()) {
        if let Some(intel_name) = intel_name {
            return ComfyInstallRecommendation {
                gpu_name: Some(intel_name),
                driver_version: None,
                torch_profile: "torch2141_xpu".to_string(),
                torch_label: "Torch 2.14.1 + XPU".to_string(),
                reason: if selection == "intel" {
                    "Selected Intel GPU; using XPU install profile.".to_string()
                } else {
                    "Detected Intel GPU; selecting XPU install profile.".to_string()
                },
                detection_pending,
            };
        }
        if selection == "intel" {
            return ComfyInstallRecommendation {
                gpu_name: None,
                driver_version: None,
                torch_profile: "torch2141_xpu".to_string(),
                torch_label: "Torch 2.14.1 + XPU".to_string(),
                reason: "Selected Intel GPU is still being detected.".to_string(),
                detection_pending,
            };
        }
    }
    if crate::shared::nvidia_supports_cu130(
        gpu.name.as_deref(),
        gpu.driver_version.as_deref(),
        gpu.compute_capability.as_deref(),
    ) {
        return ComfyInstallRecommendation {
            gpu_name: gpu.name,
            driver_version: gpu.driver_version,
            torch_profile: "torch2141_cu130".to_string(),
            torch_label: "Torch 2.14.1 + cu130".to_string(),
            reason: "Selected NVIDIA GPU (Turing or newer) with driver >= 580; using CUDA 13."
                .to_string(),
            detection_pending,
        };
    }

    // Drivers older than 580 (or pre-Turing GPUs) stay on CUDA 12.8.
    if gpu_name.contains("rtx 30") {
        return ComfyInstallRecommendation {
            gpu_name: gpu.name,
            driver_version: gpu.driver_version,
            torch_profile: "torch271_cu128".to_string(),
            torch_label: "Torch 2.7.1 + cu128".to_string(),
            reason: "Selected NVIDIA RTX 3000 series (Ampere).".to_string(),
            detection_pending,
        };
    }

    if gpu_name.contains("rtx 40") {
        return ComfyInstallRecommendation {
            gpu_name: gpu.name,
            driver_version: gpu.driver_version,
            torch_profile: "torch280_cu128".to_string(),
            torch_label: "Torch 2.8.0 + cu128".to_string(),
            reason: "Selected NVIDIA RTX 4000 series (Ada).".to_string(),
            detection_pending,
        };
    }

    if gpu_name.contains("rtx 50") {
        return ComfyInstallRecommendation {
            gpu_name: gpu.name,
            driver_version: gpu.driver_version,
            torch_profile: "torch280_cu128".to_string(),
            torch_label: "Torch 2.8.0 + cu128".to_string(),
            reason: "Selected NVIDIA RTX 5000 series with older driver; using safer fallback."
                .to_string(),
            detection_pending,
        };
    }

    let reason = if gpu.name.is_some() {
        "Detected NVIDIA GPU; using default CUDA recommendation.".to_string()
    } else {
        "Unknown or non-NVIDIA GPU; using default recommendation.".to_string()
    };
    ComfyInstallRecommendation {
        gpu_name: gpu.name,
        driver_version: gpu.driver_version,
        torch_profile: "torch280_cu128".to_string(),
        torch_label: "Torch 2.8.0 + cu128".to_string(),
        reason,
        detection_pending,
    }
}

pub(crate) fn normalize_path(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Install folder is required.".to_string());
    }
    let normalized_input = trimmed.replace('\\', "/");

    let mut path = PathBuf::from(normalized_input);
    if !path.is_absolute() {
        path = std::env::current_dir()
            .map_err(|err| err.to_string())?
            .join(path);
    }
    Ok(normalize_canonical_path(&path))
}

pub(crate) fn write_extra_model_paths_yaml(
    comfy_dir: &Path,
    base_path: &Path,
    is_default: bool,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(base_path).map_err(|err| {
        format!(
            "failed to prepare extra models folder '{}': {err}",
            base_path.display()
        )
    })?;

    let target = comfy_dir.join("extra_model_paths.yaml");
    let example = comfy_dir.join("extra_model_paths.yaml.example");
    if !target.exists() {
        if example.exists() {
            std::fs::rename(&example, &target).map_err(|err| {
                format!(
                    "failed to rename '{}' to '{}': {err}",
                    example.display(),
                    target.display()
                )
            })?;
        } else {
            return Err(
                "extra_model_paths.yaml.example was not found in ComfyUI install folder."
                    .to_string(),
            );
        }
    }

    let base = yaml_single_quote(&normalize_canonical_path(base_path).to_string_lossy());
    let default_value = if is_default { "true" } else { "false" };
    let yaml = [
        "# Managed by ComfyPilot.".to_string(),
        "comfyui:".to_string(),
        format!("  base_path: {base}"),
        format!("  is_default: {default_value}"),
        "  checkpoints: models/checkpoints/".to_string(),
        "  text_encoders: |".to_string(),
        "    models/text_encoders/".to_string(),
        "    models/clip/".to_string(),
        "  clip_vision: models/clip_vision/".to_string(),
        "  configs: models/configs/".to_string(),
        "  controlnet: models/controlnet/".to_string(),
        "  diffusion_models: |".to_string(),
        "    models/diffusion_models/".to_string(),
        "    models/unet/".to_string(),
        "  embeddings: models/embeddings/".to_string(),
        "  loras: models/loras/".to_string(),
        "  upscale_models: models/upscale_models/".to_string(),
        "  vae: models/vae/".to_string(),
        "  audio_encoders: models/audio_encoders/".to_string(),
        "  model_patches: models/model_patches/".to_string(),
    ]
    .join("\n")
        + "\n";

    std::fs::write(&target, yaml).map_err(|err| {
        format!(
            "failed to write extra model paths config '{}': {err}",
            target.display()
        )
    })?;

    Ok(target)
}

fn is_forbidden_install_path(path: &Path) -> bool {
    // Mirrors app_windows.rs's is_forbidden_install_path: block the
    // filesystem root and well-known system directories so an install/base
    // folder can't be pointed at a location whose contents the app might
    // later create/clear/overwrite (see run_comfyui_preflight,
    // set_comfyui_install_base).
    let normalized = path.to_string_lossy().trim_end_matches('/').to_string();
    let normalized = if normalized.is_empty() {
        "/".to_string()
    } else {
        normalized
    };

    if normalized == "/" {
        return true;
    }

    const BLOCKED_EXACT: &[&str] = &[
        "/bin", "/boot", "/dev", "/etc", "/lib", "/lib32", "/lib64", "/libx32", "/proc", "/root",
        "/run", "/sbin", "/sys", "/usr", "/var",
    ];
    if BLOCKED_EXACT
        .iter()
        .any(|entry| normalized == *entry || normalized.starts_with(&format!("{entry}/")))
    {
        return true;
    }

    // Also refuse the user's home directory itself (subfolders like
    // ~/ComfyUI remain allowed) so a stray/empty install-path value can't
    // resolve to "wipe everything under my home directory".
    if let Ok(home) = std::env::var("HOME") {
        let home = home.trim_end_matches('/');
        if !home.is_empty() && normalized == home {
            return true;
        }
    }

    false
}

fn normalize_canonical_path(path: &Path) -> PathBuf {
    // Best-effort canonicalize: most call sites already canonicalize before
    // calling this (so this is a harmless no-op re-resolve for them), but
    // `normalize_path` did not, which meant an install root containing a
    // symlink component (e.g. `~/ComfyUI` under a symlinked home directory,
    // or a bind-mounted models dir) could be stored one way at install time
    // and resolved a different way at launch time by `resolve_root_path`
    // (which does canonicalize), silently discarding the saved launch
    // settings because the two forms compared unequal. Falls back to the
    // input unchanged when the path doesn't exist yet (e.g. a fresh install
    // target that hasn't been created), matching the previous behavior for
    // that case.
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn command_available(program: &str, args: &[&str]) -> bool {
    let cmd = match build_command(program, args, None, &[]) {
        Ok(cmd) => cmd,
        Err(_) => return false,
    };
    run_with_timeout_capturing_output(cmd, COMMAND_PROBE_TIMEOUT)
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn apply_background_command_flags(_cmd: &mut std::process::Command) {
    let _ = _cmd;
}

const SYSTEM_CA_BUNDLE_CANDIDATES: &[&str] = &[
    // NixOS and Fedora-compatible location.
    "/etc/ssl/certs/ca-bundle.crt",
    // Debian, Ubuntu, and derivatives.
    "/etc/ssl/certs/ca-certificates.crt",
    // RHEL/Fedora legacy location.
    "/etc/pki/tls/certs/ca-bundle.crt",
    // Alpine and OpenSSL's common compiled-in default.
    "/etc/ssl/cert.pem",
    "/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem",
];

fn select_ssl_cert_file(
    inherited: Option<std::ffi::OsString>,
    candidates: &[PathBuf],
) -> Option<std::ffi::OsString> {
    inherited.or_else(|| {
        candidates
            .iter()
            .find(|candidate| candidate.is_file())
            .map(|candidate| candidate.as_os_str().to_os_string())
    })
}

fn resolved_ssl_cert_file() -> Option<std::ffi::OsString> {
    let candidates: Vec<PathBuf> = SYSTEM_CA_BUNDLE_CANDIDATES
        .iter()
        .map(PathBuf::from)
        .collect();
    select_ssl_cert_file(std::env::var_os("SSL_CERT_FILE"), &candidates)
}

fn apply_python_tls_environment(cmd: &mut std::process::Command) {
    if let Some(ca_bundle) = resolved_ssl_cert_file() {
        cmd.env("SSL_CERT_FILE", ca_bundle);
    }
}

fn running_in_flatpak() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists()
    }

    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

fn build_command(
    program: &str,
    args: &[&str],
    working_dir: Option<&Path>,
    envs: &[(&str, &str)],
) -> Result<std::process::Command, String> {
    let explicit_ssl_cert_file = envs.iter().any(|(key, _)| *key == "SSL_CERT_FILE");
    let ssl_cert_file = if explicit_ssl_cert_file {
        None
    } else {
        resolved_ssl_cert_file()
    };
    let mut cmd = if running_in_flatpak() && program != "flatpak-spawn" {
        let mut wrapped = std::process::Command::new("flatpak-spawn");
        wrapped.arg("--host");
        if let Some(dir) = working_dir {
            wrapped.arg(format!("--directory={}", dir.to_string_lossy()));
        }
        if let Some(ca_bundle) = ssl_cert_file.as_ref() {
            wrapped.arg(format!(
                "--env=SSL_CERT_FILE={}",
                ca_bundle.to_string_lossy()
            ));
        }
        for (key, value) in envs {
            wrapped.arg(format!("--env={key}={value}"));
        }
        wrapped.arg(program);
        wrapped.args(args);
        wrapped
    } else {
        let mut direct = std::process::Command::new(program);
        direct.args(args);
        if let Some(dir) = working_dir {
            direct.current_dir(dir);
        }
        if let Some(ca_bundle) = ssl_cert_file.as_ref() {
            direct.env("SSL_CERT_FILE", ca_bundle);
        }
        for (key, value) in envs {
            direct.env(key, value);
        }
        direct
    };
    if !(running_in_flatpak() && program != "flatpak-spawn") {
        if let Some(dir) = working_dir {
            cmd.current_dir(dir);
        }
        for (key, value) in envs {
            cmd.env(key, value);
        }
    }
    apply_background_command_flags(&mut cmd);
    Ok(cmd)
}

fn valid_shell_env_value() -> Option<String> {
    let shell_from_env = std::env::var("SHELL").ok();
    let shells_file = std::fs::read_to_string("/etc/shells").ok();
    if let (Some(shell), Some(shells)) = (shell_from_env.as_deref(), shells_file.as_deref()) {
        if shells.lines().map(str::trim).any(|line| line == shell) {
            return Some(shell.to_string());
        }
    }
    ["/bin/bash", "/bin/sh"]
        .into_iter()
        .find(|path| Path::new(path).exists())
        .map(str::to_string)
}

fn try_attach_parent_console() {}

pub(crate) fn ensure_git_available(app: &AppHandle) -> Result<(), String> {
    let _ = app;
    if command_available("git", &["--version"]) {
        return Ok(());
    }
    Err("Git is not available in PATH. Install Git and retry.".to_string())
}

fn get_hf_xet_preflight_internal(xet_enabled: bool) -> HfXetPreflightResponse {
    if !xet_enabled {
        return HfXetPreflightResponse {
            xet_enabled: false,
            hf_cli_available: false,
            hf_backend: "disabled".to_string(),
            hf_xet_installed: false,
            hub_version: None,
            detail: "HF/Xet acceleration is disabled in app settings; the default downloader will be used."
                .to_string(),
        };
    }

    let uvx_hf_available = command_available("uvx", &["hf", "--help"]);
    let hf_native_available = command_available("hf", &["--help"]);
    let hf_cli_available = uvx_hf_available || hf_native_available;
    let hf_backend = if uvx_hf_available {
        "uvx hf".to_string()
    } else if hf_native_available {
        "hf".to_string()
    } else {
        "none".to_string()
    };

    if !hf_cli_available {
        return HfXetPreflightResponse {
            xet_enabled,
            hf_cli_available,
            hf_backend,
            hf_xet_installed: false,
            hub_version: None,
            detail: "HF CLI backend not found. Install uv (`https://docs.astral.sh/uv/`) for `uvx hf`, or install `hf` (`pip install -U huggingface_hub hf_xet`).".to_string(),
        };
    }

    let env_probe = if uvx_hf_available {
        run_command_capture("uvx", &["hf", "env"], None)
    } else {
        run_command_capture("hf", &["env"], None)
    };

    match env_probe {
        Ok((stdout, _stderr)) => {
            let hf_xet_raw = parse_hf_env_value(&stdout, "hf_xet").unwrap_or_default();
            let hub_version = parse_hf_env_value(&stdout, "huggingface_hub version");
            let hf_xet_installed = {
                let normalized = hf_xet_raw.trim().to_ascii_lowercase();
                !normalized.is_empty() && normalized != "n/a" && normalized != "none"
            };

            let detail = if hf_xet_installed {
                format!(
                    "HF/Xet preflight OK via {} (huggingface_hub {}, hf_xet {}).",
                    hf_backend,
                    hub_version.clone().unwrap_or_else(|| "unknown".to_string()),
                    hf_xet_raw
                )
            } else {
                format!(
                    "HF backend {} found, but hf_xet is missing. Run `pip install -U huggingface_hub hf_xet`.",
                    hf_backend
                )
            };

            HfXetPreflightResponse {
                xet_enabled,
                hf_cli_available,
                hf_backend,
                hf_xet_installed,
                hub_version,
                detail,
            }
        }
        Err(err) => HfXetPreflightResponse {
            xet_enabled,
            hf_cli_available,
            hf_backend,
            hf_xet_installed: false,
            hub_version: None,
            detail: format!("Could not run HF env probe: {err}"),
        },
    }
}

async fn get_hf_xet_preflight(app: AppHandle) -> Result<HfXetPreflightResponse, String> {
    let xet_enabled = app
        .state::<AppState>()
        .context
        .config
        .settings()
        .hf_xet_enabled;
    crate::host::async_runtime::spawn_blocking(move || get_hf_xet_preflight_internal(xet_enabled))
        .await
        .map_err(|err| format!("HF/Xet preflight task failed: {err}"))
}

fn ensure_hf_xet_runtime_installed(always_upgrade: bool) -> Result<(), String> {
    let before = get_hf_xet_preflight_internal(true);

    let mut attempts: Vec<String> = Vec::new();
    if !command_available("uv", &["--version"]) {
        return Err("uv is required for HF/Xet setup but was not found in PATH.".to_string());
    }
    if always_upgrade || !before.hf_xet_installed {
        match run_command_capture(
            "uv",
            &[
                "tool",
                "install",
                "--upgrade",
                "--force",
                "huggingface_hub[hf_xet]",
            ],
            None,
        ) {
            Ok(_) => attempts.push(
                "uv tool install --upgrade --force huggingface_hub[hf_xet] => ok".to_string(),
            ),
            Err(err) => {
                attempts.push(format!(
                    "uv tool install --upgrade --force huggingface_hub[hf_xet] => {err}"
                ));
            }
        }
    }

    let after = get_hf_xet_preflight_internal(true);
    if after.hf_cli_available && after.hf_xet_installed {
        Ok(())
    } else {
        Err(format!(
            "Could not prepare HF/Xet runtime. {}. attempts: {}",
            after.detail,
            attempts.join(" | ")
        ))
    }
}

async fn set_hf_xet_enabled(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<AppSettings, String> {
    if enabled {
        // `uv tool install ... huggingface_hub[hf_xet]` fetches and
        // installs a package over the network; run it off the command
        // dispatch thread like the rest of this file's install/mutation
        // commands.
        crate::host::async_runtime::spawn_blocking(|| ensure_hf_xet_runtime_installed(true))
            .await
            .map_err(|err| format!("HF/Xet setup task failed: {err}"))??;
    }
    state
        .context
        .config
        .update_settings(|settings| settings.hf_xet_enabled = enabled)
        .map_err(|err| err.to_string())
}

fn run_comfyui_preflight(
    state: State<'_, AppState>,
    request: ComfyInstallRequest,
) -> ComfyPreflightResponse {
    let mut items: Vec<PreflightItem> = Vec::new();
    let mut ok = true;

    if request.install_root.trim().is_empty() {
        push_preflight(
            &mut items,
            "warn",
            "Install base folder",
            "Select an install folder to run full preflight checks.",
        );
        return ComfyPreflightResponse {
            ok: false,
            summary: "Install folder not selected yet.".to_string(),
            items,
        };
    }

    let base_root = match normalize_path(&request.install_root) {
        Ok(path) => path,
        Err(err) => {
            push_preflight(&mut items, "fail", "Install base folder", err);
            return ComfyPreflightResponse {
                ok: false,
                summary: "Preflight failed.".to_string(),
                items,
            };
        }
    };

    if is_forbidden_install_path(&base_root) {
        ok = false;
        push_preflight(
            &mut items,
            "fail",
            "Install base folder",
            "Folder is blocked (avoid system directories).",
        );
    } else {
        push_preflight(
            &mut items,
            "pass",
            "Install base folder",
            format!("Using {}", base_root.display()),
        );
    }

    if std::fs::create_dir_all(&base_root).is_ok() {
        let probe = base_root.join(".arctic-write-test");
        match std::fs::write(&probe, b"ok") {
            Ok(_) => {
                let _ = std::fs::remove_file(&probe);
                push_preflight(
                    &mut items,
                    "pass",
                    "Write permission",
                    "Folder is writable.",
                );
            }
            Err(err) => {
                ok = false;
                push_preflight(
                    &mut items,
                    "fail",
                    "Write permission",
                    format!("Cannot write to selected folder: {err}"),
                );
            }
        }
    } else {
        ok = false;
        push_preflight(
            &mut items,
            "fail",
            "Write permission",
            "Could not create selected base folder.",
        );
    }

    match fs2::available_space(&base_root) {
        Ok(bytes) => {
            let gb = bytes as f64 / 1024f64 / 1024f64 / 1024f64;
            if gb < 40.0 {
                ok = false;
                push_preflight(
                    &mut items,
                    "fail",
                    "Disk space",
                    format!("Only {gb:.1} GB free. Recommended at least 40 GB."),
                );
            } else if gb < 80.0 {
                push_preflight(
                    &mut items,
                    "warn",
                    "Disk space",
                    format!(
                        "{gb:.1} GB free. Installation should work but more free space is safer."
                    ),
                );
            } else {
                push_preflight(
                    &mut items,
                    "pass",
                    "Disk space",
                    format!("{gb:.1} GB free."),
                );
            }
        }
        Err(err) => {
            push_preflight(
                &mut items,
                "warn",
                "Disk space",
                format!("Unable to check free space: {err}"),
            );
        }
    }

    if command_available("git", &["--version"]) {
        push_preflight(&mut items, "pass", "Git", "Git is available.");
    } else {
        ok = false;
        push_preflight(
            &mut items,
            "fail",
            "Git",
            "Git is not available in PATH. Install Git and retry.",
        );
    }

    let dns_ok = has_dns("github.com", 443) && has_dns("pypi.org", 443);
    if dns_ok {
        push_preflight(
            &mut items,
            "pass",
            "Network",
            "DNS lookup for required hosts is available.",
        );
    } else {
        push_preflight(
            &mut items,
            "warn",
            "Network",
            "Could not resolve one or more hosts (github.com, pypi.org). Install may fail offline.",
        );
    }

    if let Some(found) = discover_uv_binary() {
        let detail = if found == "uv" {
            "System uv detected.".to_string()
        } else {
            format!("uv detected at {}.", found)
        };
        push_preflight(&mut items, "pass", "uv runtime", detail);
    } else {
        push_preflight(
            &mut items,
            "warn",
            "uv runtime",
            "uv not found. Installer will auto-install uv for current user during ComfyUI install.",
        );
    }

    let hf_xet = get_hf_xet_preflight_internal(state.context.config.settings().hf_xet_enabled);
    if !hf_xet.hf_cli_available {
        push_preflight(&mut items, "warn", "HF/Xet acceleration", hf_xet.detail);
    } else if hf_xet.hf_xet_installed && hf_xet.xet_enabled {
        push_preflight(&mut items, "pass", "HF/Xet acceleration", hf_xet.detail);
    } else {
        push_preflight(&mut items, "warn", "HF/Xet acceleration", hf_xet.detail);
    }

    match get_linux_prereq_cache_or_scan() {
        Ok(scan) => {
            if scan.missing_required.is_empty() {
                push_preflight(
                    &mut items,
                    "pass",
                    "Linux system packages",
                    format!("{} prerequisites are installed.", scan.distro),
                );
            } else if scan.distro == "nixos" {
                ok = false;
                push_preflight(
                    &mut items,
                    "fail",
                    "Linux system packages",
                    format!(
                        "The current Nix environment is missing: {}. Run the packaged ComfyPilot or enter this repository's updated `nix develop` shell; NixOS prerequisites cannot be installed imperatively.",
                        scan.missing_required.join(", ")
                    ),
                );
            } else {
                push_preflight(
                    &mut items,
                    "warn",
                    "Linux system packages",
                    format!(
                        "Missing required packages for {}: {}. Installer will attempt to install them automatically.",
                        scan.distro,
                        scan.missing_required.join(", ")
                    ),
                );
            }
            if !scan.missing_optional.is_empty() {
                push_preflight(
                    &mut items,
                    "warn",
                    "Linux optional packages",
                    format!(
                        "Missing optional packages (installer may continue): {}",
                        scan.missing_optional.join(", ")
                    ),
                );
            }
        }
        Err(err) => {
            push_preflight(
                &mut items,
                "warn",
                "Linux system packages",
                format!("Could not evaluate distro package prerequisites: {err}"),
            );
        }
    }

    let selected_attention = [
        request.include_sage_attention,
        request.include_sage_attention3,
        request.include_flash_attention,
        request.include_nunchaku,
    ]
    .into_iter()
    .filter(|v| *v)
    .count();
    if selected_attention > 1 {
        ok = false;
        push_preflight(
            &mut items,
            "fail",
            "Attention add-on selection",
            "Select only one of SageAttention / SageAttention3 / FlashAttention / Nunchaku.",
        );
    } else {
        push_preflight(
            &mut items,
            "pass",
            "Attention add-on selection",
            "Selection is valid.",
        );
    }

    if request.include_sage_attention3 {
        let gpu = detect_nvidia_gpu_details();
        let allowed = gpu
            .name
            .as_deref()
            .map(|n| n.to_ascii_lowercase().contains("rtx 50"))
            .unwrap_or(false);
        if allowed {
            push_preflight(
                &mut items,
                "pass",
                "SageAttention3 compatibility",
                "RTX 50-series detected.",
            );
        } else {
            ok = false;
            push_preflight(
                &mut items,
                "fail",
                "SageAttention3 compatibility",
                "SageAttention3 requires NVIDIA RTX 50-series.",
            );
        }
    }

    let selected_profile = request
        .torch_profile
        .clone()
        .unwrap_or_else(|| get_comfyui_install_recommendation(None).torch_profile);
    if torch_profile_is_rocm(&selected_profile) {
        let status = get_rocm_guided_status();
        if status.ready {
            push_preflight(&mut items, "pass", "ROCm runtime", status.detail);
        } else if status.requires_relogin {
            ok = false;
            push_preflight(
                &mut items,
                "fail",
                "ROCm runtime",
                "ROCm install appears incomplete for the current session. Log out and back in, or reboot, then run the ROCm check again.",
            );
        } else {
            ok = false;
            push_preflight(&mut items, "fail", "ROCm runtime", status.detail);
        }
    }
    if torch_profile_is_xpu(&selected_profile) {
        let status = get_xpu_guided_status();
        if status.ready {
            push_preflight(&mut items, "pass", "Intel XPU runtime", status.detail);
        } else if status.requires_relogin {
            ok = false;
            push_preflight(
                &mut items,
                "fail",
                "Intel XPU runtime",
                "Intel GPU setup appears incomplete for the current session. Log out and back in, or reboot, then run the Intel XPU check again.",
            );
        } else {
            ok = false;
            push_preflight(&mut items, "fail", "Intel XPU runtime", status.detail);
        }
    }
    if torch_profile_is_rocm(&selected_profile) {
        let incompatible: Vec<&str> = [
            (request.include_sage_attention, "SageAttention"),
            (request.include_sage_attention3, "SageAttention3"),
            (request.include_flash_attention, "FlashAttention"),
            (request.include_nunchaku, "Nunchaku"),
            (request.include_trellis2, "Trellis2"),
        ]
        .into_iter()
        .filter_map(|(enabled, name)| enabled.then_some(name))
        .collect();
        if incompatible.is_empty() {
            push_preflight(
                &mut items,
                "pass",
                "ROCm add-on compatibility",
                "Selected add-ons are compatible with the ROCm install profile.",
            );
        } else {
            ok = false;
            push_preflight(
                &mut items,
                "fail",
                "ROCm add-on compatibility",
                format!(
                    "These options are currently CUDA-only in this app and must be disabled for ROCm installs: {}.",
                    incompatible.join(", ")
                ),
            );
        }
    }
    if torch_profile_is_xpu(&selected_profile) {
        let incompatible: Vec<&str> = [
            (request.include_sage_attention, "SageAttention"),
            (request.include_sage_attention3, "SageAttention3"),
            (request.include_flash_attention, "FlashAttention"),
            (request.include_nunchaku, "Nunchaku"),
            (request.include_trellis2, "Trellis2"),
        ]
        .into_iter()
        .filter_map(|(enabled, name)| enabled.then_some(name))
        .collect();
        if incompatible.is_empty() {
            push_preflight(
                &mut items,
                "pass",
                "Intel XPU add-on compatibility",
                "Selected add-ons are compatible with the Intel XPU install profile.",
            );
        } else {
            ok = false;
            push_preflight(
                &mut items,
                "fail",
                "Intel XPU add-on compatibility",
                format!(
                    "These options are currently CUDA-only in this app and must be disabled for Intel XPU installs: {}.",
                    incompatible.join(", ")
                ),
            );
        }
    }

    if request.include_trellis2 {
        let trellis_supported = matches!(selected_profile.as_str(), "torch280_cu128");
        if trellis_supported {
            push_preflight(
                &mut items,
                "pass",
                "Trellis2 compatibility",
                "Compatible Torch profile selected.",
            );
        } else {
            ok = false;
            push_preflight(
                &mut items,
                "fail",
                "Trellis2 compatibility",
                "Trellis2 currently requires Torch 2.8.0 + cu128 (Torch280 wheel set).",
            );
        }
    }

    let summary = if ok {
        "Preflight passed.".to_string()
    } else {
        "Preflight has blocking issues.".to_string()
    };
    ComfyPreflightResponse { ok, summary, items }
}

fn download_http_file(url: &str, out_file: &Path) -> Result<(), String> {
    if let Some(parent) = out_file.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            format!(
                "Failed to create download directory {}: {err}",
                parent.display()
            )
        })?;
    }

    let tmp_file = out_file.with_extension("download");
    let user_agent = concat!("ComfyPilot/", env!("CARGO_PKG_VERSION"));

    let curl_output = std::process::Command::new("curl")
        .arg("-fL")
        .arg("--retry")
        .arg("3")
        .arg("--connect-timeout")
        .arg("20")
        .arg("-A")
        .arg(user_agent)
        .arg("-o")
        .arg(&tmp_file)
        .arg(url)
        .output();

    let downloaded = match curl_output {
        Ok(output) if output.status.success() => true,
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let wget_output = std::process::Command::new("wget")
                .arg("--tries=3")
                .arg("--timeout=20")
                .arg("--user-agent")
                .arg(user_agent)
                .arg("-O")
                .arg(&tmp_file)
                .arg(url)
                .output();
            match wget_output {
                Ok(wget) if wget.status.success() => true,
                Ok(wget) => {
                    let wget_err = String::from_utf8_lossy(&wget.stderr).trim().to_string();
                    return Err(format!(
                        "HTTP download failed for {url}. curl: {stderr}. wget: {wget_err}"
                    ));
                }
                Err(wget_err) => {
                    return Err(format!(
                        "HTTP download failed for {url}. curl: {stderr}. wget launch failed: {wget_err}"
                    ));
                }
            }
        }
        Err(_) => {
            let wget_output = std::process::Command::new("wget")
                .arg("--tries=3")
                .arg("--timeout=20")
                .arg("--user-agent")
                .arg(user_agent)
                .arg("-O")
                .arg(&tmp_file)
                .arg(url)
                .output();
            match wget_output {
                Ok(wget) if wget.status.success() => true,
                Ok(wget) => {
                    let wget_err = String::from_utf8_lossy(&wget.stderr).trim().to_string();
                    return Err(format!(
                        "HTTP download failed for {url}. curl and wget failed. wget: {wget_err}"
                    ));
                }
                Err(wget_err) => {
                    return Err(format!(
                        "HTTP download failed for {url}. Neither curl nor wget is available: {wget_err}"
                    ));
                }
            }
        }
    };

    if downloaded {
        std::fs::rename(&tmp_file, out_file).map_err(|err| {
            format!(
                "Failed to finalize download {} -> {}: {err}",
                tmp_file.display(),
                out_file.display()
            )
        })?;
    }

    Ok(())
}

fn run_command(program: &str, args: &[&str], working_dir: Option<&Path>) -> Result<(), String> {
    log::debug!("run_command: {} {}", program, args.join(" "));
    let cmd = build_command(program, args, working_dir, &[])?;
    let status = status_with_optional_timeout(program, cmd)
        .map_err(|err| format!("Failed to run {program}: {err}"))?;
    if !status.success() {
        return Err(format!("Command failed: {} {}", program, args.join(" ")));
    }
    Ok(())
}

fn run_command_with_env(
    program: &str,
    args: &[&str],
    working_dir: Option<&Path>,
    envs: &[(&str, &str)],
) -> Result<(), String> {
    log::debug!("run_command_with_env: {} {}", program, args.join(" "));
    let cmd = build_command(program, args, working_dir, envs)?;
    let status = status_with_optional_timeout(program, cmd)
        .map_err(|err| format!("Failed to run {program}: {err}"))?;
    if !status.success() {
        return Err(format!("Command failed: {} {}", program, args.join(" ")));
    }
    Ok(())
}

fn can_use_interactive_sudo() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// pkexec's exit codes for a dismissed or refused authorization.
const PKEXEC_AUTH_FAILED: [i32; 2] = [126, 127];

/// Runs a command as root through sudo (when it needs no password), pkexec,
/// or an interactive sudo prompt. Each is tried only while authorization
/// is the problem: a command that runs and fails is reported as such, not
/// run again through the next method.
fn run_privileged_command(
    program: &str,
    args: &[&str],
    working_dir: Option<&Path>,
) -> Result<(), String> {
    let command_line = format!("{program} {}", args.join(" "));
    let failed = |how: &str, code: Option<i32>| {
        format!(
            "{command_line} failed (run with {how}, exit code {}). See the install output above for the reason.",
            code.map(|c| c.to_string()).unwrap_or_else(|| "none".into())
        )
    };

    // `sudo -n true` only checks that sudo works without a password prompt.
    if run_command("sudo", &["-n", "true"], None).is_ok() {
        let mut sudo_args: Vec<&str> = vec!["-n", program];
        sudo_args.extend_from_slice(args);
        return match run_command_status("sudo", &sudo_args, working_dir) {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(failed("sudo", status.code())),
            Err(err) => Err(err),
        };
    }

    let mut pkexec_args: Vec<&str> = vec![program];
    pkexec_args.extend_from_slice(args);
    match run_command_status("pkexec", &pkexec_args, working_dir) {
        Ok(status) if status.success() => return Ok(()),
        Ok(status)
            if !status
                .code()
                .is_some_and(|code| PKEXEC_AUTH_FAILED.contains(&code)) =>
        {
            return Err(failed("pkexec", status.code()));
        }
        // Authorization refused or pkexec missing: try an interactive sudo.
        _ => {}
    }

    if can_use_interactive_sudo() {
        let mut sudo_args: Vec<&str> = vec![program];
        sudo_args.extend_from_slice(args);
        return match run_command_status("sudo", &sudo_args, working_dir) {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(failed("sudo", status.code())),
            Err(err) => Err(err),
        };
    }

    Err(format!(
        "Could not get administrator rights for {command_line}. If running from the desktop, make sure a PolicyKit agent is running and approve the prompt; otherwise run the app with --nerdstats from a terminal so sudo can prompt."
    ))
}

/// Runs a command and returns its exit status (a non-zero status is not an
/// error here).
fn run_command_status(
    program: &str,
    args: &[&str],
    working_dir: Option<&Path>,
) -> Result<std::process::ExitStatus, String> {
    log::debug!("run_command_status: {} {}", program, args.join(" "));
    let cmd = build_command(program, args, working_dir, &[])?;
    status_with_optional_timeout(program, cmd)
        .map_err(|err| format!("Failed to run {program}: {err}"))
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn rocm_group_setup_steps(user: &str) -> Vec<String> {
    vec![
        "echo Adding current user to render/video groups...".to_string(),
        "getent group render >/dev/null || groupadd -f render".to_string(),
        "getent group video >/dev/null || groupadd -f video".to_string(),
        format!(
            "usermod -aG render,video {} || echo Warning: could not update render/video groups automatically.",
            shell_single_quote(user)
        ),
    ]
}

fn run_privileged_shell_streaming(
    app: &AppHandle,
    script: &str,
    working_dir: Option<&Path>,
) -> Result<(), String> {
    let shell_env_value = valid_shell_env_value();
    let shell_env: Vec<(&str, &str)> = shell_env_value
        .as_deref()
        .map(|shell| vec![("SHELL", shell)])
        .unwrap_or_default();
    // Decide how to get root before running the script, so a script that
    // runs and fails is reported instead of being run again via pkexec.
    if can_use_interactive_sudo() {
        emit_guided_setup_event(
            app,
            "step",
            "Requesting sudo authentication for guided ROCm setup...",
        );
        run_command_with_env("sudo", &["-v"], working_dir, &shell_env)?;
    }
    if run_command("sudo", &["-n", "true"], None).is_ok() {
        return run_command_streaming_with_env(
            app,
            "step",
            "sudo",
            &["-n", "sh", "-lc", script],
            working_dir,
            &shell_env,
        );
    }
    let sudo_err = "sudo: a password is required".to_string();

    emit_guided_setup_event(
        app,
        "warn",
        "Trying PolicyKit authentication for guided ROCm setup...",
    );
    let pkexec_result = run_command_streaming_with_env(
        app,
        "step",
        "pkexec",
        &["sh", "-lc", script],
        working_dir,
        &shell_env,
    );
    if pkexec_result.is_ok() {
        return Ok(());
    }

    let pkexec_err = pkexec_result.err().unwrap_or_default();
    let lower_pkexec = pkexec_err.to_ascii_lowercase();
    let lower_sudo = sudo_err.to_ascii_lowercase();
    if lower_sudo.contains("password is required")
        && (lower_pkexec.contains("error getting authority")
            || lower_pkexec.contains("could not connect"))
    {
        return Err(
            "Guided ROCm setup could not authenticate in this environment. PolicyKit is unavailable, and sudo cannot prompt here. In distrobox or terminal testing, run `sudo -v` in the same shell first, then retry Guided ROCm Setup."
                .to_string(),
        );
    }

    Err(format!(
        "Privilege escalation failed for guided ROCm setup. sudo: {} pkexec: {}",
        sudo_err, pkexec_err
    ))
}

pub(crate) fn run_command_capture(
    program: &str,
    args: &[&str],
    working_dir: Option<&Path>,
) -> Result<(String, String), String> {
    log::debug!("run_command_capture: {} {}", program, args.join(" "));
    let cmd = build_command(program, args, working_dir, &[])?;
    let is_hf_env_probe =
        (program == "uvx" && args == ["hf", "env"]) || (program == "hf" && args == ["env"]);
    let output = if is_hf_env_probe {
        run_with_timeout_capturing_output(cmd, HF_ENV_PROBE_TIMEOUT)
    } else {
        output_with_optional_timeout(program, cmd)
    }
    .map_err(|err| format!("Failed to run {program}: {err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        let tail = if stderr.trim().is_empty() {
            stdout
                .lines()
                .rev()
                .take(8)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            stderr
                .lines()
                .rev()
                .take(8)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n")
        };
        return Err(format!(
            "Command failed: {} {} :: {}",
            program,
            args.join(" "),
            tail
        ));
    }
    Ok((stdout, stderr))
}

fn run_command_with_retry(
    program: &str,
    args: &[&str],
    working_dir: Option<&Path>,
    retries: usize,
) -> Result<(), String> {
    let attempts = retries.max(1);
    let mut last_err = String::new();
    for attempt in 1..=attempts {
        match run_command_capture(program, args, working_dir) {
            Ok(_) => return Ok(()),
            Err(err) => {
                last_err = err;
                if attempt < attempts {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                }
            }
        }
    }
    Err(last_err)
}

fn run_command_env(
    program: &str,
    args: &[&str],
    working_dir: Option<&Path>,
    envs: &[(&str, &str)],
) -> Result<(), String> {
    log::debug!("run_command_env: {} {}", program, args.join(" "));
    let cmd = build_command(program, args, working_dir, envs)?;
    let status = status_with_optional_timeout(program, cmd)
        .map_err(|err| format!("Failed to run {program}: {err}"))?;
    if !status.success() {
        return Err(format!("Command failed: {} {}", program, args.join(" ")));
    }
    Ok(())
}

fn set_comfyui_root(
    state: State<'_, AppState>,
    comfyui_root: String,
) -> Result<AppSettings, String> {
    let trimmed = comfyui_root.trim();
    let normalized = if trimmed.is_empty() {
        None
    } else {
        let mut path = std::path::PathBuf::from(trimmed);
        if !path.is_absolute() {
            if let Ok(cwd) = std::env::current_dir() {
                path = cwd.join(path);
            }
        }
        Some(normalize_canonical_path(
            &std::fs::canonicalize(&path).unwrap_or(path),
        ))
    };
    if let Some(resolved) = normalized.as_ref() {
        if is_forbidden_install_path(resolved) {
            return Err(
                "That folder can't be used as a ComfyUI root (system directory).".to_string(),
            );
        }
    }
    let detected_attention = normalized.as_ref().map(|root| {
        detect_launch_attention_backend_for_root(root).unwrap_or_else(|| "none".to_string())
    });
    let detected_profile = normalized
        .as_ref()
        .and_then(|root| detect_torch_profile_for_root(root));

    state
        .context
        .config
        .update_settings(|settings| {
            settings.comfyui_root = normalized.clone();
            settings.comfyui_attention_backend = detected_attention
                .clone()
                .or_else(|| Some("none".to_string()));
            settings.comfyui_torch_profile = detected_profile.clone();
        })
        .map_err(|err| err.to_string())
}

async fn check_updates_now(state: State<'_, AppState>) -> Result<UpdateCheckResponse, String> {
    let package_manager = external_package_manager();
    let updater = state.context.updater.clone();
    let result = updater.check_for_update().await;

    match result {
        Ok(Ok(Some(update))) => {
            let notes = package_manager.as_deref().map_or(update.notes, |manager| {
                Some(format!(
                    "ComfyPilot v{} is available. This installation is managed by {manager}; update the Nix profile or declarative configuration that installed it. Profile installations can run: nix profile upgrade --refresh arctic-comfyui-helper",
                    update.version
                ))
            });
            Ok(UpdateCheckResponse {
                available: true,
                version: Some(update.version.to_string()),
                notes,
                managed_externally: package_manager.is_some(),
            })
        }
        Ok(Ok(None)) => Ok(UpdateCheckResponse {
            available: false,
            version: None,
            notes: package_manager.as_ref().map(|manager| {
                format!("This installation is managed by {manager} and is up to date.")
            }),
            managed_externally: package_manager.is_some(),
        }),
        Ok(Err(err)) => Err(format!("Update check failed: {err:#}")),
        Err(join_err) => Err(format!("Update task failed: {join_err}")),
    }
}

async fn auto_update_startup(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<UpdateCheckResponse, String> {
    if let Some(manager) = external_package_manager() {
        return Ok(UpdateCheckResponse {
            available: false,
            version: None,
            notes: Some(format!(
                "Updates are managed by {manager}. Update this application through your package manager."
            )),
            managed_externally: true,
        });
    }

    if !auto_update_enabled() {
        return Ok(UpdateCheckResponse {
            available: false,
            version: None,
            notes: Some("Auto update disabled by environment.".to_string()),
            managed_externally: false,
        });
    }

    let updater = state.context.updater.clone();

    let checked = updater.check_for_update().await;

    let Some(update) = (match checked {
        Ok(Ok(Some(update))) => Some(update),
        Ok(Ok(None)) => {
            return Ok(UpdateCheckResponse {
                available: false,
                version: None,
                notes: None,
                managed_externally: false,
            });
        }
        Ok(Err(err)) => return Err(format!("Update check failed: {err:#}")),
        Err(join_err) => return Err(format!("Update task failed: {join_err}")),
    }) else {
        return Ok(UpdateCheckResponse {
            available: false,
            version: None,
            notes: None,
            managed_externally: false,
        });
    };

    let _ = app.emit(
        "update-state",
        DownloadProgressEvent {
            kind: "update".to_string(),
            phase: "available".to_string(),
            artifact: None,
            index: None,
            total: None,
            received: None,
            size: None,
            folder: None,
            message: Some(format!("Update v{} available; installing.", update.version)),
        },
    );

    let install = updater.download_and_install(update.clone()).await;

    match install {
        Ok(Ok(applied)) => {
            let _ = app.emit(
                "update-state",
                DownloadProgressEvent {
                    kind: "update".to_string(),
                    phase: "restarting".to_string(),
                    artifact: None,
                    index: None,
                    total: None,
                    received: None,
                    size: None,
                    folder: None,
                    message: Some(format!(
                        "Update v{} installed; restarting application.",
                        applied.version
                    )),
                },
            );
            if let Err(err) = arctic_downloader::updater::relaunch_after_exit() {
                log::warn!("The update is installed but the app won't reopen by itself: {err:#}");
            }
            app.exit(0);
            Ok(UpdateCheckResponse {
                available: true,
                version: Some(applied.version.to_string()),
                notes: Some("Standalone update apply launched.".to_string()),
                managed_externally: false,
            })
        }
        Ok(Err(err)) => Err(format!("Update install failed: {err:#}")),
        Err(join_err) => Err(format!("Update install task failed: {join_err}")),
    }
}

pub(crate) fn effective_download_root(root: &Path) -> PathBuf {
    match comfy_extra_model_config(root) {
        Some(config) if config.is_default => {
            log::info!(
                "Using extra model base path for downloads: {}",
                config.base_path.display()
            );
            config.base_path
        }
        _ => root.to_path_buf(),
    }
}

pub(crate) fn comfy_extra_model_config(comfy_root: &Path) -> Option<ComfyExtraModelConfig> {
    let path = comfy_root.join("extra_model_paths.yaml");
    let content = std::fs::read_to_string(path).ok()?;
    let mut in_comfyui = false;
    let mut base_path: Option<PathBuf> = None;
    let mut is_default = false;

    for line in content.lines() {
        let without_comment = line.split('#').next().unwrap_or_default();
        if without_comment.trim().is_empty() {
            continue;
        }

        let indent = without_comment
            .chars()
            .take_while(|c| c.is_whitespace())
            .count();
        let trimmed = without_comment.trim();

        if trimmed == "comfyui:" {
            in_comfyui = true;
            continue;
        }

        if in_comfyui {
            if let Some(raw) = trimmed.strip_prefix("base_path:") {
                let scalar = parse_yaml_scalar(raw);
                if !scalar.trim().is_empty() {
                    let parsed = PathBuf::from(scalar.trim());
                    let resolved = if parsed.is_absolute() {
                        parsed
                    } else {
                        comfy_root.join(parsed)
                    };
                    base_path = Some(normalize_canonical_path(&resolved));
                }
                continue;
            }

            if let Some(raw) = trimmed.strip_prefix("is_default:") {
                let scalar = parse_yaml_scalar(raw);
                if let Some(parsed) = parse_yaml_bool(&scalar) {
                    is_default = parsed;
                }
                continue;
            }
        }

        if indent == 0 && trimmed.ends_with(':') {
            in_comfyui = false;
            continue;
        }

        if !in_comfyui {
            continue;
        }

        if let Some(raw) = trimmed.strip_prefix("base_path:") {
            let scalar = parse_yaml_scalar(raw);
            if scalar.trim().is_empty() {
                continue;
            }
            let parsed = PathBuf::from(scalar.trim());
            let resolved = if parsed.is_absolute() {
                parsed
            } else {
                comfy_root.join(parsed)
            };
            base_path = Some(normalize_canonical_path(&resolved));
            continue;
        }
    }

    base_path.map(|base| ComfyExtraModelConfig {
        base_path: base,
        is_default,
    })
}

fn inspect_comfyui_path(path: String) -> Result<ComfyPathInspection, String> {
    let selected = path.trim();
    if selected.is_empty() {
        return Err("Folder is empty.".to_string());
    }
    let selected_path = PathBuf::from(selected);
    if !selected_path.exists() || !selected_path.is_dir() {
        return Err("Folder does not exist.".to_string());
    }
    let normalized = std::fs::canonicalize(&selected_path).unwrap_or(selected_path.clone());
    let normalized = normalize_canonical_path(&normalized).to_path_buf();
    let detected_root = detect_existing_comfyui_root(&normalized)
        .map(|p| normalize_canonical_path(&p).to_string_lossy().to_string());
    Ok(ComfyPathInspection {
        selected: normalize_canonical_path(&normalized)
            .to_string_lossy()
            .to_string(),
        detected_root,
    })
}

fn open_folder(path: String) -> Result<String, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("Folder path is empty.".to_string());
    }
    let mut target = std::path::PathBuf::from(trimmed);
    if !target.is_absolute() {
        if let Ok(cwd) = std::env::current_dir() {
            target = cwd.join(target);
        }
    }
    if target.is_file() {
        if let Some(parent) = target.parent() {
            target = parent.to_path_buf();
        }
    }
    if let Ok(canon) = std::fs::canonicalize(&target) {
        target = canon;
    }
    if !target.exists() {
        return Err("Folder does not exist.".to_string());
    }

    open::that(target).map_err(|err| format!("Failed to open folder: {err}"))?;
    Ok(path)
}

fn open_external_url(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        return Err("Only http/https links are allowed.".to_string());
    }

    open::that(trimmed).map_err(|err| format!("Failed to open link: {err}"))?;
    Ok(())
}

fn pip_has_package(root: &Path, package: &str) -> bool {
    let mut cmd = python_for_root(root);
    cmd.arg("-m").arg("pip").arg("show").arg(package);
    cmd.current_dir(root);
    cmd.output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComfyComponentToggleRequest {
    #[serde(default)]
    comfyui_root: Option<String>,
    component: String,
    enabled: bool,
}

async fn apply_attention_backend_change(
    app: AppHandle,
    state: State<'_, AppState>,
    request: AttentionBackendChangeRequest,
) -> Result<String, String> {
    let was_running = stop_comfyui_for_mutation(&app, &state)?;
    let root = resolve_root_path(&state.context, request.comfyui_root)?;
    let target = request.target_backend.trim().to_ascii_lowercase();
    if !matches!(
        target.as_str(),
        "none" | "sage" | "sage3" | "flash" | "nunchaku"
    ) {
        return Err("Unknown attention backend target.".to_string());
    }
    if target == "sage3" {
        let gpu = detect_nvidia_gpu_details();
        let is_50_series = gpu
            .name
            .as_deref()
            .map(|name| name.to_ascii_lowercase().contains("rtx 50"))
            .unwrap_or(false);
        if !is_50_series {
            return Err(
                "SageAttention3 is available only for NVIDIA RTX 50-series GPUs.".to_string(),
            );
        }
    }

    let py_path = {
        let probe = python_for_root(&root);
        probe.get_program().to_string_lossy().to_string()
    };
    let py_exe = PathBuf::from(&py_path);
    let _ = kill_python_processes_for_root(&root, &py_exe);

    let shared_runtime_root = state.context.config.cache_path().join("comfyui-runtime");
    let uv_bin = resolve_uv_binary(&shared_runtime_root, &app)?;
    let uv_python_install_dir = shared_runtime_root
        .join(".python")
        .to_string_lossy()
        .to_string();
    let profile = if let Some(profile) = request.torch_profile.clone() {
        profile
    } else {
        profile_from_torch_env(&root)?
    };
    let hopper_sm90 = is_nvidia_hopper_sm90();
    let triton_pkg = triton_package_for_profile_linux(&profile);

    // Everything below is git clone + `uv pip install` + wheel downloads --
    // potentially minutes of network and subprocess I/O -- so it runs off
    // the command dispatch thread, matching
    // apply_comfyui_component_toggle's equivalent branch. The originals are
    // still available below the `.await` -- only clones are moved in.
    {
        let root = root.clone();
        let py_path = py_path.clone();
        let profile = profile.clone();
        let target = target.clone();
        let uv_bin = uv_bin.clone();
        let uv_python_install_dir = uv_python_install_dir.clone();
        let app = app.clone();
        crate::host::async_runtime::spawn_blocking(move || -> Result<(), String> {
            force_cleanup_attention_backends(&root, &py_path)?;

            match target.as_str() {
                "none" => {}
                "sage" => {
                    run_uv_pip_strict(
                        &uv_bin,
                        &py_path,
                        &["install", "--upgrade", "--force-reinstall", triton_pkg],
                        Some(&root),
                        &[("UV_PYTHON_INSTALL_DIR", &uv_python_install_dir)],
                    )?;
                    install_sageattention_linux(&root, &py_path, &profile, hopper_sm90)?;
                }
                "flash" => {
                    run_uv_pip_strict(
                        &uv_bin,
                        &py_path,
                        &["install", "--upgrade", "--force-reinstall", triton_pkg],
                        Some(&root),
                        &[("UV_PYTHON_INSTALL_DIR", &uv_python_install_dir)],
                    )?;
                    install_flashattention_linux(&root, &py_path, &profile, hopper_sm90)?;
                }
                "sage3" => {
                    run_uv_pip_strict(
                        &uv_bin,
                        &py_path,
                        &["install", "--upgrade", "--force-reinstall", triton_pkg],
                        Some(&root),
                        &[("UV_PYTHON_INSTALL_DIR", &uv_python_install_dir)],
                    )?;
                    install_linux_wheel_for_profile(
                        &root, &py_path, &profile, "sage3", hopper_sm90, true,
                    )?;
                    // Keep sageattention installed for ComfyUI --use-sage-attention compatibility checks.
                    install_sageattention_linux(&root, &py_path, &profile, hopper_sm90)?;
                }
                "nunchaku" => {
                    ensure_git_available(&app)?;
                    let custom_nodes_root = root.join("custom_nodes");
                    std::fs::create_dir_all(&custom_nodes_root).map_err(|err| err.to_string())?;
                    let nunchaku_node = root.join("custom_nodes").join("ComfyUI-nunchaku");
                    clone_or_update_repo(
                        &root,
                        &nunchaku_node,
                        "https://github.com/nunchaku-ai/ComfyUI-nunchaku",
                    )?;
                    let versions_json = nunchaku_node.join("nunchaku_versions.json");
                    let _ = download_http_file(
                        "https://nunchaku.tech/cdn/nunchaku_versions.json",
                        &versions_json,
                    );
                    run_uv_pip_strict(
                        &uv_bin,
                        &py_path,
                        &["install", "--upgrade", "--force-reinstall", triton_pkg],
                        Some(&root),
                        &[("UV_PYTHON_INSTALL_DIR", &uv_python_install_dir)],
                    )?;
                    install_insightface(&root, &uv_bin, &py_path, &uv_python_install_dir)?;
                    install_nunchaku_node_requirements(
                        &root,
                        &uv_bin,
                        &py_path,
                        &uv_python_install_dir,
                        &nunchaku_node,
                    )?;
                    install_linux_wheel_for_profile(
                        &root,
                        &py_path,
                        &profile,
                        "nunchaku",
                        hopper_sm90,
                        true,
                    )?;
                    if !nunchaku_backend_present(&root) {
                        return Err(
                            "Nunchaku backend install incomplete: module or custom node not detected."
                                .to_string(),
                        );
                    }
                }
                _ => return Err("Unknown attention backend target.".to_string()),
            }

            if target == "none" {
                let mut lingering: Vec<&str> = Vec::new();
                for pkg in [
                    "sageattention",
                    "sageattn3",
                    "flash-attn",
                    "flash_attn",
                    "nunchaku",
                ] {
                    if pip_has_package(&root, pkg) {
                        lingering.push(pkg);
                    }
                }
                let mut lingering_nodes: Vec<&str> = Vec::new();
                for node in ["ComfyUI-nunchaku", "nunchaku_nodes"] {
                    if custom_node_exists(&root, node) {
                        lingering_nodes.push(node);
                    }
                }
                if !lingering.is_empty() || !lingering_nodes.is_empty() {
                    let mut detail = String::new();
                    if !lingering.is_empty() {
                        detail.push_str(&format!(
                            "packages still installed: {}",
                            lingering.join(", ")
                        ));
                    }
                    if !lingering_nodes.is_empty() {
                        if !detail.is_empty() {
                            detail.push_str("; ");
                        }
                        detail.push_str(&format!(
                            "nodes still present: {}",
                            lingering_nodes.join(", ")
                        ));
                    }
                    return Err(format!(
                        "Attention backend removal incomplete ({detail}). Stop ComfyUI and retry."
                    ));
                }
            }

            Ok(())
        })
        .await
        .map_err(|err| format!("Attention backend change task failed: {err}"))??;
    }

    let target_setting = match target.as_str() {
        "sage" => Some("sage".to_string()),
        "sage3" => Some("sage3".to_string()),
        "flash" => Some("flash".to_string()),
        "nunchaku" => Some("nunchaku".to_string()),
        _ => Some("none".to_string()),
    };
    let _ = state.context.config.update_settings(|settings| {
        settings.comfyui_attention_backend = target_setting;
        settings.comfyui_torch_profile = Some(profile.clone());
    });

    restart_comfyui_after_mutation(&app, &state, was_running)?;
    Ok(format!("Applied attention backend: {target}"))
}

async fn set_comfyui_launch_attention_backend(
    app: AppHandle,
    state: State<'_, AppState>,
    request: LaunchAttentionFlagRequest,
) -> Result<String, String> {
    let was_running = stop_comfyui_for_mutation(&app, &state)?;
    let root = resolve_root_path(&state.context, request.comfyui_root)?;
    let target = request.target_backend.trim().to_ascii_lowercase();
    if !matches!(target.as_str(), "none" | "sage" | "sage3" | "flash") {
        return Err("Unknown launch attention backend target.".to_string());
    }

    // Module-importability checks shell out to Python; run off the command
    // dispatch thread like this file's other install/mutation commands.
    let unavailable = {
        let root = root.clone();
        let target = target.clone();
        crate::host::async_runtime::spawn_blocking(move || -> Option<&'static str> {
            match target.as_str() {
                "sage"
                    if !(python_module_importable(&root, "sageattention")
                        || python_module_importable(&root, "sageattn3")) =>
                {
                    Some("SageAttention launch flag is unavailable because SageAttention is not installed.")
                }
                "sage3" if !python_module_importable(&root, "sageattn3") => Some(
                    "SageAttention3 launch flag is unavailable because SageAttention3 is not installed.",
                ),
                "flash" if !python_module_importable(&root, "flash_attn") => Some(
                    "FlashAttention launch flag is unavailable because FlashAttention is not installed.",
                ),
                _ => None,
            }
        })
        .await
        .map_err(|err| format!("Launch attention backend check task failed: {err}"))?
    };
    if let Some(message) = unavailable {
        return Err(message.to_string());
    }

    let target_setting = match target.as_str() {
        "sage" => Some("sage".to_string()),
        "sage3" => Some("sage3".to_string()),
        "flash" => Some("flash".to_string()),
        _ => Some("none".to_string()),
    };
    state
        .context
        .config
        .update_settings(|settings| settings.comfyui_attention_backend = target_setting)
        .map_err(|err| err.to_string())?;

    restart_comfyui_after_mutation(&app, &state, was_running)?;
    Ok(match target.as_str() {
        "none" => "ComfyUI launch attention flags disabled.".to_string(),
        "sage" => "ComfyUI will launch with SageAttention.".to_string(),
        "sage3" => "ComfyUI will launch with SageAttention3.".to_string(),
        "flash" => "ComfyUI will launch with FlashAttention.".to_string(),
        _ => unreachable!(),
    })
}

async fn update_selected_comfyui(
    app: AppHandle,
    state: State<'_, AppState>,
    comfyui_root: Option<String>,
) -> Result<String, String> {
    let was_running = stop_comfyui_for_mutation(&app, &state)?;
    let root = resolve_root_path(&state.context, comfyui_root)?;
    if !root.join("main.py").is_file() {
        return Err("Selected folder is not a valid ComfyUI root.".to_string());
    }
    if !root.join(".git").exists() {
        return Err("Selected ComfyUI install is not git-based.".to_string());
    }

    let Some((latest_tag, latest_version)) = git_latest_release_tag(&root) else {
        return Err("Could not resolve latest ComfyUI release tag from remote.".to_string());
    };
    let installed_version_norm =
        read_comfyui_installed_version(&root).and_then(|v| normalize_release_version(&v));
    if let Some(current) = installed_version_norm {
        let current_triplet = parse_semver_triplet(&current);
        let latest_triplet = parse_semver_triplet(&latest_version);
        if matches!(
            (current_triplet, latest_triplet),
            (Some(local), Some(latest)) if local >= latest
        ) {
            return Ok(format!(
                "ComfyUI is already on latest release tag (v{latest_version})."
            ));
        }
    }

    let shared_runtime_root = state.context.config.cache_path().join("comfyui-runtime");
    let uv_bin = resolve_uv_binary(&shared_runtime_root, &app)?;
    let uv_python_install_dir = shared_runtime_root
        .join(".python")
        .to_string_lossy()
        .to_string();
    let selected_profile = resolve_desired_torch_profile(&state.context.config.settings(), &root);
    let latest_tag_for_task = latest_tag.clone();
    let latest_version_for_task = latest_version.clone();
    let branch_for_task_raw = git_current_branch(&root).unwrap_or_else(|| "master".to_string());
    let branch_for_task = if branch_for_task_raw.eq_ignore_ascii_case("head") {
        "master".to_string()
    } else {
        branch_for_task_raw
    };
    crate::host::async_runtime::spawn_blocking(move || -> Result<String, String> {
        // A snapshot to roll back to from ComfyUI-Manager's Snapshot Manager.
        if let Ok(py) = python_exe_for_root(&root) {
            match crate::shared::snapshots::save_comfyui_snapshot(
                &root,
                &py,
                Some(&uv_bin),
                "arctic-before-update",
            ) {
                Ok(path) => log::info!("Saved ComfyUI snapshot {}", path.display()),
                Err(err) => log::warn!("ComfyUI snapshot before update failed: {err}"),
            }
        }
        run_command_with_retry("git", &["fetch", "--tags", "origin"], Some(&root), 2)?;
        if let Err(err) =
            run_command_with_retry("git", &["merge", "--ff-only", &latest_tag_for_task], Some(&root), 2)
        {
            let lower = err.to_ascii_lowercase();
            let can_repoint_branch = lower.contains("unrelated histories")
                || lower.contains("not possible to fast-forward")
                || lower.contains("not possible to fast forward")
                || lower.contains("cannot fast-forward")
                || lower.contains("diverging");
            if can_repoint_branch {
                // Preserve recoverability before repointing branch tip to release tag.
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let backup_branch = format!("arctic-backup-before-tag-update-{ts}");
                run_command_with_retry("git", &["branch", &backup_branch], Some(&root), 1)
                    .map_err(|backup_err| {
                        format!(
                            "Failed to create backup branch before tag migration ({backup_branch}). Details: {backup_err}"
                        )
                    })?;
                run_command_with_retry(
                    "git",
                    &["checkout", "-B", &branch_for_task, &latest_tag_for_task],
                    Some(&root),
                    1,
                )
                .map_err(|checkout_err| {
                    format!(
                        "Failed to switch branch '{}' to release tag {} after merge fast-forward failed. Backup branch: {}. Details: {}",
                        branch_for_task, latest_tag_for_task, backup_branch, checkout_err
                    )
                })?;
            } else {
                return Err(format!(
                    "Failed to fast-forward ComfyUI to release tag {latest_tag_for_task}. Resolve local git divergence first. Details: {err}"
                ));
            }
        }

        let py = python_exe_for_root(&root)?;
        let req = root.join("requirements.txt");
        if req.exists() {
            run_uv_pip_strict(
                &uv_bin,
                py.to_string_lossy().as_ref(),
                &["install", "-r", "requirements.txt", "--no-cache"],
                Some(&root),
                &[("UV_PYTHON_INSTALL_DIR", &uv_python_install_dir)],
            )
            .map_err(|err| format!("Failed to install ComfyUI requirements: {err}"))?;
            // ComfyUI pins its built-in ComfyUI-Manager; follow the new pin.
            if let Some(manager_req) = builtin_manager_requirements(&root)
                .filter(|_| builtin_manager_installed(&py))
            {
                run_uv_pip_strict(
                    &uv_bin,
                    py.to_string_lossy().as_ref(),
                    &["install", "-r", &manager_req.to_string_lossy(), "--no-cache"],
                    Some(&root),
                    &[("UV_PYTHON_INSTALL_DIR", &uv_python_install_dir)],
                )
                .map_err(|err| format!("Failed to update ComfyUI-Manager: {err}"))?;
            }
            enforce_torch_profile_linux(
                &uv_bin,
                py.to_string_lossy().as_ref(),
                &root,
                &selected_profile,
                &uv_python_install_dir,
            )
            .map_err(|err| format!("Failed to re-apply selected torch profile: {err}"))?;
        }
        Ok(format!(
            "ComfyUI updated successfully to release tag {latest_tag_for_task} (v{latest_version_for_task})."
        ))
    })
    .await
    .map_err(|err| format!("ComfyUI update task failed: {err}"))??;

    restart_comfyui_after_mutation(&app, &state, was_running)?;
    Ok(format!(
        "ComfyUI updated successfully to release tag {latest_tag} (v{latest_version})."
    ))
}

fn cancel_active_download(state: State<'_, AppState>) -> Result<bool, String> {
    let mut active = recover_lock(state.active_cancel.lock());
    let mut abort = recover_lock(state.active_abort.lock());
    if let Some(token) = active.as_ref() {
        token.cancel();
        // Cancelling the token only stops the download at its next
        // cooperative checkpoint; force-abort the task too so a transfer
        // blocked in a non-cooperative call (e.g. a stalled read) is
        // actually torn down instead of continuing in the background after
        // the UI reports "cancelled". Matches app_windows.rs.
        if let Some(handle) = abort.take() {
            handle.abort();
        }
        *active = None;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Reads the command-line flags, starts logging and builds the app state.
/// The UI (`main.rs`) runs after this.
pub fn init() -> AppState {
    let args: Vec<String> = std::env::args().collect();
    let nerdstats = args
        .iter()
        .any(|arg| arg.eq_ignore_ascii_case("--nerdstats"));
    let fakeamd = args.iter().any(|arg| arg.eq_ignore_ascii_case("--fakeamd"));
    let fakeintel = args
        .iter()
        .any(|arg| arg.eq_ignore_ascii_case("--fakeintel"));
    let fakeamd_allow_rocm_setup = args
        .iter()
        .any(|arg| arg.eq_ignore_ascii_case("--fakeamd-allow-rocm-setup"));
    let fakeintel_allow_xpu_setup = args
        .iter()
        .any(|arg| arg.eq_ignore_ascii_case("--fakeintel-allow-xpu-setup"));
    if nerdstats {
        std::env::set_var("ARCTIC_NERDSTATS", "1");
    }
    if fakeamd {
        std::env::set_var("ARCTIC_FAKE_AMD", "1");
    }
    if fakeintel {
        std::env::set_var("ARCTIC_FAKE_INTEL", "1");
    }
    if fakeamd_allow_rocm_setup {
        std::env::set_var("ARCTIC_FAKE_AMD_ALLOW_ROCM_SETUP", "1");
    }
    if fakeintel_allow_xpu_setup {
        std::env::set_var("ARCTIC_FAKE_INTEL_ALLOW_XPU_SETUP", "1");
    }
    if nerdstats {
        try_attach_parent_console();
    }
    env_logger::Builder::from_default_env()
        .filter_level(if nerdstats {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Info
        })
        // Stdout plus the log file users can attach to bug reports.
        .target(env_logger::Target::Pipe(Box::new(
            arctic_downloader::applog::log_writer(),
        )))
        .init();

    if nerdstats {
        log::info!("Nerdstats mode enabled (verbose runtime logging).");
    }
    if fakeamd {
        if fakeamd_allow_rocm_setup {
            log::info!("Fake AMD mode enabled with real guided ROCm setup allowed for testing.");
        } else {
            log::info!("Fake AMD mode enabled (UI simulation only; guided ROCm setup disabled).");
        }
    }
    if fakeintel {
        if fakeintel_allow_xpu_setup {
            log::info!("Fake Intel mode enabled with real guided Intel setup allowed for testing.");
        } else {
            log::info!(
                "Fake Intel mode enabled (UI simulation only; guided Intel setup disabled)."
            );
        }
    }

    let context = match build_context() {
        Ok(context) => context,
        Err(err) => {
            eprintln!("Failed to initialize app context: {err:#}");
            std::process::exit(1);
        }
    };
    #[cfg(target_os = "linux")]
    warm_linux_prereq_cache_background();

    AppState {
        context,
        active_cancel: Mutex::new(None),
        active_abort: Mutex::new(None),
        install_cancel: Mutex::new(None),
        comfyui_process: Mutex::new(None),
        comfyui_process_root: Mutex::new(None),
        quitting: Mutex::new(false),
    }
}

// Renamed from `gpu_detection_tests`: the one test that was actually about
// GPU *detection* (parsing `lspci` output) moved to
// `app_linux/gpu_detection.rs` alongside the code it tests. What's left here
// tests install-recommendation/torch-profile/HF-Xet logic that happens to
// take GPU details as input, not GPU detection itself.
#[cfg(test)]
mod install_recommendation_tests {
    use super::*;

    #[test]
    fn mixed_gpu_recommendation_prefers_nvidia_cuda() {
        let recommendation = comfy_install_recommendation_for(
            NvidiaGpuDetails {
                name: Some("NVIDIA GeForce RTX 5060 Ti".to_string()),
                vram_mb: Some(16_384),
                driver_version: Some("580.95.05".to_string()),
                compute_capability: Some("12.0".to_string()),
            },
            Some("AMD Radeon RX 6700 XT".to_string()),
            None,
            None,
            false,
        );

        assert_eq!(
            recommendation.gpu_name.as_deref(),
            Some("NVIDIA GeForce RTX 5060 Ti")
        );
        assert_eq!(recommendation.torch_profile, "torch2141_cu130");
        assert!(!recommendation.detection_pending);
    }

    #[test]
    fn nvidia_recommendation_needs_driver_580_for_cuda13() {
        let gpu = |driver: &str| NvidiaGpuDetails {
            name: Some("NVIDIA GeForce RTX 3080".to_string()),
            vram_mb: Some(10_240),
            driver_version: Some(driver.to_string()),
            compute_capability: Some("8.6".to_string()),
        };
        let current = comfy_install_recommendation_for(gpu("580.95.05"), None, None, None, false);
        assert_eq!(current.torch_profile, "torch2141_cu130");
        let old_driver =
            comfy_install_recommendation_for(gpu("570.172.08"), None, None, None, false);
        assert_eq!(old_driver.torch_profile, "torch271_cu128");
    }

    #[test]
    fn explicit_amd_selection_uses_rocm_on_mixed_gpu_system() {
        let recommendation = comfy_install_recommendation_for(
            NvidiaGpuDetails {
                name: Some("NVIDIA GeForce RTX 5060 Ti".to_string()),
                vram_mb: Some(16_384),
                driver_version: Some("580.95.05".to_string()),
                compute_capability: Some("12.0".to_string()),
            },
            Some("AMD Radeon RX 6700 XT".to_string()),
            None,
            Some("amd"),
            false,
        );

        assert_eq!(
            recommendation.gpu_name.as_deref(),
            Some("AMD Radeon RX 6700 XT")
        );
        assert_eq!(recommendation.torch_profile, "torch2141_rocm72");
    }

    #[test]
    fn rocm72_profile_uses_matching_pytorch_packages_and_is_detectable() {
        use super::torch_env::{torch_profile_from_versions, torch_profile_to_packages_linux};
        assert_eq!(
            torch_profile_to_packages_linux("torch211_rocm72"),
            (
                "2.11.0",
                "0.26.0",
                "2.11.0",
                "https://download.pytorch.org/whl/rocm7.2",
            )
        );
        assert_eq!(
            torch_profile_from_versions("2.11.0+rocm7.2", "7.2.41134").as_deref(),
            Some("torch211_rocm72")
        );
    }

    #[test]
    fn torch214_profiles_pin_decoupled_torchaudio_and_are_detectable() {
        use super::torch_env::{torch_profile_from_versions, torch_profile_to_packages_linux};
        assert_eq!(
            torch_profile_to_packages_linux("torch2141_rocm72"),
            (
                "2.14.1",
                "0.29.1",
                "2.11.0",
                "https://download.pytorch.org/whl/rocm7.2",
            )
        );
        assert_eq!(
            torch_profile_to_packages_linux("torch2141_xpu"),
            (
                "2.14.1",
                "0.29.1",
                "2.11.0",
                "https://download.pytorch.org/whl/xpu",
            )
        );
        assert_eq!(
            torch_profile_from_versions("2.14.1+rocm7.2", "7.2.53211").as_deref(),
            Some("torch2141_rocm72")
        );
        assert_eq!(
            torch_profile_from_versions("2.14.1+xpu", "xpu").as_deref(),
            Some("torch2141_xpu")
        );
        // Existing installs on the previous profiles stay recognised.
        assert_eq!(
            torch_profile_from_versions("2.9.1+xpu", "xpu").as_deref(),
            Some("torch291_xpu")
        );
    }

    #[test]
    fn disabled_hf_xet_preflight_does_not_require_a_cli_probe() {
        let preflight = get_hf_xet_preflight_internal(false);
        assert!(!preflight.xet_enabled);
        assert!(!preflight.hf_cli_available);
        assert_eq!(preflight.hf_backend, "disabled");
        assert!(preflight.detail.contains("default downloader"));
    }
}

#[cfg(test)]
mod tls_environment_tests {
    use super::*;

    #[test]
    fn existing_ssl_cert_file_override_wins() {
        let inherited = std::ffi::OsString::from("/custom/ca-bundle.pem");
        let candidates = vec![std::env::current_exe().expect("current executable path")];

        assert_eq!(
            select_ssl_cert_file(Some(inherited.clone()), &candidates),
            Some(inherited)
        );
    }

    #[test]
    fn first_existing_ca_bundle_is_selected() {
        let existing = std::env::current_exe().expect("current executable path");
        let candidates = vec![
            PathBuf::from("/definitely/missing/arctic-helper-ca-bundle.pem"),
            existing.clone(),
        ];

        assert_eq!(
            select_ssl_cert_file(None, &candidates),
            Some(existing.into_os_string())
        );
    }
}
