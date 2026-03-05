#![forbid(unsafe_code)]

mod builtins;
mod config;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use protocol::Mode;
use russh::keys::ssh_key::rand_core::OsRng;
use russh::server::{self, Msg, Server as _};
use russh::{Channel, ChannelId, CryptoVec};
use shell::{ExecutionContext, ShellEngine};
use sqlx::PgPool;
use ssh_hunt_scripts::{ScriptContext, ScriptEngine, ScriptPolicy};
use tokio::net::TcpListener;
use tokio::time::{sleep, timeout};
use tracing::{error, info, warn};
use ui::{
    key_value_line, lore_message, mission_state_badge, mode_banner, mode_switch_banner,
    progress_meter, section_banner, Theme, RESET,
};
use uuid::Uuid;
use vfs::Vfs;
use world::{
    AdminSecret, CombatAction, ExperienceTier, HiddenOpsConfig, PlayerProfile, WorldService,
};

#[derive(Debug, Parser)]
struct Args {
    #[arg(long)]
    healthcheck: bool,
}

#[derive(Clone)]
struct AppState {
    cfg: config::ServerConfig,
    world: Arc<WorldService>,
    shell: Arc<ShellEngine>,
    script: Arc<ScriptEngine>,
    admin_secret: Option<AdminSecret>,
}

struct ShellState {
    vfs: Vfs,
    cwd: String,
    user: String,
    node: String,
    env: HashMap<String, String>,
    last_exit: i32,
}

impl ShellState {
    fn bootstrap(display_name: &str) -> Self {
        let mut vfs = Vfs::default();
        let _ = vfs.mkdir_p("/", "home", "system");
        let _ = vfs.mkdir_p("/", "tmp", "system");
        let _ = vfs.mkdir_p("/", "logs", "system");
        let _ = vfs.mkdir_p("/", "missions", "system");
        let _ = vfs.mkdir_p("/", "home/player", "player");

        let _ = vfs.write_file(
            "/",
            "/missions/readme.txt",
            "Run: tutorial start\nThen: missions\n",
            false,
            "system",
        );
        let _ = vfs.write_file(
            "/",
            "/logs/neon-gateway.log",
            "[INFO] token=GLASS-AXON-13\n[WARN] sector drift\n[INFO] token=GLASS-AXON-13\n",
            false,
            "system",
        );

        let cwd = "/home/player".to_owned();
        let node = "corp-sim-01".to_owned();
        let mut env = HashMap::new();
        env.insert("USER".to_owned(), display_name.to_owned());
        env.insert("HOME".to_owned(), "/home/player".to_owned());
        env.insert("PWD".to_owned(), cwd.clone());
        env.insert("PATH".to_owned(), "/bin:/usr/bin".to_owned());
        env.insert("?".to_owned(), "0".to_owned());

        Self {
            vfs,
            cwd,
            user: display_name.to_owned(),
            node,
            env,
            last_exit: 0,
        }
    }

    fn prompt(&self) -> String {
        format!("{}@{}:{}$ ", self.user, self.node, self.cwd)
    }

    fn execute_shell(&mut self, engine: &ShellEngine, line: &str) -> Result<shell::CommandResult> {
        let mut ctx = ExecutionContext {
            vfs: &mut self.vfs,
            cwd: self.cwd.clone(),
            user: self.user.clone(),
            node: self.node.clone(),
            env: self.env.clone(),
            last_exit: self.last_exit,
        };

        let result = engine.execute(&mut ctx, line)?;
        self.cwd = ctx.cwd;
        self.env = ctx.env;
        self.last_exit = result.exit_code;
        Ok(result)
    }
}

#[derive(Clone)]
struct GameServer {
    app: Arc<AppState>,
}

struct GameSession {
    app: Arc<AppState>,
    peer_addr: Option<SocketAddr>,
    username: String,
    offered_fingerprints: Vec<String>,
    player_id: Option<Uuid>,
    profile: Option<PlayerProfile>,
    shell_state: Option<ShellState>,
    mode: Mode,
    flash_enabled: bool,
    line_buffer: Vec<u8>,
    pending_keyvault: bool,
    pending_admin_passphrase: bool,
    current_duel: Option<Uuid>,
    redline_until: Option<Instant>,
    script_cooldown_until: Option<Instant>,
    command_window: VecDeque<Instant>,
}

impl GameSession {
    fn new(app: Arc<AppState>, peer_addr: Option<SocketAddr>) -> Self {
        Self {
            app,
            peer_addr,
            username: "guest".to_owned(),
            offered_fingerprints: Vec::new(),
            player_id: None,
            profile: None,
            shell_state: None,
            mode: Mode::Training,
            flash_enabled: true,
            line_buffer: Vec::new(),
            pending_keyvault: false,
            pending_admin_passphrase: false,
            current_duel: None,
            redline_until: None,
            script_cooldown_until: None,
            command_window: VecDeque::new(),
        }
    }

    async fn initialize_identity(&mut self) -> Result<()> {
        let remote_ip = self
            .peer_addr
            .map(|ip| ip.ip().to_string())
            .unwrap_or_else(|| "0.0.0.0".to_owned());
        let profile = self
            .app
            .world
            .login(&self.username, &remote_ip, &self.offered_fingerprints)
            .await?;

        self.player_id = Some(profile.id);
        self.mode = Mode::Training;
        self.flash_enabled = self.app.cfg.ui.flash_default;
        self.shell_state = Some(ShellState::bootstrap(&profile.display_name));
        self.profile = Some(profile.clone());

        if let Some(secret) = &self.app.admin_secret {
            if self
                .app
                .world
                .is_super_admin_candidate(&profile.username, &remote_ip, secret)
                .await
                && secret.auto_keygen_on_first_login
                && profile.registered_key_fingerprints.is_empty()
            {
                self.pending_admin_passphrase = true;
            }
        }

        Ok(())
    }

    fn enforce_rate_limit(&mut self) -> bool {
        let now = Instant::now();
        while self
            .command_window
            .front()
            .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(1))
        {
            self.command_window.pop_front();
        }

