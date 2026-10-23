// Platform backend composed from focused sibling modules; see
// docs/cross-platform-development.md ("Consolidation status").
mod addons;
pub(crate) mod commands;
mod custom_nodes;
mod gpu_detection;
mod install;
mod install_state;
mod process_guard;
mod runtime;
mod torch_env;
// `start_comfyui_install`/`apply_comfyui_component_toggle` are
// `#[tauri::command]`s with no other caller in this file, so they're
// referenced by qualified path directly in `generate_handler!` in `run()`
// below -- see `install_state.rs`'s doc comment for why.
pub(crate) use process_guard::{release_comfy_job_object, track_comfy_job_object};
pub(crate) use runtime::{
    git_latest_release_tag, host_comfyui_root_among, kill_python_processes_for_root,
    python_exe_for_root, python_for_root, resolve_root_path, restart_comfyui_after_mutation,
    spawn_comfyui_start_monitor, start_comfyui_root_impl, stop_comfyui_root_impl,
};
// `set_comfyui_install_base`/`list_comfyui_installations`/`get_comfyui_addon_state`
// are `#[tauri::command]`s with no other caller in this file, so they're
// referenced by qualified path directly in `generate_handler!` in `run()`
// below rather than re-exported here -- see `install_state.rs`'s doc comment
// (and the matching one in `app_linux.rs`) for why.
pub(crate) use crate::shared::manager::{
    comfyui_manager_installed, install_comfyui_manager, install_named_custom_node,
    uninstall_comfyui_manager,
};
pub(crate) use addons::{
    ensure_insightface_runtime_compat, finalize_nunchaku_install, install_insightface,
    install_nunchaku_node_requirements, install_trellis2, uninstall_insightface,
    uninstall_trellis2,
};
pub(crate) use custom_nodes::{custom_node_spec, install_custom_node, CUSTOM_NODES};
// Unlike Linux, nothing outside gpu_detection.rs names `NvidiaGpuDetails`
// directly (Windows' install-recommendation code only uses a `detect_*()`
// return value through inference, never a literal struct), so it isn't
// re-exported here -- doing so anyway would just be an unused-import
// warning waiting to happen.
pub(crate) use crate::shared::tray::update_tray_comfy_status;
pub(crate) use gpu_detection::{
    detect_amd_gpu_details, detect_intel_gpu_details, detect_nvidia_gpu_details,
};
pub(crate) use torch_env::{
    apply_intel_xpu_launch_env, attention_wheel_url, comfyui_launch_args,
    detect_launch_attention_backend_for_root, detect_torch_profile_for_root,
    ensure_uv_python_installed, ensure_venv_pip, find_file_recursive, install_wheel_no_deps,
    is_non_cuda_profile, is_rocm_profile, is_xpu_profile, nunchaku_backend_present,
    profile_from_torch_env, python_module_import_error, python_module_importable,
    reassert_torch_stack_for_profile, resolve_uv_binary, rocm_device_target, run_uv_pip_strict,
    selected_attention_backend, uv_pip_uninstall_best_effort,
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
    emit_comfyui_runtime_event, emit_install_event, git_current_branch, has_dns, nerdstats_enabled,
    normalize_release_version, output_with_optional_timeout, parse_hf_env_value,
    parse_semver_triplet, parse_yaml_bool, parse_yaml_scalar, push_preflight,
    read_comfyui_installed_version, recover_lock, run_with_timeout_capturing_output,
    status_with_optional_timeout, stop_comfyui_for_mutation, yaml_single_quote, AppState,
    ComfyExtraModelConfig, DownloadProgressEvent,
};
use arctic_downloader::{
    app::build_context, config::AppSettings, env_flags::auto_update_enabled,
    ram::detect_ram_profile,
};
use serde::Deserialize;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

const COMMAND_PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const HF_ENV_PROBE_TIMEOUT: Duration = Duration::from_secs(30);

