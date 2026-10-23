use serde::{Deserialize, Serialize};
use sysinfo::System;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RamTier {
    TierA,
    TierB,
    TierC,
}

#[derive(Clone, Copy, Debug)]
pub struct RamProfile {
    pub total_gb: f64,
    pub tier: RamTier,
}

impl RamTier {
    pub fn all() -> &'static [RamTier] {
        use RamTier::*;
        &[TierA, TierB, TierC]
    }

    pub const fn identifier(self) -> &'static str {
        match self {
            RamTier::TierA => "tier_a",
            RamTier::TierB => "tier_b",
            RamTier::TierC => "tier_c",
        }
    }

    pub const fn min_ram_gb(self) -> u32 {
        match self {
            RamTier::TierA => 64,
            RamTier::TierB => 32,
            RamTier::TierC => 0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RamTier::TierA => "Tier A",
            RamTier::TierB => "Tier B",
            RamTier::TierC => "Tier C",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            RamTier::TierA => "Tier A (64, 96, or 128 GB)",
            RamTier::TierB => "Tier B (32 GB)",
            RamTier::TierC => "Tier C (8 or 16 GB)",
        }
    }

    pub fn shorthand(self) -> &'static str {
        match self {
            RamTier::TierA => "A",
            RamTier::TierB => "B",
            RamTier::TierC => "C",
        }
    }

    pub fn from_identifier(id: &str) -> Option<Self> {
        match id {
            "tier_a" | "A" | "a" => Some(RamTier::TierA),
            "tier_b" | "B" | "b" => Some(RamTier::TierB),
            "tier_c" | "C" | "c" => Some(RamTier::TierC),
            _ => None,
        }
    }

    pub fn index(self) -> usize {
        match self {
            RamTier::TierA => 0,
            RamTier::TierB => 1,
            RamTier::TierC => 2,
        }
    }

    pub fn next_stronger(self) -> Option<RamTier> {
        match self {
            RamTier::TierA => None,
            RamTier::TierB => Some(RamTier::TierA),
            RamTier::TierC => Some(RamTier::TierB),
        }
    }

    pub fn from_total_gb(total_gb: f64) -> Self {
        if total_gb >= RamTier::TierA.min_ram_gb() as f64 {
            RamTier::TierA
        } else if total_gb >= RamTier::TierB.min_ram_gb() as f64 {
            RamTier::TierB
        } else {
            RamTier::TierC
        }
    }

    pub fn satisfies(self, requirement: RamTier) -> bool {
        self.index() <= requirement.index()
    }
}

impl std::fmt::Display for RamTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.description())
    }
}

pub fn detect_total_ram_gb() -> Option<f64> {
    let mut system = System::new();
    system.refresh_memory();
    // sysinfo reports bytes (since 0.26; this used to divide KiB).
    let total_bytes = system.total_memory();
    if total_bytes == 0 {
        return None;
    }
    Some(total_bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

pub fn detect_ram_profile() -> Option<RamProfile> {
    let total_gb = detect_total_ram_gb()?;
    let tier = RamTier::from_total_gb(total_gb);
    Some(RamProfile { total_gb, tier })
}
