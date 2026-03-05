#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};

use shell::{BuiltinRegistry, CommandResult};

pub fn default_registry() -> BuiltinRegistry {
    let mut reg = BuiltinRegistry::default();

    reg.register("pwd", |ctx, _, _| {
        CommandResult::ok(format!("{}\n", ctx.cwd))
    });

    reg.register("cd", |ctx, args, _| {
        let target = args.first().map(String::as_str).unwrap_or("/");
        match ctx.vfs.cd(&ctx.cwd, target) {
            Ok(path) => {
                ctx.cwd = path;
                ctx.env.insert("PWD".to_owned(), ctx.cwd.clone());
                CommandResult::ok(String::new())
            }
            Err(err) => CommandResult::err(format!("cd: {err}\n"), 1),
        }
    });

    reg.register("ls", |ctx, args, _| {
        let arg = args.first().map(String::as_str);
        match ctx.vfs.ls(&ctx.cwd, arg) {
            Ok(entries) => CommandResult::ok(format!("{}\n", entries.join("  "))),
            Err(err) => CommandResult::err(format!("ls: {err}\n"), 1),
        }
    });

    reg.register("cat", |ctx, args, stdin| {
        if args.is_empty() {
            return CommandResult::ok(stdin.to_owned());
        }
        let mut out = String::new();
        for path in args {
            match ctx.vfs.read_file(&ctx.cwd, path) {
                Ok(content) => out.push_str(&content),
                Err(err) => return CommandResult::err(format!("cat: {err}\n"), 1),
            }
        }
        CommandResult::ok(out)
    });

    reg.register("less", |_ctx, _args, stdin| {
        CommandResult::ok(stdin.to_owned())
    });

    reg.register("head", |_ctx, args, stdin| {
        let n = args
            .first()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(10);
        let out = stdin.lines().take(n).collect::<Vec<_>>().join("\n");
        CommandResult::ok(format!("{out}\n"))
    });

    reg.register("tail", |_ctx, args, stdin| {
        let n = args
            .first()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(10);
        let lines = stdin.lines().collect::<Vec<_>>();
        let start = lines.len().saturating_sub(n);
        let out = lines[start..].join("\n");
        CommandResult::ok(format!("{out}\n"))
    });

    reg.register("touch", |ctx, args, _| {
        if args.is_empty() {
            return CommandResult::err("touch: missing operand\n", 1);
        }
        for path in args {
            if let Err(err) = ctx.vfs.touch(&ctx.cwd, path, &ctx.user) {
                return CommandResult::err(format!("touch: {err}\n"), 1);
            }
        }
        CommandResult::ok(String::new())
    });

    reg.register("mkdir", |ctx, args, _| {
        if args.is_empty() {
            return CommandResult::err("mkdir: missing operand\n", 1);
        }
        for path in args {
            if let Err(err) = ctx.vfs.mkdir_p(&ctx.cwd, path, &ctx.user) {
                return CommandResult::err(format!("mkdir: {err}\n"), 1);
            }
        }
        CommandResult::ok(String::new())
    });

    reg.register("rm", |ctx, args, _| {
        if args.is_empty() {
            return CommandResult::err("rm: missing operand\n", 1);
        }
        for path in args {
            if let Err(err) = ctx.vfs.remove(&ctx.cwd, path) {
                return CommandResult::err(format!("rm: {err}\n"), 1);
            }
        }
        CommandResult::ok(String::new())
    });

    reg.register("cp", |ctx, args, _| {
        if args.len() != 2 {
            return CommandResult::err("cp: expected source and destination\n", 1);
        }
        match ctx.vfs.copy(&ctx.cwd, &args[0], &args[1]) {
            Ok(()) => CommandResult::ok(String::new()),
            Err(err) => CommandResult::err(format!("cp: {err}\n"), 1),
        }
    });

    reg.register("mv", |ctx, args, _| {
        if args.len() != 2 {
            return CommandResult::err("mv: expected source and destination\n", 1);
        }
        match ctx.vfs.mv(&ctx.cwd, &args[0], &args[1]) {
            Ok(()) => CommandResult::ok(String::new()),
            Err(err) => CommandResult::err(format!("mv: {err}\n"), 1),
        }
    });

    reg.register("echo", |_ctx, args, _| {
        CommandResult::ok(format!("{}\n", args.join(" ")))
    });

    reg.register("printf", |_ctx, args, _| {
        if args.is_empty() {
            return CommandResult::ok(String::new());
        }
        let fmt = &args[0];
        let joined = args[1..].join(" ");
        let out = fmt.replace("%s", &joined).replace("\\n", "\n");
        CommandResult::ok(out)
    });

    reg.register("grep", |_ctx, args, stdin| {
        if args.is_empty() {
            return CommandResult::err("grep: missing pattern\n", 1);
        }
        let pat = &args[0];
        let out = stdin
            .lines()
            .filter(|line| line.contains(pat))
            .collect::<Vec<_>>()
            .join("\n");
        CommandResult::ok(format!("{out}\n"))
    });

    reg.register("find", |ctx, args, _| {
        let root = args.first().map(String::as_str).unwrap_or(".");
        let pattern = args.windows(2).find_map(|w| {
            if w[0] == "-name" {
                Some(w[1].as_str())
            } else {
                None
            }
        });
        match ctx.vfs.find(&ctx.cwd, root, pattern) {
            Ok(found) => CommandResult::ok(format!("{}\n", found.join("\n"))),
            Err(err) => CommandResult::err(format!("find: {err}\n"), 1),
        }
    });

    reg.register("xargs", |_ctx, args, stdin| {
        let cmd = args.first().cloned().unwrap_or_else(|| "echo".to_owned());
        let tail = stdin.split_whitespace().collect::<Vec<_>>().join(" ");
        CommandResult::ok(format!("{cmd} {tail}\n"))
    });

    reg.register("sort", |_ctx, args, stdin| {
        let mut lines = stdin.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
        if args.iter().any(|a| a == "-r") {
            lines.sort_by(|a, b| b.cmp(a));
        } else {
            lines.sort();
        }
        CommandResult::ok(format!("{}\n", lines.join("\n")))
    });

    reg.register("uniq", |_ctx, _, stdin| {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for line in stdin.lines() {
            if seen.insert(line.to_owned()) {
                out.push(line.to_owned());
            }
        }
        CommandResult::ok(format!("{}\n", out.join("\n")))
    });

    reg.register("wc", |_ctx, args, stdin| {
        let lines = stdin.lines().count();
        let words = stdin.split_whitespace().count();
        let bytes = stdin.len();

        if args.iter().any(|a| a == "-l") {
            return CommandResult::ok(format!("{lines}\n"));
        }
        if args.iter().any(|a| a == "-w") {
            return CommandResult::ok(format!("{words}\n"));
        }
        if args.iter().any(|a| a == "-c") {
            return CommandResult::ok(format!("{bytes}\n"));
        }
        CommandResult::ok(format!("{lines} {words} {bytes}\n"))
    });

    reg.register("cut", |_ctx, args, stdin| {
        let mut delimiter = "\t";
        let mut field: usize = 1;
        let mut i = 0usize;
        while i < args.len() {
            match args[i].as_str() {
                "-d" if i + 1 < args.len() => {
                    delimiter = &args[i + 1];
                    i += 1;
                }
                "-f" if i + 1 < args.len() => {
                    field = args[i + 1].parse::<usize>().unwrap_or(1);
                    i += 1;
                }
                _ => {}
            }
            i += 1;
        }

        let out = stdin
            .lines()
            .map(|line| {
                line.split(delimiter)
                    .nth(field.saturating_sub(1))
                    .unwrap_or("")
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n");
        CommandResult::ok(format!("{out}\n"))
    });

    reg.register("tr", |_ctx, args, stdin| {
        if args.len() < 2 {
            return CommandResult::err("tr: requires FROM and TO sets\n", 1);
        }
        let from = args[0].chars().collect::<Vec<_>>();
        let to = args[1].chars().collect::<Vec<_>>();
        let map = from
            .iter()
            .enumerate()
            .map(|(idx, ch)| (*ch, to.get(idx).copied().unwrap_or(*ch)))
            .collect::<HashMap<_, _>>();
        let out = stdin
            .chars()
            .map(|c| map.get(&c).copied().unwrap_or(c))
            .collect::<String>();
        CommandResult::ok(out)
    });

    reg.register("sed", |_ctx, args, stdin| {
        if args.is_empty() {
            return CommandResult::err("sed: expected s/old/new/\n", 1);
        }
        let expr = &args[0];
        if !expr.starts_with("s/") {
            return CommandResult::err("sed: only s/old/new/ subset supported\n", 1);
        }
        let parts = expr.trim_start_matches("s/").split('/').collect::<Vec<_>>();
        if parts.len() < 2 {
            return CommandResult::err("sed: invalid expression\n", 1);
        }
        let out = stdin.replace(parts[0], parts[1]);
        CommandResult::ok(out)
    });

    reg.register("awk", |_ctx, args, stdin| {
        if args.is_empty() {
            return CommandResult::err("awk: only '{print $N}' subset supported\n", 1);
        }
        let expr = args.join(" ");
        let column = expr
            .split('$')
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(1);
        let out = stdin
            .lines()
            .map(|line| {
                line.split_whitespace()
                    .nth(column.saturating_sub(1))
                    .unwrap_or("")
            })
            .collect::<Vec<_>>()
            .join("\n");
        CommandResult::ok(format!("{out}\n"))
    });

    reg.register("ps", |_ctx, _args, _| {
        CommandResult::ok("PID TTY      STAT   TIME COMMAND\n101 pts/0    S+     0:00 shell-sim\n202 pts/0    S+     0:00 net-proxy-sim\n")
    });
    reg.register("top", |_ctx, _args, _| {
        CommandResult::ok("top - 22:22:22 up 13 days,  2 users, load average: 0.42, 0.37, 0.31\nTasks: 17 total\n")
    });
    reg.register("uname", |_ctx, _args, _| {
        CommandResult::ok("Linux neon-grid 6.66.0-ssh-hunt #1 SMP PREEMPT\n")
    });
    reg.register("whoami", |ctx, _args, _| {
        CommandResult::ok(format!("{}\n", ctx.user))
    });
    reg.register("id", |ctx, _args, _| {
        CommandResult::ok(format!(
            "uid=1000({}) gid=1000({}) groups=1000({})\n",
            ctx.user, ctx.user, ctx.user
        ))
    });
    reg.register("df", |_ctx, _args, _| CommandResult::ok("Filesystem      Size  Used Avail Use% Mounted on\nvfs://root       64G   20G   44G  32% /\n"));
    reg.register("free", |_ctx, _args, _| CommandResult::ok("              total        used        free\nMem:        8388608     3145728     5242880\n"));
    reg.register("ip", |_ctx, _args, _| CommandResult::ok("2: eth0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500\n    inet 10.77.0.15/24 brd 10.77.0.255 scope global eth0\n"));
    reg.register("ss", |_ctx, _args, _| CommandResult::ok("Netid State   Recv-Q Send-Q Local Address:Port Peer Address:Port\ntcp   LISTEN  0      128    10.77.0.15:443  0.0.0.0:*\n"));

    reg
}
