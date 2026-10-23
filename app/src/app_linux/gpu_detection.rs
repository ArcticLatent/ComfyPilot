//! NVIDIA/AMD/Intel GPU probing and caching for the Linux backend.
//!
//! Extracted from `app_linux.rs` as the first slice of the consolidation
//! roadmap's directory-module split (see `docs/cross-platform-development.md`).
//! This is Linux-only logic that stays platform-local by design, not a
//! candidate for `shared.rs`: the caching/retry semantics and the
//! `NvidiaGpuDetails` struct genuinely differ from Windows' equivalent (see
//! the comment on the struct below), so moving this into its own file within
//! `app_linux/` organizes it without pretending it's shared.

use crate::shared::{
    amd_gpu_details_cache, detect_gpu_details_cached, intel_gpu_details_cache, AmdGpuDetails,
    IntelGpuDetails,
};
// `run_command_capture` lives in the parent `app_linux` module itself (not
// `shared.rs`), and `shared.rs` only imports it privately for its own use --
// so it has to come from `super`, the same way every other sibling function
// in `app_linux.rs` reaches it.
use super::run_command_capture;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex, OnceLock,
};

// Not moved to `shared.rs`: the probing differs per platform (Linux falls back
// to lspci; Linux alone uses `compute_capability` for Hopper/SM90 detection).
// The fields now match Windows', since both use `compute_capability` for CUDA
// 13 eligibility.
// All fields `pub(crate)`, not just `name`/`vram_mb`: `driver_version` and
// `compute_capability` used to be module-private when this struct and its
// only other users (comfy_install_recommendation_for and its tests) lived
// in the same file. Splitting GPU detection into its own module makes that
// distinction real, and those callers still construct/read every field --
// so the visibility that was already effectively crate-wide (one file, one
// compilation unit) stays crate-wide rather than becoming newly unreachable.
#[derive(Clone, Debug, Default)]
pub(crate) struct NvidiaGpuDetails {
    pub(crate) name: Option<String>,
    pub(crate) vram_mb: Option<u64>,
    pub(crate) driver_version: Option<String>,
    pub(crate) compute_capability: Option<String>,
}

static GPU_DETAILS_CACHE: OnceLock<Mutex<Option<NvidiaGpuDetails>>> = OnceLock::new();
static GPU_DETAILS_PROBE_STARTED: AtomicBool = AtomicBool::new(false);
static AMD_GPU_DETAILS_PROBE_STARTED: AtomicBool = AtomicBool::new(false);
static INTEL_GPU_DETAILS_PROBE_STARTED: AtomicBool = AtomicBool::new(false);

fn gpu_details_cache() -> &'static Mutex<Option<NvidiaGpuDetails>> {
    GPU_DETAILS_CACHE.get_or_init(|| Mutex::new(None))
}

fn query_nvidia_gpu_details_blocking() -> NvidiaGpuDetails {
    let detailed = run_command_capture(
        "nvidia-smi",
        &[
            "--query-gpu=name,memory.total,driver_version,compute_cap",
            "--format=csv,noheader,nounits",
        ],
        None,
    );
    let (stdout, has_compute_capability) = match detailed {
        Ok((stdout, _)) => (stdout, true),
        Err(_) => match run_command_capture(
            "nvidia-smi",
            &[
                "--query-gpu=name,memory.total,driver_version",
                "--format=csv,noheader,nounits",
            ],
            None,
        ) {
            Ok((stdout, _)) => (stdout, false),
            Err(_) => {
                return NvidiaGpuDetails {
                    name: query_nvidia_gpu_name_from_lspci(),
                    ..NvidiaGpuDetails::default()
                };
            }
        },
    };
    let first = stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    if first.is_empty() {
        return NvidiaGpuDetails {
            name: query_nvidia_gpu_name_from_lspci(),
            ..NvidiaGpuDetails::default()
        };
    }

    let mut parts = first.split(',').map(str::trim);
    let name = parts
        .next()
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let vram_mb = parts.next().and_then(|value| value.parse::<u64>().ok());
    let driver_version = parts
        .next()
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let compute_capability = has_compute_capability
        .then(|| parts.next())
        .flatten()
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    NvidiaGpuDetails {
        name,
        vram_mb,
        driver_version,
        compute_capability,
    }
}

fn query_nvidia_gpu_name_from_lspci() -> Option<String> {
    #[cfg(not(target_os = "linux"))]
    {
        None
    }

    #[cfg(target_os = "linux")]
    {
        let (stdout, _) = run_command_capture("lspci", &["-nn"], None).ok()?;
        find_nvidia_gpu_name_in_lspci(&stdout)
    }
}

fn find_nvidia_gpu_name_in_lspci(stdout: &str) -> Option<String> {
    stdout.lines().map(str::trim).find_map(|line| {
        let lower = line.to_ascii_lowercase();
        let is_display_controller = lower.contains("vga compatible controller")
            || lower.contains("3d controller")
            || lower.contains("display controller");
        let is_nvidia =
            lower.contains("nvidia") || lower.contains("[10de:") || lower.contains(" 10de:");
        if !is_display_controller || !is_nvidia {
            return None;
        }
        line.split_once(": ")
            .map(|(_, name)| name.trim().to_string())
            .filter(|name| !name.is_empty())
    })
}