fn get_app_snapshot(state: State<'_, AppState>) -> AppSnapshot {
    let catalog = state.context.catalog.catalog_snapshot();
    let (nvidia_gpu_name, nvidia_gpu_vram_mb) = detect_nvidia_gpu();
    let amd_gpu_name = detect_amd_gpu_name();
    let intel_gpu_name = detect_intel_gpu_name();
    let gpu_detection_pending =
        nvidia_gpu_name.is_none() && amd_gpu_name.is_none() && intel_gpu_name.is_none();
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

fn windows_rocm_supported_gpu(name: &str) -> bool {
    rocm_device_target(name).is_some()
}

fn windows_xpu_supported_gpu(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    ["intel arc", "arc a", "arc b", "arc 1", "core ultra"]
        .iter()
        .any(|needle| lower.contains(needle))
}

fn get_comfyui_install_recommendation(gpu_selection: Option<String>) -> ComfyInstallRecommendation {
    let selection = gpu_selection.unwrap_or_else(|| "auto".to_string());
    let selection = selection.trim().to_ascii_lowercase();
    let gpu = detect_nvidia_gpu_details();
    let gpu_name = gpu.name.clone().unwrap_or_default().to_ascii_lowercase();
    let amd_name = detect_amd_gpu_name();
    let intel_name = detect_intel_gpu_name();
    if selection == "amd" || (selection == "auto" && gpu.name.is_none()) {
        if let Some(amd_name) = amd_name.clone() {
            let reason = if windows_rocm_supported_gpu(&amd_name) {
                if selection == "amd" {
                    "Selected supported AMD GPU; using Windows ROCm install profile.".to_string()
                } else {
                    "Detected supported AMD GPU; selecting Windows ROCm install profile."
                        .to_string()
                }
            } else {
                "Selected AMD GPU. It isn't on the ROCm GPU list, so every ROCm device target will be installed (a larger download); ROCm on Windows supports RDNA 2 and newer Radeon and Ryzen AI GPUs."
                    .to_string()
            };
            return ComfyInstallRecommendation {
                gpu_name: Some(amd_name),
                driver_version: None,
                torch_profile: "torch2140_rocm101".to_string(),
                torch_label: "Torch 2.14.0 + ROCm 10.1".to_string(),
                reason,
                detection_pending: false,
            };
        }
        if selection == "amd" {
            return ComfyInstallRecommendation {
                gpu_name: None,
                driver_version: None,
                torch_profile: "torch2140_rocm101".to_string(),
                torch_label: "Torch 2.14.0 + ROCm 10.1".to_string(),
                reason: "Selected AMD GPU is still being detected.".to_string(),
                detection_pending: true,
            };
        }
    }
    if selection == "intel" || (selection == "auto" && gpu.name.is_none() && amd_name.is_none()) {
        if let Some(intel_name) = intel_name {
            let reason = if windows_xpu_supported_gpu(&intel_name) {
                if selection == "intel" {
                    "Selected Intel GPU; using PyTorch XPU install profile.".to_string()
                } else {
                    "Detected Intel GPU; selecting PyTorch XPU install profile.".to_string()
                }
            } else {
                "Detected Intel GPU. PyTorch XPU supports Intel Arc GPUs and Core Ultra processors with Arc graphics; other Intel GPUs may not work."
                    .to_string()
            };
            return ComfyInstallRecommendation {
                gpu_name: Some(intel_name),
                driver_version: None,
                torch_profile: "torch2141_xpu".to_string(),
                torch_label: "Torch 2.14.1 + XPU".to_string(),
                reason,
                detection_pending: false,
            };
        }
        if selection == "intel" {
            return ComfyInstallRecommendation {
                gpu_name: None,
                driver_version: None,
                torch_profile: "torch2141_xpu".to_string(),
                torch_label: "Torch 2.14.1 + XPU".to_string(),
                reason: "Selected Intel GPU is still being detected.".to_string(),
                detection_pending: true,
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
            reason: "Detected NVIDIA GPU (Turing or newer) with driver >= 580; using CUDA 13."
                .to_string(),
            detection_pending: false,
        };
    }

    // Drivers older than 580 (or pre-Turing GPUs) stay on CUDA 12.8.
    if gpu_name.contains("rtx 30") {
        return ComfyInstallRecommendation {
            gpu_name: gpu.name,
            driver_version: gpu.driver_version,
            torch_profile: "torch271_cu128".to_string(),
            torch_label: "Torch 2.7.1 + cu128".to_string(),
            reason: "Detected RTX 3000 series (Ampere).".to_string(),
            detection_pending: false,
        };
    }

    if gpu_name.contains("rtx 40") {
        return ComfyInstallRecommendation {
            gpu_name: gpu.name,
            driver_version: gpu.driver_version,
            torch_profile: "torch280_cu128".to_string(),
            torch_label: "Torch 2.8.0 + cu128".to_string(),
            reason: "Detected RTX 4000 series (Ada).".to_string(),
            detection_pending: false,
        };
    }

    if gpu_name.contains("rtx 50") {
        return ComfyInstallRecommendation {
            gpu_name: gpu.name,
            driver_version: gpu.driver_version,
            torch_profile: "torch280_cu128".to_string(),
            torch_label: "Torch 2.8.0 + cu128".to_string(),
            reason: "Detected RTX 5000 series with older driver; using safer fallback.".to_string(),
            detection_pending: false,
        };
    }

    ComfyInstallRecommendation {
        detection_pending: gpu.name.is_none(),
        gpu_name: gpu.name,
        driver_version: gpu.driver_version,
        torch_profile: "torch280_cu128".to_string(),
        torch_label: "Torch 2.8.0 + cu128".to_string(),
        reason: "Unknown or non-NVIDIA GPU; using default recommendation.".to_string(),
    }
}

pub(crate) fn normalize_path(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Install folder is required.".to_string());
    }
    let mut path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        path = std::env::current_dir()
            .map_err(|err| err.to_string())?
            .join(path);
    }
    // Best-effort canonicalize (falls back to the input unchanged if the
    // path doesn't exist yet, e.g. a fresh install target that hasn't been
    // created). Unlike this function's other callers -- `resolve_root_path`
    // canonicalizes explicitly, `set_comfyui_root`/`set_comfyui_install_base`
    // too -- this path didn't, so a root containing a reparse
    // point/junction could be stored one way at install time and resolved
    // a different way at launch time, silently discarding saved launch
    // settings because the two forms compared unequal.
    let canonical = std::fs::canonicalize(&path).unwrap_or(path);
    Ok(strip_windows_verbatim_prefix(&canonical))
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

    let base = yaml_single_quote(&strip_windows_verbatim_prefix(base_path).to_string_lossy());
    let default_value = if is_default { "true" } else { "false" };
    let yaml = format!(
        r#"# Managed by ComfyPilot.
comfyui:
  base_path: {base}
  is_default: {default_value}
  checkpoints: models/checkpoints/
  text_encoders: |
    models/text_encoders/
    models/clip/
  clip_vision: models/clip_vision/
  configs: models/configs/
  controlnet: models/controlnet/
  diffusion_models: |
    models/diffusion_models/
    models/unet/
  embeddings: models/embeddings/
  loras: models/loras/
  upscale_models: models/upscale_models/
  vae: models/vae/
  audio_encoders: models/audio_encoders/
  model_patches: models/model_patches/
"#
    );

    std::fs::write(&target, yaml).map_err(|err| {
        format!(
            "failed to write extra model paths config '{}': {err}",
            target.display()
        )
    })?;

    Ok(target)
}

