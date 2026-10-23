//! A plain-text diagnostics report for bug reports: app and OS, GPUs, the
//! selected ComfyUI install, launch settings and the end of the log. Token
//! values are never included, only whether they are set.

use super::{
    builtin_manager_installed, read_comfyui_installed_version, torch_build, AppState,
    ComfyEndpoint, PortOccupant,
};
use crate::host::State;
use crate::platform::{
    detect_amd_gpu_details, detect_intel_gpu_details, detect_nvidia_gpu_details,
    python_exe_for_root,
};
use arctic_downloader::applog;
use serde::Serialize;
use std::fmt::Write as _;

#[derive(Debug, Serialize)]
pub struct DiagnosticsReport {
    pub report: String,
    /// Where the report was saved (beside the log file), if it could be.
    pub path: Option<String>,
}

fn os_description() -> String {
    #[cfg(target_os = "linux")]
    {
        // Inside Flatpak the host's os-release is mounted under /run/host.
        let release = ["/run/host/os-release", "/etc/os-release"]
            .iter()
            .find_map(|path| std::fs::read_to_string(path).ok())
            .unwrap_or_default();
        release
            .lines()
            .find_map(|line| line.strip_prefix("PRETTY_NAME="))
            .map(|name| name.trim_matches('"').to_string())
            .unwrap_or_else(|| "Linux".to_string())
    }
    #[cfg(not(target_os = "linux"))]
    {
        crate::platform::run_command_capture("cmd", &["/c", "ver"], None)
            .map(|(stdout, _)| stdout.trim().to_string())
            .unwrap_or_else(|_| std::env::consts::OS.to_string())
    }
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

/// Builds the diagnostics report and saves it next to the log file.
pub fn write_diagnostics_report(state: State<'_, AppState>) -> DiagnosticsReport {
    let settings = state.context.config.settings();
    let mut out = String::new();
    let _ = writeln!(out, "ComfyPilot diagnostics");
    let _ = writeln!(out, "App version: {}", state.context.display_version);
    let _ = writeln!(
        out,
        "OS: {} ({} {})",
        os_description(),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    if let Some(manager) = arctic_downloader::env_flags::external_package_manager() {
        let _ = writeln!(out, "Installed via: {manager}");
    }
    if let Some(ram) = state.context.ram_profile {
        let _ = writeln!(out, "RAM: {:.1} GB ({})", ram.total_gb, ram.tier.label());
    }

    let nvidia = detect_nvidia_gpu_details();
    if let Some(name) = nvidia.name.as_deref() {
        let _ = writeln!(
            out,
            "NVIDIA GPU: {name} | VRAM {} MB | driver {} | compute capability {}",
            nvidia
                .vram_mb
                .map(|v| v.to_string())
                .unwrap_or_else(|| "?".into()),
            nvidia.driver_version.as_deref().unwrap_or("?"),
            nvidia.compute_capability.as_deref().unwrap_or("?")
        );
    }
    if let Some(name) = detect_amd_gpu_details().name {
        let _ = writeln!(out, "AMD GPU: {name}");
    }
    if let Some(name) = detect_intel_gpu_details().name {
        let _ = writeln!(out, "Intel GPU: {name}");
    }

    let _ = writeln!(out, "\n[ComfyUI]");
    match settings.comfyui_root.as_deref() {
        Some(root) => {
            let _ = writeln!(out, "Folder: {}", root.display());
            let _ = writeln!(
                out,
                "Version: {}",
                read_comfyui_installed_version(root).unwrap_or_else(|| "unknown".into())
            );
            match python_exe_for_root(root) {
                Ok(py) => {
                    let _ = writeln!(out, "Python: {}", py.display());
                    match torch_build(&py) {
                        Some(build) => {
                            let _ = writeln!(
                                out,
                                "Torch: {} (cuda {}, hip {}, xpu {})",
                                build.version,
                                build.cuda.as_deref().unwrap_or("-"),
                                build.hip.as_deref().unwrap_or("-"),
                                build.xpu.as_deref().unwrap_or("-")
                            );
                        }
                        None => {
                            let _ = writeln!(out, "Torch: not found");
                        }
                    }
                    let _ = writeln!(
                        out,
                        "Built-in ComfyUI-Manager: {}",
                        yes_no(builtin_manager_installed(&py))
                    );
                }
                Err(err) => {
                    let _ = writeln!(out, "Python: {err}");
                }
            }
        }
        None => {
            let _ = writeln!(out, "Folder: not set");
        }
    }
    let endpoint = ComfyEndpoint::from_settings(&settings);
    let port_state = match endpoint.probe() {
        PortOccupant::Free => "not running",
        PortOccupant::ComfyUi => "ComfyUI is answering",
        PortOccupant::Unresponsive => "port open, not answering",
        PortOccupant::Other => "used by another program",
    };
    let _ = writeln!(out, "URL: {} ({port_state})", endpoint.url());

    let _ = writeln!(out, "\n[Settings]");
    let _ = writeln!(
        out,
        "Torch profile: {} | GPU selection: {}",
        settings.comfyui_torch_profile.as_deref().unwrap_or("-"),
        settings.comfyui_gpu_selection.as_deref().unwrap_or("auto")
    );
    let _ = writeln!(
        out,
        "Attention backend: {} | listen {} | lowvram {} | bf16-unet {} | async-offload {} | disable-smart-memory {} | disable-dynamic-vram {} | fast-disk {} | pinned memory {}",
        settings.comfyui_attention_backend.as_deref().unwrap_or("auto"),
        yes_no(settings.comfyui_listen_enabled),
        yes_no(settings.comfyui_lowvram_enabled),
        yes_no(settings.comfyui_bf16_unet_enabled),
        yes_no(settings.comfyui_async_offload_enabled),
        yes_no(settings.comfyui_disable_smart_memory_enabled),
        yes_no(settings.comfyui_disable_dynamic_vram_enabled),
        yes_no(settings.comfyui_fast_disk_enabled),
        yes_no(settings.comfyui_pinned_memory_enabled),
    );
    let _ = writeln!(
        out,
        "Custom launch args: {}",
        if settings.comfyui_custom_launch_args.is_empty() {
            "-"
        } else {
            settings.comfyui_custom_launch_args.as_str()
        }
    );
    let _ = writeln!(
        out,
        "HF Xet: {} | Hugging Face token set: {} | Civitai token set: {}",
        yes_no(settings.hf_xet_enabled),
        yes_no(settings.hf_token.is_some()),
        yes_no(settings.civitai_token.is_some())
    );
    let _ = writeln!(
        out,
        "Catalog: {}",
        state.context.catalog.refresh_status().message()
    );

    let _ = writeln!(out, "\n[Log, last 150 lines]");
    out.push_str(&applog::tail(150));
    out.push('\n');

    let path = applog::log_dir().and_then(|dir| {
        let path = dir.join("diagnostics.txt");
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(&path, &out).ok()?;
        Some(path.to_string_lossy().to_string())
    });
    DiagnosticsReport { report: out, path }
}

/// Opens the folder with the log file in the system file manager.
pub fn open_log_folder() -> Result<(), String> {
    let dir = applog::log_dir().ok_or_else(|| "No log folder on this system.".to_string())?;
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    open::that(&dir).map_err(|err| format!("Failed to open {}: {err}", dir.display()))
}
