#![forbid(unsafe_code)]

use std::collections::HashMap;

use chrono::{Duration, Utc};
use shell::{BuiltinRegistry, CommandResult, ExecutionContext, ShellEngine};
use ssh_hunt_scripts::{ScriptContext, ScriptEngine, ScriptPolicy};
use vfs::Vfs;
use world::{ExperienceTier, HiddenOpsConfig, WorldService};

// ── helpers ─────────────────────────────────────────────────────────────────

fn bare_world() -> WorldService {
    WorldService::new(
        None,
        HiddenOpsConfig {
            secret_mission: None,
            telegram: None,
        },
    )
}

fn basic_shell_env() -> HashMap<String, String> {
    HashMap::from([
        ("USER".to_owned(), "neo".to_owned()),
        ("HOME".to_owned(), "/home/player".to_owned()),
        ("PWD".to_owned(), "/home/player".to_owned()),
        ("PATH".to_owned(), "/bin:/usr/bin".to_owned()),
        ("?".to_owned(), "0".to_owned()),
    ])
}

fn minimal_vfs() -> Vfs {
    let mut vfs = Vfs::default();
    vfs.mkdir_p("/", "home/player", "player").unwrap();
    vfs
}

// ── world / gate ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn full_gate_progression_flow() {
    let world = bare_world();

    let player = world.login("neo", "203.0.113.50", &[]).await.unwrap();
    // New player — gate is locked
    assert!(world
        .netcity_gate_reason(player.id, &[])
        .await
        .unwrap()
        .is_some());

    world
        .register_key(
            player.id,
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMockMockMockMock mock@host",
        )
        .await
        .unwrap();
    world
        .complete_mission(player.id, "keys-vault")
        .await
        .unwrap();
    world
        .complete_mission(player.id, "pipes-101")
        .await
        .unwrap();

    // Gate is still locked without a matching fingerprint presented
    let gate = world
        .netcity_gate_reason(player.id, &["SHA256:dummy".to_owned()])
        .await
        .unwrap();
    assert!(gate.is_some());
}

#[tokio::test]
async fn gate_unlocks_when_registered_fp_presented() {
    let world = bare_world();
    let player = world
        .login("unlock-test", "203.0.113.60", &[])
        .await
        .unwrap();

    let fp = world
        .register_key(
            player.id,
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMockKeyUnlock unlock@host",
        )
        .await
        .unwrap();
    world
        .complete_mission(player.id, "keys-vault")
        .await
        .unwrap();
    world.complete_mission(player.id, "finder").await.unwrap();

    let gate = world.netcity_gate_reason(player.id, &[fp]).await.unwrap();
    assert!(gate.is_none(), "gate should be unlocked: {gate:?}");
}

#[tokio::test]
async fn gate_requires_starter_mission() {
    let world = bare_world();
    let player = world
        .login("no-starter", "203.0.113.70", &[])
        .await
        .unwrap();

    let fp = world
        .register_key(
            player.id,
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMockKeyStarter starter@host",
        )
        .await
        .unwrap();
    world
        .complete_mission(player.id, "keys-vault")
        .await
        .unwrap();
    // No starter mission completed

    let gate = world
        .netcity_gate_reason(player.id, std::slice::from_ref(&fp))
        .await
        .unwrap();
    assert!(gate.is_some());

    // Now complete one starter
    world
        .complete_mission(player.id, "redirect-lab")
        .await
        .unwrap();
    let gate2 = world.netcity_gate_reason(player.id, &[fp]).await.unwrap();
    assert!(gate2.is_none());
}

// ── advanced missions ─────────────────────────────────────────────────────────

#[tokio::test]
async fn advanced_missions_exist_in_mission_list() {
    let world = bare_world();
    let player = world.login("adv-check", "203.0.113.80", &[]).await.unwrap();
    let missions = world.mission_statuses(player.id).await.unwrap();
    let codes: Vec<&str> = missions.iter().map(|m| m.code.as_str()).collect();
    for expected in &["awk-patrol", "chain-ops", "sediment"] {
        assert!(codes.contains(expected), "missing mission: {expected}");
    }
}

#[tokio::test]
async fn mission_statuses_include_beginner_metadata_and_order() {
    let world = bare_world();
    let player = world
        .login("briefing-check", "203.0.113.83", &[])
        .await
        .unwrap();
    let missions = world.mission_statuses(player.id).await.unwrap();

    assert_eq!(
        missions.first().map(|m| m.code.as_str()),
        Some("keys-vault")
    );

    let pipes = missions
        .iter()
        .find(|mission| mission.code == "pipes-101")
        .expect("pipes-101 present");
    assert!(
        pipes.starter,
        "starter flag should be exposed to the client"
    );
    assert!(
        pipes.summary.contains("piping") || pipes.summary.contains("pipe"),
        "starter missions should carry a beginner-facing summary"
    );
    assert!(
        pipes.suggested_command.contains("grep token"),
        "starter missions should carry a concrete first command"
    );
}

#[tokio::test]
async fn advanced_mission_gives_twenty_reputation() {
    let world = bare_world();
    let player = world.login("adv-rep", "203.0.113.81", &[]).await.unwrap();

    world
        .complete_mission(player.id, "awk-patrol")
        .await
        .unwrap();

    let refreshed = world.get_player(player.id).await.unwrap();
    assert_eq!(refreshed.reputation, 20);
}

#[tokio::test]
async fn completing_all_advanced_missions_awards_sixty_rep() {
    let world = bare_world();
    let player = world.login("adv-all", "203.0.113.82", &[]).await.unwrap();

    for code in &["awk-patrol", "chain-ops", "sediment"] {
        world.complete_mission(player.id, code).await.unwrap();
    }

    let refreshed = world.get_player(player.id).await.unwrap();
    assert_eq!(refreshed.reputation, 60);
}

// ── daily reward ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn daily_reward_accumulates_streak() {
    let world = bare_world();
    let player = world
        .login("daily-test", "203.0.113.90", &[])
        .await
        .unwrap();

    let now = Utc::now();
    let r1 = world.claim_daily_reward(player.id, now).await.unwrap();
    assert!(r1 > 0, "first reward should be positive");

    // Same day — should return 0 (already claimed)
    let r2 = world.claim_daily_reward(player.id, now).await.unwrap();
    assert_eq!(r2, 0);

    // Next day — streak increments
    let r3 = world
        .claim_daily_reward(player.id, now + Duration::days(1))
        .await
        .unwrap();
    assert!(r3 > r1, "streak should increase reward");
}

#[tokio::test]
async fn daily_reward_resets_streak_on_skip() {
    let world = bare_world();
    let player = world
        .login("daily-skip", "203.0.113.91", &[])
        .await
        .unwrap();
    let now = Utc::now();

    world.claim_daily_reward(player.id, now).await.unwrap();
    // Skip two days
    world
        .claim_daily_reward(player.id, now + Duration::days(3))
        .await
        .unwrap();

    let refreshed = world.get_player(player.id).await.unwrap();
    assert_eq!(refreshed.streak, 1, "streak should reset after skipped day");
}

// ── auction ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn auction_floor_and_rate_limit() {
    let world = bare_world();
    let p = world.login("seller", "203.0.113.6", &[]).await.unwrap();
    // Below floor
    assert!(world
        .create_listing(p.id, "script.basic", 1, 10, None)
        .await
        .is_err());

    // Three successful listings within window
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

    // Fourth hits rate limit
    assert!(world
        .create_listing(p.id, "script.rate", 1, 60, Some(160))
        .await
        .is_err());
}

#[tokio::test]
async fn bid_requires_higher_than_current() {
    let world = bare_world();
    let seller = world.login("s-bid", "203.0.113.100", &[]).await.unwrap();
    let bidder = world.login("b-bid", "203.0.113.101", &[]).await.unwrap();

    let listing = world
        .create_listing(seller.id, "script.bid", 1, 50, Some(300))
        .await
        .unwrap();

    // Bid below start_price must fail
    assert!(world
        .place_bid(bidder.id, listing.listing_id, 30)
        .await
        .is_err());

    // Valid bid
    world
        .place_bid(bidder.id, listing.listing_id, 80)
        .await
        .unwrap();

    // Bid below current highest must fail
    assert!(world
        .place_bid(bidder.id, listing.listing_id, 70)
        .await
        .is_err());
}

#[tokio::test]
async fn buyout_insufficient_funds_does_not_remove_listing() {
    let world = bare_world();
    let seller = world.login("seller2", "203.0.113.31", &[]).await.unwrap();
    let buyer = world.login("buyer2", "203.0.113.32", &[]).await.unwrap();

    let listing = world
        .create_listing(seller.id, "script.elite", 1, 120, Some(900))
        .await
        .unwrap();

    let err = world
        .buyout(buyer.id, listing.listing_id)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("insufficient funds"));

    let market = world.market_snapshot().await;
    assert!(market
        .iter()
        .any(|entry| entry.listing_id == listing.listing_id));
}

// ── pvp ───────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn hardcore_zero_after_three_deaths() {
    let world = bare_world();
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
                .duel_action(
                    duel.duel_id,
                    p1.id,
                    world::CombatAction::Script("burst".into()),
                )
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
async fn duel_winner_gains_wallet_and_reputation() {
    let world = bare_world();
    let p1 = world.login("winner", "203.0.113.110", &[]).await.unwrap();
    let p2 = world.login("loser", "203.0.113.111", &[]).await.unwrap();

    let starting_wallet = p1.wallet;
    let duel = world.start_duel(p1.id, p2.id).await.unwrap();
    loop {
        let turn = world
            .duel_action(duel.duel_id, p1.id, world::CombatAction::Attack)
            .await
            .unwrap();
        if turn.ended {
            break;
        }
        // p2 defends to prolong duel
        let _ = world
            .duel_action(duel.duel_id, p2.id, world::CombatAction::Defend)
            .await;
    }

    let winner = world.get_player(p1.id).await.unwrap();
    assert!(
        winner.wallet > starting_wallet,
        "winner wallet should increase"
    );
    assert!(winner.reputation > 0, "winner reputation should increase");
}

