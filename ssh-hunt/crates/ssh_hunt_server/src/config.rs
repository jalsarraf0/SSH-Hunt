#![forbid(unsafe_code)]

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use world::{AdminSecret, HiddenOpsConfig, SecretMissionConfig, TelegramRelayConfig};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub server: ServerSection,
    pub ui: UiSection,
    pub redline: RedlineSection,
    pub world: WorldSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerSection {
    pub listen: String,
    pub rate_limit_per_second: u32,
    pub burst: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiSection {
    pub default_mode: String,
    pub flash_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedlineSection {
    pub duration_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldSection {
    pub daily_reward_cap: u32,
    pub style_bonus_daily_cap: u32,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            server: ServerSection {
                listen: "0.0.0.0:22222".to_owned(),
                rate_limit_per_second: 8,
                burst: 20,
            },
            ui: UiSection {
                default_mode: "training".to_owned(),
                flash_default: true,
            },
            redline: RedlineSection {
                duration_seconds: 300,
            },
            world: WorldSection {
                daily_reward_cap: 7,
                style_bonus_daily_cap: 5,
            },
        }
    }
}

pub fn load_config(path: &str) -> Result<ServerConfig> {
    if !Path::new(path).exists() {
        return Ok(ServerConfig::default());
    }
    let raw = std::fs::read_to_string(path).context("failed to read config.yaml")?;
    let cfg: ServerConfig = serde_yaml::from_str(&raw).context("invalid config.yaml")?;
    Ok(cfg)
}

pub fn load_admin_secret(path: &str) -> Result<Option<AdminSecret>> {
    if !Path::new(path).exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(path).context("failed to read admin secret")?;
    let cfg: AdminSecret = serde_yaml::from_str(&raw).context("invalid admin secret yaml")?;
    Ok(Some(cfg))
}

pub fn load_hidden_ops(path: &str) -> Result<HiddenOpsConfig> {
    if !Path::new(path).exists() {
        return Ok(HiddenOpsConfig {
            secret_mission: None,
            telegram: None,
        });
    }
    let raw = std::fs::read_to_string(path).context("failed to read hidden ops secret")?;

    #[derive(Debug, Deserialize)]
    struct RawHiddenOps {
        secret_mission: Option<SecretMissionConfig>,
        telegram: Option<TelegramRelayConfig>,
    }

    let parsed: RawHiddenOps = serde_yaml::from_str(&raw).context("invalid hidden ops yaml")?;
    Ok(HiddenOpsConfig {
        secret_mission: parsed.secret_mission,
        telegram: parsed.telegram,
    })
}
