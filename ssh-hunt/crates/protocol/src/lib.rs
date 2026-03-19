#![forbid(unsafe_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Mode {
    Training,
    NetCity,
    Redline,
}

impl Mode {
    pub fn as_label(&self) -> &'static str {
        match self {
            Self::Training => "SOLO TRAINING SIM",
            Self::NetCity => "MULTIPLAYER NETCITY MMO",
            Self::Redline => "REDLINE",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerIdentity {
    pub player_id: Uuid,
    pub username: String,
    pub remote_ip: String,
    pub display_name: String,
    pub key_fingerprints: Vec<String>,
    pub observed_key_fingerprints: Vec<String>,
}

impl PlayerIdentity {
    pub fn new(player_id: Uuid, username: String, remote_ip: String) -> Self {
        let display_name = format!("{username}@{remote_ip}");
        Self {
            player_id,
            username,
            remote_ip,
            display_name,
            key_fingerprints: Vec::new(),
            observed_key_fingerprints: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionContext {
    pub session_id: Uuid,
    pub identity: PlayerIdentity,
    pub node: String,
    pub cwd: String,
    pub mode: Mode,
    pub flash_enabled: bool,
    pub last_exit_code: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandRequest {
    pub raw: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MissionState {
    Locked,
    Available,
    Active,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionStatus {
    pub code: String,
    pub title: String,
    pub state: MissionState,
    pub progress: u8,
    pub required: bool,
    pub starter: bool,
    pub summary: String,
    pub suggested_command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InventoryItem {
    pub item_id: Uuid,
    pub sku: String,
    pub qty: u32,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuctionListing {
    pub listing_id: Uuid,
    pub seller_id: Uuid,
    pub item_sku: String,
    pub qty: u32,
    pub start_price: i64,
    pub buyout_price: Option<i64>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: Uuid,
    pub channel: String,
    pub sender_display: String,
    pub body: String,
    pub sent_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldEvent {
    pub id: Uuid,
    pub sector: String,
    pub title: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailMessage {
    pub id: Uuid,
    pub from: String,
    pub subject: String,
    pub body: String,
    pub read: bool,
    pub received_at: DateTime<Utc>,
}

/// Player's combat stance — determines whether other players can challenge them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum CombatStance {
    #[default]
    Pve,
    Pvp,
}

/// A record of an NPC defeat or succession event in the NetCity history ledger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub event: String,
    pub npc_name: String,
    pub npc_role: String,
    pub generation: u32,
    pub defeated_by: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptRunResult {
    pub output: String,
    pub exit_code: i32,
    pub consumed_ops: u64,
    pub elapsed_ms: u64,
}