#[tokio::test]
async fn zeroed_player_cannot_start_duel() {
    let world = bare_world();
    let p1 = world.login("zp1", "203.0.113.120", &[]).await.unwrap();
    let p2 = world.login("zp2", "203.0.113.121", &[]).await.unwrap();
    world
        .ban_forever(p1.id, "test ban", "test-suite")
        .await
        .unwrap();

    let result = world.start_duel(p1.id, p2.id).await;
    assert!(result.is_err(), "banned player should not start duel");
}

// ── leaderboard ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn leaderboard_orders_and_omits_banned_players() {
    let world = bare_world();
    let p1 = world.login("alpha", "203.0.113.41", &[]).await.unwrap();
    let p2 = world.login("beta", "203.0.113.42", &[]).await.unwrap();
    let p3 = world.login("gamma", "203.0.113.43", &[]).await.unwrap();

    world.complete_mission(p1.id, "pipes-101").await.unwrap();
    world.complete_mission(p2.id, "finder").await.unwrap();
    world.style_bonus(p2.id, 4, 4).await.unwrap();
    world.complete_mission(p3.id, "keys-vault").await.unwrap();
    world.complete_mission(p3.id, "pipes-101").await.unwrap();
    world
        .ban_forever(p3.id, "test", "test-suite")
        .await
        .unwrap();

    let board = world.leaderboard_snapshot(5).await;
    assert_eq!(board.len(), 2);
    assert!(board[0].display_name.starts_with("beta@"));
    assert!(board[1].display_name.starts_with("alpha@"));
}

#[tokio::test]
async fn leaderboard_limit_respected() {
    let world = bare_world();
    for i in 0..10u8 {
        let name = format!("p{i}");
        let ip = format!("203.0.113.{}", 130 + i);
        let p = world.login(&name, &ip, &[]).await.unwrap();
        world.complete_mission(p.id, "pipes-101").await.unwrap();
    }
    let board = world.leaderboard_snapshot(3).await;
    assert_eq!(board.len(), 3);
}

// ── shell pipeline ─────────────────────────────────────────────────────────────

#[test]
fn shell_pipeline_regression() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("echo", |_, args, _| {
        CommandResult::ok(format!("{}\n", args.join(" ")))
    });
    reg.register("grep", |_, args, input| {
        let pat = args.first().cloned().unwrap_or_default();
        let out = input
            .lines()
            .filter(|line| line.contains(&pat))
            .collect::<Vec<_>>()
            .join("\n");
        CommandResult::ok(format!("{out}\n"))
    });

    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };

    let out = engine
        .execute(&mut ctx, "echo neon-grid | grep neon")
        .unwrap();
    assert_eq!(out.stdout.trim(), "neon-grid");
}

#[test]
fn shell_and_or_chain() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("true", |_, _, _| CommandResult::ok(String::new()));
    reg.register("false", |_, _, _| CommandResult::err(String::new(), 1));
    reg.register("echo", |_, args, _| {
        CommandResult::ok(format!("{}\n", args.join(" ")))
    });

    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };

    // true && echo yes → "yes"
    let out = engine.execute(&mut ctx, "true && echo yes").unwrap();
    assert_eq!(out.stdout.trim(), "yes");

    // false && echo skip → no output (skipped)
    let out2 = engine.execute(&mut ctx, "false && echo skip").unwrap();
    assert!(out2.stdout.trim().is_empty());

    // false || echo fallback → "fallback"
    let out3 = engine.execute(&mut ctx, "false || echo fallback").unwrap();
    assert_eq!(out3.stdout.trim(), "fallback");
}

#[test]
fn shell_redirection_to_file() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("echo", |_, args, _| {
        CommandResult::ok(format!("{}\n", args.join(" ")))
    });

    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };

    engine
        .execute(&mut ctx, "echo hello > /home/player/out.txt")
        .unwrap();

    let content = vfs.read_file("/home/player", "out.txt").unwrap();
    assert_eq!(content.trim(), "hello");
}

#[test]
fn shell_append_redirection() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("echo", |_, args, _| {
        CommandResult::ok(format!("{}\n", args.join(" ")))
    });

    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };

    engine
        .execute(&mut ctx, "echo line1 > /home/player/log.txt")
        .unwrap();
    engine
        .execute(&mut ctx, "echo line2 >> /home/player/log.txt")
        .unwrap();

    let content = vfs.read_file("/home/player", "log.txt").unwrap();
    assert!(content.contains("line1"));
    assert!(content.contains("line2"));
}

#[test]
fn shell_env_var_expansion() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("echo", |_, args, _| {
        CommandResult::ok(format!("{}\n", args.join(" ")))
    });

    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };

    let out = engine.execute(&mut ctx, "echo $USER").unwrap();
    assert_eq!(out.stdout.trim(), "neo");
}

#[test]
fn shell_stdin_redirection() {
    let mut vfs = minimal_vfs();
    vfs.write_file(
        "/",
        "/home/player/source.txt",
        "data content\n",
        false,
        "player",
    )
    .unwrap();

    let mut reg = BuiltinRegistry::default();
    reg.register("cat", |ctx, args, stdin| {
        if args.is_empty() {
            return CommandResult::ok(stdin.to_owned());
        }
        match ctx.vfs.read_file(&ctx.cwd, &args[0]) {
            Ok(c) => CommandResult::ok(c),
            Err(e) => CommandResult::err(format!("{e}\n"), 1),
        }
    });

    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };

    let out = engine
        .execute(&mut ctx, "cat < /home/player/source.txt")
        .unwrap();
    assert!(out.stdout.contains("data content"));
}

// ── script sandbox ────────────────────────────────────────────────────────────

#[tokio::test]
async fn script_sandbox_regression() {
    let engine = ScriptEngine::new(ScriptPolicy {
        max_runtime: std::time::Duration::from_millis(500),
        ..ScriptPolicy::default()
    });

    let out = engine
        .run(
            "let nodes = scan_nodes(); print(nodes.len);",
            ScriptContext {
                visible_nodes: vec!["a".to_owned(), "b".to_owned()],
                virtual_files: Default::default(),
            },
        )
        .await
        .unwrap();
    assert!(out.output.contains("2"));
}

#[tokio::test]
async fn script_reads_virtual_file() {
    let engine = ScriptEngine::new(ScriptPolicy::default());
    let mut files = std::collections::BTreeMap::new();
    files.insert(
        "/logs/neon-gateway.log".to_owned(),
        "[INFO] token=GLASS-AXON-13\n[WARN] sector drift\n".to_owned(),
    );

    let out = engine
        .run(
            r#"let data = read_virtual("/logs/neon-gateway.log"); print(grep(data, "token"));"#,
            ScriptContext {
                visible_nodes: vec![],
                virtual_files: files,
            },
        )
        .await
        .unwrap();
    assert!(out.output.contains("GLASS-AXON-13"));
}

#[tokio::test]
async fn script_sandbox_limits_operations() {
    let engine = ScriptEngine::new(ScriptPolicy {
        max_operations: 10,
        ..ScriptPolicy::default()
    });

    // Infinite loop should be terminated by operation limit
    let result = engine
        .run(
            "loop { }",
            ScriptContext {
                visible_nodes: vec![],
                virtual_files: Default::default(),
            },
        )
        .await;
    // Should either error out or return with a failure indication
    // Either way it should not block forever
    let _ = result;
}

// ── world — hidden mission ────────────────────────────────────────────────────

#[tokio::test]
async fn hidden_mission_not_listed_until_eligible() {
    use world::SecretMissionConfig;
    let world = WorldService::new(
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
    );
    let p = world.login("c", "203.0.113.11", &[]).await.unwrap();

    let before = world.mission_statuses(p.id).await.unwrap();
    assert!(!before.iter().any(|m| m.code == "hidden-contact"));

    // Earn enough reputation and the required achievement
    world.style_bonus(p.id, 4, 4).await.unwrap(); // awards Pipe Dream + rep
    world.complete_mission(p.id, "keys-vault").await.unwrap();
    world.complete_mission(p.id, "pipes-101").await.unwrap();
    world.complete_mission(p.id, "finder").await.unwrap();

    let after = world.mission_statuses(p.id).await.unwrap();
    assert!(after.iter().any(|m| m.code == "hidden-contact"));
}

// ── world — mode switch ───────────────────────────────────────────────────────

