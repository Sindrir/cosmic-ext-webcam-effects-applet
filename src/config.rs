// SPDX-License-Identifier: GPL-3.0

use cosmic::cosmic_config::{self, cosmic_config_derive::CosmicConfigEntry, CosmicConfigEntry};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EffectMode {
    None,
    Blur,
    Replace,
    SolidColor,
}

impl Default for EffectMode {
    fn default() -> Self {
        Self::None
    }
}

impl EffectMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Blur => "Blur",
            Self::Replace => "Replace",
            Self::SolidColor => "SolidColor",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "None" => Some(Self::None),
            "Blur" => Some(Self::Blur),
            "Replace" => Some(Self::Replace),
            "SolidColor" => Some(Self::SolidColor),
            _ => None,
        }
    }
}

#[derive(Debug, Default, Clone, CosmicConfigEntry, Eq, PartialEq)]
#[version = 1]
pub struct Config {
    pub webcam_device: String,
    pub effect_mode: EffectMode,
    pub blur_intensity: u32,
    pub background_image_path: String,
    pub background_color: [u8; 3],
    pub auto_start: bool,
}