fn is_forbidden_install_path(path: &Path) -> bool {
    let normalized = path
        .to_string_lossy()
        .to_ascii_lowercase()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_string();

    if normalized == "c:" {
        return true;
    }

    let blocked_prefixes = [
        "c:\\windows",
        "c:\\program files",
        "c:\\program files (x86)",
    ];
    blocked_prefixes
        .iter()
        .any(|entry| normalized == *entry || normalized.starts_with(&format!("{entry}\\")))
}

fn strip_windows_verbatim_prefix(path: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let raw = path.to_string_lossy().to_string();
        if let Some(stripped) = raw.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{}", stripped));
        }
        if let Some(stripped) = raw.strip_prefix(r"\\?\") {
            return PathBuf::from(stripped);
        }
        PathBuf::from(raw)
    }

    #[cfg(not(target_os = "windows"))]
    {
        path.to_path_buf()
    }
}

fn command_available(program: &str, args: &[&str]) -> bool {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    apply_background_command_flags(&mut cmd);
    run_with_timeout_capturing_output(cmd, COMMAND_PROBE_TIMEOUT)
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn apply_background_command_flags(cmd: &mut std::process::Command) {
    #[cfg(target_os = "windows")]
    {
        // Prevent Windows from opening a new console window per installer subprocess.
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
}

#[cfg(target_os = "windows")]
fn try_attach_parent_console() {
    // ATTACH_PARENT_PROCESS from Win32 API
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    unsafe extern "system" {
        fn AttachConsole(dw_process_id: u32) -> i32;
    }
    // Best-effort: if no parent console exists, this simply fails.
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

#[cfg(not(target_os = "windows"))]
fn try_attach_parent_console() {}

fn refresh_git_path_for_current_process() {
    #[cfg(target_os = "windows")]
    {
        let mut values: Vec<String> = std::env::var_os("PATH")
            .map(|value| {
                std::env::split_paths(&value)
                    .map(|p| p.to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default();

        let mut add_candidate = |path: PathBuf| {
            if !path.exists() {
                return;
            }
            let value = path.to_string_lossy().to_string();
            if !values
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&value))
            {
                values.push(value);
            }
        };

        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            add_candidate(PathBuf::from(program_files).join("Git").join("cmd"));
        }
        if let Some(program_files_x86) = std::env::var_os("ProgramFiles(x86)") {
            add_candidate(PathBuf::from(program_files_x86).join("Git").join("cmd"));
        }
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            add_candidate(
                PathBuf::from(local_app_data)
                    .join("Programs")
                    .join("Git")
                    .join("cmd"),
            );
        }

        if let Ok(joined) = std::env::join_paths(values.iter().map(PathBuf::from)) {
            std::env::set_var("PATH", joined);
        }
    }
}

pub(crate) fn ensure_git_available(app: &AppHandle) -> Result<(), String> {
    if command_available("git", &["--version"]) {
        return Ok(());
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        return Err("Git is not available in PATH. Install Git and retry.".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        if !command_available("winget", &["--version"]) {
            return Err(
                "Git is missing and winget is unavailable. Install Git manually and retry."
                    .to_string(),
            );
        }

        emit_install_event(app, "step", "Git not found; installing Git via winget...");
        let mut winget_cmd = std::process::Command::new("winget");
        winget_cmd.args([
            "install",
            "--id",
            "Git.Git",
            "-e",
            "--source",
            "winget",
            "--accept-package-agreements",
            "--accept-source-agreements",
        ]);
        apply_background_command_flags(&mut winget_cmd);
        let status = winget_cmd
            .status()
            .map_err(|err| format!("Failed to launch winget: {err}"))?;

        if !status.success() {
            return Err(
                "Git installation via winget failed. Install Git manually and retry.".to_string(),
            );
        }

        refresh_git_path_for_current_process();
        if command_available("git", &["--version"]) {
            emit_install_event(app, "info", "Git installed successfully.");
            Ok(())
        } else {
            Err(
                "Git installed but not available in PATH for this session. Restart app and retry."
                    .to_string(),
            )
        }
    }
}

fn prepend_path_entry_if_missing(entry: &Path) {
    let abs_entry = match std::fs::canonicalize(entry) {
        Ok(path) => path,
        Err(_) => entry.to_path_buf(),
    };
    let mut values: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|value| std::env::split_paths(&value).collect())
        .unwrap_or_default();
    let entry_s = abs_entry.to_string_lossy().to_ascii_lowercase();
    let already_present = values
        .iter()
        .any(|p| p.to_string_lossy().to_ascii_lowercase() == entry_s);
    if already_present {
        return;
    }
    values.insert(0, abs_entry);
    if let Ok(joined) = std::env::join_paths(values) {
        std::env::set_var("PATH", joined);
    }
}

fn add_local_uv_tools_to_path(shared_runtime_root: &Path) {
    let local_root = shared_runtime_root.join(".tools").join("uv");
    if local_root.exists() {
        prepend_path_entry_if_missing(&local_root);
    }
    if let Some(found) = find_file_recursive(&local_root, "uv.exe") {
        if let Some(parent) = found.parent() {
            prepend_path_entry_if_missing(parent);
        }
    }
    if let Some(legacy_runtime_root) = shared_runtime_root
        .parent()
        .map(|parent| parent.join("comfy_runtime"))
    {
        let legacy_local_root = legacy_runtime_root.join(".tools").join("uv");
        if legacy_local_root.exists() {
            prepend_path_entry_if_missing(&legacy_local_root);
        }
        if let Some(found) = find_file_recursive(&legacy_local_root, "uv.exe") {
            if let Some(parent) = found.parent() {
                prepend_path_entry_if_missing(parent);
            }
        }
    }
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
    let state = app.state::<AppState>();
    let shared_runtime_root = state.context.config.cache_path().join("comfyui-runtime");
    add_local_uv_tools_to_path(&shared_runtime_root);
    let xet_enabled = state.context.config.settings().hf_xet_enabled;
    crate::host::async_runtime::spawn_blocking(move || get_hf_xet_preflight_internal(xet_enabled))
        .await
        .map_err(|err| format!("HF/Xet preflight task failed: {err}"))
}

fn ensure_hf_xet_runtime_installed(
    app: &AppHandle,
    shared_runtime_root: &Path,
    always_upgrade: bool,
) -> Result<(), String> {
    add_local_uv_tools_to_path(shared_runtime_root);
    let before = get_hf_xet_preflight_internal(true);

    let mut attempts: Vec<String> = Vec::new();
    let uv_bin = resolve_uv_binary(shared_runtime_root, app)?;
    if uv_bin != "uv" {
        if let Some(parent) = Path::new(&uv_bin).parent() {
            prepend_path_entry_if_missing(parent);
        }
    }
    if always_upgrade || !before.hf_xet_installed {
        match run_command_capture(
            &uv_bin,
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
                    "{} tool install --upgrade --force huggingface_hub[hf_xet] => {err}",
                    uv_bin
                ));
            }
        }
    }

    add_local_uv_tools_to_path(shared_runtime_root);
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