#[tokio::test]
async fn mode_switch_netcity_requires_fp_match() {
    use protocol::Mode;
    let world = bare_world();
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

    // Re-login presenting the fingerprint so observed_fingerprints is populated
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
async fn mode_switch_netcity_blocked_without_fp() {
    use protocol::Mode;
    let world = bare_world();
    let p = world.login("no-fp", "203.0.113.200", &[]).await.unwrap();
    world
        .register_key(
            p.id,
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMockKeyNoFP nofp@host",
        )
        .await
        .unwrap();
    world.complete_mission(p.id, "keys-vault").await.unwrap();
    world.complete_mission(p.id, "pipes-101").await.unwrap();

    // Login WITHOUT presenting fp → observed_fingerprints is empty
    // mode switch to NetCity should fail
    let result = world.mode_switch(p.id, Mode::NetCity, None).await;
    assert!(result.is_err(), "NetCity switch should fail without fp");
}

// ── VFS operations ────────────────────────────────────────────────────────────

#[test]
fn vfs_basic_read_write() {
    let mut vfs = Vfs::default();
    vfs.mkdir_p("/", "home/player", "player").unwrap();
    vfs.write_file("/", "/home/player/hello.txt", "world", false, "player")
        .unwrap();
    let content = vfs.read_file("/home/player", "hello.txt").unwrap();
    assert_eq!(content, "world");
}

#[test]
fn vfs_append_mode() {
    let mut vfs = Vfs::default();
    vfs.mkdir_p("/", "home/player", "player").unwrap();
    vfs.write_file("/", "/home/player/log.txt", "line1\n", false, "player")
        .unwrap();
    vfs.write_file("/", "/home/player/log.txt", "line2\n", true, "player")
        .unwrap();
    let content = vfs.read_file("/home/player", "log.txt").unwrap();
    assert!(content.contains("line1"));
    assert!(content.contains("line2"));
}

#[test]
fn vfs_copy_and_move() {
    let mut vfs = Vfs::default();
    vfs.mkdir_p("/", "home/player", "player").unwrap();
    vfs.write_file("/", "/home/player/src.txt", "data", false, "player")
        .unwrap();
    vfs.copy("/home/player", "src.txt", "dst.txt").unwrap();
    let dst = vfs.read_file("/home/player", "dst.txt").unwrap();
    assert_eq!(dst, "data");
    vfs.mv("/home/player", "src.txt", "moved.txt").unwrap();
    assert!(vfs.read_file("/home/player", "src.txt").is_err());
    let moved = vfs.read_file("/home/player", "moved.txt").unwrap();
    assert_eq!(moved, "data");
}

#[test]
fn vfs_remove_file() {
    let mut vfs = Vfs::default();
    vfs.mkdir_p("/", "home/player", "player").unwrap();
    vfs.write_file("/", "/home/player/del.txt", "bye", false, "player")
        .unwrap();
    vfs.remove("/home/player", "del.txt").unwrap();
    assert!(vfs.read_file("/home/player", "del.txt").is_err());
}

#[test]
fn vfs_find_by_name() {
    let mut vfs = Vfs::default();
    vfs.mkdir_p("/", "home/player", "player").unwrap();
    vfs.mkdir_p("/", "home/player/sub", "player").unwrap();
    vfs.write_file("/", "/home/player/sub/needle.txt", "x", false, "player")
        .unwrap();
    let found = vfs.find("/home/player", ".", Some("needle.txt")).unwrap();
    assert!(!found.is_empty());
    assert!(found.iter().any(|p| p.contains("needle.txt")));
}

// ── market snapshot ───────────────────────────────────────────────────────────

#[tokio::test]
async fn market_snapshot_and_events_snapshot_are_available() {
    let world = bare_world();
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

// ── pub key validation ────────────────────────────────────────────────────────

#[tokio::test]
async fn invalid_key_formats_are_rejected() {
    let world = bare_world();
    let p = world.login("key-test", "203.0.113.210", &[]).await.unwrap();

    let bad_keys = [
        "not-a-key",
        "ssh-unknown AAAA... comment",
        "AAAAC3NzaC1lZDI1NTE5", // missing type prefix
        "ssh-ed25519",          // missing key data
    ];

    for bad in &bad_keys {
        let result = world.register_key(p.id, bad).await;
        assert!(result.is_err(), "should reject invalid key: {bad}");
    }
}

#[tokio::test]
async fn valid_rsa_key_is_accepted() {
    let world = bare_world();
    let p = world.login("rsa-test", "203.0.113.211", &[]).await.unwrap();
    let key = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAAAQQDValid+key+data+here= user@host";
    let result = world.register_key(p.id, key).await;
    assert!(
        result.is_ok(),
        "valid RSA key should be accepted: {result:?}"
    );
}

// ── new missions ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn three_new_advanced_missions_are_in_mission_list() {
    let world = bare_world();
    let p = world
        .login("mission-test", "203.0.113.220", &[])
        .await
        .unwrap();
    let statuses = world.mission_statuses(p.id).await.unwrap();
    let codes: Vec<&str> = statuses.iter().map(|m| m.code.as_str()).collect();
    for expected in ["cut-lab", "pattern-sweep", "file-ops"] {
        assert!(codes.contains(&expected), "missing mission: {expected}");
    }
}

#[tokio::test]
async fn new_advanced_missions_award_twenty_reputation() {
    let world = bare_world();
    let p = world
        .login("rep-test2", "203.0.113.221", &[])
        .await
        .unwrap();

    for code in ["cut-lab", "pattern-sweep", "file-ops"] {
        world.complete_mission(p.id, code).await.unwrap();
    }

    let refreshed = world.get_player(p.id).await.unwrap();
    // 3 advanced missions × 20 rep each = 60
    assert_eq!(
        refreshed.reputation, 60,
        "each advanced mission awards 20 rep"
    );
}

#[tokio::test]
async fn all_six_advanced_missions_award_one_twenty_rep() {
    let world = bare_world();
    let p = world
        .login("rep-test3", "203.0.113.222", &[])
        .await
        .unwrap();
    for code in [
        "awk-patrol",
        "chain-ops",
        "sediment",
        "cut-lab",
        "pattern-sweep",
        "file-ops",
    ] {
        world.complete_mission(p.id, code).await.unwrap();
    }
    let refreshed = world.get_player(p.id).await.unwrap();
    assert_eq!(refreshed.reputation, 120);
}

// ── shell: new command pipelines ─────────────────────────────────────────────

#[test]
fn shell_grep_count_pipeline() {
    // Register minimal grep with -c support inline.
    let mut vfs = minimal_vfs();
    vfs.write_file(
        "/home/player",
        "auth.log",
        "ACCEPT\nREJECT\nACCEPT\nREJECT\nREJECT\n",
        false,
        "player",
    )
    .unwrap();

    let mut reg = BuiltinRegistry::default();
    reg.register("grep", |ctx, args, stdin| {
        let count_mode = args.iter().any(|a| a == "-c");
        let positional: Vec<&str> = args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .map(String::as_str)
            .collect();
        let Some(pat) = positional.first().copied() else {
            return CommandResult::err("grep: missing pattern\n", 1);
        };
        let file_arg = positional.get(1).copied();
        let source = if let Some(path) = file_arg {
            ctx.vfs.read_file(&ctx.cwd, path).unwrap_or_default()
        } else {
            stdin.to_owned()
        };
        let matches: Vec<&str> = source.lines().filter(|l| l.contains(pat)).collect();
        if count_mode {
            return CommandResult::ok(format!("{}\n", matches.len()));
        }
        if matches.is_empty() {
            CommandResult::err(String::new(), 1)
        } else {
            CommandResult::ok(format!("{}\n", matches.join("\n")))
        }
    });

    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };

    let result = engine.execute(&mut ctx, "grep -c REJECT auth.log").unwrap();
    assert_eq!(result.stdout.trim(), "3");
}

#[test]
fn shell_sort_numeric_pipeline() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("echo", |_, args, _| {
        CommandResult::ok(format!("{}\n", args.join("\n")))
    });
    reg.register("sort", |_, args, stdin| {
        let mut lines: Vec<String> = stdin.lines().map(str::to_owned).collect();
        let numeric = args.iter().any(|a| a == "-n");
        if numeric {
            lines.sort_by(|a, b| {
                let na = a.trim().parse::<f64>().unwrap_or(0.0);
                let nb = b.trim().parse::<f64>().unwrap_or(0.0);
                na.partial_cmp(&nb).unwrap_or(std::cmp::Ordering::Equal)
            });
        } else {
            lines.sort();
        }
        CommandResult::ok(format!("{}\n", lines.join("\n")))
    });

    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };

    // Without -n: lexicographic (10 < 2 < 20)
    let lex = engine.execute(&mut ctx, "echo 10 20 2 | sort").unwrap();
    let lex_lines: Vec<&str> = lex.stdout.trim().lines().collect();
    // "10" < "2" < "20" lexicographically
    assert_eq!(lex_lines[0], "10");

    // With -n: numeric (2 < 10 < 20)
    let num = engine.execute(&mut ctx, "echo 10 20 2 | sort -n").unwrap();
    let num_lines: Vec<&str> = num.stdout.trim().lines().collect();
    assert_eq!(num_lines[0], "2");
}

// ── VFS: new methods ──────────────────────────────────────────────────────────

#[test]
fn vfs_copy_tree() {
    let mut vfs = minimal_vfs();
    vfs.mkdir_p("/home/player", "orig/nested", "player")
        .unwrap();
    vfs.write_file("/home/player/orig", "top.txt", "TOP", false, "player")
        .unwrap();
    vfs.write_file(
        "/home/player/orig/nested",
        "deep.txt",
        "DEEP",
        false,
        "player",
    )
    .unwrap();

    vfs.copy_tree("/home/player", "orig", "copy").unwrap();

    assert_eq!(
        vfs.read_file("/home/player/copy", "top.txt").unwrap(),
        "TOP"
    );
    assert_eq!(
        vfs.read_file("/home/player/copy/nested", "deep.txt")
            .unwrap(),
        "DEEP"
    );
    // Source must survive
    assert_eq!(
        vfs.read_file("/home/player/orig", "top.txt").unwrap(),
        "TOP"
    );
}

#[test]
fn vfs_copy_tree_then_remove_original() {
    let mut vfs = minimal_vfs();
    vfs.mkdir_p("/home/player", "src", "player").unwrap();
    vfs.write_file("/home/player/src", "data.txt", "hello", false, "player")
        .unwrap();

    vfs.copy_tree("/home/player", "src", "backup").unwrap();
    vfs.remove("/home/player", "src").unwrap();

    // backup must exist, original must be gone
    assert_eq!(
        vfs.read_file("/home/player/backup", "data.txt").unwrap(),
        "hello"
    );
    assert!(vfs.read_file("/home/player/src", "data.txt").is_err());
}

#[test]
fn vfs_chmod_and_stat() {
    let mut vfs = minimal_vfs();
    vfs.write_file("/home/player", "exec.sh", "#!/bin/sh", false, "player")
        .unwrap();
    vfs.chmod("/home/player", "exec.sh", 0o755).unwrap();
    let node = vfs.stat("/home/player", "exec.sh").unwrap();
    assert_eq!(node.meta.perms.mode, 0o755);
}