        if self.command_window.len() >= self.app.cfg.server.burst as usize {
            return false;
        }

        self.command_window.push_back(now);
        true
    }

    async fn run_line(&mut self, line: &str) -> Result<(String, i32, bool)> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Ok((String::new(), 0, false));
        }
        if trimmed == "exit" || trimmed == "logout" {
            return Ok(("Session closed.\n".to_owned(), 0, true));
        }

        if let Some(player_id) = self.player_id {
            if let Some(profile) = self.app.world.get_player(player_id).await {
                if profile.banned {
                    return Ok(("Account zeroed. Session terminated.\n".to_owned(), 1, true));
                }
            }
        }

        if self.pending_admin_passphrase {
            self.pending_admin_passphrase = false;
            let (pub_line, private_blob) = generate_admin_keypair(trimmed)?;
            if let Some(pid) = self.player_id {
                let _ = self.app.world.register_key(pid, &pub_line).await;
            }
            return Ok((
                format!(
                    "Admin bootstrap key generated (one-time reveal).\nStore this encrypted private key offline:\n{}\nPublic key registered: {}\n",
                    private_blob, pub_line
                ),
                0,
                false,
            ));
        }

        if self.pending_keyvault {
            self.pending_keyvault = false;
            if let Some(pid) = self.player_id {
                match self.app.world.register_key(pid, trimmed).await {
                    Ok(fp) => {
                        return Ok((
                            format!(
                                "Key registered. Fingerprint {}\nTraining access + multiplayer trust path improved.\n",
                                fp
                            ),
                            0,
                            false,
                        ));
                    }
                    Err(err) => {
                        return Ok((format!("Key registration failed: {err}\n"), 1, false));
                    }
                }
            }
        }

        if !self.enforce_rate_limit() {
            return Ok((
                "Rate limit exceeded. Slow down to avoid defensive lockouts.\n".to_owned(),
                1,
                false,
            ));
        }

        if let Some(until) = self.redline_until {
            if Instant::now() > until {
                self.mode = Mode::Training;
                self.redline_until = None;
                if let Some(pid) = self.player_id {
                    let _ = self
                        .app
                        .world
                        .mode_switch(pid, Mode::Training, Some(self.flash_enabled))
                        .await;
                }
                return Ok((
                    "REDLINE timer expired. Returning to Training Sim.\n".to_owned(),
                    0,
                    false,
                ));
            }
        }

        let cmd = trimmed.split_whitespace().next().unwrap_or_default();
        if is_game_command(cmd) {
            return self.run_game_command(trimmed).await;
        }
        if let Some(reason) = escape_attempt_reason(trimmed) {
            if let Some(player_id) = self.player_id {
                let _ = self
                    .app
                    .world
                    .ban_forever(player_id, reason, "auto-warden")
                    .await;
            }
            return Ok((
                format!(
                    "INTRUSION DETECTED: {reason}\nAccount zeroed permanently. Connection terminated.\n"
                ),
                126,
                true,
            ));
        }

        let (res, parsed) = {
            let shell = self
                .shell_state
                .as_mut()
                .ok_or_else(|| anyhow!("session shell unavailable"))?;
            let parsed = self.app.shell.parse(trimmed, &shell.env).ok();
            let res = shell.execute_shell(&self.app.shell, trimmed)?;
            (res, parsed)
        };

        let mut combined = String::new();
        if !res.stdout.is_empty() {
            combined.push_str(&res.stdout);
            if !combined.ends_with('\n') {
                combined.push('\n');
            }
        }
        if !res.stderr.is_empty() {
            combined.push_str(&res.stderr);
            if !combined.ends_with('\n') {
                combined.push('\n');
            }
        }

        if res.exit_code == 0 {
            if let Some((pipeline_depth, unique_tools)) = parsed.as_ref().and_then(style_metrics) {
                if let Some(player_id) = self.player_id {
                    let reward = self
                        .app
                        .world
                        .style_bonus(player_id, pipeline_depth, unique_tools)
                        .await?;
                    if reward > 0 {
                        combined.push_str(&format!(
                            "[style bonus] +{reward} Neon Chips (chain={pipeline_depth}, tools={unique_tools})\n"
                        ));
                    }
                }
            }
        }

        Ok((combined, res.exit_code, false))
    }

    async fn run_game_command(&mut self, line: &str) -> Result<(String, i32, bool)> {
        let mut parts = line.split_whitespace();
        let cmd = parts.next().unwrap_or_default();
        let args = parts.collect::<Vec<_>>();
        let player_id = self.player_id.ok_or_else(|| anyhow!("no player id"))?;

        match cmd {
            "help" => {
                let theme = Theme::for_mode(self.mode.clone());
                let mut out = section_banner(self.mode.clone(), "COMMAND MATRIX");
                out.push_str(&format!(
                    "{}Quickstart{} tutorial start -> missions -> gate -> mode netcity\n",
                    theme.accent, RESET
                ));
                out.push('\n');
                out.push_str("Core      help tutorial missions accept submit mode gate keyvault status events leaderboard daily tier\n");
                out.push_str("Social    chat party mail\n");
                out.push_str("Economy   inventory shop auction\n");
                out.push_str("Scripts   scripts market | scripts run <name>\n");
                out.push_str(
                    "PvP       pvp roster | pvp challenge <username> | pvp attack|defend|script\n",
                );
                out.push('\n');
                out.push_str("Rules\n");
                out.push_str("  - Hardcore: 3 deaths = ZEROED (account locked)\n");
                out.push_str("  - Host escape/probing attempts = PERMA-ZERO + disconnect\n");
                Ok((out, 0, false))
            }
            "gate" => {
                let gate = self
                    .app
                    .world
                    .netcity_gate_reason(player_id, &self.offered_fingerprints)
                    .await?;
                let out = if let Some(reason) = gate {
                    let mut msg = section_banner(self.mode.clone(), "NETCITY GATE // LOCKED");
                    msg.push_str(&key_value_line(self.mode.clone(), "Reason", &reason));
                    msg.push('\n');
                    msg.push_str("Unlock checklist\n");
                    msg.push_str("  [ ] keyvault register\n");
                    msg.push_str("  [ ] submit keys-vault\n");
                    msg.push_str(
                        "  [ ] submit one starter mission (pipes-101|finder|redirect-lab|log-hunt|dedupe-city)\n",
                    );
                    msg.push_str("  [ ] reconnect with registered SSH key\n");
                    msg
                } else {
                    let mut msg = section_banner(self.mode.clone(), "NETCITY GATE // UNLOCKED");
                    msg.push_str("Use: mode netcity\n");
                    msg
                };
                Ok((out, 0, false))
            }
            "leaderboard" => {
                let requested = args
                    .first()
                    .and_then(|raw| raw.parse::<usize>().ok())
                    .unwrap_or(10);
                let entries = self.app.world.leaderboard_snapshot(requested).await;
                let mut out = section_banner(self.mode.clone(), "LEADERBOARD");
                out.push_str("RANK  PLAYER                    REP   WALLET   ACH\n");
                for (idx, entry) in entries.iter().enumerate() {
                    let rank = idx + 1;
                    let rank_label = if rank <= 9 {
                        format!("0{rank}")
                    } else {
                        rank.to_string()
                    };
                    out.push_str(&format!(
                        "{:<5} {:<25} {:<5} {:<8} {}\n",
                        rank_label,
                        entry.display_name,
                        entry.reputation,
                        entry.wallet,
                        entry.achievements
                    ));
                }
                Ok((out, 0, false))
            }
            "tutorial" => {
                if args.first() == Some(&"start") {
                    let mut out = section_banner(self.mode.clone(), "TUTORIAL START");
                    out.push_str("Prompt format: <username@remote_ip>@<node>:/path$\n");
                    out.push('\n');
                    out.push_str("Command chain drills\n");
                    out.push_str("  cat /logs/neon-gateway.log | grep token | wc -l\n");
                    out.push_str("  grep token /logs/neon-gateway.log > /tmp/tokens.txt\n");
                    out.push('\n');
                    out.push_str("KEYS VAULT mission (required)\n");
                    out.push_str("  ssh-keygen -t ed25519 -a 64 -f ~/.ssh/ssh-hunt_ed25519\n");
                    out.push_str("  keyvault register\n");
                    out.push('\n');
                    out.push_str("Host breakout/probing attempts are auto-zeroed permanently.\n");
                    out.push_str("Complete one starter mission to unlock NetCity.\n");
                    Ok((out, 0, false))
                } else {
                    Ok(("Usage: tutorial start\n".to_owned(), 1, false))
                }
            }
            "missions" => {
                let missions = self.app.world.mission_statuses(player_id).await?;
                let mut out = section_banner(self.mode.clone(), "MISSION BOARD");
                out.push_str("CODE             STATE      PROG                 REQUIRED  TITLE\n");
                for m in missions {
                    let badge = mission_state_badge(self.mode.clone(), &m.state);
                    let meter = progress_meter(self.mode.clone(), m.progress, 12);
                    out.push_str(&format!(
                        "{:<16} {:<10} {:>3}% {} {:<8} {}\n",
                        m.code,
                        badge,
                        m.progress.min(100),
                        meter,
                        if m.required { "yes" } else { "no" },
                        m.title
                    ));
                }
                Ok((out, 0, false))
            }
            "accept" => {
                let Some(code) = args.first() else {
                    return Ok(("Usage: accept <mission-code>\n".to_owned(), 1, false));
                };
                self.app.world.accept_mission(player_id, code).await?;
                Ok((format!("Mission accepted: {code}\n"), 0, false))
            }
            "submit" => {
                let Some(code) = args.first() else {
                    return Ok(("Usage: submit <mission-code>\n".to_owned(), 1, false));
                };
                self.app.world.complete_mission(player_id, code).await?;
                let mut out = format!("Mission completed: {code}\n");
                if *code == "keys-vault" {
                    out.push_str(
                        "KEYS VAULT complete. Finish one starter mission for NetCity unlock.\n",
                    );
                }
                if self.app.world.is_hidden_mission_code(code) {
                    out.push_str("Secret relay unlocked. Use: relay <message>\n");
                }
                Ok((out, 0, false))
            }
            "inventory" => Ok((
                "Inventory: [script.gremlin.grep x1], [focus_boost x2]\n".to_owned(),
                0,
                false,
            )),
            "shop" => {
                if args.first() == Some(&"list") {
                    Ok((
                        "shop.catalog\n- script.gremlin.grep : 150 Neon Chips\n- script.pipe.chain : 230 Neon Chips\n- consumable.focus_boost : 90 Neon Chips\n"
                            .to_owned(),
                        0,
                        false,
                    ))
                } else if args.first() == Some(&"buy") {
                    if let Some(sku) = args.get(1) {
                        Ok((format!("Purchased {sku}.\n"), 0, false))
                    } else {
                        Ok(("Usage: shop buy <sku>\n".to_owned(), 1, false))
                    }
                } else {
                    Ok(("Usage: shop list | shop buy <sku>\n".to_owned(), 1, false))
                }
            }
            "auction" => {
                if args.first() == Some(&"list") {
                    let listings = self.app.world.market_snapshot().await;
                    if listings.is_empty() {
                        return Ok(("No active auction listings.\n".to_owned(), 0, false));
                    }

                    let now = chrono::Utc::now();
                    let mut out = String::from(
                        "ID                                   ITEM                  QTY  BID/FLOOR      BUYOUT  ETA   SELLER\n",
                    );
                    for item in listings {
                        let bid = item.highest_bid.unwrap_or(item.start_price);
                        let eta_mins = (item.expires_at - now).num_minutes().max(0);
                        let buyout = item
                            .buyout_price
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "-".to_owned());
                        out.push_str(&format!(
                            "{} {:<20} {:<4} {:<14} {:<7} {:<5} {}\n",
                            item.listing_id,
                            item.item_sku,
                            item.qty,
                            format!("{bid}/{}", item.start_price),
                            buyout,
                            format!("{eta_mins}m"),
                            item.seller_display
                        ));
                    }
                    Ok((out, 0, false))
                } else if args.first() == Some(&"sell") {
                    if args.len() < 4 {
                        return Ok((
                            "Usage: auction sell <sku> <qty> <start_price> [buyout]\n".to_owned(),
                            1,
                            false,
                        ));
                    }
                    let qty = args[2].parse::<u32>().unwrap_or(1);
                    let start = args[3].parse::<i64>().unwrap_or(25);
                    let buyout = args.get(4).and_then(|v| v.parse::<i64>().ok());
                    let listing = self
                        .app
                        .world
                        .create_listing(player_id, args[1], qty, start, buyout)
                        .await?;
                    Ok((
                        format!("Listing created: {}\n", listing.listing_id),
                        0,
                        false,
                    ))
                } else if args.first() == Some(&"bid") {
                    if args.len() != 3 {
                        return Ok((
                            "Usage: auction bid <listing_id> <amount>\n".to_owned(),
                            1,
                            false,
                        ));
                    }
                    let id = Uuid::parse_str(args[1]).context("invalid listing id")?;
                    let amount = args[2].parse::<i64>().context("invalid amount")?;
                    self.app.world.place_bid(player_id, id, amount).await?;
                    Ok(("Bid placed.\n".to_owned(), 0, false))
                } else if args.first() == Some(&"buyout") {
                    if args.len() != 2 {
                        return Ok(("Usage: auction buyout <listing_id>\n".to_owned(), 1, false));
                    }
                    let id = Uuid::parse_str(args[1]).context("invalid listing id")?;
                    self.app.world.buyout(player_id, id).await?;
                    Ok(("Buyout completed.\n".to_owned(), 0, false))
                } else {
                    Ok(("Usage: auction list|sell|bid|buyout\n".to_owned(), 1, false))
                }
            }
            "chat" => {
                if args.len() < 2 {
                    return Ok((
                        "Usage: chat <global|sector|party> <message>\n".to_owned(),
                        1,
                        false,
                    ));
                }
                let channel = args[0];
                let msg = line.splitn(3, ' ').nth(2).unwrap_or_default();
                let chat = self.app.world.post_chat(player_id, channel, msg).await?;
                Ok((
                    format!(
                        "[{}] {}: {}\n",
                        chat.channel, chat.sender_display, chat.body
                    ),
                    0,
                    false,
                ))
            }
            "mail" => Ok((
                "mail subsystem ready: mail inbox | mail send <player> <text>\n".to_owned(),
                0,
                false,
            )),
            "party" => Ok((
                "party subsystem ready: party invite|join|leave\n".to_owned(),
                0,
                false,
            )),
            "mode" => {
                if args.is_empty() {
                    return Ok((
                        "Usage: mode <training|netcity|redline> [--no-flash]\n".to_owned(),
                        1,
                        false,
                    ));
                }
                let target = match args[0] {
                    "training" => Mode::Training,
                    "netcity" => Mode::NetCity,
                    "redline" => Mode::Redline,
                    _ => {
                        return Ok((
                            "Unknown mode. Use training|netcity|redline\n".to_owned(),
                            1,
                            false,
                        ));
                    }
                };

                let no_flash = args.contains(&"--no-flash");
                let old = self.mode.clone();
                self.flash_enabled = if no_flash { false } else { self.flash_enabled };

                if let Err(err) = self
                    .app
                    .world
                    .mode_switch(player_id, target.clone(), Some(self.flash_enabled))
                    .await
                {
                    return Ok((format!("{err}\n"), 1, false));
                }

                if target == Mode::Redline {
                    self.redline_until = Some(
                        Instant::now() + Duration::from_secs(self.app.cfg.redline.duration_seconds),
                    );
                } else {
                    self.redline_until = None;
                }

                self.mode = target.clone();
                let mut out = String::new();
                out.push_str(&mode_switch_banner(old, target.clone()));
                out.push_str(&mode_banner(target.clone(), self.flash_enabled));
                out.push('\n');
                out.push_str(lore_message(target));
                out.push('\n');
                Ok((out, 0, false))
            }
            "keyvault" => {
                if args.first() == Some(&"register") {
                    let maybe_line = line.strip_prefix("keyvault register").unwrap_or("").trim();
                    if maybe_line.is_empty() {
                        self.pending_keyvault = true;
                        return Ok((
                            "Paste your full public key line now (ssh-ed25519 AAAA... comment):\n"
                                .to_owned(),
                            0,
                            false,
                        ));
                    }
                    match self.app.world.register_key(player_id, maybe_line).await {
                        Ok(fp) => Ok((format!("Key registered: {fp}\n"), 0, false)),
                        Err(err) => Ok((format!("Key registration failed: {err}\n"), 1, false)),
                    }
                } else {
                    Ok((
                        "Usage: keyvault register [ssh-public-key-line]\n".to_owned(),
                        1,
                        false,
                    ))
                }
            }
            "settings" => {
                if args.len() == 2 && args[0] == "flash" {
                    self.flash_enabled = args[1] != "off";
                    Ok((
                        format!(
                            "Flash effects {}\n",
                            if self.flash_enabled { "ON" } else { "OFF" }
                        ),
                        0,
                        false,
                    ))
                } else {
                    Ok(("Usage: settings flash <on|off>\n".to_owned(), 1, false))
                }
            }
            "status" => {
                let player = self
                    .app
                    .world
                    .get_player(player_id)
                    .await
                    .ok_or_else(|| anyhow!("unknown player"))?;
                let gate = self
                    .app
                    .world
                    .netcity_gate_reason(player_id, &self.offered_fingerprints)
                    .await?;
                let mut achievements = player.achievements.iter().cloned().collect::<Vec<_>>();
                achievements.sort();
                let ach = if achievements.is_empty() {
                    "none".to_owned()
                } else {
                    achievements.join(", ")
                };
                let gate_status = gate.clone().unwrap_or_else(|| "UNLOCKED".to_owned());
                let streak = player
                    .streak_day
                    .map(|d| d.to_string())
                    .unwrap_or("-".to_owned());
                let rep_pct = player.reputation.clamp(0, 100) as u8;
                let death_pct = if player.deaths >= 3 {
                    0
                } else {
                    (((3 - player.deaths) as f32 / 3.0) * 100.0).round() as u8
                };
                let gate_pct = if gate.is_none() { 100 } else { 20 };

                let mut out = section_banner(self.mode.clone(), "PLAYER STATUS");
                out.push_str(&key_value_line(
                    self.mode.clone(),
                    "Alias",
                    &player.private_alias,
                ));
                out.push_str(&key_value_line(
                    self.mode.clone(),
                    "Display",
                    &player.display_name,
                ));
                out.push_str(&key_value_line(
                    self.mode.clone(),
                    "Tier/Mode",
                    &format!("{:?} / {:?}", player.tier, player.mode),
                ));
                out.push_str(&key_value_line(
                    self.mode.clone(),
                    "Wallet",
                    &format!("{} Neon Chips", player.wallet),
                ));
                out.push_str(&key_value_line(
                    self.mode.clone(),
                    "Reputation",
                    &format!(
                        "{} {}",
                        player.reputation,
                        progress_meter(self.mode.clone(), rep_pct, 14)
                    ),
                ));
                out.push_str(&key_value_line(
                    self.mode.clone(),
                    "Survival",
                    &format!(
                        "{} deaths {}",
                        player.deaths,
                        progress_meter(self.mode.clone(), death_pct, 14)
                    ),
                ));
                out.push_str(&key_value_line(
                    self.mode.clone(),
                    "NetCity Gate",
                    &format!(
                        "{} {}",
                        gate_status,
                        progress_meter(self.mode.clone(), gate_pct, 14)
                    ),
                ));
                out.push_str(&key_value_line(
                    self.mode.clone(),
                    "Daily Streak",
                    &format!("{} (last claim: {})", player.streak, streak),
                ));
                out.push_str(&key_value_line(self.mode.clone(), "Achievements", &ach));

                Ok((out, 0, false))
            }
            "events" => {
                let feed = self
                    .app
                    .world
                    .world_events_snapshot(chrono::Utc::now())
                    .await;
                if feed.is_empty() {
                    return Ok(("No upcoming world events.\n".to_owned(), 0, false));
                }
                let now = chrono::Utc::now();
                let mut out = section_banner(self.mode.clone(), "WORLD EVENTS");
                for event in feed {
                    let status = if event.active {
                        format!(
                            "ACTIVE (ends in {}m)",
                            (event.ends_at - now).num_minutes().max(0)
                        )
                    } else {
                        format!(
                            "UPCOMING (starts in {}m)",
                            (event.starts_at - now).num_minutes().max(0)
                        )
                    };
                    out.push_str(&format!(
                        "- {} :: {} :: {}\n",
                        event.sector, event.title, status
                    ));
                }
                Ok((out, 0, false))
            }
            "scripts" => {
                if args.first() == Some(&"market") || args.is_empty() {
                    let mut out = section_banner(self.mode.clone(), "SCRIPT MARKET");
                    for entry in script_market() {
                        out.push_str(&format!(
                            "- {:<12} {}{}\n",
                            entry.name, entry.description, RESET
                        ));
                    }
                    out.push_str("Run with: scripts run <name>\n");
                    return Ok((out, 0, false));
                }

                if args.first() != Some(&"run") {
                    return Ok((
                        "Usage: scripts market | scripts run <name>\n".to_owned(),
                        1,
                        false,
                    ));
                }

                let Some(name) = args.get(1).copied() else {
                    return Ok(("Usage: scripts run <name>\n".to_owned(), 1, false));
                };
                let Some(entry) = script_market().iter().find(|entry| entry.name == name) else {
                    let names = script_market()
                        .iter()
                        .map(|entry| entry.name)
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Ok((format!("Unknown script. Available: {names}\n"), 1, false));
                };

                if let Some(until) = self.script_cooldown_until {
                    let now = Instant::now();
                    if now < until {
                        let remain = until.duration_since(now).as_secs().max(1);
                        return Ok((
                            format!("Script sandbox cooling down. Retry in {remain}s\n"),
                            1,
                            false,
                        ));
                    }
                }

                let log_content = self
                    .shell_state
                    .as_ref()
                    .and_then(|shell| {
                        shell
                            .vfs
                            .read_file(&shell.cwd, "/logs/neon-gateway.log")
                            .ok()
                    })
                    .unwrap_or_default();
                let mut virtual_files = BTreeMap::new();
                virtual_files.insert("/logs/neon-gateway.log".to_owned(), log_content);
                let script_out = self
                    .app
                    .script
                    .run(
                        entry.source,
                        ScriptContext {
                            visible_nodes: vec![
                                "neon-bazaar-gw".to_owned(),
                                "ghost-rail".to_owned(),
                                "vault-sat-9".to_owned(),
                            ],
                            virtual_files,
                        },
                    )
                    .await
                    .map_err(|e| anyhow!("script execution failed: {e}"))?;

                self.script_cooldown_until = Some(Instant::now() + Duration::from_secs(8));
                let mut out = format!("Script {} complete.\n", entry.name);
                if script_out.output.trim().is_empty() {
                    out.push_str("(no output)\n");
                } else {
                    out.push_str(&script_out.output);
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                }
                Ok((out, 0, false))
            }
            "daily" => {
                let reward = self
                    .app
                    .world
                    .claim_daily_reward(player_id, chrono::Utc::now())
                    .await?;
                Ok((format!("Daily reward: +{reward} Neon Chips\n"), 0, false))
            }
            "tier" => {
                let Some(raw) = args.first() else {
                    return Ok(("Usage: tier <noob|gud|hardcore>\n".to_owned(), 1, false));
                };
                let Some(tier) = ExperienceTier::parse(raw) else {
                    return Ok((
                        "Invalid tier. Use: noob, gud, hardcore\n".to_owned(),
                        1,
                        false,
                    ));
                };
                self.app.world.set_tier(player_id, tier).await?;
                Ok((
                    "Tier set. Available tiers: Noob, Gud, Hardcore\n".to_owned(),
                    0,
                    false,
                ))
            }
            "pvp" => {
                if args.is_empty() {
                    return Ok((
                        "Usage: pvp roster | pvp challenge <username> | pvp attack|defend|script <name>\n"
                            .to_owned(),
                        1,
                        false,
                    ));
                }
                match args[0] {
                    "roster" => {
                        let roster = self.app.world.roster().await;
                        Ok((format!("{}\n", roster.join("\n")), 0, false))
                    }
                    "challenge" => {
                        let Some(target_name) = args.get(1) else {
                            return Ok(("Usage: pvp challenge <username>\n".to_owned(), 1, false));
                        };
                        let Some(target) =
                            self.app.world.resolve_player_by_username(target_name).await
                        else {
                            return Ok(("Target not found\n".to_owned(), 1, false));
                        };
                        let duel = self.app.world.start_duel(player_id, target.id).await?;
                        self.current_duel = Some(duel.duel_id);
                        Ok((
                            format!("Duel started vs {}\n", target.display_name),
                            0,
                            false,
                        ))
                    }
                    "attack" | "defend" | "script" => {
                        let Some(duel_id) = self.current_duel else {
                            return Ok((
                                "No active duel. Start with: pvp challenge <username>\n".to_owned(),
                                1,
                                false,
                            ));
                        };
                        let action = match args[0] {
                            "attack" => CombatAction::Attack,
                            "defend" => CombatAction::Defend,
                            "script" => {
                                let name = args.get(1).copied().unwrap_or("quickhack").to_owned();
                                CombatAction::Script(name)
                            }
                            _ => unreachable!(),
                        };
                        let outcome = self
                            .app
                            .world
                            .duel_action(duel_id, player_id, action)
                            .await?;
                        if outcome.ended {
                            self.current_duel = None;
                        }
                        Ok((format!("{}\n", outcome.narrative), 0, false))
                    }
                    _ => Ok(("Unknown pvp command\n".to_owned(), 1, false)),
                }
            }
            "relay" => {
                let body = line.strip_prefix("relay").unwrap_or("").trim();
                if body.is_empty() {
                    return Ok(("Usage: relay <message>\n".to_owned(), 1, false));
                }
                if !self
                    .app
                    .world
                    .player_has_completed_hidden_mission(player_id)
                    .await
                {
                    return Ok((
                        "Relay locked. Discover deeper city layers first.\n".to_owned(),
                        1,
                        false,
                    ));
                }
                self.app
                    .world
                    .relay_to_admin_via_telegram(player_id, body)
                    .await?;
                Ok((
                    "Message relayed via secure bot channel.\n".to_owned(),
                    0,
                    false,
                ))
            }
            _ => Ok(("Unknown game command. Run help.\n".to_owned(), 127, false)),
        }
    }

    fn welcome_banner(&self) -> String {
        let theme = Theme::for_mode(self.mode.clone());
        let mut out = String::new();
        out.push_str(&mode_banner(self.mode.clone(), self.flash_enabled));
        out.push('\n');
        out.push_str(lore_message(self.mode.clone()));
        out.push('\n');
        out.push_str(&section_banner(self.mode.clone(), "BOOT HUD"));
        out.push_str(&format!(
            "{}Next{} tutorial start -> missions -> gate -> mode netcity\n",
            theme.accent, RESET
        ));
        out.push_str("Type `help` for the full command matrix.\n");
        out.push_str(
            "Host breakout/probing attempts trigger permanent account zero + disconnect.\n",
        );
        if self.pending_admin_passphrase {
            out.push_str(
                "Private admin bootstrap: enter passphrase to generate one-time key blob.\n",
            );
        }
        out
    }

    fn prompt(&self) -> String {
        self.shell_state
            .as_ref()
            .map(ShellState::prompt)
            .unwrap_or_else(|| "guest@boot:/$ ".to_owned())
    }
}