fn set_hf_xet_enabled(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<AppSettings, String> {
    if enabled {
        let shared_runtime_root = state.context.config.cache_path().join("comfyui-runtime");
        ensure_hf_xet_runtime_installed(&app, &shared_runtime_root, true)?;
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
            "Folder is blocked (avoid C:\\, Windows, Program Files).",
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
    } else if command_available("winget", &["--version"]) {
        push_preflight(
            &mut items,
            "warn",
            "Git",
            "Git is missing in PATH. Installer will attempt winget install automatically.",
        );
    } else {
        ok = false;
        push_preflight(
            &mut items,
            "fail",
            "Git",
            "Git is not available and winget is missing. Install Git manually.",
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

    let cache_root = state.context.config.cache_path();
    let runtime_roots = [
        cache_root.join("comfyui-runtime"),
        cache_root.join("comfy_runtime"),
    ];
    let local_uv_exists = runtime_roots.iter().any(|runtime_root| {
        let local_uv_root = runtime_root.join(".tools").join("uv");
        local_uv_root.join("uv.exe").exists()
            || local_uv_root.join("uv").exists()
            || find_file_recursive(&local_uv_root, "uv.exe").is_some()
            || find_file_recursive(&local_uv_root, "uv").is_some()
    });

    if command_available("uv", &["--version"]) {
        push_preflight(&mut items, "pass", "uv runtime", "System uv detected.");
    } else if local_uv_exists {
        push_preflight(
            &mut items,
            "pass",
            "uv runtime",
            "Local uv runtime already available.",
        );
    } else {
        push_preflight(
            &mut items,
            "warn",
            "uv runtime",
            "System uv not found. Installer will download a local uv runtime.",
        );
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

    if request.include_trellis2 {
        let recommendation = get_comfyui_install_recommendation(None);
        let selected_profile = request
            .torch_profile
            .clone()
            .unwrap_or(recommendation.torch_profile);
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

fn powershell_download(url: &str, out_file: &Path) -> Result<(), String> {
    let parent = out_file
        .parent()
        .ok_or_else(|| "Invalid output path.".to_string())?;
    std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    // `url` and `out_file` are frequently derived from a user-chosen install
    // folder (e.g. a path containing an apostrophe like `O'Brien`), so they
    // must be escaped for PowerShell's single-quoted string literals before
    // being spliced into `-Command`. Without this, a quote in the path
    // breaks out of the string and the remainder is executed as PowerShell.
    let url_escaped = powershell_single_quote(url);
    let out_file_escaped = powershell_single_quote(&out_file.display().to_string());
    let command = format!(
        "try {{ Invoke-WebRequest '{}' -OutFile '{}' -UseBasicParsing -ErrorAction Stop }} catch {{ curl.exe -L '{}' -o '{}' }}",
        url_escaped,
        out_file_escaped,
        url_escaped,
        out_file_escaped
    );
    let mut cmd = std::process::Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &command,
    ]);
    apply_background_command_flags(&mut cmd);
    let status = cmd
        .status()
        .map_err(|err| format!("Failed to launch downloader: {err}"))?;
    if !status.success() {
        return Err(format!("Download failed: {url}"));
    }
    Ok(())
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

    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        // Bound connection setup so an unreachable/stalled host fails fast
        // instead of hanging indefinitely; `timeout` is a generous overall
        // safety net (not a per-chunk stall timeout) sized to still allow a
        // large, slow multi-GB model download to complete.
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(2 * 60 * 60))
        .build()
        .map_err(|err| format!("Failed to build HTTP client: {err}"))?;

    let mut response = client
        .get(url)
        .header(
            "User-Agent",
            format!("ComfyPilot/{}", env!("CARGO_PKG_VERSION")),
        )
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|err| format!("HTTP download failed for {url}: {err}"))?;

    let tmp_file = out_file.with_extension("download");
    let mut file = std::fs::File::create(&tmp_file)
        .map_err(|err| format!("Failed to create file {}: {err}", tmp_file.display()))?;

    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = response
            .read(&mut buffer)
            .map_err(|err| format!("Failed while reading {url}: {err}"))?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .map_err(|err| format!("Failed writing {}: {err}", tmp_file.display()))?;
    }
    file.flush()
        .map_err(|err| format!("Failed to flush {}: {err}", tmp_file.display()))?;

    std::fs::rename(&tmp_file, out_file).map_err(|err| {
        format!(
            "Failed to finalize download {} -> {}: {err}",
            tmp_file.display(),
            out_file.display()
        )
    })?;
    Ok(())
}

fn download_nunchaku_versions_json(app: &AppHandle, out_file: &Path) -> Result<(), String> {
    let url = "https://nunchaku.tech/cdn/nunchaku_versions.json";
    if let Ok(()) = powershell_download(url, out_file) {
        return Ok(());
    }

    // Fallback for systems with strict revocation/cert path issues.
    let mut curl_cmd = std::process::Command::new("curl.exe");
    curl_cmd
        .args(["-L", "--ssl-no-revoke", url, "-o"])
        .arg(out_file);
    apply_background_command_flags(&mut curl_cmd);
    let curl_status = curl_cmd.status();
    match curl_status {
        Ok(status) if status.success() => Ok(()),
        _ => {
            emit_comfyui_runtime_event(
                app,
                "warn",
                "Could not download nunchaku_versions.json; continuing without it.",
            );
            Err("nunchaku_versions.json download failed".to_string())
        }
    }
}

fn run_command(program: &str, args: &[&str], working_dir: Option<&Path>) -> Result<(), String> {
    log::debug!("run_command: {} {}", program, args.join(" "));
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }
    apply_background_command_flags(&mut cmd);
    let status = status_with_optional_timeout(program, cmd)
        .map_err(|err| format!("Failed to run {program}: {err}"))?;
    if !status.success() {
        return Err(format!("Command failed: {} {}", program, args.join(" ")));
    }
    Ok(())
}

