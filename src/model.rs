use crate::{ram::RamTier, vram::VramTier};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModelCatalog {
    pub catalog_version: u32,
    pub models: Vec<MasterModel>,
    #[serde(default)]
    pub loras: Vec<LoraDefinition>,
    #[serde(default)]
    pub workflows: Vec<WorkflowDefinition>,
    /// Known-good commits for the app's built-in custom nodes; installs check
    /// these out instead of the repository's latest commit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_node_pins: Vec<CustomNodePin>,
}

/// A custom node repository pinned to a commit.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CustomNodePin {
    /// Repository URL, e.g. `https://github.com/kijai/ComfyUI-KJNodes`.
    pub repo: String,
    pub commit: String,
}

impl Default for ModelCatalog {
    fn default() -> Self {
        Self {
            catalog_version: 1,
            models: Vec::new(),
            loras: Vec::new(),
            workflows: Vec::new(),
            custom_node_pins: Vec::new(),
        }
    }
}

impl ModelCatalog {
    /// The pinned commit for a custom node repository, if the catalog pins
    /// it and the commit looks like a git hash. Repository URLs match
    /// regardless of case, a trailing `/` or `.git`.
    pub fn custom_node_pin(&self, repo_url: &str) -> Option<&str> {
        let key = |url: &str| {
            url.trim()
                .trim_end_matches('/')
                .trim_end_matches(".git")
                .to_ascii_lowercase()
        };
        let wanted = key(repo_url);
        self.custom_node_pins
            .iter()
            .find(|pin| key(&pin.repo) == wanted)
            .map(|pin| pin.commit.trim())
            .filter(|commit| {
                (7..=40).contains(&commit.len()) && commit.chars().all(|c| c.is_ascii_hexdigit())
            })
    }

    pub fn find_model(&self, id: &str) -> Option<&MasterModel> {
        self.models.iter().find(|model| model.id == id)
    }

    pub fn lora_families(&self) -> Vec<String> {
        let mut families: Vec<String> = self
            .loras
            .iter()
            .filter_map(|lora| lora.family.clone())
            .collect();
        families.sort();
        families.dedup();
        families
    }

    pub fn find_lora(&self, id: &str) -> Option<LoraDefinition> {
        self.loras.iter().find(|l| l.id == id).cloned()
    }

    pub fn model_families(&self) -> Vec<String> {
        let mut families: Vec<String> = self
            .models
            .iter()
            .map(|model| model.family.clone())
            .collect();
        families.sort();
        families.dedup();
        families
    }

    pub fn workflow_families(&self) -> Vec<String> {
        let mut families: Vec<String> = self
            .workflows
            .iter()
            .map(|workflow| workflow.family.clone())
            .collect();
        families.sort();
        families.dedup();
        families
    }

