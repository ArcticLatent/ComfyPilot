use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug, Default)]
pub struct AmdGpuDetails {
    pub name: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct IntelGpuDetails {
    pub name: Option<String>,
}

static AMD_GPU_DETAILS_CACHE: OnceLock<Mutex<Option<AmdGpuDetails>>> = OnceLock::new();
static INTEL_GPU_DETAILS_CACHE: OnceLock<Mutex<Option<IntelGpuDetails>>> = OnceLock::new();

pub fn amd_gpu_details_cache() -> &'static Mutex<Option<AmdGpuDetails>> {
    AMD_GPU_DETAILS_CACHE.get_or_init(|| Mutex::new(None))
}

pub fn intel_gpu_details_cache() -> &'static Mutex<Option<IntelGpuDetails>> {
    INTEL_GPU_DETAILS_CACHE.get_or_init(|| Mutex::new(None))
}