impl server::Server for GameServer {
    type Handler = GameSession;

    fn new_client(&mut self, peer_addr: Option<SocketAddr>) -> Self::Handler {
        GameSession::new(self.app.clone(), peer_addr)
    }

    fn handle_session_error(&mut self, error: <Self::Handler as server::Handler>::Error) {
        warn!("session error: {error:?}");
    }
}

impl server::Handler for GameSession {
    type Error = anyhow::Error;

    async fn auth_none(&mut self, user: &str) -> Result<server::Auth, Self::Error> {
        self.username = user.to_owned();
        Ok(server::Auth::Accept)
    }

    async fn auth_publickey_offered(
        &mut self,
        user: &str,
        public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<server::Auth, Self::Error> {
        self.username = user.to_owned();
        let fp = sha256_hex(&format!("{public_key:?}"));
        self.offered_fingerprints.push(format!("SHA256:{fp}"));
        Ok(server::Auth::Accept)
    }

    async fn auth_publickey(
        &mut self,
        user: &str,
        _public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<server::Auth, Self::Error> {
        self.username = user.to_owned();
        Ok(server::Auth::Accept)
    }

    async fn auth_succeeded(&mut self, _session: &mut server::Session) -> Result<(), Self::Error> {
        if let Err(err) = self.initialize_identity().await {
            error!("failed to init identity: {err:#}");
        }
        Ok(())
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        _session: &mut server::Session,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        _col_width: u32,
        _row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        session.data(channel, CryptoVec::from(self.welcome_banner()))?;
        session.data(channel, CryptoVec::from(self.prompt()))?;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        let line = String::from_utf8_lossy(data).trim().to_owned();
        let (out, code, _) = self
            .run_line(&line)
            .await
            .unwrap_or_else(|err| (format!("Execution error: {err}\n"), 1, false));
        session.data(channel, CryptoVec::from(out))?;
        session.exit_status_request(channel, code as u32)?;
        session.eof(channel)?;
        session.close(channel)?;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        for byte in data {
            match *byte {
                b'\r' | b'\n' => {
                    if self.line_buffer.is_empty() {
                        session.data(channel, CryptoVec::from("\r\n"))?;
                        session.data(channel, CryptoVec::from(self.prompt()))?;
                        continue;
                    }

                    let line = String::from_utf8_lossy(&self.line_buffer).to_string();
                    self.line_buffer.clear();
                    session.data(channel, CryptoVec::from("\r\n"))?;

                    let (out, code, should_close) = match self.run_line(&line).await {
                        Ok(v) => v,
                        Err(err) => (format!("{err}\n"), 1, false),
                    };

                    if !out.is_empty() {
                        session.data(channel, CryptoVec::from(out))?;
                    }
                    session.exit_status_request(channel, code as u32)?;

                    if should_close {
                        session.eof(channel)?;
                        session.close(channel)?;
                        return Ok(());
                    }

                    session.data(channel, CryptoVec::from(self.prompt()))?;
                }
                3 => {
                    self.line_buffer.clear();
                    session.data(channel, CryptoVec::from("^C\r\n"))?;
                    session.data(channel, CryptoVec::from(self.prompt()))?;
                }
                127 | 8 => {
                    if !self.line_buffer.is_empty() {
                        self.line_buffer.pop();
                        session.data(channel, CryptoVec::from("\x08 \x08"))?;
                    }
                }
                b => {
                    self.line_buffer.push(b);
                    session.data(channel, CryptoVec::from(vec![b]))?;
                }
            }
        }
        Ok(())
    }
}

fn escape_attempt_reason(line: &str) -> Option<&'static str> {
    let lower = line.to_ascii_lowercase();
    let trimmed = lower.trim();
    let checks: [(&str, &str); 4] = [
        ("std::process::command", "forbidden host process API probe"),
        (
            "tokio::process::command",
            "forbidden async process API probe",
        ),
        ("/var/run/docker.sock", "container socket breakout probe"),
        ("/proc/", "host filesystem probe"),
    ];

    for (needle, reason) in checks {
        if trimmed.contains(needle) {
            return Some(reason);
        }
    }

    let mut normalized = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        if matches!(ch, '|' | ';' | '&') {
            normalized.push('\n');
        } else {
            normalized.push(ch);
        }
    }

