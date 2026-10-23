//! NVIDIA/AMD/Intel GPU probing and caching for the Windows backend.
//!
//! Windows counterpart to `app_linux/gpu_detection.rs` -- same directory-
//! module split, but **not** a shared module: the `NvidiaGpuDetails` fields
//! now match Linux's (`compute_capability` decides CUDA 13 eligibility), but
//! there's no `gpu_detection_pending`/`is_nvidia_hopper_sm90`/
//! `fake_*_allow_*_setup_enabled` here at all -- those are Linux-only, as
//! already documented in `docs/cross-platform-development.md`. Keeping this
//! a separate file per platform (rather than trying to unify it) is the
//! point, not an oversight -- see that doc's "Consolidation roadmap" step 2
//! for why a shared `gpu/` module was tried and rejected.

use crate::shared::{
    amd_gpu_details_cache, detect_gpu_details_cached, intel_gpu_details_cache, AmdGpuDetails,
    IntelGpuDetails,
};
// `apply_background_command_flags` (suppresses the console-window flash when
// spawning `nvidia-smi`/`powershell` from a GUI app) lives in the parent
// `app_windows` module, not `shared.rs`, and is used well beyond GPU probing.
use super::apply_background_command_flags;
use std::sync::{atomic::AtomicBool, Mutex, OnceLock};

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

fn run_nvidia_smi_query(fields: &str) -> Option<std::process::Output> {
    let mut cmd = std::process::Command::new("nvidia-smi");
    cmd.arg(format!("--query-gpu={fields}"))
        .arg("--format=csv,noheader,nounits");
    apply_background_command_flags(&mut cmd);
    cmd.output().ok().filter(|output| output.status.success())
}

fn query_nvidia_gpu_details_blocking() -> NvidiaGpuDetails {
    // Drivers too old to know `compute_cap` reject the whole query.
    let (output, has_compute_capability) =
        match run_nvidia_smi_query("name,memory.total,driver_version,compute_cap") {
            Some(output) => (output, true),
            None => match run_nvidia_smi_query("name,memory.total,driver_version") {
                Some(output) => (output, false),
                None => return NvidiaGpuDetails::default(),
            },
        };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let first = stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    if first.is_empty() {
        return NvidiaGpuDetails::default();
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

pub(crate) fn detect_nvidia_gpu_details() -> NvidiaGpuDetails {
    detect_gpu_details_cached(
        gpu_details_cache(),
        &GPU_DETAILS_PROBE_STARTED,
        |d| d.name.is_some(),
        query_nvidia_gpu_details_blocking,
    )
}

fn query_amd_gpu_details_blocking() -> AmdGpuDetails {
    let script = "$gpu = Get-CimInstance Win32_VideoController | Where-Object { $_.Name -match 'AMD|Radeon|Ryzen AI|RyzenAI|ATI' } | Select-Object -First 1 -ExpandProperty Name; if ($gpu) { $gpu }";
    let mut cmd = std::process::Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        script,
    ]);
    apply_background_command_flags(&mut cmd);
    let output = match cmd.output() {
        Ok(output) => output,
        Err(_) => return AmdGpuDetails::default(),
    };
    if !output.status.success() {
        return AmdGpuDetails::default();
    }
    let name = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToOwned::to_owned);
    AmdGpuDetails { name }
}

pub(crate) fn detect_amd_gpu_details() -> AmdGpuDetails {
    detect_gpu_details_cached(
        amd_gpu_details_cache(),
        &AMD_GPU_DETAILS_PROBE_STARTED,
        |d| d.name.is_some(),
        query_amd_gpu_details_blocking,
    )
}

fn query_intel_gpu_details_blocking() -> IntelGpuDetails {
    let script = "$gpu = Get-CimInstance Win32_VideoController | Where-Object { $_.Name -match 'Intel|Arc|Iris|UHD|Xe' } | Select-Object -First 1 -ExpandProperty Name; if ($gpu) { $gpu }";
    let mut cmd = std::process::Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        script,
    ]);
    apply_background_command_flags(&mut cmd);
    let output = match cmd.output() {
        Ok(output) => output,
        Err(_) => return IntelGpuDetails::default(),
    };
    if !output.status.success() {
        return IntelGpuDetails::default();
    }
    let name = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToOwned::to_owned);
    IntelGpuDetails { name }
}

pub(crate) fn detect_intel_gpu_details() -> IntelGpuDetails {
    detect_gpu_details_cached(
        intel_gpu_details_cache(),
        &INTEL_GPU_DETAILS_PROBE_STARTED,
        |d| d.name.is_some(),
        query_intel_gpu_details_blocking,
    )
}