    pub fn find_workflow(&self, id: &str) -> Option<WorkflowDefinition> {
        self.workflows
            .iter()
            .find(|workflow| workflow.id == id)
            .cloned()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MasterModel {
    pub id: String,
    pub display_name: String,
    pub family: String,
    pub variants: Vec<ModelVariant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub always: Vec<AlwaysGroup>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ram_tier_thresholds: Option<RamTierThresholds>,
}

impl MasterModel {
    pub fn best_variant_for_tier(&self, tier: VramTier) -> Option<&ModelVariant> {
        self.variants
            .iter()
            .find(|variant| variant.tier == tier)
            .or_else(|| {
                self.variants
                    .iter()
                    .filter(|variant| variant.tier.strength() < tier.strength())
                    .max_by_key(|variant| variant.tier.strength())
            })
    }

    pub fn variants_for_tier(&self, tier: VramTier) -> Vec<ModelVariant> {
        self.variants
            .iter()
            .filter(|variant| variant.tier == tier)
            .cloned()
            .collect()
    }

    pub fn find_variant(&self, variant_id: &str) -> Option<&ModelVariant> {
        self.variants
            .iter()
            .find(|variant| variant.id == variant_id)
    }

    pub fn synthetic_always_only_variant(&self, tier: VramTier) -> Option<ModelVariant> {
        if self.variants.is_empty() && !self.always.is_empty() {
            Some(ModelVariant {
                id: "__always_only__".to_string(),
                tier,
                model_size: None,
                quantization: None,
                note: Some("Always Artifacts Only".to_string()),
                artifacts: Vec::new(),
            })
        } else {
            None
        }
    }

    pub fn artifacts_for_variant(
        &self,
        variant: &ModelVariant,
        ram_tier: Option<RamTier>,
    ) -> Vec<ModelArtifact> {
        let mut artifacts = Vec::new();

        for group in &self.always {
            for artifact in &group.artifacts {
                if artifact.is_supported_on_ram(ram_tier) {
                    artifacts.push(artifact.clone());
                }
            }
        }

        for artifact in &variant.artifacts {
            if artifact.is_supported_on_ram(ram_tier) {
                artifacts.push(artifact.clone());
            }
        }

        artifacts
    }

    pub fn resolved_ram_thresholds(&self) -> ResolvedRamTierThresholds {
        ResolvedRamTierThresholds::new(self.ram_tier_thresholds.as_ref())
    }

    pub fn ram_tier_range_label(&self, tier: RamTier) -> String {
        self.resolved_ram_thresholds().range_label(tier)
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedModel {
    pub master: MasterModel,
    pub variant: ModelVariant,
}

impl ResolvedModel {
    pub fn artifacts_for_download(&self, ram_tier: Option<RamTier>) -> Vec<ModelArtifact> {
        self.master.artifacts_for_variant(&self.variant, ram_tier)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AlwaysGroup {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub artifacts: Vec<ModelArtifact>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct RamTierThresholds {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_a_min_gb: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_b_min_gb: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_c_min_gb: Option<f64>,
}

impl RamTierThresholds {
    pub fn min_for(&self, tier: RamTier) -> Option<f64> {
        match tier {
            RamTier::TierA => self.tier_a_min_gb,
            RamTier::TierB => self.tier_b_min_gb,
            RamTier::TierC => self.tier_c_min_gb,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tier_a_min_gb.is_none() && self.tier_b_min_gb.is_none() && self.tier_c_min_gb.is_none()
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedRamTierThresholds {
    mins: [f64; 3],
}

impl Default for ResolvedRamTierThresholds {
    fn default() -> Self {
        Self::new(None)
    }
}

impl ResolvedRamTierThresholds {
    pub fn new(overrides: Option<&RamTierThresholds>) -> Self {
        let mut mins = [0.0; 3];
        for tier in RamTier::all() {
            let idx = tier.index();
            mins[idx] = overrides
                .and_then(|o| o.min_for(*tier))
                .unwrap_or(tier.min_ram_gb() as f64);
        }
        Self { mins }
    }

    pub fn min(&self, tier: RamTier) -> f64 {
        self.mins[tier.index()]
    }

    pub fn range_label(&self, tier: RamTier) -> String {
        let min = self.min(tier);
        if let Some(next) = tier.next_stronger() {
            let next_min = self.min(next);
            if min <= 0.0 {
                format!("< {} GB", format_gb(next_min))
            } else {
                format!("{}-{} GB", format_gb(min), format_gb(next_min))
            }
        } else {
            format!("≥ {} GB", format_gb(min))
        }
    }
}

fn format_gb(value: f64) -> String {
    let rounded = (value * 10.0).round() / 10.0;
    if (rounded.fract()).abs() < 0.05 {
        format!("{:.0}", rounded.round())
    } else {
        format!("{:.1}", rounded)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LoraDefinition {
    pub id: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    pub download_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
}

impl LoraDefinition {
    pub fn derived_file_name(&self) -> String {
        if let Some(file) = &self.file_name {
            return file.clone();
        }

        let url = self.download_url.trim();
        let last_segment = url.rsplit(['/', '\\']).next().unwrap_or("lora.safetensors");
        let cleaned = last_segment.split('?').next().unwrap_or(last_segment);
        if cleaned.is_empty() {
            format!("{}-lora.safetensors", self.id)
        } else {
            cleaned.to_string()
        }
    }

    pub fn label_with_index(&self, index: usize) -> String {
        format!("{}. {}", index, self.display_name)
    }

    pub fn matches_family(&self, family_filter: &Option<String>) -> bool {
        match family_filter {
            None => true,
            Some(filter) if filter.is_empty() => true,
            Some(filter) => self
                .family
                .as_deref()
                .map(|family| family.eq_ignore_ascii_case(filter))
                .unwrap_or(false),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WorkflowDefinition {
    pub id: String,
    pub display_name: String,
    pub family: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub workflow_json_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_link_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patreon_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_image_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub youtube_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModelVariant {
    pub id: String,
    pub tier: VramTier,
    #[serde(default)]
    pub model_size: Option<String>,
    #[serde(default)]
    pub quantization: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    pub artifacts: Vec<ModelArtifact>,
}

impl ModelVariant {
    pub fn selection_label(&self) -> String {
        let mut parts = Vec::new();
        if let Some(size) = &self.model_size {
            parts.push(size.clone());
        }
        if let Some(quant) = &self.quantization {
            parts.push(quant.clone());
        }
        if let Some(note) = &self.note {
            parts.push(note.clone());
        }
        parts.push(self.tier.label().to_string());
        parts.join(" • ")
    }

    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(size) = &self.model_size {
            parts.push(size.clone());
        }
        if let Some(quant) = &self.quantization {
            parts.push(quant.clone());
        }
        if let Some(note) = &self.note {
            parts.push(note.clone());
        }
        parts.push(self.tier.label().to_string());
        parts.join(" • ")
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModelArtifact {
    pub repo: String,
    pub path: String,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_estimate: Option<MemoryEstimate>,
    pub target_category: TargetCategory,
    #[serde(default)]
    pub license_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ram_bucket: Option<RamTier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_ram_tier: Option<RamTier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MemoryEstimate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_ram_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl ModelArtifact {
    pub fn file_name(&self) -> &str {
        self.path
            .rsplit_once('/')
            .map(|(_, file)| file)
            .unwrap_or(&self.path)
    }

    pub fn is_supported_on_ram(&self, available: Option<RamTier>) -> bool {
        if let Some(bucket) = self.ram_bucket {
            return available.map(|tier| tier == bucket).unwrap_or(false);
        }
        match self.min_ram_tier {
            None => true,
            Some(required) => available
                .map(|tier| tier.satisfies(required))
                .unwrap_or(false),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetCategory {
    DiffusionModels(Option<String>),
    Vae(Option<String>),
    TextEncoders(Option<String>),
    ClipVision(Option<String>),
    Unet(Option<String>),
    Loras(Option<String>),
    Ipadapter(Option<String>),
    Controlnet(Option<String>),
    Pulid(Option<String>),
    Custom(String),
}

impl TargetCategory {
    pub fn slug(&self) -> &str {
        match self {
            TargetCategory::DiffusionModels(alias) => {
                alias.as_deref().unwrap_or("diffusion_models")
            }
            TargetCategory::Vae(alias) => alias.as_deref().unwrap_or("vae"),
            TargetCategory::TextEncoders(alias) => alias.as_deref().unwrap_or("text_encoders"),
            TargetCategory::ClipVision(alias) => alias.as_deref().unwrap_or("clip_vision"),
            TargetCategory::Unet(alias) => alias.as_deref().unwrap_or("unet"),
            TargetCategory::Loras(alias) => alias.as_deref().unwrap_or("loras"),
            TargetCategory::Ipadapter(alias) => alias.as_deref().unwrap_or("ipadapter"),
            TargetCategory::Controlnet(alias) => alias.as_deref().unwrap_or("controlnet"),
            TargetCategory::Pulid(alias) => alias.as_deref().unwrap_or("pulid"),
            TargetCategory::Custom(value) => value,
        }
    }

    pub fn from_slug(slug: &str) -> Self {
        let trimmed = slug.trim();
        let normalized = trimmed.to_ascii_lowercase();
        match normalized.as_str() {
            "diffusion_models" | "checkpoints" => {
                TargetCategory::DiffusionModels(alias_override(trimmed, "diffusion_models"))
            }
            "vae" => TargetCategory::Vae(alias_override(trimmed, "vae")),
            "text_encoders" | "clip" => {
                TargetCategory::TextEncoders(alias_override(trimmed, "text_encoders"))
            }
            "clip_vision" => TargetCategory::ClipVision(alias_override(trimmed, "clip_vision")),
            "unet" => TargetCategory::Unet(alias_override(trimmed, "unet")),
            "loras" => TargetCategory::Loras(alias_override(trimmed, "loras")),
            "ipadapter" => TargetCategory::Ipadapter(alias_override(trimmed, "ipadapter")),
            "controlnet" => TargetCategory::Controlnet(alias_override(trimmed, "controlnet")),
            "pulid" => TargetCategory::Pulid(alias_override(trimmed, "pulid")),
            _ => TargetCategory::Custom(trimmed.to_string()),
        }
    }

    pub fn comfyui_subdir(&self) -> String {
        match self {
            TargetCategory::DiffusionModels(alias) => {
                format!("models/{}", alias.as_deref().unwrap_or("diffusion_models"))
            }
            TargetCategory::Vae(alias) => format!("models/{}", alias.as_deref().unwrap_or("vae")),
            TargetCategory::TextEncoders(alias) => {
                format!("models/{}", alias.as_deref().unwrap_or("text_encoders"))
            }
            TargetCategory::ClipVision(alias) => {
                format!("models/{}", alias.as_deref().unwrap_or("clip_vision"))
            }
            TargetCategory::Unet(alias) => format!("models/{}", alias.as_deref().unwrap_or("unet")),
            TargetCategory::Loras(alias) => {
                format!("models/{}", alias.as_deref().unwrap_or("loras"))
            }
            TargetCategory::Ipadapter(alias) => {
                format!("models/{}", alias.as_deref().unwrap_or("ipadapter"))
            }
            TargetCategory::Controlnet(alias) => {
                format!("models/{}", alias.as_deref().unwrap_or("controlnet"))
            }
            TargetCategory::Pulid(alias) => {
                format!("models/{}", alias.as_deref().unwrap_or("pulid"))
            }
            TargetCategory::Custom(slug) => format!("models/{slug}"),
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            TargetCategory::DiffusionModels(_) => "Diffusion Model".to_string(),
            TargetCategory::Vae(_) => "VAE".to_string(),
            TargetCategory::TextEncoders(_) => "Text Encoder".to_string(),
            TargetCategory::ClipVision(_) => "CLIP Vision".to_string(),
            TargetCategory::Unet(_) => "UNet".to_string(),
            TargetCategory::Loras(_) => "LoRA".to_string(),
            TargetCategory::Ipadapter(_) => "IP-Adapter".to_string(),
            TargetCategory::Controlnet(_) => "ControlNet".to_string(),
            TargetCategory::Pulid(_) => "PuLID".to_string(),
            TargetCategory::Custom(slug) => slug.clone(),
        }
    }

    pub fn from_display_name(name: &str) -> Option<Self> {
        match name {
            "Diffusion Model" => Some(TargetCategory::DiffusionModels(None)),
            "VAE" => Some(TargetCategory::Vae(None)),
            "Text Encoder" => Some(TargetCategory::TextEncoders(None)),
            "CLIP Vision" => Some(TargetCategory::ClipVision(None)),
            "UNet" => Some(TargetCategory::Unet(None)),
            "LoRA" => Some(TargetCategory::Loras(None)),
            "IP-Adapter" => Some(TargetCategory::Ipadapter(None)),
            "ControlNet" => Some(TargetCategory::Controlnet(None)),
            "PuLID" => Some(TargetCategory::Pulid(None)),
            other => Some(TargetCategory::Custom(other.to_string())),
        }
    }
}

fn alias_override(input: &str, canonical: &str) -> Option<String> {
    if input == canonical {
        None
    } else {
        Some(input.to_string())
    }
}

impl Serialize for TargetCategory {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.slug())
    }
}

impl<'de> Deserialize<'de> for TargetCategory {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(TargetCategory::from_slug(&value))
    }
}

#[cfg(test)]
mod custom_node_pin_tests {
    use super::{CustomNodePin, ModelCatalog};

    #[test]
    fn pins_match_repository_urls_loosely_and_need_a_hash() {
        let catalog = ModelCatalog {
            custom_node_pins: vec![
                CustomNodePin {
                    repo: "https://github.com/kijai/ComfyUI-KJNodes".into(),
                    commit: "74b7f1731cfd0208ae50b9ffc55145564c25e65f".into(),
                },
                CustomNodePin {
                    repo: "https://github.com/rgthree/rgthree-comfy".into(),
                    commit: "main; rm -rf /".into(),
                },
            ],
            ..ModelCatalog::default()
        };
        let kj = Some("74b7f1731cfd0208ae50b9ffc55145564c25e65f");
        assert_eq!(
            catalog.custom_node_pin("https://github.com/kijai/ComfyUI-KJNodes"),
            kj
        );
        assert_eq!(
            catalog.custom_node_pin("https://github.com/Kijai/comfyui-kjnodes.git/"),
            kj
        );
        assert_eq!(
            catalog.custom_node_pin("https://github.com/rgthree/rgthree-comfy"),
            None
        );
        assert_eq!(
            catalog.custom_node_pin("https://github.com/city96/ComfyUI-GGUF"),
            None
        );
    }
}