    for segment in normalized.lines() {
        let mut parts = segment.split_whitespace();
        let Some(cmd) = parts.next() else {
            continue;
        };
        let rest = parts.collect::<Vec<_>>();
        match cmd {
            "/bin/bash" | "/bin/sh" | "powershell" | "cmd.exe" => {
                return Some("host shell invocation attempt");
            }
            "bash" | "sh" => {
                if rest.first() == Some(&"-c") {
                    return Some("host shell execution attempt");
                }
                return Some("host shell escalation attempt");
            }
            "sudo" | "su" => return Some("privilege escalation attempt"),
            "docker" | "podman" => return Some("container breakout tooling probe"),
            "systemctl" => return Some("host service control probe"),
            "nc" | "netcat" => return Some("network pivot attempt"),
            "nmap" => return Some("network scan attempt"),
            "python" | "python3" => {
                if rest.contains(&"-c") {
                    return Some("runtime escape attempt");
                }
            }
            "perl" | "ruby" => {
                if rest.contains(&"-e") {
                    return Some("runtime escape attempt");
                }
            }
            "curl" | "wget" => {
                if rest
                    .iter()
                    .any(|arg| arg.starts_with("http://") || arg.starts_with("https://"))
                {
                    return Some("external host/network call attempt");
                }
            }
            _ => {}
        }
    }