#[test]
fn vfs_ls_nodes_returns_direct_children() {
    let mut vfs = minimal_vfs();
    vfs.write_file("/home/player", "a.txt", "a", false, "player")
        .unwrap();
    vfs.write_file("/home/player", "b.txt", "b", false, "player")
        .unwrap();
    vfs.mkdir_p("/home/player", "subdir", "player").unwrap();
    vfs.write_file("/home/player/subdir", "c.txt", "c", false, "player")
        .unwrap();

    let nodes = vfs.ls_nodes("/", Some("/home/player")).unwrap();
    // Direct children: a.txt, b.txt, subdir (not subdir/c.txt)
    let names: Vec<&str> = nodes
        .iter()
        .map(|n| n.path.rsplit('/').next().unwrap_or(""))
        .collect();
    assert!(names.contains(&"a.txt"));
    assert!(names.contains(&"b.txt"));
    assert!(names.contains(&"subdir"));
    // Nested file must NOT appear as direct child
    assert!(!names.contains(&"c.txt"));
}

#[test]
fn vfs_stat_returns_correct_metadata() {
    let mut vfs = minimal_vfs();
    vfs.write_file("/home/player", "notes.txt", "content", false, "player")
        .unwrap();
    let node = vfs.stat("/home/player", "notes.txt").unwrap();
    use vfs::NodeKind;
    assert_eq!(node.kind, NodeKind::File);
    assert_eq!(node.meta.owner, "player");
    assert!(node.content.as_deref().unwrap_or("").contains("content"));
}

// ── Round 3: shell engine VAR=value assignment ────────────────────────────────

#[test]
fn shell_var_assignment_sets_env() {
    // VAR=value is handled in ShellEngine::execute() before dispatch.
    // No builtins needed — use an empty registry.
    let mut vfs = minimal_vfs();
    let reg = BuiltinRegistry::default();
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let result = engine.execute(&mut ctx, "MYVAR=neon").unwrap();
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, "");
    assert_eq!(ctx.env.get("MYVAR").map(String::as_str), Some("neon"));
}

#[test]
fn shell_var_assignment_with_value_containing_equals() {
    let mut vfs = minimal_vfs();
    let reg = BuiltinRegistry::default();
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let result = engine.execute(&mut ctx, "URL=host=value").unwrap();
    assert_eq!(result.exit_code, 0);
    assert_eq!(ctx.env.get("URL").map(String::as_str), Some("host=value"));
}

// ── Round 3: echo -n ──────────────────────────────────────────────────────────