pub(crate) fn run_command_capture(
    program: &str,
    args: &[&str],
    working_dir: Option<&Path>,
) -> Result<(String, String), String> {
    log::debug!("run_command_capture: {} {}", program, args.join(" "));
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }
    apply_background_command_flags(&mut cmd);
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
        return Err(enrich_command_failure_message(
            program, args, &stdout, &stderr,
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

fn enrich_command_failure_message(
    program: &str,
    args: &[&str],
    stdout: &str,
    stderr: &str,
) -> String {
    let tail = if stderr.trim().is_empty() {
        stdout
            .lines()
            .rev()
            .take(12)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        stderr
            .lines()
            .rev()
            .take(12)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n")
    };
    let combined = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    let msvc_hint = if combined.contains("microsoft visual c++ 14.0 or greater is required")
        || combined.contains("msvc")
        || combined.contains("visual studio build tools")
        || combined.contains("unable to find vcvarsall.bat")
    {
        " Missing dependency: Microsoft Visual C++ Build Tools 14.0 or newer. Install Visual Studio Build Tools with the C++ build tools workload, then retry."
    } else {
        ""
    };
    if tail.trim().is_empty() {
        format!(
            "Command failed: {} {}.{}",
            program,
            args.join(" "),
            msvc_hint
        )
    } else {
        format!(
            "Command failed: {} {} :: {}{}",
            program,
            args.join(" "),
            tail,
            msvc_hint
        )
    }
}

fn looks_like_missing_msvc_tools(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("microsoft visual c++ 14.0 or greater is required")
        || lower.contains("visual studio build tools")
        || lower.contains("unable to find vcvarsall.bat")
        || lower.contains("error: microsoft visual c++")
}

fn powershell_single_quote(raw: &str) -> String {
    raw.replace('\'', "''")
}

fn vswhere_path() -> Option<PathBuf> {
    // The VS Installer normally lives under `Program Files (x86)` on x86_64
    // Windows, but on ARM64 Windows (a real PyTorch/ComfyUI target) it can
    // live under plain `Program Files` instead -- checking only the former
    // made `visual_cpp_build_tools_installed()` always report "not
    // installed" there, triggering a redundant/unwanted elevated reinstall
    // even when Build Tools were already present.
    let mut candidates = Vec::new();
    for env_var in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Ok(base) = std::env::var(env_var) {
            candidates.push(
                PathBuf::from(base)
                    .join("Microsoft Visual Studio")
                    .join("Installer")
                    .join("vswhere.exe"),
            );
        }
    }
    // Defensive fallback if neither env var is set.
    candidates.push(PathBuf::from(
        r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe",
    ));
    candidates.push(PathBuf::from(
        r"C:\Program Files\Microsoft Visual Studio\Installer\vswhere.exe",
    ));

    candidates.into_iter().find(|candidate| candidate.exists())
}

fn visual_cpp_build_tools_installed() -> bool {
    let Some(vswhere) = vswhere_path() else {
        return false;
    };
    let output = run_command_capture(
        &vswhere.to_string_lossy(),
        &[
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ],
        None,
    );
    match output {
        Ok((stdout, _)) => stdout.lines().any(|line| !line.trim().is_empty()),
        Err(_) => false,
    }
}

fn install_visual_cpp_build_tools(app: &AppHandle) -> Result<(), String> {
    if visual_cpp_build_tools_installed() {
        emit_install_event(
            app,
            "info",
            "Microsoft Visual C++ Build Tools already installed.",
        );
        return Ok(());
    }

    let state = app.state::<AppState>();
    let deps_root = state
        .context
        .config
        .cache_path()
        .join("deps")
        .join("vs-build-tools");
    std::fs::create_dir_all(&deps_root).map_err(|err| err.to_string())?;
    let bootstrapper = deps_root.join("vs_BuildTools.exe");
    let installer_log = deps_root.join("vs_buildtools_install.log");

    emit_install_event(
        app,
        "step",
        "Downloading Microsoft Visual C++ Build Tools bootstrapper...",
    );
    powershell_download(
        "https://aka.ms/vs/17/release/vs_BuildTools.exe",
        &bootstrapper,
    )?;

    emit_install_event(
        app,
        "step",
        "Installing Microsoft Visual C++ Build Tools automatically...",
    );

    let bootstrapper_s = powershell_single_quote(&bootstrapper.to_string_lossy());
    let log_s = powershell_single_quote(&installer_log.to_string_lossy());
    let command = format!(
        "$args = @('--quiet','--wait','--norestart','--nocache','--installWhileDownloading','--add','Microsoft.VisualStudio.Workload.VCTools','--includeRecommended','--log','{log}'); \
         $p = Start-Process -FilePath '{exe}' -ArgumentList $args -Verb RunAs -Wait -PassThru; \
         exit $p.ExitCode",
        exe = bootstrapper_s,
        log = log_s,
    );

    let mut cmd = std::process::Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &command,
    ]);
    apply_background_command_flags(&mut cmd);
    let output = cmd
        .output()
        .map_err(|err| format!("Failed to launch Visual Studio Build Tools installer: {err}"))?;
    let code = output.status.code().unwrap_or(-1);
    if !matches!(code, 0 | 3010 | 1641) {
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(format!(
            "Automatic Visual C++ Build Tools install failed (exit code {code}). {}",
            enrich_command_failure_message(
                "vs_BuildTools.exe",
                &["--quiet", "--wait"],
                &stdout,
                &stderr
            )
        ));
    }

    if !visual_cpp_build_tools_installed() {
        return Err(format!(
            "Visual C++ Build Tools installer completed, but the required VC toolchain was not detected. Check {}",
            installer_log.display()
        ));
    }

    emit_install_event(
        app,
        "info",
        "Microsoft Visual C++ Build Tools installed successfully.",
    );
    if code == 3010 || code == 1641 {
        emit_install_event(
            app,
            "warn",
            "Build Tools installation reported that a reboot may be required.",
        );
    }
    Ok(())
}

