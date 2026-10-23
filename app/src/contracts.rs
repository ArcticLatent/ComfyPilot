//! Stable data contracts shared by every supported platform.

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformKind {
    Linux,
    Windows,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuBackend {
    Nvidia,
    Amd,
    Intel,
    Cpu,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TorchProfile {
    #[serde(rename = "torch271_cu128")]
    Torch271Cu128,
    #[serde(rename = "torch280_cu128")]
    Torch280Cu128,
    #[serde(rename = "torch291_cu130")]
    Torch291Cu130,
    #[serde(rename = "torch2141_cu130")]
    Torch2141Cu130,
    #[serde(rename = "torch211_rocm72")]
    Torch211Rocm72,
    #[serde(rename = "torch2141_rocm72")]
    Torch2141Rocm72,
    #[serde(rename = "torch2140_rocm101")]
    Torch2140Rocm101,
    #[serde(rename = "torch291_rocm64")]
    Torch291Rocm64,
    #[serde(rename = "torch291_rocm72")]
    Torch291Rocm72,
    #[serde(rename = "torch291_xpu")]
    Torch291Xpu,
    #[serde(rename = "torch2141_xpu")]
    Torch2141Xpu,
    #[serde(rename = "torchxpu_nightly")]
    TorchXpuNightly,
}

#[derive(Clone, Debug, Serialize)]
pub struct TorchProfileCapability {
    pub value: TorchProfile,
    pub label: &'static str,
    pub backend: GpuBackend,
}

#[derive(Clone, Debug, Serialize)]
pub struct PlatformCapabilities {
    pub platform: PlatformKind,
    pub torch_profiles: Vec<TorchProfileCapability>,
    pub supports_rocm_guided_setup: bool,
    pub supports_xpu_guided_setup: bool,
    pub opens_browser_on_comfyui_start: bool,
    pub supports_lingering_python_cleanup: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct AppSnapshot {
    pub(crate) version: String,
    pub(crate) total_ram_gb: Option<f64>,
    pub(crate) ram_tier: Option<String>,
    pub(crate) nvidia_gpu_name: Option<String>,
    pub(crate) nvidia_gpu_vram_mb: Option<u64>,
    pub(crate) amd_gpu_name: Option<String>,
    pub(crate) intel_gpu_name: Option<String>,
    pub(crate) gpu_detection_pending: bool,
    pub(crate) model_count: usize,
    pub(crate) lora_count: usize,
    /// Set when the settings file was damaged and defaults were used.
    pub(crate) settings_notice: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct UpdateCheckResponse {
    pub(crate) available: bool,
    pub(crate) version: Option<String>,
    pub(crate) notes: Option<String>,
    pub(crate) managed_externally: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct HfXetPreflightResponse {
    pub(crate) xet_enabled: bool,
    pub(crate) hf_cli_available: bool,
    pub(crate) hf_backend: String,
    pub(crate) hf_xet_installed: bool,
    pub(crate) hub_version: Option<String>,
    pub(crate) detail: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ComfyInstallRecommendation {
    pub(crate) gpu_name: Option<String>,
    pub(crate) driver_version: Option<String>,
    pub(crate) torch_profile: String,
    pub(crate) torch_label: String,
    pub(crate) reason: String,
    pub(crate) detection_pending: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ComfyInstallRequest {
    pub(crate) install_root: String,
    #[serde(default)]
    pub(crate) extra_model_root: Option<String>,
    #[serde(default)]
    pub(crate) extra_model_use_default: bool,
    pub(crate) torch_profile: Option<String>,
    pub(crate) include_sage_attention: bool,
    pub(crate) include_sage_attention3: bool,
    pub(crate) include_flash_attention: bool,
    pub(crate) include_insight_face: bool,
    pub(crate) include_nunchaku: bool,
    #[serde(default)]
    pub(crate) include_trellis2: bool,
    #[serde(default = "default_true")]
    pub(crate) include_pinned_memory: bool,
    pub(crate) node_comfyui_manager: bool,
    pub(crate) node_comfyui_easy_use: bool,
    pub(crate) node_rgthree_comfy: bool,
    pub(crate) node_comfyui_gguf: bool,
    pub(crate) node_comfyui_kjnodes: bool,
    #[serde(default)]
    pub(crate) node_comfyui_crystools: bool,
    #[serde(default)]
    pub(crate) force_fresh: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct PreflightItem {
    pub(crate) status: String,
    pub(crate) title: String,
    pub(crate) detail: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ComfyPreflightResponse {
    pub(crate) ok: bool,
    pub(crate) summary: String,
    pub(crate) items: Vec<PreflightItem>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ComfyPathInspection {
    pub(crate) selected: String,
    pub(crate) detected_root: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttentionBackendChangeRequest {
    #[serde(default)]
    pub(crate) comfyui_root: Option<String>,
    pub(crate) target_backend: String,
    #[serde(default)]
    pub(crate) torch_profile: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LaunchAttentionFlagRequest {
    #[serde(default)]
    pub(crate) comfyui_root: Option<String>,
    pub(crate) target_backend: String,
}

#[cfg(test)]
mod tests {
    use super::{default_true, TorchProfile};

    #[test]
    fn boolean_defaults_used_by_older_install_requests_remain_enabled() {
        assert!(default_true());
    }

    #[test]
    fn torch_profiles_serialize_to_existing_persisted_values() {
        assert_eq!(
            serde_json::to_string(&TorchProfile::Torch291Cu130).unwrap(),
            "\"torch291_cu130\""
        );
        assert_eq!(
            serde_json::to_string(&TorchProfile::Torch211Rocm72).unwrap(),
            "\"torch211_rocm72\""
        );
        assert_eq!(
            serde_json::to_string(&TorchProfile::TorchXpuNightly).unwrap(),
            "\"torchxpu_nightly\""
        );
    }
}
