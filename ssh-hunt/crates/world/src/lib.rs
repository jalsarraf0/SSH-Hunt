#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use ipnet::IpNet;
use protocol::{AuctionListing, ChatMessage, MissionState, MissionStatus, Mode, WorldEvent};
use rand::{rng, Rng};
use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::sync::RwLock;
use uuid::Uuid;

const KEYS_VAULT: &str = "keys-vault";
const STARTER_CODES: [&str; 5] = [
    "pipes-101",
    "finder",
    "redirect-lab",
    "log-hunt",
    "dedupe-city",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExperienceTier {
    Noob,
    Gud,
    Hardcore,
}

impl ExperienceTier {
    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "noob" => Some(Self::Noob),
            "gud" => Some(Self::Gud),
            "hardcore" => Some(Self::Hardcore),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminSecret {
    pub username: String,
    pub allowed_cidrs: Vec<String>,
    pub auto_keygen_on_first_login: bool,
    pub required_key_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretMissionConfig {
    pub code: String,
    pub min_reputation: i64,
    pub required_achievement: Option<String>,
    pub prompt_ciphertext_b64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramRelayConfig {
    pub bot_token: String,
    pub chat_id: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HiddenOpsConfig {
    pub secret_mission: Option<SecretMissionConfig>,
    pub telegram: Option<TelegramRelayConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionDefinition {
    pub code: String,
    pub title: String,
    pub required: bool,
    pub starter: bool,
    pub hidden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerProfile {
    pub id: Uuid,
    pub username: String,
    pub remote_ip: String,
    pub display_name: String,
    pub tier: ExperienceTier,
    pub mode: Mode,
    pub deaths: u32,
    pub banned: bool,
    pub wallet: i64,
    pub streak: u32,
    pub streak_day: Option<NaiveDate>,
    pub registered_key_fingerprints: HashSet<String>,
    pub observed_fingerprints: HashSet<String>,
    pub completed_missions: HashSet<String>,
    pub active_missions: HashSet<String>,
    pub achievements: HashSet<String>,
    pub reputation: i64,
    pub daily_style_bonus_claims: u8,
    pub last_style_bonus_day: Option<NaiveDate>,
    pub private_alias: String,
}

impl PlayerProfile {
    pub fn new(username: &str, remote_ip: &str) -> Self {
        let id = Uuid::new_v4();
        Self {
            id,
            username: username.to_owned(),
            remote_ip: remote_ip.to_owned(),
            display_name: format!("{username}@{remote_ip}"),
            tier: ExperienceTier::Noob,
            mode: Mode::Training,
            deaths: 0,
            banned: false,
            wallet: 500,
            streak: 0,
            streak_day: None,
            registered_key_fingerprints: HashSet::new(),
            observed_fingerprints: HashSet::new(),
            completed_missions: HashSet::new(),
            active_missions: HashSet::new(),
            achievements: HashSet::new(),
            reputation: 0,
            daily_style_bonus_claims: 0,
            last_style_bonus_day: None,
            private_alias: format!("hunter-{}", &id.to_string()[..8]),
        }
    }

    pub fn can_access_netcity(&self) -> bool {
        self.completed_missions.contains(KEYS_VAULT)
            && STARTER_CODES
                .iter()
                .any(|code| self.completed_missions.contains(*code))
    }
}

#[derive(Debug, Clone)]
pub struct AuctionListingState {
    pub listing: AuctionListing,
    pub highest_bid: Option<i64>,
    pub highest_bidder: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct DuelState {
    pub duel_id: Uuid,
    pub left: Uuid,
    pub right: Uuid,
    pub left_hp: i32,
    pub right_hp: i32,
    pub left_defending: bool,
    pub right_defending: bool,
    pub started_at: DateTime<Utc>,
    pub last_actor: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub enum CombatAction {
    Attack,
    Defend,
    Script(String),
}

#[derive(Debug, Clone)]
pub struct CombatResult {
    pub narrative: String,
    pub ended: bool,
    pub winner: Option<Uuid>,
    pub loser: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct AuctionListingSnapshot {
    pub listing_id: Uuid,
    pub seller_display: String,
    pub item_sku: String,
    pub qty: u32,
    pub start_price: i64,
    pub highest_bid: Option<i64>,
    pub buyout_price: Option<i64>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct WorldEventSnapshot {
    pub sector: String,
    pub title: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub active: bool,
}

#[derive(Debug, Default)]
struct WorldState {
    players: HashMap<Uuid, PlayerProfile>,
    players_by_username: HashMap<String, Vec<Uuid>>,
    missions: HashMap<String, MissionDefinition>,
    auctions: HashMap<Uuid, AuctionListingState>,
    chats: Vec<ChatMessage>,
    events: Vec<WorldEvent>,
    duels: HashMap<Uuid, DuelState>,
    daily_claimed: HashMap<(Uuid, NaiveDate), bool>,
    listing_count_window: HashMap<Uuid, (DateTime<Utc>, u32)>,
}

pub struct WorldService {
    pool: Option<PgPool>,
    state: Arc<RwLock<WorldState>>,
    hidden_ops: HiddenOpsConfig,
    telegram_client: Client,
}

impl WorldService {
    pub fn new(pool: Option<PgPool>, hidden_ops: HiddenOpsConfig) -> Self {
        let mut state = WorldState::default();
        for mission in seed_missions() {
            state.missions.insert(mission.code.clone(), mission);
        }
        if let Some(secret) = &hidden_ops.secret_mission {
            state.missions.insert(
                secret.code.clone(),
                MissionDefinition {
                    code: secret.code.clone(),
                    title: "Encrypted Contact Thread".to_owned(),
                    required: false,
                    starter: false,
                    hidden: true,
                },
            );
        }
        state.events = seed_events();

        Self {
            pool,
            state: Arc::new(RwLock::new(state)),
            hidden_ops,
            telegram_client: Client::new(),
        }
    }

    pub async fn login(
        &self,
        username: &str,
        remote_ip: &str,
        offered_fingerprints: &[String],
    ) -> Result<PlayerProfile> {
        let mut guard = self.state.write().await;
        let candidates = guard
            .players_by_username
            .get(username)
            .cloned()
            .unwrap_or_default();

        let mut selected: Option<Uuid> = None;
        for id in candidates {
            if let Some(p) = guard.players.get(&id) {
                if p.registered_key_fingerprints
                    .iter()
                    .any(|fp| offered_fingerprints.iter().any(|offered| offered == fp))
                {
                    selected = Some(id);
                    break;
                }
            }
        }

        let player_id = if let Some(id) = selected {
            id
        } else if let Some(existing) = guard
            .players_by_username
            .get(username)
            .and_then(|ids| ids.first())
            .copied()
        {
            existing
        } else {
            let profile = PlayerProfile::new(username, remote_ip);
            let id = profile.id;
            guard.players.insert(id, profile);
            guard
                .players_by_username
                .entry(username.to_owned())
                .or_default()
                .push(id);
            id
        };

        let player = guard
            .players
            .get_mut(&player_id)
            .context("player not found after login")?;
        player.remote_ip = remote_ip.to_owned();
        player.display_name = format!("{username}@{remote_ip}");
        player
            .observed_fingerprints
            .extend(offered_fingerprints.iter().cloned());

        if let Some(pool) = &self.pool {
            persist_player_login(pool, player).await?;
        }

        Ok(player.clone())
    }

    pub async fn get_player(&self, player_id: Uuid) -> Option<PlayerProfile> {
        self.state.read().await.players.get(&player_id).cloned()
    }

    pub fn hidden_mission_code(&self) -> Option<&str> {
        self.hidden_ops
            .secret_mission
            .as_ref()
            .map(|cfg| cfg.code.as_str())
    }

    pub async fn player_has_completed_hidden_mission(&self, player_id: Uuid) -> bool {
        let Some(code) = self.hidden_mission_code() else {
            return false;
        };
        let guard = self.state.read().await;
        guard
            .players
            .get(&player_id)
            .map(|p| p.completed_missions.contains(code))
            .unwrap_or(false)
    }

    pub async fn resolve_player_by_username(&self, username: &str) -> Option<PlayerProfile> {
        let guard = self.state.read().await;
        let id = guard
            .players_by_username
            .get(username)
            .and_then(|ids| ids.first())
            .copied()?;
        guard.players.get(&id).cloned()
    }

    pub async fn roster(&self) -> Vec<String> {
        let guard = self.state.read().await;
        let mut out = guard
            .players
            .values()
            .filter(|p| !p.banned)
            .map(|p| p.display_name.clone())
            .collect::<Vec<_>>();
        out.sort();
        out
    }

    pub async fn set_tier(&self, player_id: Uuid, tier: ExperienceTier) -> Result<PlayerProfile> {
        let mut guard = self.state.write().await;
        let player = guard
            .players
            .get_mut(&player_id)
            .ok_or_else(|| anyhow!("unknown player"))?;
        player.tier = tier;
        Ok(player.clone())
    }

    pub async fn ban_forever(
        &self,
        player_id: Uuid,
        reason: &str,
        actor: &str,
    ) -> Result<PlayerProfile> {
        let mut guard = self.state.write().await;
        let player = guard
            .players
            .get_mut(&player_id)
            .ok_or_else(|| anyhow!("unknown player"))?;
        player.banned = true;

        if let Some(pool) = &self.pool {
            sqlx::query("UPDATE players SET banned = true, updated_at = now() WHERE id = $1")
                .bind(player_id)
                .execute(pool)
                .await?;

            sqlx::query(
                r#"
                INSERT INTO moderation_actions(id, actor, action, target, reason, created_at)
                VALUES($1, $2, 'ban', $3, $4, now())
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(actor)
            .bind(player.display_name.clone())
            .bind(reason)
            .execute(pool)
            .await?;
        }

        Ok(player.clone())
    }

    pub async fn register_key(&self, player_id: Uuid, pubkey_line: &str) -> Result<String> {
        validate_pubkey_line(pubkey_line)?;
        let fingerprint = fingerprint(pubkey_line);
        let mut guard = self.state.write().await;
        let player = guard
            .players
            .get_mut(&player_id)
            .ok_or_else(|| anyhow!("unknown player"))?;
        player
            .registered_key_fingerprints
            .insert(fingerprint.clone());

        if let Some(pool) = &self.pool {
            sqlx::query(
                r#"
                INSERT INTO player_keys(player_id, fingerprint, public_key)
                VALUES ($1, $2, $3)
                ON CONFLICT DO NOTHING
                "#,
            )
            .bind(player_id)
            .bind(&fingerprint)
            .bind(pubkey_line)
            .execute(pool)
            .await?;
        }

        Ok(fingerprint)
    }

    pub async fn accept_mission(&self, player_id: Uuid, code: &str) -> Result<()> {
        let mut guard = self.state.write().await;
        let mission = guard
            .missions
            .get(code)
            .ok_or_else(|| anyhow!("unknown mission"))?;
        if mission.hidden && !self.player_can_see_hidden(&guard, player_id) {
            return Err(anyhow!("unknown mission"));
        }
        let player = guard
            .players
            .get_mut(&player_id)
            .ok_or_else(|| anyhow!("unknown player"))?;
        player.active_missions.insert(code.to_owned());
        Ok(())
    }

    pub async fn complete_mission(&self, player_id: Uuid, code: &str) -> Result<()> {
        let mut guard = self.state.write().await;
        if !guard.missions.contains_key(code) {
            return Err(anyhow!("unknown mission"));
        }

        let player = guard
            .players
            .get_mut(&player_id)
            .ok_or_else(|| anyhow!("unknown player"))?;
        player.active_missions.remove(code);
        player.completed_missions.insert(code.to_owned());
        player.reputation += if code == KEYS_VAULT { 15 } else { 10 };

        if let Some(pool) = &self.pool {
            sqlx::query(
                r#"
                INSERT INTO mission_progress(player_id, mission_code, completed_at)
                VALUES ($1, $2, now())
                ON CONFLICT (player_id, mission_code)
                DO UPDATE SET completed_at = now()
                "#,
            )
            .bind(player_id)
            .bind(code)
            .execute(pool)
            .await?;
        }
        Ok(())
    }

    pub async fn mission_statuses(&self, player_id: Uuid) -> Result<Vec<MissionStatus>> {
        let guard = self.state.read().await;
        let player = guard
            .players
            .get(&player_id)
            .ok_or_else(|| anyhow!("unknown player"))?;

        let mut statuses = Vec::new();
        for mission in guard.missions.values() {
            if mission.hidden && !self.player_can_see_hidden(&guard, player_id) {
                continue;
            }

            let state = if player.completed_missions.contains(&mission.code) {
                MissionState::Completed
            } else if player.active_missions.contains(&mission.code) {
                MissionState::Active
            } else {
                MissionState::Available
            };

            statuses.push(MissionStatus {
                code: mission.code.clone(),
                title: mission.title.clone(),
                state,
                progress: if player.completed_missions.contains(&mission.code) {
                    100
                } else {
                    0
                },
                required: mission.required,
            });
        }
        statuses.sort_by(|a, b| a.code.cmp(&b.code));
        Ok(statuses)
    }

    pub async fn netcity_gate_reason(
        &self,
        player_id: Uuid,
        offered_fingerprints: &[String],
    ) -> Result<Option<String>> {
        let guard = self.state.read().await;
        let player = guard
            .players
            .get(&player_id)
            .ok_or_else(|| anyhow!("unknown player"))?;

        if !player.completed_missions.contains(KEYS_VAULT) {
            return Ok(Some("Complete mission KEYS VAULT first".to_owned()));
        }
        if !STARTER_CODES
            .iter()
            .any(|code| player.completed_missions.contains(*code))
        {
            return Ok(Some(
                "Complete one starter mission to unlock NetCity".to_owned(),
            ));
        }

        if player.registered_key_fingerprints.is_empty() {
            return Ok(Some(
                "Register an SSH public key with keyvault register".to_owned(),
            ));
        }

        let offered_match = offered_fingerprints
            .iter()
            .any(|fp| player.registered_key_fingerprints.contains(fp));

        if !offered_match {
            return Ok(Some(
                "This login did not present your registered SSH key. Training Sim allowed; NetCity locked."
                    .to_owned(),
            ));
        }

        if player.banned {
            return Ok(Some("Account is zeroed and locked".to_owned()));
        }

        Ok(None)
    }

    pub async fn claim_daily_reward(&self, player_id: Uuid, now: DateTime<Utc>) -> Result<i64> {
        let day = now.date_naive();
        let mut guard = self.state.write().await;

        if guard
            .daily_claimed
            .get(&(player_id, day))
            .copied()
            .unwrap_or(false)
        {
            return Ok(0);
        }

        let player = guard
            .players
            .get_mut(&player_id)
            .ok_or_else(|| anyhow!("unknown player"))?;

        if let Some(last) = player.streak_day {
            if last + Duration::days(1) == day {
                player.streak = (player.streak + 1).min(7);
            } else if last != day {
                player.streak = 1;
            }
        } else {
            player.streak = 1;
        }

        player.streak_day = Some(day);
        let reward = 50 + (player.streak as i64 * 15).min(120);
        player.wallet += reward;
        guard.daily_claimed.insert((player_id, day), true);
        Ok(reward)
    }

    pub async fn style_bonus(
        &self,
        player_id: Uuid,
        pipeline_depth: usize,
        unique_tools: usize,
    ) -> Result<i64> {
        let today = Utc::now().date_naive();
        let mut guard = self.state.write().await;
        let player = guard
            .players
            .get_mut(&player_id)
            .ok_or_else(|| anyhow!("unknown player"))?;

        if player.last_style_bonus_day != Some(today) {
            player.last_style_bonus_day = Some(today);
            player.daily_style_bonus_claims = 0;
        }

        if player.daily_style_bonus_claims >= 5 {
            return Ok(0);
        }

        let score = ((pipeline_depth as i64 * 8) + (unique_tools as i64 * 5)).min(75);
        let diminished =
            (score as f64 * (1.0 - (player.daily_style_bonus_claims as f64 * 0.2))) as i64;
        let reward = diminished.max(0);
        player.daily_style_bonus_claims += 1;
        player.wallet += reward;

        if pipeline_depth >= 3 {
            player.achievements.insert("Pipe Dream".to_owned());
        }
        if unique_tools >= 4 {
            player.achievements.insert("Gremlin Grep".to_owned());
        }
        if pipeline_depth >= 2 {
            player.achievements.insert("Redirection Wizard".to_owned());
        }

        Ok(reward)
    }

    pub async fn create_listing(
        &self,
        seller: Uuid,
        item_sku: &str,
        qty: u32,
        start_price: i64,
        buyout: Option<i64>,
    ) -> Result<AuctionListing> {
        const MIN_PRICE_FLOOR: i64 = 25;
        const LISTING_FEE: i64 = 10;
        const MAX_LISTINGS_PER_30S: u32 = 3;

        if start_price < MIN_PRICE_FLOOR {
            return Err(anyhow!("price below floor"));
        }

        let now = Utc::now();
        let mut guard = self.state.write().await;
        let current_wallet = guard
            .players
            .get(&seller)
            .ok_or_else(|| anyhow!("unknown player"))?
            .wallet;
        if current_wallet < LISTING_FEE {
            return Err(anyhow!("insufficient funds for listing fee"));
        }

        {
            let window = guard.listing_count_window.entry(seller).or_insert((now, 0));
            if now - window.0 > Duration::seconds(30) {
                *window = (now, 0);
            }
            if window.1 >= MAX_LISTINGS_PER_30S {
                return Err(anyhow!("listing rate limit exceeded"));
            }
            window.1 += 1;
        }

        if let Some(player) = guard.players.get_mut(&seller) {
            player.wallet -= LISTING_FEE;
        }

        let listing = AuctionListing {
            listing_id: Uuid::new_v4(),
            seller_id: seller,
            item_sku: item_sku.to_owned(),
            qty,
            start_price,
            buyout_price: buyout,
            expires_at: now + Duration::hours(12),
        };
        let state = AuctionListingState {
            listing: listing.clone(),
            highest_bid: None,
            highest_bidder: None,
            created_at: now,
        };
        guard.auctions.insert(listing.listing_id, state);
        Ok(listing)
    }

    pub async fn place_bid(&self, bidder: Uuid, listing_id: Uuid, amount: i64) -> Result<()> {
        let mut guard = self.state.write().await;
        let player_wallet = guard
            .players
            .get(&bidder)
            .ok_or_else(|| anyhow!("unknown bidder"))?
            .wallet;
        let listing = guard
            .auctions
            .get_mut(&listing_id)
            .ok_or_else(|| anyhow!("listing not found"))?;

        if Utc::now() > listing.listing.expires_at {
            return Err(anyhow!("listing expired"));
        }

        let min = listing.highest_bid.unwrap_or(listing.listing.start_price);
        if amount <= min {
            return Err(anyhow!("bid too low"));
        }

        if player_wallet < amount {
            return Err(anyhow!("insufficient funds"));
        }

        listing.highest_bid = Some(amount);
        listing.highest_bidder = Some(bidder);
        Ok(())
    }

    pub async fn buyout(&self, buyer: Uuid, listing_id: Uuid) -> Result<()> {
        const TAX_BPS: i64 = 300;
        let mut guard = self.state.write().await;
        let listing = guard
            .auctions
            .remove(&listing_id)
            .ok_or_else(|| anyhow!("listing not found"))?;
        let buyout = listing
            .listing
            .buyout_price
            .ok_or_else(|| anyhow!("listing has no buyout"))?;

        let buyer_wallet = guard
            .players
            .get(&buyer)
            .ok_or_else(|| anyhow!("unknown buyer"))?
            .wallet;
        if buyer_wallet < buyout {
            return Err(anyhow!("insufficient funds"));
        }

        let tax = buyout * TAX_BPS / 10_000;
        if let Some(buyer_state) = guard.players.get_mut(&buyer) {
            buyer_state.wallet -= buyout;
        }
        if let Some(seller_state) = guard.players.get_mut(&listing.listing.seller_id) {
            seller_state.wallet += buyout - tax;
        }
        Ok(())
    }

    pub async fn market_snapshot(&self) -> Vec<AuctionListingSnapshot> {
        let guard = self.state.read().await;
        let mut out = guard
            .auctions
            .values()
            .map(|entry| AuctionListingSnapshot {
                listing_id: entry.listing.listing_id,
                seller_display: guard
                    .players
                    .get(&entry.listing.seller_id)
                    .map(|p| p.display_name.clone())
                    .unwrap_or_else(|| "unknown".to_owned()),
                item_sku: entry.listing.item_sku.clone(),
                qty: entry.listing.qty,
                start_price: entry.listing.start_price,
                highest_bid: entry.highest_bid,
                buyout_price: entry.listing.buyout_price,
                expires_at: entry.listing.expires_at,
            })
            .collect::<Vec<_>>();
        out.sort_by(|a, b| a.expires_at.cmp(&b.expires_at));
        out
    }

    pub async fn world_events_snapshot(&self, now: DateTime<Utc>) -> Vec<WorldEventSnapshot> {
        let guard = self.state.read().await;
        let mut out = guard
            .events
            .iter()
            .filter(|event| event.ends_at >= now)
            .map(|event| WorldEventSnapshot {
                sector: event.sector.clone(),
                title: event.title.clone(),
                starts_at: event.starts_at,
                ends_at: event.ends_at,
                active: event.starts_at <= now && event.ends_at >= now,
            })
            .collect::<Vec<_>>();
        out.sort_by(|a, b| a.starts_at.cmp(&b.starts_at));
        out
    }

    pub async fn post_chat(&self, sender: Uuid, channel: &str, body: &str) -> Result<ChatMessage> {
        let mut guard = self.state.write().await;
        let sender_display = guard
            .players
            .get(&sender)
            .ok_or_else(|| anyhow!("unknown sender"))?
            .display_name
            .clone();

        let msg = ChatMessage {
            id: Uuid::new_v4(),
            channel: channel.to_owned(),
            sender_display,
            body: body.to_owned(),
            sent_at: Utc::now(),
        };
        guard.chats.push(msg.clone());
        Ok(msg)
    }

    pub async fn mode_switch(
        &self,
        player_id: Uuid,
        mode: Mode,
        flash: Option<bool>,
    ) -> Result<String> {
        if mode == Mode::NetCity {
            let offered = {
                let guard = self.state.read().await;
                let player = guard
                    .players
                    .get(&player_id)
                    .ok_or_else(|| anyhow!("unknown player"))?;
                if player.banned {
                    return Err(anyhow!("account zeroed"));
                }
                player
                    .observed_fingerprints
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
            };

            if let Some(reason) = self.netcity_gate_reason(player_id, &offered).await? {
                return Err(anyhow!(reason));
            }
        }

        let mut guard = self.state.write().await;
        let player = guard
            .players
            .get_mut(&player_id)
            .ok_or_else(|| anyhow!("unknown player"))?;
        if player.banned {
            return Err(anyhow!("account zeroed"));
        }

        player.mode = mode.clone();
        let transition = match mode {
            Mode::Training => "MODE SWITCH: NETCITY MMO/REDLINE -> TRAINING SIM",
            Mode::NetCity => "MODE SWITCH: TRAINING SIM -> NETCITY MMO",
            Mode::Redline => "MODE SWITCH: TRAINING/NETCITY -> REDLINE",
        };

        if let Some(_flash_on) = flash {
            // session-level toggle handled at transport layer; accepted for command compatibility.
        }

        Ok(transition.to_owned())
    }

    pub async fn start_duel(&self, left: Uuid, right: Uuid) -> Result<DuelState> {
        let mut guard = self.state.write().await;
        ensure_not_zeroed(&guard, left)?;
        ensure_not_zeroed(&guard, right)?;

        let duel = DuelState {
            duel_id: Uuid::new_v4(),
            left,
            right,
            left_hp: 100,
            right_hp: 100,
            left_defending: false,
            right_defending: false,
            started_at: Utc::now(),
            last_actor: None,
        };
        guard.duels.insert(duel.duel_id, duel.clone());
        Ok(duel)
    }

    pub async fn duel_action(
        &self,
        duel_id: Uuid,
        actor: Uuid,
        action: CombatAction,
    ) -> Result<CombatResult> {
        let mut guard = self.state.write().await;
        let duel = guard
            .duels
            .get_mut(&duel_id)
            .ok_or_else(|| anyhow!("duel not found"))?;
        let actor_is_left = actor == duel.left;
        if !actor_is_left && actor != duel.right {
            return Err(anyhow!("not a duel participant"));
        }

        let (attacker_hp, defender_hp, attacker_def, defender_def, defender_id) = if actor_is_left {
            (
                &mut duel.left_hp,
                &mut duel.right_hp,
                &mut duel.left_defending,
                &mut duel.right_defending,
                duel.right,
            )
        } else {
            (
                &mut duel.right_hp,
                &mut duel.left_hp,
                &mut duel.right_defending,
                &mut duel.left_defending,
                duel.left,
            )
        };

        let mut narrative = match action {
            CombatAction::Defend => {
                *attacker_def = true;
                "Defensive shell hardening enabled (+mitigation)".to_owned()
            }
            CombatAction::Attack => {
                let mut dmg = rng().random_range(14..=30);
                if *defender_def {
                    dmg = (dmg / 2).max(5);
                    *defender_def = false;
                }
                *defender_hp -= dmg;
                *attacker_def = false;
                format!("Exploit chain landed for {dmg} integrity damage")
            }
            CombatAction::Script(script_name) => {
                let mut dmg = 10 + (script_name.len() as i32 % 17);
                if *defender_def {
                    dmg = (dmg / 2).max(4);
                    *defender_def = false;
                }
                *defender_hp -= dmg;
                *attacker_def = false;
                format!("Script `{script_name}` executed, causing {dmg} disruption")
            }
        };

        duel.last_actor = Some(actor);
        let ended = *defender_hp <= 0 || *attacker_hp <= 0;
        if ended {
            let (winner, loser) = if duel.left_hp > duel.right_hp {
                (duel.left, duel.right)
            } else {
                (duel.right, duel.left)
            };
            guard.duels.remove(&duel_id);

            if let Some(w) = guard.players.get_mut(&winner) {
                w.wallet += 40;
                w.reputation += 3;
            }
            if let Some(l) = guard.players.get_mut(&loser) {
                l.deaths += 1;
                if l.tier == ExperienceTier::Hardcore && l.deaths >= 3 {
                    l.banned = true;
                }
            }

            narrative.push_str(". Duel complete.");
            return Ok(CombatResult {
                narrative,
                ended: true,
                winner: Some(winner),
                loser: Some(loser),
            });
        }

        let _ = defender_id;

        Ok(CombatResult {
            narrative,
            ended: false,
            winner: None,
            loser: None,
        })
    }

    pub async fn is_super_admin_candidate(
        &self,
        username: &str,
        remote_ip: &str,
        secret: &AdminSecret,
    ) -> bool {
        if username != secret.username {
            return false;
        }
        let Ok(ip) = IpAddr::from_str(remote_ip) else {
            return false;
        };
        secret.allowed_cidrs.iter().any(|raw| {
            IpNet::from_str(raw)
                .map(|cidr| cidr.contains(&ip))
                .unwrap_or(false)
        })
    }

    pub async fn relay_to_admin_via_telegram(&self, player_id: Uuid, message: &str) -> Result<()> {
        let Some(cfg) = &self.hidden_ops.telegram else {
            return Ok(());
        };
        if !cfg.enabled {
            return Ok(());
        }

        let alias = {
            let guard = self.state.read().await;
            guard
                .players
                .get(&player_id)
                .ok_or_else(|| anyhow!("unknown player"))?
                .private_alias
                .clone()
        };

        // PII-safe: only alias and message body are sent.
        let payload = serde_json::json!({
            "chat_id": cfg.chat_id,
            "text": format!("[SSH-Hunt secret relay] {alias}: {message}"),
            "disable_web_page_preview": true,
        });

        let url = format!("https://api.telegram.org/bot{}/sendMessage", cfg.bot_token);
        self.telegram_client
            .post(url)
            .json(&payload)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    fn player_can_see_hidden(&self, guard: &WorldState, player_id: Uuid) -> bool {
        let Some(secret) = &self.hidden_ops.secret_mission else {
            return false;
        };
        let Some(player) = guard.players.get(&player_id) else {
            return false;
        };

        if player.reputation < secret.min_reputation {
            return false;
        }

        if let Some(required) = &secret.required_achievement {
            player.achievements.contains(required)
        } else {
            true
        }
    }
}

fn ensure_not_zeroed(guard: &WorldState, player_id: Uuid) -> Result<()> {
    let player = guard
        .players
        .get(&player_id)
        .ok_or_else(|| anyhow!("unknown player"))?;
    if player.banned {
        Err(anyhow!("player zeroed"))
    } else {
        Ok(())
    }
}

fn seed_missions() -> Vec<MissionDefinition> {
    vec![
        MissionDefinition {
            code: KEYS_VAULT.to_owned(),
            title: "KEYS VAULT: Secure Your Access".to_owned(),
            required: true,
            starter: false,
            hidden: false,
        },
        MissionDefinition {
            code: "pipes-101".to_owned(),
            title: "Pipe Dream: Signals Through Neon".to_owned(),
            required: false,
            starter: true,
            hidden: false,
        },
        MissionDefinition {
            code: "finder".to_owned(),
            title: "Ghost Index: Find and Chain".to_owned(),
            required: false,
            starter: true,
            hidden: false,
        },
        MissionDefinition {
            code: "redirect-lab".to_owned(),
            title: "Data Splice: Redirect Lab".to_owned(),
            required: false,
            starter: true,
            hidden: false,
        },
        MissionDefinition {
            code: "log-hunt".to_owned(),
            title: "Corp Leak: Parse the Logs".to_owned(),
            required: false,
            starter: true,
            hidden: false,
        },
        MissionDefinition {
            code: "dedupe-city".to_owned(),
            title: "Signal Noise: Sort and Uniq".to_owned(),
            required: false,
            starter: true,
            hidden: false,
        },
    ]
}

fn seed_events() -> Vec<WorldEvent> {
    let now = Utc::now();
    vec![
        WorldEvent {
            id: Uuid::new_v4(),
            sector: "Neon Bazaar".to_owned(),
            title: "Black Ice Storm".to_owned(),
            starts_at: now + Duration::minutes(25),
            ends_at: now + Duration::minutes(40),
        },
        WorldEvent {
            id: Uuid::new_v4(),
            sector: "Ghost Rail".to_owned(),
            title: "Datavault Breach Drill".to_owned(),
            starts_at: now + Duration::minutes(60),
            ends_at: now + Duration::minutes(80),
        },
    ]
}

fn validate_pubkey_line(pubkey_line: &str) -> Result<()> {
    let re = Regex::new(r"^ssh-(ed25519|rsa)\s+[A-Za-z0-9+/=]+(?:\s+.+)?$")
        .map_err(|e| anyhow!("failed to build key regex: {e}"))?;
    if !re.is_match(pubkey_line.trim()) {
        return Err(anyhow!("invalid OpenSSH public key format"));
    }
    Ok(())
}

fn fingerprint(pubkey_line: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(pubkey_line.trim().as_bytes());
    let out = hasher.finalize();
    format!("SHA256:{:x}", out)
}

async fn persist_player_login(pool: &PgPool, player: &PlayerProfile) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO players (id, username, display_name, tier, deaths, banned, wallet, reputation)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (id)
        DO UPDATE SET
            username = EXCLUDED.username,
            display_name = EXCLUDED.display_name,
            tier = EXCLUDED.tier,
            deaths = EXCLUDED.deaths,
            banned = EXCLUDED.banned,
            wallet = EXCLUDED.wallet,
            reputation = EXCLUDED.reputation,
            updated_at = now()
        "#,
    )
    .bind(player.id)
    .bind(&player.username)
    .bind(&player.display_name)
    .bind(format!("{:?}", player.tier).to_lowercase())
    .bind(player.deaths as i32)
    .bind(player.banned)
    .bind(player.wallet)
    .bind(player.reputation)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO player_ips(player_id, remote_ip, seen_at)
        VALUES($1, $2, now())
        "#,
    )
    .bind(player.id)
    .bind(&player.remote_ip)
    .execute(pool)
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> WorldService {
        WorldService::new(
            None,
            HiddenOpsConfig {
                secret_mission: Some(SecretMissionConfig {
                    code: "hidden-contact".to_owned(),
                    min_reputation: 20,
                    required_achievement: Some("Pipe Dream".to_owned()),
                    prompt_ciphertext_b64: "AA==".to_owned(),
                }),
                telegram: None,
            },
        )
    }

    #[tokio::test]
    async fn key_vault_unlock_gate() {
        let world = service();
        let player = world.login("neo", "203.0.113.4", &[]).await.unwrap();
        assert!(world
            .netcity_gate_reason(player.id, &[])
            .await
            .unwrap()
            .is_some());

        let key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMockKeyData user@host";
        let fp = world.register_key(player.id, key).await.unwrap();
        world
            .complete_mission(player.id, "keys-vault")
            .await
            .unwrap();
        world
            .complete_mission(player.id, "pipes-101")
            .await
            .unwrap();

        let reason = world.netcity_gate_reason(player.id, &[fp]).await.unwrap();
        assert!(reason.is_none());
    }

    #[tokio::test]
    async fn auction_floor_and_rate_limit() {
        let world = service();
        let p = world.login("seller", "203.0.113.6", &[]).await.unwrap();
        assert!(world
            .create_listing(p.id, "script.basic", 1, 10, None)
            .await
            .is_err());

        world
            .create_listing(p.id, "script.basic", 1, 30, Some(120))
            .await
            .unwrap();
        world
            .create_listing(p.id, "script.fast", 1, 40, Some(140))
            .await
            .unwrap();
        world
            .create_listing(p.id, "script.pro", 1, 50, Some(150))
            .await
            .unwrap();

        assert!(world
            .create_listing(p.id, "script.rate", 1, 60, Some(160))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn hardcore_zero_after_three_deaths() {
        let world = service();
        let p1 = world.login("a", "203.0.113.8", &[]).await.unwrap();
        let p2 = world.login("b", "203.0.113.9", &[]).await.unwrap();
        world
            .set_tier(p2.id, ExperienceTier::Hardcore)
            .await
            .unwrap();

        for _ in 0..3 {
            let duel = world.start_duel(p1.id, p2.id).await.unwrap();
            loop {
                let turn = world
                    .duel_action(duel.duel_id, p1.id, CombatAction::Script("burst".into()))
                    .await
                    .unwrap();
                if turn.ended {
                    break;
                }
            }
        }

        let refreshed = world.get_player(p2.id).await.unwrap();
        assert!(refreshed.deaths >= 3);
        assert!(refreshed.banned);
    }

    #[tokio::test]
    async fn hidden_mission_not_listed_until_eligible() {
        let world = service();
        let p = world.login("c", "203.0.113.11", &[]).await.unwrap();

        let before = world.mission_statuses(p.id).await.unwrap();
        assert!(!before.iter().any(|m| m.code == "hidden-contact"));

        world.style_bonus(p.id, 4, 4).await.unwrap();
        world.complete_mission(p.id, "keys-vault").await.unwrap();
        world.complete_mission(p.id, "pipes-101").await.unwrap();
        world.complete_mission(p.id, "finder").await.unwrap();

        let after = world.mission_statuses(p.id).await.unwrap();
        assert!(after.iter().any(|m| m.code == "hidden-contact"));
    }

    #[tokio::test]
    async fn mode_switch_netcity_returns_without_deadlock() {
        let world = service();
        let p = world.login("switcher", "203.0.113.17", &[]).await.unwrap();
        let fp = world
            .register_key(
                p.id,
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMockSwitchData switch@host",
            )
            .await
            .unwrap();
        world.complete_mission(p.id, "keys-vault").await.unwrap();
        world.complete_mission(p.id, "pipes-101").await.unwrap();

        let relog = world
            .login("switcher", "203.0.113.17", std::slice::from_ref(&fp))
            .await
            .unwrap();
        assert_eq!(relog.id, p.id);

        let switched = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            world.mode_switch(p.id, Mode::NetCity, Some(true)),
        )
        .await
        .expect("mode switch timed out")
        .unwrap();
        assert!(switched.contains("NETCITY"));
    }

    #[tokio::test]
    async fn market_snapshot_and_events_snapshot_are_available() {
        let world = service();
        let seller = world.login("vendor", "203.0.113.21", &[]).await.unwrap();
        let listing = world
            .create_listing(seller.id, "script.gremlin.grep", 2, 120, Some(250))
            .await
            .unwrap();

        let market = world.market_snapshot().await;
        assert!(market.iter().any(|entry| {
            entry.listing_id == listing.listing_id
                && entry.item_sku == "script.gremlin.grep"
                && entry.seller_display.contains("vendor@203.0.113.21")
        }));

        let now = Utc::now();
        let feed = world
            .world_events_snapshot(now + Duration::minutes(30))
            .await;
        assert!(feed.iter().any(|event| event.active));
    }
}