fn run_command_env(
    program: &str,
    args: &[&str],
    working_dir: Option<&Path>,
    envs: &[(&str, &str)],
) -> Result<(), String> {
    log::debug!("run_command_env: {} {}", program, args.join(" "));
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }
    for (key, value) in envs {
        cmd.env(key, value);
    }
    apply_background_command_flags(&mut cmd);
    let output = output_with_optional_timeout(program, cmd)
        .map_err(|err| format!("Failed to run {program}: {err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return Err(enrich_command_failure_message(
            program, args, &stdout, &stderr,
        ));
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
        Some(strip_windows_verbatim_prefix(
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
    state
        .context
        .config
        .update_settings(|settings| {
            settings.comfyui_root = normalized.clone();
        })
        .map_err(|err| err.to_string())
}

async fn check_updates_now(state: State<'_, AppState>) -> Result<UpdateCheckResponse, String> {
    let updater = state.context.updater.clone();
    let result = updater.check_for_update().await;

    match result {
        Ok(Ok(Some(update))) => Ok(UpdateCheckResponse {
            available: true,
            version: Some(update.version.to_string()),
            notes: update.notes,
            managed_externally: false,
        }),
        Ok(Ok(None)) => Ok(UpdateCheckResponse {
            available: false,
            version: None,
            notes: None,
            managed_externally: false,
        }),
        Ok(Err(err)) => Err(format!("Update check failed: {err:#}")),
        Err(join_err) => Err(format!("Update task failed: {join_err}")),
    }
}

async fn auto_update_startup(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<UpdateCheckResponse, String> {
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
                    base_path = Some(strip_windows_verbatim_prefix(
                        &std::fs::canonicalize(&resolved).unwrap_or(resolved),
                    ));
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
            base_path = Some(strip_windows_verbatim_prefix(
                &std::fs::canonicalize(&resolved).unwrap_or(resolved),
            ));
            continue;
        }
    }

    base_path.map(|base| ComfyExtraModelConfig {
        base_path: base,
        is_default,
    })
}