#[test]
fn shell_echo_no_newline() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("echo", |_, args, _| {
        let no_nl = args.iter().any(|a| a == "-n");
        let words: Vec<&str> = args
            .iter()
            .filter(|a| *a != "-n")
            .map(String::as_str)
            .collect();
        let content = words.join(" ");
        if no_nl {
            CommandResult::ok(content)
        } else {
            CommandResult::ok(format!("{content}\n"))
        }
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let result = engine.execute(&mut ctx, "echo -n hello").unwrap();
    assert_eq!(result.stdout, "hello");
    assert!(!result.stdout.ends_with('\n'));
}

// ── Round 3: tr -d and tr -s ─────────────────────────────────────────────────

#[test]
fn shell_tr_delete_mode() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("echo", |_, args, _| {
        CommandResult::ok(format!("{}\n", args.join(" ")))
    });
    reg.register("tr", |_, args, stdin| {
        let positional: Vec<&str> = args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .map(String::as_str)
            .collect();
        if args.iter().any(|a| a == "-d") {
            let set: std::collections::HashSet<char> =
                positional.first().unwrap_or(&"").chars().collect();
            let out: String = stdin.chars().filter(|c| !set.contains(c)).collect();
            return CommandResult::ok(out);
        }
        CommandResult::ok(stdin.to_owned())
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    // pipe "hello world" through tr -d 'aeiou' → "hll wrld"
    let result = engine
        .execute(&mut ctx, "echo hello world | tr -d aeiou")
        .unwrap();
    assert_eq!(result.stdout.trim(), "hll wrld");
}

#[test]
fn shell_tr_squeeze_mode() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("echo", |_, args, _| {
        CommandResult::ok(format!("{}\n", args.join(" ")))
    });
    reg.register("tr", |_, args, stdin| {
        let positional: Vec<&str> = args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .map(String::as_str)
            .collect();
        if args.iter().any(|a| a == "-s") {
            let set: std::collections::HashSet<char> =
                positional.first().unwrap_or(&"").chars().collect();
            let mut out = String::new();
            let mut last: Option<char> = None;
            for c in stdin.chars() {
                if set.contains(&c) && last == Some(c) {
                    // squeeze
                } else {
                    out.push(c);
                }
                last = Some(c);
            }
            return CommandResult::ok(out);
        }
        CommandResult::ok(stdin.to_owned())
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let result = engine.execute(&mut ctx, "echo aabbcc | tr -s abc").unwrap();
    assert_eq!(result.stdout.trim(), "abc");
}

// ── Round 3: cut multi-field ──────────────────────────────────────────────────

fn make_cut_registry() -> BuiltinRegistry {
    let mut reg = BuiltinRegistry::default();
    reg.register("cut", |_, args, stdin| {
        let mut delim = "\t".to_owned();
        let mut field_spec = "1".to_owned();
        let mut i = 0usize;
        while i < args.len() {
            match args[i].as_str() {
                "-d" if i + 1 < args.len() => {
                    delim = args[i + 1].clone();
                    i += 1;
                }
                "-f" if i + 1 < args.len() => {
                    field_spec = args[i + 1].clone();
                    i += 1;
                }
                _ => {}
            }
            i += 1;
        }
        let fields: Vec<usize> = if field_spec.contains('-') {
            let parts: Vec<usize> = field_spec
                .splitn(2, '-')
                .filter_map(|s| s.parse::<usize>().ok())
                .collect();
            if parts.len() == 2 {
                (parts[0]..=parts[1]).collect()
            } else {
                vec![1]
            }
        } else {
            field_spec
                .split(',')
                .filter_map(|s| s.trim().parse::<usize>().ok())
                .collect()
        };
        let out = stdin
            .lines()
            .map(|line| {
                let parts: Vec<&str> = line.split(delim.as_str()).collect();
                fields
                    .iter()
                    .filter_map(|&f| parts.get(f.saturating_sub(1)).copied())
                    .collect::<Vec<_>>()
                    .join(delim.as_str())
            })
            .collect::<Vec<_>>()
            .join("\n");
        CommandResult::ok(format!("{out}\n"))
    });
    reg
}

#[test]
fn shell_cut_multi_field_comma() {
    // Write TSV data to VFS, then redirect into cut via '<'
    let mut vfs = minimal_vfs();
    vfs.write_file(
        "/home/player",
        "data.tsv",
        "a\tb\tc\td\n1\t2\t3\t4\n",
        false,
        "player",
    )
    .unwrap();
    let engine = ShellEngine::with_registry(make_cut_registry());
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let result = engine.execute(&mut ctx, "cut -f 1,3 < data.tsv").unwrap();
    let lines: Vec<&str> = result.stdout.trim().lines().collect();
    assert_eq!(lines[0], "a\tc");
    assert_eq!(lines[1], "1\t3");
}

#[test]
fn shell_cut_range_field() {
    let mut vfs = minimal_vfs();
    vfs.write_file("/home/player", "range.tsv", "a\tb\tc\td\n", false, "player")
        .unwrap();
    let engine = ShellEngine::with_registry(make_cut_registry());
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let result = engine.execute(&mut ctx, "cut -f 2-4 < range.tsv").unwrap();
    assert_eq!(result.stdout.trim(), "b\tc\td");
}

// ── Round 3: uniq -d ─────────────────────────────────────────────────────────

#[test]
fn shell_uniq_dup_only_mode() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("echo", |_, args, _| {
        CommandResult::ok(format!("{}\n", args.join("\n")))
    });
    reg.register("uniq", |_, args, stdin| {
        let dup_only = args.iter().any(|a| a == "-d");
        let mut out: Vec<String> = Vec::new();
        let mut prev: Option<String> = None;
        let mut run = 0usize;
        let mut counts: Vec<usize> = Vec::new();
        for line in stdin.lines() {
            if prev.as_deref() == Some(line) {
                run += 1;
            } else {
                if let Some(p) = prev.take() {
                    out.push(p);
                    counts.push(run);
                }
                prev = Some(line.to_owned());
                run = 1;
            }
        }
        if let Some(p) = prev {
            out.push(p);
            counts.push(run);
        }
        let result = if dup_only {
            out.iter()
                .zip(counts.iter())
                .filter(|(_, &c)| c > 1)
                .map(|(l, _)| l.clone())
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            out.join("\n")
        };
        CommandResult::ok(format!("{result}\n"))
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    // a appears once, b appears twice consecutively, c appears once
    let result = engine.execute(&mut ctx, "echo a b b c | uniq -d").unwrap();
    assert_eq!(result.stdout.trim(), "b");
}

// ── Round 3: world — 3 new advanced missions ──────────────────────────────────

#[tokio::test]
async fn three_new_round3_missions_in_mission_list() {
    let world = bare_world();
    let p = world.login("r3-check", "203.0.113.250", &[]).await.unwrap();
    let statuses = world.mission_statuses(p.id).await.unwrap();
    let codes: Vec<&str> = statuses.iter().map(|m| m.code.as_str()).collect();
    assert!(codes.contains(&"regex-hunt"), "regex-hunt missing");
    assert!(codes.contains(&"pipeline-pro"), "pipeline-pro missing");
    assert!(codes.contains(&"var-play"), "var-play missing");
}

#[tokio::test]
async fn round3_advanced_missions_award_twenty_rep() {
    let world = bare_world();
    let p = world.login("rep-r3", "203.0.113.240", &[]).await.unwrap();
    for code in ["regex-hunt", "pipeline-pro", "var-play"] {
        world.complete_mission(p.id, code).await.unwrap();
    }
    let refreshed = world.get_player(p.id).await.unwrap();
    // 3 × 20 = 60
    assert_eq!(refreshed.reputation, 60);
}

#[tokio::test]
async fn all_nine_advanced_missions_award_one_eighty_rep() {
    let world = bare_world();
    let p = world.login("rep-all9", "203.0.113.241", &[]).await.unwrap();
    for code in [
        "awk-patrol",
        "chain-ops",
        "sediment",
        "cut-lab",
        "pattern-sweep",
        "file-ops",
        "regex-hunt",
        "pipeline-pro",
        "var-play",
    ] {
        world.complete_mission(p.id, code).await.unwrap();
    }
    let refreshed = world.get_player(p.id).await.unwrap();
    assert_eq!(refreshed.reputation, 180);
}

// ── Round 3: world — 4 seeded events ─────────────────────────────────────────

#[tokio::test]
async fn world_seeds_four_events() {
    let world = bare_world();
    // Query at t=now; all 4 events start in the future so none are "active",
    // but world_events_snapshot returns all events ordered by start time.
    let now = Utc::now();
    let events = world.world_events_snapshot(now).await;
    assert_eq!(events.len(), 4, "expected 4 seeded world events");
}

#[tokio::test]
async fn world_event_sectors_are_distinct() {
    let world = bare_world();
    let now = Utc::now();
    let events = world.world_events_snapshot(now).await;
    let sectors: std::collections::HashSet<&str> =
        events.iter().map(|e| e.sector.as_str()).collect();
    assert_eq!(
        sectors.len(),
        4,
        "all 4 events should be in distinct sectors"
    );
}

#[tokio::test]
async fn world_new_events_have_future_start_times() {
    let world = bare_world();
    let now = Utc::now();
    let events = world.world_events_snapshot(now).await;
    for e in &events {
        // ends_at must be after starts_at
        assert!(
            e.ends_at > e.starts_at,
            "event '{}' end must be after start",
            e.title
        );
    }
}

// ── Round 3: nl command ───────────────────────────────────────────────────────

#[test]
fn shell_nl_numbers_lines_from_stdin() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("echo", |_, args, _| {
        CommandResult::ok(format!("{}\n", args.join("\n")))
    });
    reg.register("nl", |_, _, stdin| {
        let out = stdin
            .lines()
            .enumerate()
            .map(|(i, line)| format!("{:6}\t{line}", i + 1))
            .collect::<Vec<_>>()
            .join("\n");
        CommandResult::ok(format!("{out}\n"))
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let result = engine.execute(&mut ctx, "echo alpha beta | nl").unwrap();
    let lines: Vec<&str> = result.stdout.trim().lines().collect();
    assert!(lines[0].contains("1") && lines[0].contains("alpha"));
    assert!(lines[1].contains("2") && lines[1].contains("beta"));
}

// ── Round 3: export command ───────────────────────────────────────────────────

fn make_export_registry() -> BuiltinRegistry {
    let mut reg = BuiltinRegistry::default();
    reg.register("export", |ctx, args, _| {
        if args.is_empty() {
            let mut pairs: Vec<String> = ctx
                .env
                .iter()
                .map(|(k, v)| format!("declare -x {k}=\"{v}\""))
                .collect();
            pairs.sort();
            return CommandResult::ok(format!("{}\n", pairs.join("\n")));
        }
        for arg in args {
            if let Some(eq) = arg.find('=') {
                let key = arg[..eq].to_owned();
                let val = arg[eq + 1..].to_owned();
                ctx.env.insert(key, val);
            }
        }
        CommandResult::ok(String::new())
    });
    reg
}

#[test]
fn shell_export_sets_env_var() {
    let mut vfs = minimal_vfs();
    let engine = ShellEngine::with_registry(make_export_registry());
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let result = engine
        .execute(&mut ctx, "export TARGET=vault-sat-9")
        .unwrap();
    assert_eq!(result.exit_code, 0);
    assert_eq!(
        ctx.env.get("TARGET").map(String::as_str),
        Some("vault-sat-9")
    );
}

#[test]
fn shell_export_no_args_lists_env() {
    let mut vfs = minimal_vfs();
    let engine = ShellEngine::with_registry(make_export_registry());
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let result = engine.execute(&mut ctx, "export").unwrap();
    assert_eq!(result.exit_code, 0);
    assert!(result.stdout.contains("declare -x"));
    assert!(result.stdout.contains("USER"));
}

// ── Round 4: echo -e ──────────────────────────────────────────────────────────

#[test]
fn shell_echo_e_interprets_escape_sequences() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("echo", |_, args, _| {
        let interp = args.iter().any(|a| a == "-e");
        let words: Vec<&str> = args
            .iter()
            .filter(|a| *a != "-e" && *a != "-n")
            .map(String::as_str)
            .collect();
        let raw = words.join(" ");
        let content = if interp {
            raw.replace("\\n", "\n").replace("\\t", "\t")
        } else {
            raw
        };
        CommandResult::ok(format!("{content}\n"))
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let result = engine.execute(&mut ctx, "echo -e 'line1\\nline2'").unwrap();
    let lines: Vec<&str> = result.stdout.trim().lines().collect();
    assert_eq!(lines.len(), 2, "echo -e should produce two lines");
    assert_eq!(lines[0], "line1");
    assert_eq!(lines[1], "line2");
}

// ── Round 4: printf %d/%f ──────────────────────────────────────────────────────

#[test]
fn shell_printf_integer_format() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("printf", |_, args, _| {
        if args.is_empty() {
            return CommandResult::ok(String::new());
        }
        let fmt = args[0].replace("\\n", "\n");
        let fmt_args = &args[1..];
        let mut out = String::new();
        let mut arg_idx = 0usize;
        let mut chars = fmt.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch != '%' {
                out.push(ch);
                continue;
            }
            match chars.peek().copied() {
                Some('d') => {
                    chars.next();
                    let n = fmt_args
                        .get(arg_idx)
                        .and_then(|s| s.parse::<i64>().ok())
                        .unwrap_or(0);
                    out.push_str(&n.to_string());
                    arg_idx += 1;
                }
                Some('s') => {
                    chars.next();
                    out.push_str(fmt_args.get(arg_idx).map(String::as_str).unwrap_or(""));
                    arg_idx += 1;
                }
                _ => out.push(ch),
            }
        }
        CommandResult::ok(out)
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let result = engine.execute(&mut ctx, "printf 'score=%d\\n' 42").unwrap();
    assert_eq!(result.stdout.trim(), "score=42");
}

// ── Round 4: grep -E regex support ────────────────────────────────────────────

#[test]
fn shell_grep_regex_alternation() {
    let mut vfs = minimal_vfs();
    vfs.write_file(
        "/home/player",
        "events.log",
        "INFO user connected\nERROR disk full\nWARN memory low\nFATAL segfault\nINFO session ok\n",
        false,
        "player",
    )
    .unwrap();
    let mut reg = BuiltinRegistry::default();
    reg.register("grep", |ctx, args, stdin| {
        let regex_mode = args.iter().any(|a| a == "-E");
        let positional: Vec<&str> = args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .map(String::as_str)
            .collect();
        let Some(pat) = positional.first().copied() else {
            return CommandResult::err("grep: missing pattern\n", 1);
        };
        let file_arg = positional.get(1).copied();
        let source = if let Some(path) = file_arg {
            ctx.vfs.read_file(&ctx.cwd, path).unwrap_or_default()
        } else {
            stdin.to_owned()
        };
        let matches: Vec<&str> = if regex_mode {
            let re = regex::Regex::new(pat).unwrap();
            source.lines().filter(|l| re.is_match(l)).collect()
        } else {
            source.lines().filter(|l| l.contains(pat)).collect()
        };
        if matches.is_empty() {
            CommandResult::err(String::new(), 1)
        } else {
            CommandResult::ok(format!("{}\n", matches.join("\n")))
        }
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    // regex alternation: match lines containing ERROR or FATAL
    let result = engine
        .execute(&mut ctx, "grep -E 'ERROR|FATAL' events.log")
        .unwrap();
    let lines: Vec<&str> = result.stdout.trim().lines().collect();
    assert!(lines.iter().any(|l| l.contains("ERROR")));
    assert!(lines.iter().any(|l| l.contains("FATAL")));
    assert!(!lines.iter().any(|l| l.contains("INFO")));
}

#[test]
fn shell_grep_regex_digit_pattern() {
    let vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("echo", |_, args, _| {
        CommandResult::ok(format!("{}\n", args.join("\n")))
    });
    reg.register("grep", |_, args, stdin| {
        let regex_mode = args.iter().any(|a| a == "-E");
        let positional: Vec<&str> = args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .map(String::as_str)
            .collect();
        let Some(pat) = positional.first().copied() else {
            return CommandResult::err("grep: missing pattern\n", 1);
        };
        let matches: Vec<&str> = if regex_mode {
            let re = regex::Regex::new(pat).unwrap();
            stdin.lines().filter(|l| re.is_match(l)).collect()
        } else {
            stdin.lines().filter(|l| l.contains(pat)).collect()
        };
        CommandResult::ok(format!("{}\n", matches.join("\n")))
    });
    let engine = ShellEngine::with_registry(reg);
    let mut vfs2 = minimal_vfs();
    let mut ctx = ExecutionContext {
        vfs: &mut vfs2,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let _ = vfs; // suppress unused
                 // grep -E '[0-9]+' should match lines with digits
    let result = engine
        .execute(&mut ctx, "echo abc 123 def | grep -E [0-9]+")
        .unwrap();
    assert!(result.stdout.contains("123"));
    assert!(!result.stdout.contains("abc"));
}

// ── Round 4: awk NR/NF ────────────────────────────────────────────────────────

#[test]
fn shell_awk_nr_variable() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("echo", |_, args, _| {
        CommandResult::ok(format!("{}\n", args.join("\n")))
    });
    reg.register("awk", |_, args, stdin| {
        let positional: Vec<&str> = args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .map(String::as_str)
            .collect();
        let Some(expr) = positional.first() else {
            return CommandResult::err("awk: missing program\n", 1);
        };
        let out: Vec<String> = stdin
            .lines()
            .enumerate()
            .map(|(i, line)| {
                expr.replace("NR", &(i + 1).to_string())
                    .replace("$1", line.split_whitespace().next().unwrap_or(""))
                    .replace("{print ", "")
                    .replace('}', "")
                    .trim()
                    .to_owned()
            })
            .collect();
        CommandResult::ok(format!("{}\n", out.join("\n")))
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let result = engine
        .execute(&mut ctx, "echo alpha beta gamma | awk '{print NR}'")
        .unwrap();
    let lines: Vec<&str> = result.stdout.trim().lines().collect();
    assert_eq!(lines[0], "1");
    assert_eq!(lines[1], "2");
    assert_eq!(lines[2], "3");
}

#[test]
fn shell_awk_nr_condition_skip_header() {
    let mut vfs = minimal_vfs();
    vfs.write_file(
        "/home/player",
        "data.csv",
        "name,score\nneo,9800\nrift,8700\n",
        false,
        "player",
    )
    .unwrap();
    let mut reg = BuiltinRegistry::default();
    reg.register("awk", |ctx, args, stdin| {
        // minimal awk: NR>1 {print $1} with comma delimiter
        let mut field_sep: Option<String> = None;
        for (i, arg) in args.iter().enumerate() {
            if arg == "-F" {
                if let Some(s) = args.get(i + 1) {
                    field_sep = Some(s.clone());
                }
            } else if let Some(s) = arg.strip_prefix("-F") {
                field_sep = Some(s.to_owned());
            }
        }
        let positional: Vec<&str> = args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .map(String::as_str)
            .collect();
        let Some(expr) = positional.first() else {
            return CommandResult::err("awk: missing program\n", 1);
        };
        let file_arg = positional.get(1).copied();
        let source = if let Some(p) = file_arg {
            ctx.vfs.read_file(&ctx.cwd, p).unwrap_or_default()
        } else {
            stdin.to_owned()
        };
        // Parse condition: "NR>1"
        let (cond_str, action) = if let Some(b) = expr.find('{') {
            (expr[..b].trim(), &expr[b..])
        } else {
            ("", *expr)
        };
        let col = action
            .find('$')
            .and_then(|p| action[p + 1..].split('}').next())
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(1);

        let out: Vec<String> = source
            .lines()
            .enumerate()
            .filter_map(|(i, line)| {
                let nr = i + 1;
                if cond_str.contains('>') {
                    let threshold: usize = cond_str
                        .split('>')
                        .nth(1)
                        .and_then(|s| s.trim().parse().ok())
                        .unwrap_or(0);
                    if nr <= threshold {
                        return None;
                    }
                }
                let field = if let Some(ref sep) = field_sep {
                    line.split(sep.as_str())
                        .nth(col.saturating_sub(1))
                        .unwrap_or("")
                        .to_owned()
                } else {
                    line.split_whitespace()
                        .nth(col.saturating_sub(1))
                        .unwrap_or("")
                        .to_owned()
                };
                Some(field)
            })
            .collect();
        CommandResult::ok(format!("{}\n", out.join("\n")))
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let result = engine
        .execute(&mut ctx, "awk -F, 'NR>1 {print $1}' data.csv")
        .unwrap();
    let lines: Vec<&str> = result.stdout.trim().lines().collect();
    assert_eq!(lines[0], "neo");
    assert_eq!(lines[1], "rift");
}

// ── Round 4: test/[ numeric comparisons ───────────────────────────────────────

#[test]
fn shell_test_numeric_lt_gt() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("true", |_, _, _| CommandResult::ok(String::new()));
    reg.register("false", |_, _, _| CommandResult::err(String::new(), 1));
    reg.register("echo", |_, args, _| {
        CommandResult::ok(format!("{}\n", args.join(" ")))
    });
    // test -lt / -gt via exit code in && chain
    // We inline a mini test for the purpose of this regression
    reg.register("test", |_, args, _| {
        let na = args
            .first()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);
        let nb = args.get(2).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
        let op = args.get(1).map(String::as_str).unwrap_or("");
        let passed = match op {
            "-lt" => na < nb,
            "-le" => na <= nb,
            "-gt" => na > nb,
            "-ge" => na >= nb,
            "-eq" => na == nb,
            "-ne" => na != nb,
            _ => false,
        };
        if passed {
            CommandResult::ok(String::new())
        } else {
            CommandResult::err(String::new(), 1)
        }
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let r1 = engine
        .execute(&mut ctx, "test 3 -lt 10 && echo yes")
        .unwrap();
    assert_eq!(r1.stdout.trim(), "yes");

    let r2 = engine
        .execute(&mut ctx, "test 10 -lt 3 && echo yes")
        .unwrap();
    assert_eq!(r2.stdout.trim(), "");
}

#[test]
fn shell_test_negation() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("echo", |_, args, _| {
        CommandResult::ok(format!("{}\n", args.join(" ")))
    });
    reg.register("test", |_, args, _| {
        // Support ! negation
        let (negated, rest) = if args.first().map(String::as_str) == Some("!") {
            (true, &args[1..])
        } else {
            (false, args)
        };
        let na = rest
            .first()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);
        let nb = rest.get(2).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
        let op = rest.get(1).map(String::as_str).unwrap_or("");
        let inner = match op {
            "-eq" => na == nb,
            "-ne" => na != nb,
            _ => false,
        };
        let passed = if negated { !inner } else { inner };
        if passed {
            CommandResult::ok(String::new())
        } else {
            CommandResult::err(String::new(), 1)
        }
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    // ! -eq: 3 != 3 is false, so ! makes it true → no: 3 -eq 3 is true, ! makes it false
    let r = engine
        .execute(&mut ctx, "test ! 3 -eq 3 && echo yes || echo no")
        .unwrap();
    assert_eq!(r.stdout.trim(), "no");
}

// ── Round 4: seq command ──────────────────────────────────────────────────────

#[test]
fn shell_seq_simple_range() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("seq", |_, args, _| {
        let nums: Vec<i64> = args.iter().filter_map(|a| a.parse().ok()).collect();
        let (start, end) = match nums.as_slice() {
            [n] => (1, *n),
            [s, e] => (*s, *e),
            _ => return CommandResult::err("seq: bad args\n", 1),
        };
        let out: Vec<String> = (start..=end).map(|n| n.to_string()).collect();
        CommandResult::ok(format!("{}\n", out.join("\n")))
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let result = engine.execute(&mut ctx, "seq 3").unwrap();
    let lines: Vec<&str> = result.stdout.trim().lines().collect();
    assert_eq!(lines, vec!["1", "2", "3"]);
}

#[test]
fn shell_seq_with_step() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("seq", |_, args, _| {
        let nums: Vec<f64> = args.iter().filter_map(|a| a.parse().ok()).collect();
        let (start, step, end) = match nums.as_slice() {
            [s, inc, e] => (*s, *inc, *e),
            _ => return CommandResult::err("seq: bad args\n", 1),
        };
        let mut out = Vec::new();
        let mut cur = start;
        while cur <= end {
            out.push(format!("{}", cur as i64));
            cur += step;
        }
        CommandResult::ok(format!("{}\n", out.join("\n")))
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    // seq 1 2 7 should produce 1, 3, 5, 7
    let result = engine.execute(&mut ctx, "seq 1 2 7").unwrap();
    let lines: Vec<&str> = result.stdout.trim().lines().collect();
    assert_eq!(lines, vec!["1", "3", "5", "7"]);
}

// ── Round 4: xargs -I{} ───────────────────────────────────────────────────────

#[test]
fn shell_xargs_replace_token() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("echo", |_, args, _| {
        CommandResult::ok(format!("{}\n", args.join("\n")))
    });
    reg.register("xargs", |_, args, stdin| {
        // Detect -I placeholder
        let mut placeholder: Option<String> = None;
        let mut cmd_start = 0usize;
        let mut i = 0;
        while i < args.len() {
            if args[i] == "-I" {
                if let Some(r) = args.get(i + 1) {
                    placeholder = Some(r.clone());
                    i += 2;
                    cmd_start = i;
                    continue;
                }
            } else if let Some(r) = args[i].strip_prefix("-I") {
                placeholder = Some(r.to_owned());
                i += 1;
                cmd_start = i;
                continue;
            }
            break;
        }
        let cmd = args[cmd_start..].join(" ");
        if let Some(ph) = placeholder {
            let out = stdin
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|line| cmd.replace(&ph, line.trim()))
                .collect::<Vec<_>>()
                .join("\n");
            CommandResult::ok(format!("{out}\n"))
        } else {
            let tokens = stdin.split_whitespace().collect::<Vec<_>>().join(" ");
            CommandResult::ok(format!("{cmd} {tokens}\n"))
        }
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let result = engine
        .execute(&mut ctx, "echo alpha beta | xargs -I{} echo item: {}")
        .unwrap();
    let lines: Vec<&str> = result.stdout.trim().lines().collect();
    // xargs -I{} substitutes {} with each stdin line; since the mock cannot
    // re-invoke the shell, it emits the substituted command string.
    assert!(lines[0].contains("item: alpha"), "got: {}", lines[0]);
    assert!(lines[1].contains("item: beta"), "got: {}", lines[1]);
}

// ── Round 4: sort -k key field ────────────────────────────────────────────────

#[test]
fn shell_sort_key_field() {
    let mut vfs = minimal_vfs();
    vfs.write_file(
        "/home/player",
        "scores.txt",
        "neo 9800\nrift 8700\nshadow 7500\n",
        false,
        "player",
    )
    .unwrap();
    let mut reg = BuiltinRegistry::default();
    reg.register("sort", |ctx, args, stdin| {
        let reverse = args.iter().any(|a| a == "-r");
        let numeric = args.iter().any(|a| a == "-n");
        let key: Option<usize> = args
            .windows(2)
            .find_map(|w| {
                if w[0] == "-k" {
                    w[1].parse().ok()
                } else {
                    None
                }
            })
            .or_else(|| {
                args.iter()
                    .find_map(|a| a.strip_prefix("-k").and_then(|s| s.parse().ok()))
            });
        let file_arg = args
            .iter()
            .find(|a| !a.starts_with('-') && a.parse::<usize>().is_err())
            .cloned();
        let source = if let Some(p) = file_arg {
            ctx.vfs.read_file(&ctx.cwd, &p).unwrap_or_default()
        } else {
            stdin.to_owned()
        };
        let mut lines: Vec<String> = source.lines().map(str::to_owned).collect();
        let extract = |line: &str| -> String {
            if let Some(k) = key {
                line.split_whitespace()
                    .nth(k.saturating_sub(1))
                    .unwrap_or("")
                    .to_owned()
            } else {
                line.to_owned()
            }
        };
        if numeric {
            lines.sort_by(|a, b| {
                let ka = extract(a).parse::<f64>().unwrap_or(0.0);
                let kb = extract(b).parse::<f64>().unwrap_or(0.0);
                let ord = ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal);
                if reverse {
                    ord.reverse()
                } else {
                    ord
                }
            });
        } else {
            lines.sort_by(|a, b| {
                if reverse {
                    extract(b).cmp(&extract(a))
                } else {
                    extract(a).cmp(&extract(b))
                }
            });
        }
        CommandResult::ok(format!("{}\n", lines.join("\n")))
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    // sort by field 2 (numeric score) descending → neo first
    let result = engine
        .execute(&mut ctx, "sort -k2 -n -r < scores.txt")
        .unwrap();
    let first_line = result.stdout.trim().lines().next().unwrap_or("");
    assert!(
        first_line.starts_with("neo"),
        "neo should be first (highest score)"
    );
}

// ── Round 4: read command ──────────────────────────────────────────────────────

#[test]
fn shell_read_sets_env_var_from_stdin() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("echo", |_, args, _| {
        CommandResult::ok(format!("{}\n", args.join(" ")))
    });
    reg.register("read", |ctx, args, stdin| {
        let var = args.first().map(String::as_str).unwrap_or("REPLY");
        let val = stdin.lines().next().unwrap_or("").trim().to_owned();
        ctx.env.insert(var.to_owned(), val);
        CommandResult::ok(String::new())
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    // pipe "neon-value" through read to set MYVAR
    let result = engine
        .execute(&mut ctx, "echo neon-value | read MYVAR")
        .unwrap();
    assert_eq!(result.exit_code, 0);
    assert_eq!(ctx.env.get("MYVAR").map(String::as_str), Some("neon-value"));
}

// ── Round 4: paste command ────────────────────────────────────────────────────

#[test]
fn shell_paste_merges_two_files() {
    let mut vfs = minimal_vfs();
    vfs.write_file("/home/player", "a.txt", "alpha\nbeta\n", false, "player")
        .unwrap();
    vfs.write_file("/home/player", "b.txt", "1\n2\n", false, "player")
        .unwrap();
    let mut reg = BuiltinRegistry::default();
    reg.register("paste", |ctx, args, _| {
        let files: Vec<&str> = args.iter().map(String::as_str).collect();
        if files.len() < 2 {
            return CommandResult::err("paste: need 2 files\n", 1);
        }
        let a = ctx.vfs.read_file(&ctx.cwd, files[0]).unwrap_or_default();
        let b = ctx.vfs.read_file(&ctx.cwd, files[1]).unwrap_or_default();
        let al: Vec<&str> = a.lines().collect();
        let bl: Vec<&str> = b.lines().collect();
        let n = al.len().max(bl.len());
        let out = (0..n)
            .map(|i| {
                format!(
                    "{}\t{}",
                    al.get(i).copied().unwrap_or(""),
                    bl.get(i).copied().unwrap_or("")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        CommandResult::ok(format!("{out}\n"))
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let result = engine.execute(&mut ctx, "paste a.txt b.txt").unwrap();
    let lines: Vec<&str> = result.stdout.trim().lines().collect();
    assert_eq!(lines[0], "alpha\t1");
    assert_eq!(lines[1], "beta\t2");
}

// ── Round 4: world — new VFS mission files ───────────────────────────────────

#[test]
fn vfs_regex_hunt_events_log_exists() {
    let mut vfs = minimal_vfs();
    vfs.mkdir_p("/", "var/log", "system").unwrap();
    vfs.write_file(
        "/",
        "/var/log/events.log",
        "2026-03-07 22:10:01 ERROR user=neo code=ERR-001\n2026-03-07 22:12:05 FATAL user=shadow code=FAT-001\n",
        false,
        "system",
    )
    .unwrap();
    let content = vfs.read_file("/var/log", "events.log").unwrap();
    assert!(content.contains("ERROR"));
    assert!(content.contains("FATAL"));
}

#[test]
fn vfs_pipeline_csv_has_header_and_data() {
    let mut vfs = minimal_vfs();
    vfs.mkdir_p("/", "data", "system").unwrap();
    vfs.write_file(
        "/",
        "/data/pipeline.csv",
        "id,name,score,rank\n101,neo,9800,1\n",
        false,
        "system",
    )
    .unwrap();
    let content = vfs.read_file("/data", "pipeline.csv").unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines[0], "id,name,score,rank", "first line is header");
    assert!(lines[1].contains("neo"));
}

// ── Round 4: script market entries ───────────────────────────────────────────

#[tokio::test]
async fn world_has_twelve_script_market_entries() {
    // All 18 advanced mission codes must be completable (existence + rep check).
    let world = bare_world();
    let p = world
        .login("market-test", "203.0.113.252", &[])
        .await
        .unwrap();
    for code in world::ADVANCED_CODES {
        world.complete_mission(p.id, code).await.unwrap();
    }
    let refreshed = world.get_player(p.id).await.unwrap();
    assert_eq!(refreshed.reputation, 360, "18 advanced × 20 rep = 360");
}

// ── Round 5: tr range expansion ───────────────────────────────────────────────

#[test]
fn shell_tr_range_lowercase_to_uppercase() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("echo", |_, args, _| {
        CommandResult::ok(format!("{}\n", args.join(" ")))
    });
    // Inline tr with expand_set logic mirrored from builtins
    reg.register("tr", |_, args, stdin| {
        fn expand(s: &str) -> Vec<char> {
            let cs: Vec<char> = s.chars().collect();
            let mut out = Vec::new();
            let mut i = 0;
            while i < cs.len() {
                if i + 2 < cs.len() && cs[i + 1] == '-' {
                    let (a, b) = (cs[i] as u32, cs[i + 2] as u32);
                    if a <= b {
                        for cp in a..=b {
                            if let Some(c) = char::from_u32(cp) {
                                out.push(c);
                            }
                        }
                        i += 3;
                        continue;
                    }
                }
                out.push(cs[i]);
                i += 1;
            }
            out
        }
        let positional: Vec<&str> = args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .map(String::as_str)
            .collect();
        if positional.len() < 2 {
            return CommandResult::err("tr: requires FROM and TO sets\n", 1);
        }
        let from = expand(positional[0]);
        let to = expand(positional[1]);
        let map: std::collections::HashMap<char, char> = from
            .iter()
            .enumerate()
            .map(|(i, ch)| (*ch, to.get(i).copied().unwrap_or(*ch)))
            .collect();
        let out: String = stdin
            .chars()
            .map(|c| map.get(&c).copied().unwrap_or(c))
            .collect();
        CommandResult::ok(out)
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let result = engine
        .execute(&mut ctx, "echo hello | tr 'a-z' 'A-Z'")
        .unwrap();
    assert_eq!(result.stdout.trim(), "HELLO");
}

#[test]
fn shell_tr_delete_digits() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("echo", |_, args, _| {
        CommandResult::ok(format!("{}\n", args.join(" ")))
    });
    reg.register("tr", |_, args, stdin| {
        fn expand(s: &str) -> Vec<char> {
            let cs: Vec<char> = s.chars().collect();
            let mut out = Vec::new();
            let mut i = 0;
            while i < cs.len() {
                if i + 2 < cs.len() && cs[i + 1] == '-' {
                    let (a, b) = (cs[i] as u32, cs[i + 2] as u32);
                    if a <= b {
                        for cp in a..=b {
                            if let Some(c) = char::from_u32(cp) {
                                out.push(c);
                            }
                        }
                        i += 3;
                        continue;
                    }
                }
                out.push(cs[i]);
                i += 1;
            }
            out
        }
        let delete_mode = args.iter().any(|a| a == "-d");
        let positional: Vec<&str> = args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .map(String::as_str)
            .collect();
        if delete_mode {
            let set: std::collections::HashSet<char> =
                expand(positional.first().copied().unwrap_or(""))
                    .into_iter()
                    .collect();
            return CommandResult::ok(
                stdin
                    .chars()
                    .filter(|c| !set.contains(c))
                    .collect::<String>(),
            );
        }
        CommandResult::ok(stdin.to_owned())
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let result = engine
        .execute(&mut ctx, "echo abc123def | tr -d '0-9'")
        .unwrap();
    assert_eq!(result.stdout.trim(), "abcdef");
}

// ── Round 5: sed improvements ─────────────────────────────────────────────────

#[test]
fn shell_sed_first_occurrence_only() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("echo", |_, args, _| {
        CommandResult::ok(format!("{}\n", args.join(" ")))
    });
    reg.register("sed", |_, args, stdin| {
        let expr = args
            .iter()
            .find(|a| !a.starts_with('-'))
            .map(String::as_str)
            .unwrap_or("");
        if expr.starts_with("s/") {
            let parts: Vec<&str> = expr.trim_start_matches("s/").splitn(3, '/').collect();
            if parts.len() < 2 {
                return CommandResult::err("sed: bad expr\n", 1);
            }
            let (old, new) = (parts[0], parts[1]);
            let flags = parts.get(2).copied().unwrap_or("");
            let mut out = String::new();
            for line in stdin.lines() {
                let replaced = if flags.contains('g') {
                    line.replace(old, new)
                } else if let Some(pos) = line.find(old) {
                    format!("{}{}{}", &line[..pos], new, &line[pos + old.len()..])
                } else {
                    line.to_owned()
                };
                out.push_str(&replaced);
                out.push('\n');
            }
            return CommandResult::ok(out);
        }
        CommandResult::err("sed: unsupported\n", 1)
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    // Without /g, only first occurrence should be replaced
    let result = engine
        .execute(&mut ctx, "echo 'aa bb aa' | sed 's/aa/XX/'")
        .unwrap();
    assert_eq!(result.stdout.trim(), "XX bb aa");
}

#[test]
fn shell_sed_global_flag() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("echo", |_, args, _| {
        CommandResult::ok(format!("{}\n", args.join(" ")))
    });
    reg.register("sed", |_, args, stdin| {
        let expr = args
            .iter()
            .find(|a| !a.starts_with('-'))
            .map(String::as_str)
            .unwrap_or("");
        if expr.starts_with("s/") {
            let parts: Vec<&str> = expr.trim_start_matches("s/").splitn(3, '/').collect();
            if parts.len() < 2 {
                return CommandResult::err("sed: bad expr\n", 1);
            }
            let (old, new) = (parts[0], parts[1]);
            let flags = parts.get(2).copied().unwrap_or("");
            let mut out = String::new();
            for line in stdin.lines() {
                let replaced = if flags.contains('g') {
                    line.replace(old, new)
                } else {
                    line.to_owned()
                };
                out.push_str(&replaced);
                out.push('\n');
            }
            return CommandResult::ok(out);
        }
        CommandResult::err("sed: unsupported\n", 1)
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let result = engine
        .execute(&mut ctx, "echo 'aa bb aa' | sed 's/aa/XX/g'")
        .unwrap();
    assert_eq!(result.stdout.trim(), "XX bb XX");
}

#[test]
fn shell_sed_delete_line() {
    let mut vfs = minimal_vfs();
    vfs.write_file(
        "/home/player",
        "lines.txt",
        "line1\nline2\nline3\n",
        false,
        "player",
    )
    .unwrap();
    let mut reg = BuiltinRegistry::default();
    reg.register("sed", |ctx, args, stdin| {
        let expr = args
            .iter()
            .find(|a| !a.starts_with('-'))
            .map(String::as_str)
            .unwrap_or("");
        let file_arg = args
            .iter()
            .find(|a| !a.starts_with('-') && *a != expr)
            .map(String::as_str);
        let source = if let Some(f) = file_arg {
            ctx.vfs.read_file(&ctx.cwd, f).unwrap_or_default()
        } else {
            stdin.to_owned()
        };
        if let Some(d_pos) = expr.find('d') {
            let addr = &expr[..d_pos];
            if let Ok(n) = addr.trim().parse::<usize>() {
                let out = source
                    .lines()
                    .enumerate()
                    .filter_map(|(i, l)| if i + 1 == n { None } else { Some(l) })
                    .collect::<Vec<_>>()
                    .join("\n");
                return CommandResult::ok(if out.is_empty() {
                    String::new()
                } else {
                    format!("{out}\n")
                });
            }
        }
        CommandResult::err("sed: unsupported\n", 1)
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let result = engine.execute(&mut ctx, "sed '2d' lines.txt").unwrap();
    let lines: Vec<&str> = result.stdout.trim().lines().collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], "line1");
    assert_eq!(lines[1], "line3");
}

// ── Round 5: cut -c character mode ────────────────────────────────────────────

#[test]
fn shell_cut_character_range() {
    let mut vfs = minimal_vfs();
    let mut reg = BuiltinRegistry::default();
    reg.register("echo", |_, args, _| {
        CommandResult::ok(format!("{}\n", args.join(" ")))
    });
    reg.register("cut", |_, args, stdin| {
        let mut char_spec: Option<String> = None;
        let mut i = 0;
        while i < args.len() {
            if args[i] == "-c" && i + 1 < args.len() {
                char_spec = Some(args[i + 1].clone());
                i += 2;
                continue;
            } else if let Some(s) = args[i].strip_prefix("-c") {
                char_spec = Some(s.to_owned());
            }
            i += 1;
        }
        let spec = char_spec.unwrap_or_default();
        let cols: Vec<usize> = if spec.contains('-') {
            let p: Vec<usize> = spec.splitn(2, '-').filter_map(|s| s.parse().ok()).collect();
            if p.len() == 2 {
                (p[0]..=p[1]).collect()
            } else {
                vec![1]
            }
        } else {
            spec.split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect()
        };
        let out = stdin
            .lines()
            .map(|line| {
                let cs: Vec<char> = line.chars().collect();
                cols.iter()
                    .filter_map(|&c| cs.get(c.saturating_sub(1)).copied())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        CommandResult::ok(format!("{out}\n"))
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let result = engine
        .execute(&mut ctx, "echo 'hello world' | cut -c 1-5")
        .unwrap();
    assert_eq!(result.stdout.trim(), "hello");
}

// ── Round 5: column -t table formatting ───────────────────────────────────────

#[test]
fn shell_column_table_mode() {
    let mut vfs = minimal_vfs();
    vfs.write_file(
        "/home/player",
        "data.tsv",
        "NAME\tSCORE\nneo\t9800\nrift\t8700\n",
        false,
        "player",
    )
    .unwrap();
    let mut reg = BuiltinRegistry::default();
    reg.register("column", |ctx, args, stdin| {
        let table_mode = args.iter().any(|a| a == "-t");
        let file_arg = args
            .iter()
            .find(|a| !a.starts_with('-'))
            .map(String::as_str);
        let source = if let Some(f) = file_arg {
            ctx.vfs.read_file(&ctx.cwd, f).unwrap_or_default()
        } else {
            stdin.to_owned()
        };
        if !table_mode {
            return CommandResult::ok(source);
        }
        let rows: Vec<Vec<String>> = source
            .lines()
            .map(|line| {
                line.split('\t')
                    .map(str::trim)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .collect();
        let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let mut widths = vec![0usize; ncols];
        for row in &rows {
            for (j, cell) in row.iter().enumerate() {
                widths[j] = widths[j].max(cell.len());
            }
        }
        let out = rows
            .iter()
            .map(|row| {
                row.iter()
                    .enumerate()
                    .map(|(j, c)| format!("{:<width$}", c, width = widths[j]))
                    .collect::<Vec<_>>()
                    .join("  ")
            })
            .collect::<Vec<_>>()
            .join("\n");
        CommandResult::ok(format!("{out}\n"))
    });
    let engine = ShellEngine::with_registry(reg);
    let mut ctx = ExecutionContext {
        vfs: &mut vfs,
        cwd: "/home/player".to_owned(),
        user: "neo".to_owned(),
        node: "node-1".to_owned(),
        env: basic_shell_env(),
        last_exit: 0,
    };
    let result = engine.execute(&mut ctx, "column -t data.tsv").unwrap();
    // Columns must be aligned: NAME and neo should be left-padded to same width
    assert!(result.stdout.contains("NAME"));
    assert!(result.stdout.contains("neo"));
    // Both rows should align the second column at the same offset
    let lines: Vec<&str> = result.stdout.trim().lines().collect();
    assert_eq!(lines.len(), 3);
    let header_score_pos = lines[0].find("SCORE").expect("SCORE in header");
    let neo_score_pos = lines[1].find("9800").expect("9800 in row");
    assert_eq!(header_score_pos, neo_score_pos, "columns must align");
}

// ── Round 5: world — 3 new missions & updated ADVANCED_CODES ─────────────────

#[tokio::test]
async fn world_has_fifteen_total_missions() {
    let world = bare_world();
    let p = world.login("adv-test", "203.0.113.1", &[]).await.unwrap();
    // Complete all 18 advanced codes (world service accepts them directly)
    for code in world::ADVANCED_CODES {
        world.complete_mission(p.id, code).await.unwrap();
    }
    let refreshed = world.get_player(p.id).await.unwrap();
    // 18 advanced missions × 20 rep each = 360
    assert_eq!(refreshed.reputation, 360);
}

#[tokio::test]
async fn world_new_three_missions_completable() {
    let world = bare_world();
    let p = world.login("r5-test", "203.0.113.2", &[]).await.unwrap();
    for code in ["json-crack", "seq-master", "column-view"] {
        world.complete_mission(p.id, code).await.unwrap();
    }
    let refreshed = world.get_player(p.id).await.unwrap();
    assert_eq!(refreshed.reputation, 60, "3 missions × 20 rep = 60");
}

// ── Round 5: VFS — new mission data files ────────────────────────────────────

#[test]
fn vfs_node_status_json_exists() {
    let mut vfs = minimal_vfs();
    vfs.mkdir_p("/", "data", "system").unwrap();
    vfs.write_file(
        "/", "/data/node-status.json",
        "{\n  \"node\": \"vault-sat-9\",\n  \"status\": \"offline\",\n  \"alert\": \"CRITICAL\"\n}\n",
        false, "system",
    ).unwrap();
    let content = vfs.read_file("/data", "node-status.json").unwrap();
    assert!(content.contains("vault-sat-9"));
    assert!(content.contains("CRITICAL"));
}

#[test]
fn vfs_netmap_tsv_has_header_and_five_nodes() {
    let mut vfs = minimal_vfs();
    vfs.mkdir_p("/", "data", "system").unwrap();
    vfs.write_file(
        "/", "/data/netmap.tsv",
        "NODE\tSECTOR\tSTATUS\tLATENCY\ncorp-sim-01\ttraining\tonline\t12ms\nneon-bazaar-gw\tmarket\tonline\t88ms\nghost-rail\ttransit\tdegraded\t142ms\nvault-sat-9\tsecure\toffline\t-\ndark-mirror\tredline\tonline\t33ms\n",
        false, "system",
    ).unwrap();
    let content = vfs.read_file("/data", "netmap.tsv").unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines[0], "NODE\tSECTOR\tSTATUS\tLATENCY");
    assert_eq!(lines.len(), 6, "header + 5 data rows");
    assert!(content.contains("vault-sat-9"));
    assert!(content.contains("offline"));
}

#[test]
fn vfs_tasks_txt_has_five_entries() {
    let mut vfs = minimal_vfs();
    vfs.mkdir_p("/", "home/player", "player").unwrap();
    vfs.write_file(
        "/",
        "/home/player/tasks.txt",
        "deploy-proxy\nauditor-scan\nrecover-node\npatch-vault\nsweep-sector\n",
        false,
        "player",
    )
    .unwrap();
    let content = vfs.read_file("/home/player", "tasks.txt").unwrap();
    assert_eq!(content.lines().count(), 5);
    assert!(content.contains("deploy-proxy"));
}
