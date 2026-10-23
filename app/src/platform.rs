//! Compile-time selection of the active operating-system implementation.
//!
//! Shared orchestration imports platform services through this module instead
//! of reaching into an application entry module directly. The implementations
//! still live in `app_linux.rs` and `app_windows.rs` for now; they can be moved
//! behind this boundary in smaller, independently testable slices.

use crate::contracts::{
    GpuBackend, PlatformCapabilities, PlatformKind, TorchProfile, TorchProfileCapability,
};

#[cfg(target_os = "linux")]
pub fn get_platform_capabilities() -> PlatformCapabilities {
    PlatformCapabilities {
        platform: PlatformKind::Linux,
        torch_profiles: vec![
            TorchProfileCapability {
                value: TorchProfile::Torch271Cu128,
                label: "Torch 2.7.1 + cu128",
                backend: GpuBackend::Nvidia,
            },
            TorchProfileCapability {
                value: TorchProfile::Torch280Cu128,
                label: "Torch 2.8.0 + cu128",
                backend: GpuBackend::Nvidia,
            },
            TorchProfileCapability {
                value: TorchProfile::Torch2141Rocm72,
                label: "Torch 2.14.1 + ROCm 7.2",
                backend: GpuBackend::Amd,
            },
            TorchProfileCapability {
                value: TorchProfile::Torch211Rocm72,
                label: "Torch 2.11.0 + ROCm 7.2 (previous)",
                backend: GpuBackend::Amd,
            },
            TorchProfileCapability {
                value: TorchProfile::Torch291Rocm64,
                label: "Torch 2.9.1 + ROCm 6.4 (Linux compatibility)",
                backend: GpuBackend::Amd,
            },
            TorchProfileCapability {
                value: TorchProfile::Torch2141Xpu,
                label: "Torch 2.14.1 + XPU",
                backend: GpuBackend::Intel,
            },
            TorchProfileCapability {
                value: TorchProfile::Torch291Xpu,
                label: "Torch 2.9.1 + XPU (previous)",
                backend: GpuBackend::Intel,
            },
            TorchProfileCapability {
                value: TorchProfile::Torch2141Cu130,
                label: "Torch 2.14.1 + cu130",
                backend: GpuBackend::Nvidia,
            },
            TorchProfileCapability {
                value: TorchProfile::Torch291Cu130,
                label: "Torch 2.9.1 + cu130",
                backend: GpuBackend::Nvidia,
            },
        ],
        supports_rocm_guided_setup: true,
        supports_xpu_guided_setup: true,
        opens_browser_on_comfyui_start: true,
        supports_lingering_python_cleanup: true,
    }
}

#[cfg(target_os = "windows")]
pub fn get_platform_capabilities() -> PlatformCapabilities {
    PlatformCapabilities {
        platform: PlatformKind::Windows,
        torch_profiles: vec![
            TorchProfileCapability {
                value: TorchProfile::Torch271Cu128,
                label: "Torch 2.7.1 + cu128",
                backend: GpuBackend::Nvidia,
            },
            TorchProfileCapability {
                value: TorchProfile::Torch280Cu128,
                label: "Torch 2.8.0 + cu128",
                backend: GpuBackend::Nvidia,
            },
            TorchProfileCapability {
                value: TorchProfile::Torch2140Rocm101,
                label: "Torch 2.14.0 + ROCm 10.1",
                backend: GpuBackend::Amd,
            },
            TorchProfileCapability {
                value: TorchProfile::Torch291Rocm72,
                label: "Torch 2.9.1 + ROCm SDK 7.2 (previous)",
                backend: GpuBackend::Amd,
            },
            TorchProfileCapability {
                value: TorchProfile::Torch2141Cu130,
                label: "Torch 2.14.1 + cu130",
                backend: GpuBackend::Nvidia,
            },
            TorchProfileCapability {
                value: TorchProfile::Torch291Cu130,
                label: "Torch 2.9.1 + cu130",
                backend: GpuBackend::Nvidia,
            },
            TorchProfileCapability {
                value: TorchProfile::Torch2141Xpu,
                label: "Torch 2.14.1 + XPU",
                backend: GpuBackend::Intel,
            },
            TorchProfileCapability {
                value: TorchProfile::TorchXpuNightly,
                label: "PyTorch XPU Nightly",
                backend: GpuBackend::Intel,
            },
        ],
        supports_rocm_guided_setup: false,
        supports_xpu_guided_setup: false,
        opens_browser_on_comfyui_start: true,
        supports_lingering_python_cleanup: true,
    }
}

#[cfg(target_os = "linux")]
pub(crate) use crate::app_linux::{
    comfy_extra_model_config, detect_amd_gpu_details, detect_intel_gpu_details,
    detect_nvidia_gpu_details, effective_download_root, ensure_git_available,
    git_latest_release_tag, host_comfyui_root_among, install_custom_node, normalize_path,
    python_exe_for_root, resolve_root_path, resolve_uv_binary, restart_comfyui_after_mutation,
    run_command_capture, run_uv_pip_strict, spawn_comfyui_start_monitor, start_comfyui_root_impl,
    stop_comfyui_root_impl, update_tray_comfy_status, write_extra_model_paths_yaml, CUSTOM_NODES,
};

#[cfg(target_os = "windows")]
pub(crate) use crate::app_windows::{
    comfy_extra_model_config, detect_amd_gpu_details, detect_intel_gpu_details,
    detect_nvidia_gpu_details, effective_download_root, ensure_git_available,
    git_latest_release_tag, host_comfyui_root_among, install_custom_node, normalize_path,
    python_exe_for_root, resolve_root_path, resolve_uv_binary, restart_comfyui_after_mutation,
    run_command_capture, run_uv_pip_strict, spawn_comfyui_start_monitor, start_comfyui_root_impl,
    stop_comfyui_root_impl, update_tray_comfy_status, write_extra_model_paths_yaml, CUSTOM_NODES,
};