pub(crate) fn effective_download_root(comfy_root: &Path) -> PathBuf {
    match comfy_extra_model_config(comfy_root) {
        Some(config) if config.is_default => config.base_path,
        _ => comfy_root.to_path_buf(),
    }
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
    let normalized = strip_windows_verbatim_prefix(&normalized).to_path_buf();
    let detected_root = detect_existing_comfyui_root(&normalized).map(|p| {
        strip_windows_verbatim_prefix(&p)
            .to_string_lossy()
            .to_string()
    });
    Ok(ComfyPathInspection {
        selected: strip_windows_verbatim_prefix(&normalized)
            .to_string_lossy()
            .to_string(),
        detected_root,
    })
}

#[cfg(target_os = "windows")]
fn normalize_explorer_path(path: &std::path::Path) -> String {
    let display = path.to_string_lossy().to_string();
    if let Some(stripped) = display.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{}", stripped);
    }
    if let Some(stripped) = display.strip_prefix(r"\\?\") {
        return stripped.to_string();
    }
    display
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

    #[cfg(target_os = "windows")]
    {
        let open_target = normalize_explorer_path(&target);
        let mut cmd = std::process::Command::new("explorer.exe");
        cmd.arg(&open_target);
        apply_background_command_flags(&mut cmd);
        cmd.spawn()
            .map_err(|err| format!("Failed to open folder: {err}"))?;
        Ok(open_target)
    }

    #[cfg(not(target_os = "windows"))]
    {
        open::that(target).map_err(|err| format!("Failed to open folder: {err}"))?;
        Ok(path)
    }
}

fn open_external_url(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        return Err("Only http/https links are allowed.".to_string());
    }

    // `url` frequently originates from remote, non-app-controlled data
    // (Civitai creator links, catalog "tutorial"/workflow links), so it must
    // never be handed to a shell that re-parses metacharacters. Unlike
    // `cmd /C start`, `open::that` invokes ShellExecuteW directly on
    // Windows (and `xdg-open`/`open` as a plain argv entry elsewhere),
    // passing the URL as a single opaque argument with no reinterpretation.
    open::that(trimmed).map_err(|err| format!("Failed to open link: {err}"))
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
    #[serde(default)]
    torch_profile: Option<String>,
}

