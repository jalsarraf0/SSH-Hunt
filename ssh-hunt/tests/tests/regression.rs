#![forbid(unsafe_code)]

use std::collections::HashMap;

use shell::{BuiltinRegistry, CommandResult, ExecutionContext, ShellEngine};
use ssh_hunt_scripts::{ScriptContext, ScriptEngine, ScriptPolicy};
use vfs::Vfs;
use world::{HiddenOpsConfig, WorldService};

#[tokio::test]
async fn full_gate_progression_flow() {
    let world = WorldService::new(
        None,
        HiddenOpsConfig {
            secret_mission: None,
            telegram: None,
        },
    );

    let player = world.login("neo", "203.0.113.50", &[]).await.unwrap();
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

    let gate = world
        .netcity_gate_reason(player.id, &["SHA256:dummy".to_owned()])
        .await
        .unwrap();
    assert!(gate.is_some());
}

#[test]
fn shell_pipeline_regression() {
    let mut vfs = Vfs::default();
    vfs.mkdir_p("/", "home/player", "player").unwrap();
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
        user: "neo@203.0.113.9".to_owned(),
        node: "node-1".to_owned(),
        env: HashMap::from([
            ("USER".to_owned(), "neo".to_owned()),
            ("HOME".to_owned(), "/home/player".to_owned()),
            ("PWD".to_owned(), "/home/player".to_owned()),
            ("PATH".to_owned(), "/bin:/usr/bin".to_owned()),
            ("?".to_owned(), "0".to_owned()),
        ]),
        last_exit: 0,
    };

    let out = engine
        .execute(&mut ctx, "echo neon-grid | grep neon")
        .unwrap();
    assert_eq!(out.stdout.trim(), "neon-grid");
}

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