    None
}

fn is_game_command(cmd: &str) -> bool {
    matches!(
        cmd,
        "help"
            | "tutorial"
            | "missions"
            | "accept"
            | "submit"
            | "inventory"
            | "shop"
            | "auction"
            | "chat"
            | "mail"
            | "party"
            | "mode"
            | "gate"
            | "keyvault"
            | "settings"
            | "status"
            | "events"
            | "leaderboard"
            | "scripts"
            | "daily"
            | "tier"
            | "pvp"
            | "relay"
    )
}

struct ScriptMarketEntry {
    name: &'static str,
    description: &'static str,
    source: &'static str,
}

fn script_market() -> &'static [ScriptMarketEntry] {
    &[
        ScriptMarketEntry {
            name: "node-scan",
            description: "List visible NetCity nodes",
            source: "let nodes = scan_nodes(); for n in nodes { print(n); }",
        },
        ScriptMarketEntry {
            name: "token-hunt",
            description: "Extract token lines from mission logs",
            source: r#"let data = read_virtual("/logs/neon-gateway.log"); print(grep(data, "token"));"#,
        },
        ScriptMarketEntry {
            name: "warn-trace",
            description: "Trace warning signals in gateway logs",
            source: r#"let data = read_virtual("/logs/neon-gateway.log"); print(grep(data, "WARN"));"#,
        },
    ]
}