fn apply_attention_backend_change(
    app: AppHandle,
    state: State<'_, AppState>,
    request: AttentionBackendChangeRequest,
) -> Result<String, String> {
    let was_running = stop_comfyui_for_mutation(&app, &state)?;
    let root = resolve_root_path(&state.context, request.comfyui_root)?;
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
    let target = request.target_backend.trim().to_ascii_lowercase();
    if !matches!(
        target.as_str(),
        "none" | "sage" | "sage3" | "flash" | "nunchaku"
    ) {
        return Err("Unknown attention backend target.".to_string());
    }
    if is_non_cuda_profile(&profile) && target != "none" {
        return Err(
            "SageAttention, SageAttention3, FlashAttention, and Nunchaku are CUDA-only and are not available with the Windows ROCm/XPU profiles."
                .to_string(),
        );
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

    uv_pip_uninstall_best_effort(
        &uv_bin,
        &py_exe,
        &root,
        &uv_python_install_dir,
        &[
            "sageattention",
            "sageattn3",
            "flash-attn",
            "flash_attn",
            "nunchaku",
        ],
    )?;

    let nunchaku_node = root.join("custom_nodes").join("ComfyUI-nunchaku");
    for folder in ["ComfyUI-nunchaku", "nunchaku_nodes"] {
        let path = root.join("custom_nodes").join(folder);
        if path.exists() {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    if target != "none" {
        let Some(whl) = attention_wheel_url(&profile, &target) else {
            return Err("No wheel mapping for selected backend/profile.".to_string());
        };
        if target == "nunchaku" {
            ensure_git_available(&app)?;
            let addon_root = root.join("custom_nodes");
            std::fs::create_dir_all(&addon_root).map_err(|err| err.to_string())?;
            run_command(
                "git",
                &[
                    "clone",
                    "https://github.com/nunchaku-ai/ComfyUI-nunchaku",
                    &nunchaku_node.to_string_lossy(),
                ],
                Some(&root),
            )?;
            install_insightface(&app, &root, &uv_bin, &py_path, &uv_python_install_dir)?;
            install_nunchaku_node_requirements(
                &root,
                &uv_bin,
                &py_path,
                &uv_python_install_dir,
                &nunchaku_node,
            )?;
        }
        install_wheel_no_deps(&uv_bin, &py_path, &root, &uv_python_install_dir, whl, true)?;
        if target == "sage3" {
            if let Some(sage_whl) = attention_wheel_url(&profile, "sage") {
                // ComfyUI's --use-sage-attention gate checks for sageattention package.
                install_wheel_no_deps(
                    &uv_bin,
                    &py_path,
                    &root,
                    &uv_python_install_dir,
                    sage_whl,
                    true,
                )?;
            }
        }
        if target == "nunchaku" {
            reassert_torch_stack_for_profile(
                &uv_bin,
                &py_path,
                &root,
                &uv_python_install_dir,
                &profile,
            )?;
            finalize_nunchaku_install(
                &app,
                &root,
                &uv_bin,
                &py_path,
                &uv_python_install_dir,
                &nunchaku_node,
            )?;
        }
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

fn set_comfyui_launch_attention_backend(
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
    if detect_torch_profile_for_root(&root)
        .as_deref()
        .map(is_non_cuda_profile)
        .unwrap_or(false)
        && target != "none"
    {
        return Err(
            "CUDA-only launch flags are not available with the Windows ROCm/XPU profiles."
                .to_string(),
        );
    }

    match target.as_str() {
        "sage" => {
            if !(python_module_importable(&root, "sageattention")
                || python_module_importable(&root, "sageattn3"))
            {
                return Err(
                    "SageAttention launch flag is unavailable because SageAttention is not installed."
                        .to_string(),
                );
            }
        }
        "sage3" => {
            if !python_module_importable(&root, "sageattn3") {
                return Err(
                    "SageAttention3 launch flag is unavailable because SageAttention3 is not installed."
                        .to_string(),
                );
            }
        }
        "flash" if !python_module_importable(&root, "flash_attn") => {
            return Err(
                "FlashAttention launch flag is unavailable because FlashAttention is not installed."
                    .to_string(),
            );
        }
        _ => {}
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
        if let Err(err) = run_command_with_retry(
            "git",
            &["merge", "--ff-only", &latest_tag_for_task],
            Some(&root),
            2,
        ) {
            let lower = err.to_ascii_lowercase();
            let can_repoint_branch = lower.contains("unrelated histories")
                || lower.contains("not possible to fast-forward")
                || lower.contains("not possible to fast forward")
                || lower.contains("cannot fast-forward")
                || lower.contains("diverging");
            if can_repoint_branch {
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
        // Taken before the requirements install, which older (unpinned)
        // installs let replace torch.
        let installed_profile = detect_torch_profile_for_root(&root);
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
            // The unpinned nightly profile would pull a new nightly every time.
            if let Some(profile) = installed_profile.filter(|p| p != "torchxpu_nightly") {
                reassert_torch_stack_for_profile(
                    &uv_bin,
                    py.to_string_lossy().as_ref(),
                    &root,
                    &uv_python_install_dir,
                    &profile,
                )
                .map_err(|err| format!("Failed to re-apply the torch stack: {err}"))?;
            }
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
    if nerdstats {
        std::env::set_var("ARCTIC_NERDSTATS", "1");
    }
    if fakeamd {
        std::env::set_var("ARCTIC_FAKE_AMD", "1");
    }
    if fakeintel {
        std::env::set_var("ARCTIC_FAKE_INTEL", "1");
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
        log::info!("Fake AMD mode enabled (Windows UI/profile simulation).");
    }
    if fakeintel {
        log::info!("Fake Intel mode enabled (Windows UI/profile simulation).");
    }

    let context = match build_context() {
        Ok(context) => context,
        Err(err) => {
            eprintln!("Failed to initialize app context: {err:#}");
            std::process::exit(1);
        }
    };

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
