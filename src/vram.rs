use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VramTier {
    TierS,
    TierA,
    TierB,
    TierC,
}

impl VramTier {
    pub fn all() -> &'static [VramTier] {
        use VramTier::*;
        &[TierS, TierA, TierB, TierC]
    }

    pub const fn identifier(self) -> &'static str {
        match self {
            VramTier::TierS => "tier_s",
            VramTier::TierA => "tier_a",
            VramTier::TierB => "tier_b",
            VramTier::TierC => "tier_c",
        }
    }

    pub const fn min_vram_gb(self) -> f64 {
        match self {
            VramTier::TierS => 31.9,
            VramTier::TierA => 15.9,
            VramTier::TierB => 11.5,
            VramTier::TierC => 0.0,
        }
    }

    pub const fn max_vram_gb(self) -> f64 {
        match self {
            VramTier::TierS => f64::MAX,
            VramTier::TierA => 31.9,
            VramTier::TierB => 15.9,
            VramTier::TierC => 11.5,
        }
    }

    pub const fn strength(self) -> u8 {
        match self {
            VramTier::TierC => 0,
            VramTier::TierB => 1,
            VramTier::TierA => 2,
            VramTier::TierS => 3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            VramTier::TierS => "Tier S",
            VramTier::TierA => "Tier A",
            VramTier::TierB => "Tier B",
            VramTier::TierC => "Tier C",
        }
    }

    pub fn shorthand(self) -> &'static str {
        match self {
            VramTier::TierS => "S",
            VramTier::TierA => "A",
            VramTier::TierB => "B",
            VramTier::TierC => "C",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            VramTier::TierS => "Tier S (32, 48, 80, or 96 GB)",
            VramTier::TierA => "Tier A (16 or 24 GB)",
            VramTier::TierB => "Tier B (12 GB)",
            VramTier::TierC => "Tier C (8 GB)",
        }
    }

    pub fn from_identifier(id: &str) -> Option<Self> {
        match id {
            "tier_s" | "S" | "s" => Some(VramTier::TierS),
            "tier_a" | "A" | "a" => Some(VramTier::TierA),
            "tier_b" | "B" | "b" => Some(VramTier::TierB),
            "tier_c" | "C" | "c" => Some(VramTier::TierC),
            _ => None,
        }
    }

    pub fn index(self) -> usize {
        match self {
            VramTier::TierS => 0,
            VramTier::TierA => 1,
            VramTier::TierB => 2,
            VramTier::TierC => 3,
        }
    }

    pub fn next_stronger(self) -> Option<VramTier> {
        match self {
            VramTier::TierS => None,
            VramTier::TierA => Some(VramTier::TierS),
            VramTier::TierB => Some(VramTier::TierA),
            VramTier::TierC => Some(VramTier::TierB),
        }
    }
}

impl std::fmt::Display for VramTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.description())
    }
}