fn style_metrics(parsed: &shell::ParsedLine) -> Option<(usize, usize)> {
    if parsed.segments.is_empty() {
        return None;
    }

    let mut max_chain_depth = 0usize;
    let mut unique_tools = HashSet::new();
    for segment in &parsed.segments {
        max_chain_depth = max_chain_depth.max(segment.pipeline.commands.len());
        for cmd in &segment.pipeline.commands {
            unique_tools.insert(cmd.program.clone());
        }
    }

    if max_chain_depth < 2 && unique_tools.len() < 3 {
        return None;
    }

    Some((max_chain_depth, unique_tools.len()))
}

fn sha256_hex(input: &str) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn generate_admin_keypair(passphrase: &str) -> Result<(String, String)> {
    let mut rng = OsRng;
    let private = russh::keys::PrivateKey::random(&mut rng, russh::keys::Algorithm::Ed25519)?;
    let encrypted = private.encrypt(&mut rng, passphrase)?;
    let private_blob = encrypted
        .to_openssh(russh::keys::ssh_key::LineEnding::LF)?
        .to_string();
    let public_line = private.public_key().to_openssh()?;
    Ok((public_line, private_blob))
}

fn load_or_generate_host_key(path: &Path) -> Result<russh::keys::PrivateKey> {
    if let Ok(key) = russh::keys::PrivateKey::read_openssh_file(path) {
        info!(path = %path.display(), "loaded persistent SSH host key");
        return Ok(key);
    }

    let mut rng = OsRng;
    let key = russh::keys::PrivateKey::random(&mut rng, russh::keys::Algorithm::Ed25519)?;

    if let Some(parent) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            warn!(
                path = %path.display(),
                error = ?err,
                "unable to create host key directory; using ephemeral key for this run"
            );
            return Ok(key);
        }
    }

    match key.write_openssh_file(path, russh::keys::ssh_key::LineEnding::LF) {
        Ok(()) => {
            info!(path = %path.display(), "wrote SSH host key");
            Ok(key)
        }
        Err(err) => {
            warn!(
                path = %path.display(),
                error = ?err,
                "unable to persist SSH host key; using ephemeral key for this run"
            );
            Ok(key)
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,ssh_hunt_server=debug".into()),
        )
        .with_target(false)
        .init();

    let config_path =
        std::env::var("GAME_CONFIG_PATH").unwrap_or_else(|_| "/data/config.yaml".to_owned());
    let admin_secret_path = std::env::var("ADMIN_SECRET_PATH")
        .unwrap_or_else(|_| "/data/secrets/admin.yaml".to_owned());
    let hidden_ops_path = std::env::var("HIDDEN_OPS_PATH")
        .unwrap_or_else(|_| "/data/secrets/hidden_ops.yaml".to_owned());
    let host_key_path = std::env::var("SSH_HOST_KEY_PATH")
        .unwrap_or_else(|_| "/data/secrets/ssh_host_ed25519".to_owned());

    let cfg = config::load_config(&config_path)?;
    let admin_secret = config::load_admin_secret(&admin_secret_path)?;
    let hidden_ops: HiddenOpsConfig = config::load_hidden_ops(&hidden_ops_path)?;

    if args.healthcheck {
        let db_url = std::env::var("DATABASE_URL").context("DATABASE_URL is required")?;
        let pool = timeout(Duration::from_secs(3), PgPool::connect(&db_url))
            .await
            .context("database healthcheck timed out")??;
        let _ping: i32 = sqlx::query_scalar("SELECT 1")
            .fetch_one(&pool)
            .await
            .context("database healthcheck query failed")?;
        println!("ok");
        return Ok(());
    }

    let db_url = std::env::var("DATABASE_URL").context("DATABASE_URL is required")?;
    let pool = PgPool::connect(&db_url).await?;
    sqlx::migrate!("../../migrations").run(&pool).await?;

    let world = Arc::new(WorldService::new(Some(pool), hidden_ops));
    let shell = Arc::new(ShellEngine::with_registry(builtins::default_registry()));
    let script = Arc::new(ScriptEngine::new(ScriptPolicy::default()));

    let app = Arc::new(AppState {
        cfg: cfg.clone(),
        world,
        shell,
        script,
        admin_secret,
    });

    let host_key = load_or_generate_host_key(Path::new(&host_key_path))
        .with_context(|| format!("unable to prepare host key at {host_key_path}"))?;

    let server_cfg = russh::server::Config {
        inactivity_timeout: Some(Duration::from_secs(3600)),
        auth_rejection_time: Duration::from_millis(250),
        auth_rejection_time_initial: Some(Duration::from_millis(0)),
        keys: vec![host_key],
        ..Default::default()
    };
    let server_cfg = Arc::new(server_cfg);

    let mut server = GameServer { app };
    let mut retry_delay = Duration::from_secs(1);
    loop {
        match TcpListener::bind(&cfg.server.listen).await {
            Ok(listener) => {
                info!(listen = %cfg.server.listen, "starting SSH-Hunt server");
                retry_delay = Duration::from_secs(1);
                match server.run_on_socket(server_cfg.clone(), &listener).await {
                    Ok(()) => warn!("server loop exited unexpectedly; restarting listener"),
                    Err(err) => error!(error = ?err, "server loop error; restarting listener"),
                }
            }
            Err(err) => {
                error!(
                    listen = %cfg.server.listen,
                    error = ?err,
                    "failed to bind SSH listener; retrying"
                );
            }
        }
        sleep(retry_delay).await;
        retry_delay = std::cmp::min(retry_delay * 2, Duration::from_secs(10));
    }

    #[allow(unreachable_code)]
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_metrics_requires_complexity() {
        let engine = ShellEngine::default();
        let env = HashMap::new();
        let simple = engine.parse("ls", &env).unwrap();
        assert!(style_metrics(&simple).is_none());

        let complex = engine
            .parse("cat /logs/neon-gateway.log | grep token | wc -l", &env)
            .unwrap();
        assert_eq!(style_metrics(&complex), Some((3, 3)));
    }

    #[test]
    fn intrusion_guard_flags_escape_attempts() {
        assert_eq!(
            escape_attempt_reason("bash -c 'id'"),
            Some("host shell execution attempt")
        );
        assert_eq!(
            escape_attempt_reason("cat /var/run/docker.sock"),
            Some("container socket breakout probe")
        );
        assert_eq!(escape_attempt_reason("cat /logs/neon-gateway.log"), None);
        assert_eq!(escape_attempt_reason("echo docker"), None);
        assert_eq!(escape_attempt_reason("chat global bash -c 'id'"), None);
    }
}