fn query_amd_gpu_details_blocking() -> AmdGpuDetails {
    #[cfg(not(target_os = "linux"))]
    {
        return AmdGpuDetails::default();
    }

    #[cfg(target_os = "linux")]
    {
        let (stdout, _) = match run_command_capture("lspci", &["-nn"], None) {
            Ok(out) => out,
            Err(_) => return AmdGpuDetails::default(),
        };
        let line = stdout
            .lines()
            .map(str::trim)
            .find(|line| {
                let lower = line.to_ascii_lowercase();
                (lower.contains("vga compatible controller")
                    || lower.contains("3d controller")
                    || lower.contains("display controller"))
                    && (lower.contains("advanced micro devices")
                        || lower.contains("amd/ati")
                        || lower.contains("radeon")
                        || lower.contains("amdgpu"))
            })
            .unwrap_or_default();
        if line.is_empty() {
            return AmdGpuDetails::default();
        }
        let name = line
            .split(": ")
            .nth(1)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        AmdGpuDetails { name }
    }
}

fn query_intel_gpu_details_blocking() -> IntelGpuDetails {
    #[cfg(not(target_os = "linux"))]
    {
        return IntelGpuDetails::default();
    }

    #[cfg(target_os = "linux")]
    {
        let (stdout, _) = match run_command_capture("lspci", &["-nn"], None) {
            Ok(out) => out,
            Err(_) => return IntelGpuDetails::default(),
        };
        let line = stdout
            .lines()
            .map(str::trim)
            .find(|line| {
                let lower = line.to_ascii_lowercase();
                (lower.contains("vga compatible controller")
                    || lower.contains("3d controller")
                    || lower.contains("display controller"))
                    && (lower.contains("intel corporation")
                        || lower.contains("intel arc")
                        || lower.contains("iris xe")
                        || lower.contains("uhd graphics"))
            })
            .unwrap_or_default();
        if line.is_empty() {
            return IntelGpuDetails::default();
        }
        let name = line
            .split(": ")
            .nth(1)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        IntelGpuDetails { name }
    }
}

pub(crate) fn is_nvidia_hopper_sm90() -> bool {
    let gpu = detect_nvidia_gpu_details();
    if gpu
        .compute_capability
        .as_deref()
        .map(str::trim)
        .map(|cc| cc == "9.0")
        .unwrap_or(false)
    {
        return true;
    }

    gpu.name
        .as_deref()
        .map(|name| {
            let n = name.to_ascii_lowercase();
            n.contains("h100") || n.contains("h200") || n.contains("gh200") || n.contains("hopper")
        })
        .unwrap_or(false)
}

pub(crate) fn detect_nvidia_gpu_details() -> NvidiaGpuDetails {
    detect_gpu_details_cached(
        gpu_details_cache(),
        &GPU_DETAILS_PROBE_STARTED,
        |d| d.name.is_some() || d.vram_mb.is_some() || d.driver_version.is_some(),
        query_nvidia_gpu_details_blocking,
    )
}

pub(crate) fn detect_amd_gpu_details() -> AmdGpuDetails {
    detect_gpu_details_cached(
        amd_gpu_details_cache(),
        &AMD_GPU_DETAILS_PROBE_STARTED,
        |d| d.name.is_some(),
        query_amd_gpu_details_blocking,
    )
}

pub(crate) fn detect_intel_gpu_details() -> IntelGpuDetails {
    detect_gpu_details_cached(
        intel_gpu_details_cache(),
        &INTEL_GPU_DETAILS_PROBE_STARTED,
        |d| d.name.is_some(),
        query_intel_gpu_details_blocking,
    )
}

pub(crate) fn gpu_detection_pending() -> bool {
    GPU_DETAILS_PROBE_STARTED.load(Ordering::SeqCst)
        || AMD_GPU_DETAILS_PROBE_STARTED.load(Ordering::SeqCst)
        || INTEL_GPU_DETAILS_PROBE_STARTED.load(Ordering::SeqCst)
}

pub(crate) fn fake_amd_allow_rocm_setup_enabled() -> bool {
    std::env::var("ARCTIC_FAKE_AMD_ALLOW_ROCM_SETUP")
        .map(|value| value == "1")
        .unwrap_or(false)
}

pub(crate) fn fake_intel_allow_xpu_setup_enabled() -> bool {
    std::env::var("ARCTIC_FAKE_INTEL_ALLOW_XPU_SETUP")
        .map(|value| value == "1")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_headless_nvidia_controller_in_mixed_lspci_output() {
        let output = concat!(
            "03:00.0 VGA compatible controller [0300]: Advanced Micro Devices, Inc. ",
            "[AMD/ATI] Navi 22 [Radeon RX 6700 XT] [1002:73df]\n",
            "09:00.0 3D controller [0302]: NVIDIA Corporation GB206 ",
            "[GeForce RTX 5060 Ti] [10de:2d04]",
        );

        assert_eq!(
            find_nvidia_gpu_name_in_lspci(output).as_deref(),
            Some("NVIDIA Corporation GB206 [GeForce RTX 5060 Ti] [10de:2d04]")
        );
    }
}
