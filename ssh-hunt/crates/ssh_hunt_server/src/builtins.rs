#![forbid(unsafe_code)]

use std::collections::HashMap;

use regex::Regex;
use shell::{BuiltinRegistry, CommandResult, ExecutionContext};
use vfs::NodeKind;

/// Parse `-n N` or bare `N` from args, returning the count or `default`.
fn parse_n_flag(args: &[String], default: usize) -> usize {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-n" {
            if let Some(v) = args.get(i + 1).and_then(|s| s.parse::<usize>().ok()) {
                return v;
            }
        } else if let Some(stripped) = args[i].strip_prefix("-n") {
            if let Ok(v) = stripped.parse::<usize>() {
                return v;
            }
        } else if let Ok(v) = args[i].parse::<usize>() {
            return v;
        }
        i += 1;
    }
    default
}

fn eval_test_expr(ctx: &mut ExecutionContext<'_>, args: &[String]) -> bool {
    if args.is_empty() {
        return false;
    }
    // ! negation
    if args[0] == "!" {
        return !eval_test_expr(ctx, &args[1..]);
    }
    // -a compound AND (split on first -a)
    if let Some(pos) = args.iter().position(|a| a == "-a") {
        return eval_test_expr(ctx, &args[..pos]) && eval_test_expr(ctx, &args[pos + 1..]);
    }
    // -o compound OR (split on first -o)
    if let Some(pos) = args.iter().position(|a| a == "-o") {
        return eval_test_expr(ctx, &args[..pos]) || eval_test_expr(ctx, &args[pos + 1..]);
    }
    match args {
        [f, p] if f == "-f" => ctx
            .vfs
            .stat(&ctx.cwd, p)
            .is_ok_and(|n| n.kind == NodeKind::File),
        [f, p] if f == "-d" => ctx
            .vfs
            .stat(&ctx.cwd, p)
            .is_ok_and(|n| n.kind == NodeKind::Dir),
        [f, p] if f == "-e" => ctx.vfs.stat(&ctx.cwd, p).is_ok(),
        [f, s] if f == "-z" => s.is_empty(),
        [f, s] if f == "-n" => !s.is_empty(),
        [a, op, b] if op == "=" || op == "==" => a == b,
        [a, op, b] if op == "!=" => a != b,
        [a, op, b] => {
            // Numeric comparisons
            let na = a.trim().parse::<i64>();
            let nb = b.trim().parse::<i64>();
            if let (Ok(na), Ok(nb)) = (na, nb) {
                match op.as_str() {
                    "-lt" => na < nb,
                    "-le" => na <= nb,
                    "-gt" => na > nb,
                    "-ge" => na >= nb,
                    "-eq" => na == nb,
                    "-ne" => na != nb,
                    _ => false,
                }
            } else {
                false
            }
        }
        _ => false,
    }
}

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
        let long_fmt = args
            .iter()
            .any(|a| a == "-l" || a == "-la" || a == "-al" || a == "-lh");
        let path_arg = args
            .iter()
            .find(|a| !a.starts_with('-'))
            .map(String::as_str);

        if long_fmt {
            match ctx.vfs.ls_nodes(&ctx.cwd, path_arg) {
                Ok(nodes) => {
                    let mut lines = Vec::new();
                    for node in &nodes {
                        let name = node.path.rsplit('/').next().unwrap_or(&node.path);
                        let kind_ch = if node.kind == NodeKind::Dir { 'd' } else { '-' };
                        let m = node.meta.perms.mode;
                        let perms = format!(
                            "{}{}{}{}{}{}{}{}{}{} ",
                            kind_ch,
                            if m & 0o400 != 0 { 'r' } else { '-' },
                            if m & 0o200 != 0 { 'w' } else { '-' },
                            if m & 0o100 != 0 { 'x' } else { '-' },
                            if m & 0o040 != 0 { 'r' } else { '-' },
                            if m & 0o020 != 0 { 'w' } else { '-' },
                            if m & 0o010 != 0 { 'x' } else { '-' },
                            if m & 0o004 != 0 { 'r' } else { '-' },
                            if m & 0o002 != 0 { 'w' } else { '-' },
                            if m & 0o001 != 0 { 'x' } else { '-' },
                        );
                        let size = node.content.as_ref().map_or(0, |c| c.len());
                        let date = node.meta.updated_at.format("%b %d %H:%M");
                        lines.push(format!(
                            "{perms}{:<8} {:>6} {date} {name}",
                            node.meta.owner, size
                        ));
                    }
                    CommandResult::ok(format!("{}\n", lines.join("\n")))
                }
                Err(err) => CommandResult::err(format!("ls: {err}\n"), 1),
            }
        } else {
            match ctx.vfs.ls(&ctx.cwd, path_arg) {
                Ok(entries) => CommandResult::ok(format!("{}\n", entries.join("  "))),
                Err(err) => CommandResult::err(format!("ls: {err}\n"), 1),
            }
        }
    });

    reg.register("cat", |ctx, args, stdin| {
        let number_lines = args.iter().any(|a| a == "-n");
        let file_args: Vec<&str> = args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .map(String::as_str)
            .collect();

        let content = if file_args.is_empty() {
            stdin.to_owned()
        } else {
            let mut out = String::new();
            for path in &file_args {
                match ctx.vfs.read_file(&ctx.cwd, path) {
                    Ok(c) => out.push_str(&c),
                    Err(err) => return CommandResult::err(format!("cat: {err}\n"), 1),
                }
            }
            out
        };

        if number_lines {
            let numbered = content
                .lines()
                .enumerate()
                .map(|(i, line)| format!("{:6}\t{line}", i + 1))
                .collect::<Vec<_>>()
                .join("\n");
            CommandResult::ok(format!("{numbered}\n"))
        } else {
            CommandResult::ok(content)
        }
    });

    reg.register("less", |_ctx, _args, stdin| {
        CommandResult::ok(stdin.to_owned())
    });

    reg.register("head", |ctx, args, stdin| {
        let n = parse_n_flag(args, 10);
        // last non-flag arg is an optional file path
        let file_arg = args
            .iter()
            .find(|a| !a.starts_with('-') && a.parse::<usize>().is_err());
        let source = if let Some(path) = file_arg {
            match ctx.vfs.read_file(&ctx.cwd, path) {
                Ok(c) => c,
                Err(e) => return CommandResult::err(format!("head: {e}\n"), 1),
            }
        } else {
            stdin.to_owned()
        };
        let out = source.lines().take(n).collect::<Vec<_>>().join("\n");
        CommandResult::ok(format!("{out}\n"))
    });

    reg.register("tail", |ctx, args, stdin| {
        let n = parse_n_flag(args, 10);
        let file_arg = args
            .iter()
            .find(|a| !a.starts_with('-') && a.parse::<usize>().is_err());
        let source = if let Some(path) = file_arg {
            match ctx.vfs.read_file(&ctx.cwd, path) {
                Ok(c) => c,
                Err(e) => return CommandResult::err(format!("tail: {e}\n"), 1),
            }
        } else {
            stdin.to_owned()
        };
        let lines = source.lines().collect::<Vec<_>>();
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
        // Strip flags (e.g. -r, -rf) so they are not treated as paths.
        let paths: Vec<&str> = args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .map(String::as_str)
            .collect();
        if paths.is_empty() {
            return CommandResult::err("rm: missing operand\n", 1);
        }
        for path in paths {
            // vfs.remove() already handles directories recursively.
            if let Err(err) = ctx.vfs.remove(&ctx.cwd, path) {
                return CommandResult::err(format!("rm: {err}\n"), 1);
            }
        }
        CommandResult::ok(String::new())
    });

    reg.register("cp", |ctx, args, _| {
        let recursive = args.iter().any(|a| a == "-r" || a == "-R" || a == "-rf");
        let positional: Vec<&str> = args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .map(String::as_str)
            .collect();
        if positional.len() != 2 {
            return CommandResult::err("cp: expected source and destination\n", 1);
        }
        let result = if recursive {
            ctx.vfs.copy_tree(&ctx.cwd, positional[0], positional[1])
        } else {
            ctx.vfs.copy(&ctx.cwd, positional[0], positional[1])
        };
        match result {
            Ok(()) => CommandResult::ok(String::new()),
            Err(err) => CommandResult::err(format!("cp: {err}\n"), 1),
        }
    });

    reg.register("mv", |ctx, args, _| {
        let positional: Vec<&str> = args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .map(String::as_str)
            .collect();
        if positional.len() != 2 {
            return CommandResult::err("mv: expected source and destination\n", 1);
        }
        match ctx.vfs.mv(&ctx.cwd, positional[0], positional[1]) {
            Ok(()) => CommandResult::ok(String::new()),
            Err(err) => CommandResult::err(format!("mv: {err}\n"), 1),
        }
    });

    reg.register("echo", |_ctx, args, _| {
        let no_newline = args.iter().any(|a| a == "-n");
        let interp = args.iter().any(|a| a == "-e");
        let words: Vec<&str> = args
            .iter()
            .filter(|a| *a != "-n" && *a != "-e")
            .map(String::as_str)
            .collect();
        let raw = words.join(" ");
        let content = if interp {
            raw.replace("\\\\", "\x00")
                .replace("\\n", "\n")
                .replace("\\t", "\t")
                .replace("\\r", "\r")
                .replace('\x00', "\\")
        } else {
            raw
        };
        if no_newline {
            CommandResult::ok(content)
        } else {
            CommandResult::ok(format!("{content}\n"))
        }
    });

    reg.register("printf", |_ctx, args, _| {
        if args.is_empty() {
            return CommandResult::ok(String::new());
        }
        let fmt = &args[0];
        let fmt_args = &args[1..];
        // Pre-process escape sequences in format string (handle \\\\ first via sentinel
        // to prevent \\n from being consumed as a newline escape).
        let fmt_str = fmt
            .replace("\\\\", "\x00")
            .replace("\\n", "\n")
            .replace("\\t", "\t")
            .replace("\\r", "\r")
            .replace('\x00', "\\");
        let mut out = String::new();
        let mut arg_idx = 0usize;
        let mut chars = fmt_str.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch != '%' {
                out.push(ch);
                continue;
            }
            match chars.peek().copied() {
                Some('s') => {
                    chars.next();
                    out.push_str(fmt_args.get(arg_idx).map(String::as_str).unwrap_or(""));
                    arg_idx += 1;
                }
                Some('d') | Some('i') => {
                    chars.next();
                    let n = fmt_args
                        .get(arg_idx)
                        .and_then(|s| s.trim().parse::<i64>().ok())
                        .unwrap_or(0);
                    out.push_str(&n.to_string());
                    arg_idx += 1;
                }
                Some('f') => {
                    chars.next();
                    let n = fmt_args
                        .get(arg_idx)
                        .and_then(|s| s.trim().parse::<f64>().ok())
                        .unwrap_or(0.0);
                    out.push_str(&format!("{n:.6}"));
                    arg_idx += 1;
                }
                Some('%') => {
                    chars.next();
                    out.push('%');
                }
                _ => out.push('%'),
            }
        }
        CommandResult::ok(out)
    });

    reg.register("grep", |ctx, args, stdin| {
        if args.is_empty() {
            return CommandResult::err("grep: missing pattern\n", 1);
        }

        let case_insensitive = args.iter().any(|a| a == "-i");
        let invert = args.iter().any(|a| a == "-v");
        let show_line_nums = args.iter().any(|a| a == "-n");
        let count_mode = args.iter().any(|a| a == "-c");
        let regex_mode = args.iter().any(|a| a == "-E" || a == "-P");

        // Collect non-flag args: pattern then optional file.
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
            match ctx.vfs.read_file(&ctx.cwd, path) {
                Ok(c) => c,
                Err(e) => return CommandResult::err(format!("grep: {e}\n"), 1),
            }
        } else {
            stdin.to_owned()
        };

        // Build regex if -E/-P, else fall back to literal contains
        let compiled_re: Option<Regex> = if regex_mode {
            let re_pat = if case_insensitive {
                format!("(?i){pat}")
            } else {
                pat.to_owned()
            };
            match Regex::new(&re_pat) {
                Ok(r) => Some(r),
                Err(e) => return CommandResult::err(format!("grep: invalid regex: {e}\n"), 1),
            }
        } else {
            None
        };

        let pat_lower = pat.to_ascii_lowercase();
        let mut out_lines = Vec::new();
        for (idx, line) in source.lines().enumerate() {
            let matched = if let Some(ref re) = compiled_re {
                re.is_match(line)
            } else {
                let haystack = if case_insensitive {
                    line.to_ascii_lowercase()
                } else {
                    line.to_owned()
                };
                let needle: &str = if case_insensitive { &pat_lower } else { pat };
                haystack.contains(needle)
            };
            let include = if invert { !matched } else { matched };
            if include {
                if show_line_nums {
                    out_lines.push(format!("{}:{}", idx + 1, line));
                } else {
                    out_lines.push(line.to_owned());
                }
            }
        }

        if count_mode {
            return CommandResult::ok(format!("{}\n", out_lines.len()));
        }

        let exit_code = if out_lines.is_empty() { 1 } else { 0 };
        let out = out_lines.join("\n");
        if exit_code == 0 {
            CommandResult::ok(format!("{out}\n"))
        } else {
            CommandResult::err(String::new(), 1)
        }
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
        // -type f = files only, -type d = directories only
        let type_filter = args.windows(2).find_map(|w| {
            if w[0] == "-type" {
                Some(w[1].as_str())
            } else {
                None
            }
        });
        match ctx.vfs.find(&ctx.cwd, root, pattern) {
            Ok(found) => {
                let filtered: Vec<&str> = found
                    .iter()
                    .filter(|path| match type_filter {
                        None => true,
                        Some("f") => ctx
                            .vfs
                            .stat("/", path)
                            .is_ok_and(|n| n.kind == NodeKind::File),
                        Some("d") => ctx
                            .vfs
                            .stat("/", path)
                            .is_ok_and(|n| n.kind == NodeKind::Dir),
                        _ => true,
                    })
                    .map(String::as_str)
                    .collect();
                CommandResult::ok(format!("{}\n", filtered.join("\n")))
            }
            Err(err) => CommandResult::err(format!("find: {err}\n"), 1),
        }
    });

    reg.register("xargs", |_ctx, args, stdin| {
        // Parse -I REPLACE_STR (e.g. -I{} or -I PLACEHOLDER)
        let mut replace_str: Option<String> = None;
        let mut cmd_start = 0usize;
        let mut i = 0usize;
        while i < args.len() {
            if args[i] == "-I" {
                if let Some(r) = args.get(i + 1) {
                    replace_str = Some(r.clone());
                    i += 2;
                    cmd_start = i;
                    continue;
                }
            } else if let Some(r) = args[i].strip_prefix("-I") {
                replace_str = Some(r.to_owned());
                i += 1;
                cmd_start = i;
                continue;
            }
            break;
        }
        let cmd_template = args[cmd_start..].join(" ");
        let cmd = if cmd_template.is_empty() {
            "echo".to_owned()
        } else {
            cmd_template
        };

        if let Some(placeholder) = replace_str {
            // -I mode: one invocation per stdin line, substitute placeholder
            let out = stdin
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|line| cmd.replace(&placeholder, line.trim()))
                .collect::<Vec<_>>()
                .join("\n");
            CommandResult::ok(format!("{out}\n"))
        } else {
            // Default: append all whitespace-separated tokens as args
            let tokens = stdin.split_whitespace().collect::<Vec<_>>().join(" ");
            CommandResult::ok(format!("{cmd} {tokens}\n"))
        }
    });

    reg.register("sort", |_ctx, args, stdin| {
        let mut lines = stdin.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
        let reverse = args.iter().any(|a| a == "-r");
        let unique = args.iter().any(|a| a == "-u");
        let numeric = args.iter().any(|a| a == "-n");

        // -k FIELD (1-indexed) key-based sorting
        let key_field: Option<usize> = args
            .windows(2)
            .find_map(|w| {
                if w[0] == "-k" {
                    w[1].parse::<usize>().ok()
                } else {
                    None
                }
            })
            .or_else(|| {
                args.iter()
                    .find_map(|a| a.strip_prefix("-k").and_then(|s| s.parse::<usize>().ok()))
            });

        // -t DELIM field delimiter for -k
        let key_delim: Option<String> = args.windows(2).find_map(|w| {
            if w[0] == "-t" {
                Some(w[1].clone())
            } else {
                None
            }
        });

        let extract_key = |line: &str| -> String {
            if let Some(k) = key_field {
                let field = if let Some(ref d) = key_delim {
                    line.split(d.as_str())
                        .nth(k.saturating_sub(1))
                        .unwrap_or("")
                        .to_owned()
                } else {
                    line.split_whitespace()
                        .nth(k.saturating_sub(1))
                        .unwrap_or("")
                        .to_owned()
                };
                field
            } else {
                line.to_owned()
            }
        };

        if numeric {
            lines.sort_by(|a, b| {
                let ka = extract_key(a).trim().parse::<f64>().unwrap_or(0.0);
                let kb = extract_key(b).trim().parse::<f64>().unwrap_or(0.0);
                let ord = ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal);
                if reverse {
                    ord.reverse()
                } else {
                    ord
                }
            });
        } else if reverse {
            lines.sort_by_key(|a| std::cmp::Reverse(extract_key(a)));
        } else {
            lines.sort_by_key(|a| extract_key(a));
        }

        if unique {
            lines.dedup();
        }
        CommandResult::ok(format!("{}\n", lines.join("\n")))
    });

    // uniq removes consecutive duplicate lines (POSIX semantics).
    reg.register("uniq", |_ctx, args, stdin| {
        let count_mode = args.iter().any(|a| a == "-c");
        let dup_only = args.iter().any(|a| a == "-d");
        let mut out: Vec<String> = Vec::new();
        let mut prev: Option<String> = None;
        let mut run: usize = 0;
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
        let result = if count_mode {
            out.iter()
                .zip(counts.iter())
                .map(|(line, &c)| format!("{c:7} {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        } else if dup_only {
            out.iter()
                .zip(counts.iter())
                .filter(|(_, &c)| c > 1)
                .map(|(line, _)| line.clone())
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            out.join("\n")
        };
        CommandResult::ok(format!("{result}\n"))
    });

    reg.register("wc", |ctx, args, stdin| {
        let file_arg = args
            .iter()
            .find(|a| !a.starts_with('-'))
            .map(String::as_str);
        let source = if let Some(path) = file_arg {
            match ctx.vfs.read_file(&ctx.cwd, path) {
                Ok(c) => c,
                Err(e) => return CommandResult::err(format!("wc: {e}\n"), 1),
            }
        } else {
            stdin.to_owned()
        };
        let lines = source.lines().count();
        let words = source.split_whitespace().count();
        let bytes = source.len();

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
        let mut delimiter = "\t".to_owned();
        let mut field_spec: Option<String> = None;
        let mut char_spec: Option<String> = None;
        let mut i = 0usize;
        while i < args.len() {
            match args[i].as_str() {
                "-d" if i + 1 < args.len() => {
                    delimiter = args[i + 1].clone();
                    i += 1;
                }
                "-f" if i + 1 < args.len() => {
                    field_spec = Some(args[i + 1].clone());
                    i += 1;
                }
                "-c" if i + 1 < args.len() => {
                    char_spec = Some(args[i + 1].clone());
                    i += 1;
                }
                s if s.starts_with("-f") => {
                    field_spec = Some(s[2..].to_owned());
                }
                s if s.starts_with("-c") => {
                    char_spec = Some(s[2..].to_owned());
                }
                _ => {}
            }
            i += 1;
        }

        // Parse a range spec "N", "N,M,…", or "N-M" into sorted indices (0-based)
        let parse_spec = |spec: &str| -> Vec<usize> {
            if spec.contains('-') {
                let parts: Vec<usize> = spec
                    .splitn(2, '-')
                    .filter_map(|s| s.parse::<usize>().ok())
                    .collect();
                if parts.len() == 2 {
                    return (parts[0]..=parts[1]).collect();
                }
            }
            spec.split(',')
                .filter_map(|s| s.trim().parse::<usize>().ok())
                .collect()
        };

        // -c: character cut (1-indexed positions)
        if let Some(spec) = char_spec {
            let cols = parse_spec(&spec);
            let out = stdin
                .lines()
                .map(|line| {
                    let chars: Vec<char> = line.chars().collect();
                    cols.iter()
                        .filter_map(|&c| chars.get(c.saturating_sub(1)).copied())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n");
            return CommandResult::ok(format!("{out}\n"));
        }

        // -f: field cut (default)
        let spec = field_spec.unwrap_or_else(|| "1".to_owned());
        let fields = parse_spec(&spec);
        let out = stdin
            .lines()
            .map(|line| {
                let parts: Vec<&str> = line.split(delimiter.as_str()).collect();
                let selected: Vec<&str> = fields
                    .iter()
                    .filter_map(|&f| parts.get(f.saturating_sub(1)).copied())
                    .collect();
                selected.join(delimiter.as_str())
            })
            .collect::<Vec<_>>()
            .join("\n");
        CommandResult::ok(format!("{out}\n"))
    });

    reg.register("tr", |_ctx, args, stdin| {
        // Expand POSIX-style character ranges like a-z into full char lists.
        fn expand_set(s: &str) -> Vec<char> {
            let chars: Vec<char> = s.chars().collect();
            let mut out = Vec::new();
            let mut i = 0;
            while i < chars.len() {
                if i + 2 < chars.len() && chars[i + 1] == '-' {
                    let start = chars[i] as u32;
                    let end = chars[i + 2] as u32;
                    if start <= end {
                        for cp in start..=end {
                            if let Some(c) = char::from_u32(cp) {
                                out.push(c);
                            }
                        }
                        i += 3;
                        continue;
                    }
                }
                out.push(chars[i]);
                i += 1;
            }
            out
        }

        let delete_mode = args.iter().any(|a| a == "-d");
        let squeeze_mode = args.iter().any(|a| a == "-s");
        let positional: Vec<&str> = args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .map(String::as_str)
            .collect();

        if delete_mode {
            let Some(set) = positional.first() else {
                return CommandResult::err("tr: -d requires a SET argument\n", 1);
            };
            let delete_set: std::collections::HashSet<char> = expand_set(set).into_iter().collect();
            let out: String = stdin.chars().filter(|c| !delete_set.contains(c)).collect();
            return CommandResult::ok(out);
        }
        if squeeze_mode {
            let Some(set) = positional.first() else {
                return CommandResult::err("tr: -s requires a SET argument\n", 1);
            };
            let squeeze_set: std::collections::HashSet<char> =
                expand_set(set).into_iter().collect();
            let mut out = String::new();
            let mut last: Option<char> = None;
            for c in stdin.chars() {
                if squeeze_set.contains(&c) && last == Some(c) {
                    // suppress consecutive duplicate in set
                } else {
                    out.push(c);
                }
                last = Some(c);
            }
            return CommandResult::ok(out);
        }
        // Basic character mapping: tr SET1 SET2
        if positional.len() < 2 {
            return CommandResult::err("tr: requires FROM and TO sets\n", 1);
        }
        let from = expand_set(positional[0]);
        let to = expand_set(positional[1]);
        let map: HashMap<char, char> = from
            .iter()
            .enumerate()
            .map(|(idx, ch)| (*ch, to.get(idx).copied().unwrap_or(*ch)))
            .collect();
        let out = stdin
            .chars()
            .map(|c| map.get(&c).copied().unwrap_or(c))
            .collect::<String>();
        CommandResult::ok(out)
    });

    reg.register("sed", |_ctx, args, stdin| {
        if args.is_empty() {
            return CommandResult::err("sed: expected expression\n", 1);
        }

        let silent = args.iter().any(|a| a == "-n");
        // Find the expression: first non-flag arg
        let expr = args
            .iter()
            .find(|a| !a.starts_with('-'))
            .map(String::as_str)
            .unwrap_or("");

        // s/old/new/[g] substitution
        if expr.starts_with("s/") {
            let parts: Vec<&str> = expr.trim_start_matches("s/").splitn(3, '/').collect();
            if parts.len() < 2 {
                return CommandResult::err("sed: invalid s expression\n", 1);
            }
            let old = parts[0];
            let new = parts[1];
            let flags = parts.get(2).copied().unwrap_or("");
            let global = flags.contains('g');
            let print_matches = flags.contains('p');
            let mut out = String::new();
            for line in stdin.lines() {
                let replaced = if global {
                    line.replace(old, new)
                } else {
                    // first occurrence only
                    if let Some(pos) = line.find(old) {
                        format!("{}{}{}", &line[..pos], new, &line[pos + old.len()..])
                    } else {
                        line.to_owned()
                    }
                };
                let changed = replaced != line;
                if !silent || (print_matches && changed) {
                    out.push_str(&replaced);
                    out.push('\n');
                }
            }
            return CommandResult::ok(out);
        }

        // -n '/pattern/p' — print only matching lines
        if silent && expr.starts_with('/') && expr.ends_with("/p") {
            let pat = &expr[1..expr.len() - 2];
            let out = stdin
                .lines()
                .filter(|l| l.contains(pat))
                .collect::<Vec<_>>()
                .join("\n");
            return if out.is_empty() {
                CommandResult::err(String::new(), 1)
            } else {
                CommandResult::ok(format!("{out}\n"))
            };
        }

        // Nd or N,Md — delete line(s) by number
        if let Some(d_pos) = expr.find('d') {
            let addr = &expr[..d_pos];
            let lines: Vec<&str> = stdin.lines().collect();
            if addr.contains(',') {
                // range N,M
                let mut rng = addr.splitn(2, ',');
                let from = rng
                    .next()
                    .and_then(|s| s.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                let to = rng
                    .next()
                    .and_then(|s| s.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                let out = lines
                    .iter()
                    .enumerate()
                    .filter_map(|(i, l)| {
                        if (from..=to).contains(&(i + 1)) {
                            None
                        } else {
                            Some(*l)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                return CommandResult::ok(if out.is_empty() {
                    String::new()
                } else {
                    format!("{out}\n")
                });
            } else if let Ok(n) = addr.trim().parse::<usize>() {
                let out = lines
                    .iter()
                    .enumerate()
                    .filter_map(|(i, l)| if i + 1 == n { None } else { Some(*l) })
                    .collect::<Vec<_>>()
                    .join("\n");
                return CommandResult::ok(if out.is_empty() {
                    String::new()
                } else {
                    format!("{out}\n")
                });
            }
        }

        CommandResult::err(format!("sed: unsupported expression: {expr}\n"), 1)
    });

    reg.register("awk", |ctx, args, stdin| {
        if args.is_empty() {
            return CommandResult::err("awk: missing program\n", 1);
        }

        // Parse -F field-separator flag (e.g. -F: or -F ,)
        let mut field_sep: Option<String> = None;
        let mut skip = 0usize;
        for (i, arg) in args.iter().enumerate() {
            if arg == "-F" {
                if let Some(sep) = args.get(i + 1) {
                    field_sep = Some(sep.clone());
                    skip = i + 2;
                }
                break;
            } else if let Some(sep) = arg.strip_prefix("-F") {
                field_sep = Some(sep.to_owned());
                skip = i + 1;
                break;
            }
        }

        // Remaining positional args: program [file]
        let positional: Vec<&str> = args[skip..]
            .iter()
            .filter(|a| !a.starts_with('-'))
            .map(String::as_str)
            .collect();

        let Some(expr) = positional.first() else {
            return CommandResult::err("awk: missing program\n", 1);
        };
        let file_arg = positional.get(1).copied();

        let source = if let Some(path) = file_arg {
            match ctx.vfs.read_file(&ctx.cwd, path) {
                Ok(c) => c,
                Err(e) => return CommandResult::err(format!("awk: {e}\n"), 1),
            }
        } else {
            stdin.to_owned()
        };

        // Parse optional condition before '{': e.g. "NR>1 {print $2}"
        let (condition, action) = if let Some(brace) = expr.find('{') {
            let cond = expr[..brace].trim();
            (
                if cond.is_empty() { None } else { Some(cond) },
                &expr[brace..],
            )
        } else {
            (None, *expr)
        };

        // Extract print field tokens from action: "{print $1, NR, NF, $0}"
        let print_body = action.trim_start_matches('{').trim_end_matches('}').trim();
        let print_args_str = print_body
            .strip_prefix("print")
            .unwrap_or(print_body)
            .trim();

        // Helper: get field from a line (1-indexed; $0 = whole line)
        let get_field = |line: &str, n: usize| -> String {
            if n == 0 {
                return line.to_owned();
            }
            if let Some(ref sep) = field_sep {
                line.split(sep.as_str())
                    .nth(n.saturating_sub(1))
                    .unwrap_or("")
                    .to_owned()
            } else {
                line.split_whitespace()
                    .nth(n.saturating_sub(1))
                    .unwrap_or("")
                    .to_owned()
            }
        };

        // Evaluate a simple NR-/NF-based condition
        let cond_passes = |line: &str, nr: usize| -> bool {
            let Some(cond) = condition else {
                return true;
            };
            let nf = if let Some(ref sep) = field_sep {
                line.split(sep.as_str()).count()
            } else {
                line.split_whitespace().count()
            };
            // Substitute NR and NF, then evaluate simple comparison
            let resolved = cond
                .replace("NR", &nr.to_string())
                .replace("NF", &nf.to_string());
            // Try to evaluate: <lhs><op><rhs>
            for op in [">=", "<=", "!=", ">", "<", "=="] {
                if let Some(pos) = resolved.find(op) {
                    let lhs = resolved[..pos].trim().parse::<i64>();
                    let rhs = resolved[pos + op.len()..].trim().parse::<i64>();
                    if let (Ok(l), Ok(r)) = (lhs, rhs) {
                        return match op {
                            ">" => l > r,
                            ">=" => l >= r,
                            "<" => l < r,
                            "<=" => l <= r,
                            "==" => l == r,
                            "!=" => l != r,
                            _ => true,
                        };
                    }
                }
            }
            // /regex/ condition
            if cond.starts_with('/') && cond.ends_with('/') {
                let pat = &cond[1..cond.len() - 1];
                return Regex::new(pat).is_ok_and(|re| re.is_match(line));
            }
            true
        };

        // Evaluate print_args_str: comma-separated field specs
        let field_specs: Vec<&str> = print_args_str
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();

        let mut out_lines = Vec::new();
        for (i, line) in source.lines().enumerate() {
            let nr = i + 1;
            if !cond_passes(line, nr) {
                continue;
            }
            let nf = if let Some(ref sep) = field_sep {
                line.split(sep.as_str()).count()
            } else {
                line.split_whitespace().count()
            };

            if field_specs.is_empty() {
                out_lines.push(line.to_owned());
                continue;
            }

            let parts: Vec<String> = field_specs
                .iter()
                .map(|spec| {
                    if *spec == "NR" {
                        nr.to_string()
                    } else if *spec == "NF" {
                        nf.to_string()
                    } else if *spec == "$0" {
                        line.to_owned()
                    } else if let Some(rest) = spec.strip_prefix('$') {
                        let n = rest.trim().parse::<usize>().unwrap_or(1);
                        get_field(line, n)
                    } else {
                        // bare string literal in print (unusual but handle it)
                        spec.trim_matches('"').to_owned()
                    }
                })
                .collect();
            out_lines.push(parts.join(" "));
        }

        CommandResult::ok(format!("{}\n", out_lines.join("\n")))
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

    // env — display environment variables
    reg.register("env", |ctx, args, _| {
        if args.is_empty() {
            let mut pairs = ctx
                .env
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>();
            pairs.sort();
            return CommandResult::ok(format!("{}\n", pairs.join("\n")));
        }
        // env VAR=val cmd … not implemented in sim; just show env
        let mut pairs = ctx
            .env
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>();
        pairs.sort();
        CommandResult::ok(format!("{}\n", pairs.join("\n")))
    });

    // basename — strip directory and optional suffix from path
    reg.register("basename", |_ctx, args, _| {
        let Some(path) = args.first() else {
            return CommandResult::err("basename: missing operand\n", 1);
        };
        let base = path.rsplit('/').next().unwrap_or(path.as_str());
        let result = if let Some(suffix) = args.get(1) {
            base.strip_suffix(suffix.as_str()).unwrap_or(base)
        } else {
            base
        };
        CommandResult::ok(format!("{result}\n"))
    });

    // dirname — strip last component of path
    reg.register("dirname", |_ctx, args, _| {
        let Some(path) = args.first() else {
            return CommandResult::err("dirname: missing operand\n", 1);
        };
        let dir = if let Some(pos) = path.rfind('/') {
            if pos == 0 {
                "/".to_owned()
            } else {
                path[..pos].to_owned()
            }
        } else {
            ".".to_owned()
        };
        CommandResult::ok(format!("{dir}\n"))
    });

    // tee — pass stdin through and write a copy to a file
    reg.register("tee", |ctx, args, stdin| {
        let append = args.iter().any(|a| a == "-a");
        let file_arg = args.iter().find(|a| !a.starts_with('-'));
        if let Some(path) = file_arg {
            let normalized = vfs::normalize_path(&ctx.cwd, path).unwrap_or_else(|_| path.clone());
            if let Err(e) = ctx
                .vfs
                .write_file("/", &normalized, stdin, append, &ctx.user)
            {
                return CommandResult::err(format!("tee: {e}\n"), 1);
            }
        }
        CommandResult::ok(stdin.to_owned())
    });

    // date — display a simulated in-game timestamp
    reg.register("date", |_ctx, _args, _| {
        CommandResult::ok("Sat Mar  7 22:22:22 UTC 2026 [NEON-GRID STANDARD TIME]\n")
    });

    // seq — generate a numeric sequence
    reg.register("seq", |_ctx, args, _| {
        let nums: Vec<f64> = args
            .iter()
            .filter_map(|a| a.trim().parse::<f64>().ok())
            .collect();
        let (start, step, end) = match nums.as_slice() {
            [n] => (1.0, 1.0, *n),
            [s, e] => (*s, 1.0, *e),
            [s, inc, e] => (*s, *inc, *e),
            _ => return CommandResult::err("seq: invalid arguments\n", 1),
        };
        if step == 0.0 {
            return CommandResult::err("seq: zero increment\n", 1);
        }
        let mut out = Vec::new();
        let mut cur = start;
        // Detect integer output to avoid floating point noise
        let is_int = start.fract() == 0.0 && step.fract() == 0.0 && end.fract() == 0.0;
        while (step > 0.0 && cur <= end) || (step < 0.0 && cur >= end) {
            if is_int {
                out.push(format!("{}", cur as i64));
            } else {
                out.push(format!("{cur}"));
            }
            cur += step;
        }
        CommandResult::ok(format!("{}\n", out.join("\n")))
    });

    // read — read a line from stdin and assign to a variable
    reg.register("read", |ctx, args, stdin| {
        let var_name = args.first().map(String::as_str).unwrap_or("REPLY");
        let value = stdin.lines().next().unwrap_or("").trim().to_owned();
        ctx.env.insert(var_name.to_owned(), value);
        CommandResult::ok(String::new())
    });

    // paste — merge lines from two inputs (stdin only: paste - - merges pairs)
    reg.register("paste", |ctx, args, stdin| {
        // Support `paste file1 file2` or `paste - -` (merge pairs from stdin)
        let positional: Vec<&str> = args
            .iter()
            .filter(|a| **a != "-")
            .map(String::as_str)
            .collect();

        if positional.len() >= 2 {
            let a_src = match ctx.vfs.read_file(&ctx.cwd, positional[0]) {
                Ok(c) => c,
                Err(e) => return CommandResult::err(format!("paste: {e}\n"), 1),
            };
            let b_src = match ctx.vfs.read_file(&ctx.cwd, positional[1]) {
                Ok(c) => c,
                Err(e) => return CommandResult::err(format!("paste: {e}\n"), 1),
            };
            let a_lines: Vec<&str> = a_src.lines().collect();
            let b_lines: Vec<&str> = b_src.lines().collect();
            let n = a_lines.len().max(b_lines.len());
            let out = (0..n)
                .map(|i| {
                    format!(
                        "{}\t{}",
                        a_lines.get(i).copied().unwrap_or(""),
                        b_lines.get(i).copied().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            return CommandResult::ok(format!("{out}\n"));
        }

        // Stdin mode: pair up consecutive lines
        let lines: Vec<&str> = stdin.lines().collect();
        let out = lines
            .chunks(2)
            .map(|chunk| {
                let a = chunk.first().copied().unwrap_or("");
                let b = chunk.get(1).copied().unwrap_or("");
                format!("{a}\t{b}")
            })
            .collect::<Vec<_>>()
            .join("\n");
        CommandResult::ok(format!("{out}\n"))
    });

    // column — format whitespace/tab-separated input into aligned columns
    reg.register("column", |_ctx, args, stdin| {
        let table_mode = args.iter().any(|a| a == "-t");
        let sep = args
            .windows(2)
            .find_map(|w| {
                if w[0] == "-s" {
                    Some(w[1].as_str())
                } else {
                    None
                }
            })
            .unwrap_or(if table_mode { "\t" } else { " " });

        if !table_mode {
            return CommandResult::ok(stdin.to_owned());
        }

        // Collect rows split by separator
        let rows: Vec<Vec<String>> = stdin
            .lines()
            .map(|line| {
                line.split(sep)
                    .map(str::trim)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .collect();

        // Compute max width per column
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
                let padded: Vec<String> = row
                    .iter()
                    .enumerate()
                    .map(|(j, cell)| format!("{:<width$}", cell, width = widths[j]))
                    .collect();
                padded.join("  ")
            })
            .collect::<Vec<_>>()
            .join("\n");
        CommandResult::ok(format!("{out}\n"))
    });

    // true / false — always succeed / fail
    reg.register("true", |_ctx, _args, _| CommandResult::ok(String::new()));
    reg.register("false", |_ctx, _args, _| {
        CommandResult::err(String::new(), 1)
    });

    // nl — number lines (like cat -n but as a standalone command)
    reg.register("nl", |ctx, args, stdin| {
        let file_arg = args
            .iter()
            .find(|a| !a.starts_with('-'))
            .map(String::as_str);
        let source = if let Some(path) = file_arg {
            match ctx.vfs.read_file(&ctx.cwd, path) {
                Ok(c) => c,
                Err(e) => return CommandResult::err(format!("nl: {e}\n"), 1),
            }
        } else {
            stdin.to_owned()
        };
        let out = source
            .lines()
            .enumerate()
            .map(|(i, line)| format!("{:6}\t{line}", i + 1))
            .collect::<Vec<_>>()
            .join("\n");
        CommandResult::ok(format!("{out}\n"))
    });

    // export — show or set environment variables
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
            // export VAR (without =) is a no-op in the sim
        }
        CommandResult::ok(String::new())
    });

    // test / [ — evaluate conditional expressions
    reg.register("test", |ctx, args, _| {
        let passed = eval_test_expr(ctx, args);
        if passed {
            CommandResult::ok(String::new())
        } else {
            CommandResult::err(String::new(), 1)
        }
    });
    reg.register("[", |ctx, args, _| {
        // Strip trailing ']' before evaluating
        let inner: Vec<String> = args.iter().filter(|a| a.as_str() != "]").cloned().collect();
        let passed = eval_test_expr(ctx, &inner);
        if passed {
            CommandResult::ok(String::new())
        } else {
            CommandResult::err(String::new(), 1)
        }
    });

    // diff — line-by-line file comparison
    reg.register("diff", |ctx, args, _| {
        let files: Vec<&str> = args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .map(String::as_str)
            .collect();
        if files.len() < 2 {
            return CommandResult::err("diff: requires two file operands\n", 1);
        }
        let a = match ctx.vfs.read_file(&ctx.cwd, files[0]) {
            Ok(c) => c,
            Err(e) => return CommandResult::err(format!("diff: {e}\n"), 1),
        };
        let b = match ctx.vfs.read_file(&ctx.cwd, files[1]) {
            Ok(c) => c,
            Err(e) => return CommandResult::err(format!("diff: {e}\n"), 1),
        };
        if a == b {
            return CommandResult::ok(String::new());
        }
        let a_lines: Vec<&str> = a.lines().collect();
        let b_lines: Vec<&str> = b.lines().collect();
        let mut out = Vec::new();
        let max = a_lines.len().max(b_lines.len());
        for i in 0..max {
            match (a_lines.get(i), b_lines.get(i)) {
                (Some(al), Some(bl)) if al != bl => {
                    out.push(format!("< {al}"));
                    out.push(format!("> {bl}"));
                }
                (Some(al), None) => out.push(format!("< {al}")),
                (None, Some(bl)) => out.push(format!("> {bl}")),
                _ => {}
            }
        }
        // diff exits 1 when files differ (POSIX)
        CommandResult::ok(format!("{}\n", out.join("\n")))
    });

    // which — report command location (all built-ins live in /bin in the sim)
    reg.register("which", |_ctx, args, _| {
        let Some(cmd) = args.first() else {
            return CommandResult::err("which: missing argument\n", 1);
        };
        CommandResult::ok(format!("/bin/{cmd}\n"))
    });

    // chmod — update VFS permission bits
    reg.register("chmod", |ctx, args, _| {
        let positional: Vec<&str> = args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .map(String::as_str)
            .collect();
        if positional.len() < 2 {
            return CommandResult::err("chmod: missing operand\n", 1);
        }
        let mode = u16::from_str_radix(positional[0], 8).unwrap_or(0o644);
        for path in &positional[1..] {
            if let Err(err) = ctx.vfs.chmod(&ctx.cwd, path, mode) {
                return CommandResult::err(format!("chmod: {err}\n"), 1);
            }
        }
        CommandResult::ok(String::new())
    });

    // history — show command history from the session VFS file
    reg.register("history", |ctx, args, _| {
        match ctx.vfs.read_file("/", "/tmp/.history") {
            Ok(content) => {
                let lines: Vec<&str> = content.lines().collect();
                let n = parse_n_flag(args, lines.len());
                let start = lines.len().saturating_sub(n);
                let out: String = lines[start..].iter().map(|l| format!("{l}\n")).collect();
                CommandResult::ok(out)
            }
            Err(_) => CommandResult::ok("No history yet.\n".to_owned()),
        }
    });

    // clear — emit ANSI clear sequence
    reg.register("clear", |_ctx, _args, _| {
        CommandResult::ok("\x1b[2J\x1b[H".to_owned())
    });

    // uptime — simulated uptime
    reg.register("uptime", |_ctx, _args, _| {
        CommandResult::ok(
            " 14:22:07 up 3 days, 7:14,  1 user,  load average: 0.42, 0.31, 0.28\n".to_owned(),
        )
    });

    // rev — reverse lines
    reg.register("rev", |_ctx, _args, stdin| {
        let out: String = stdin
            .lines()
            .map(|l| {
                let reversed: String = l.chars().rev().collect();
                format!("{reversed}\n")
            })
            .collect();
        CommandResult::ok(out)
    });

    reg
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use shell::{BuiltinRegistry, CommandResult, ExecutionContext};
    use vfs::Vfs;

    fn ctx_with_file(path: &str, content: &str) -> (Vfs, HashMap<String, String>) {
        let mut vfs = Vfs::default();
        vfs.mkdir_p("/", "home/player", "player").unwrap();
        let normalized =
            vfs::normalize_path("/home/player", path).unwrap_or_else(|_| path.to_owned());
        vfs.write_file("/", &normalized, content, false, "player")
            .unwrap();
        let env = HashMap::from([
            ("USER".to_owned(), "player".to_owned()),
            ("HOME".to_owned(), "/home/player".to_owned()),
            ("PWD".to_owned(), "/home/player".to_owned()),
            ("PATH".to_owned(), "/bin:/usr/bin".to_owned()),
            ("?".to_owned(), "0".to_owned()),
        ]);
        (vfs, env)
    }

    fn run(
        reg: &BuiltinRegistry,
        vfs: &mut Vfs,
        env: &HashMap<String, String>,
        cmd: &str,
        args: &[&str],
        stdin: &str,
    ) -> CommandResult {
        let handler = reg.get(cmd).unwrap();
        let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut ctx = ExecutionContext {
            vfs,
            cwd: "/home/player".to_owned(),
            user: "player".to_owned(),
            node: "corp-sim-01".to_owned(),
            env: env.clone(),
            last_exit: 0,
        };
        handler(&mut ctx, &args_owned, stdin)
    }

    #[test]
    fn head_with_n_flag() {
        let reg = super::default_registry();
        let mut vfs = Vfs::default();
        vfs.mkdir_p("/", "home/player", "p").unwrap();
        let env = HashMap::new();
        let mut ctx = ExecutionContext {
            vfs: &mut vfs,
            cwd: "/home/player".to_owned(),
            user: "p".to_owned(),
            node: "n".to_owned(),
            env,
            last_exit: 0,
        };
        let handler = reg.get("head").unwrap();
        let result = handler(&mut ctx, &["-n".to_owned(), "2".to_owned()], "a\nb\nc\nd");
        assert_eq!(result.stdout.trim(), "a\nb");
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn tail_with_n_flag() {
        let reg = super::default_registry();
        let mut vfs = Vfs::default();
        vfs.mkdir_p("/", "home/player", "p").unwrap();
        let env = HashMap::new();
        let mut ctx = ExecutionContext {
            vfs: &mut vfs,
            cwd: "/home/player".to_owned(),
            user: "p".to_owned(),
            node: "n".to_owned(),
            env,
            last_exit: 0,
        };
        let handler = reg.get("tail").unwrap();
        let result = handler(&mut ctx, &["-n".to_owned(), "2".to_owned()], "a\nb\nc\nd");
        assert_eq!(result.stdout.trim(), "c\nd");
    }

    #[test]
    fn grep_case_insensitive() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("test.log", "Token=A\ntoken=B\nTOKEN=C\nfoo");
        let result = run(
            &reg,
            &mut vfs,
            &env,
            "grep",
            &["-i", "token"],
            "Token=A\nfoo\nTOKEN=C",
        );
        assert!(result.stdout.contains("Token=A"));
        assert!(result.stdout.contains("TOKEN=C"));
        assert!(!result.stdout.contains("foo"));
    }

    #[test]
    fn grep_invert() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("t.txt", "");
        let result = run(
            &reg,
            &mut vfs,
            &env,
            "grep",
            &["-v", "token"],
            "token=A\nfoo\nbar",
        );
        assert!(!result.stdout.contains("token=A"));
        assert!(result.stdout.contains("foo"));
        assert!(result.stdout.contains("bar"));
    }

    #[test]
    fn grep_line_numbers() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("t.txt", "");
        let result = run(
            &reg,
            &mut vfs,
            &env,
            "grep",
            &["-n", "hit"],
            "miss\nhit\nmiss",
        );
        assert!(result.stdout.contains("2:hit"));
    }

    #[test]
    fn grep_no_match_exits_nonzero() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("t.txt", "");
        let result = run(&reg, &mut vfs, &env, "grep", &["nothing"], "foo\nbar");
        assert_ne!(result.exit_code, 0);
    }

    #[test]
    fn grep_file_arg() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("/home/player/data.log", "alpha\nbeta\ngamma");
        let result = run(&reg, &mut vfs, &env, "grep", &["beta", "data.log"], "");
        assert_eq!(result.stdout.trim(), "beta");
    }

    #[test]
    fn uniq_consecutive_only() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("t.txt", "");
        // "a" appears non-consecutively — uniq should keep both
        let result = run(&reg, &mut vfs, &env, "uniq", &[], "a\nb\na");
        let lines: Vec<&str> = result.stdout.trim().lines().collect();
        assert_eq!(lines, vec!["a", "b", "a"]);
    }

    #[test]
    fn uniq_consecutive_dedup() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("t.txt", "");
        let result = run(&reg, &mut vfs, &env, "uniq", &[], "a\na\nb\nb\na");
        let lines: Vec<&str> = result.stdout.trim().lines().collect();
        assert_eq!(lines, vec!["a", "b", "a"]);
    }

    #[test]
    fn uniq_count_mode() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("t.txt", "");
        let result = run(&reg, &mut vfs, &env, "uniq", &["-c"], "a\na\nb");
        assert!(result.stdout.contains("2 a"));
        assert!(result.stdout.contains("1 b"));
    }

    #[test]
    fn sort_unique_flag() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("t.txt", "");
        let result = run(&reg, &mut vfs, &env, "sort", &["-u"], "b\na\nb\na");
        let lines: Vec<&str> = result.stdout.trim().lines().collect();
        assert_eq!(lines, vec!["a", "b"]);
    }

    #[test]
    fn sort_reverse() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("t.txt", "");
        let result = run(&reg, &mut vfs, &env, "sort", &["-r"], "a\nc\nb");
        let lines: Vec<&str> = result.stdout.trim().lines().collect();
        assert_eq!(lines, vec!["c", "b", "a"]);
    }

    #[test]
    fn wc_with_file_arg() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("/home/player/lines.txt", "a\nb\nc");
        let result = run(&reg, &mut vfs, &env, "wc", &["-l", "lines.txt"], "");
        assert_eq!(result.stdout.trim(), "3");
    }

    #[test]
    fn basename_and_dirname() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("t.txt", "");
        let result = run(&reg, &mut vfs, &env, "basename", &["/foo/bar/baz.txt"], "");
        assert_eq!(result.stdout.trim(), "baz.txt");
        let result2 = run(
            &reg,
            &mut vfs,
            &env,
            "basename",
            &["/foo/bar/baz.txt", ".txt"],
            "",
        );
        assert_eq!(result2.stdout.trim(), "baz");
        let result3 = run(&reg, &mut vfs, &env, "dirname", &["/foo/bar/baz.txt"], "");
        assert_eq!(result3.stdout.trim(), "/foo/bar");
    }

    #[test]
    fn env_command_shows_variables() {
        let reg = super::default_registry();
        let (mut vfs, mut env) = ctx_with_file("t.txt", "");
        env.insert("MYVAR".to_owned(), "hello".to_owned());
        let result = run(&reg, &mut vfs, &env, "env", &[], "");
        assert!(result.stdout.contains("MYVAR=hello"));
    }

    #[test]
    fn tee_writes_file_and_passes_through() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("existing.txt", "old");
        let result = run(&reg, &mut vfs, &env, "tee", &["output.txt"], "new content");
        assert_eq!(result.stdout, "new content");
        let written = vfs.read_file("/home/player", "output.txt").unwrap();
        assert_eq!(written, "new content");
    }

    // ── New tests for added/fixed functionality ─────────────────────────────

    #[test]
    fn grep_count_mode() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("t.txt", "");
        let result = run(
            &reg,
            &mut vfs,
            &env,
            "grep",
            &["-c", "hit"],
            "hit\nmiss\nhit\nhit\nmiss",
        );
        assert_eq!(result.stdout.trim(), "3");
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn cat_line_numbers() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("t.txt", "");
        let result = run(&reg, &mut vfs, &env, "cat", &["-n"], "alpha\nbeta\ngamma");
        assert!(result.stdout.contains("     1\talpha"));
        assert!(result.stdout.contains("     2\tbeta"));
        assert!(result.stdout.contains("     3\tgamma"));
    }

    #[test]
    fn ls_long_format_shows_metadata() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("/home/player/note.txt", "hello");
        let result = run(&reg, &mut vfs, &env, "ls", &["-l"], "");
        // Long format must include the filename and the owner
        assert!(result.stdout.contains("note.txt"));
        assert!(result.stdout.contains("player"));
        // Must show a permissions string starting with '-' or 'd'
        assert!(result.stdout.contains("-rw") || result.stdout.contains("drw"));
    }

    #[test]
    fn find_type_filter_files_only() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("/home/player/f.txt", "x");
        vfs.mkdir_p("/home/player", "subdir", "player").unwrap();
        let result = run(&reg, &mut vfs, &env, "find", &[".", "-type", "f"], "");
        assert!(result.stdout.contains("f.txt"));
        assert!(!result.stdout.contains("subdir"));
    }

    #[test]
    fn find_type_filter_dirs_only() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("/home/player/f.txt", "x");
        vfs.mkdir_p("/home/player", "mydir", "player").unwrap();
        let result = run(&reg, &mut vfs, &env, "find", &[".", "-type", "d"], "");
        assert!(result.stdout.contains("mydir") || result.stdout.contains("player"));
        assert!(!result.stdout.contains("f.txt"));
    }

    #[test]
    fn cp_recursive_copies_directory_tree() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("placeholder.txt", "");
        vfs.mkdir_p("/home/player", "src", "player").unwrap();
        vfs.write_file("/home/player/src", "a.txt", "alpha", false, "player")
            .unwrap();
        vfs.write_file("/home/player/src", "b.txt", "beta", false, "player")
            .unwrap();
        let result = run(&reg, &mut vfs, &env, "cp", &["-r", "src", "dst"], "");
        assert_eq!(result.exit_code, 0);
        assert_eq!(vfs.read_file("/home/player/dst", "a.txt").unwrap(), "alpha");
        assert_eq!(vfs.read_file("/home/player/dst", "b.txt").unwrap(), "beta");
    }

    #[test]
    fn rm_strips_flag_args() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("del.txt", "bye");
        let result = run(&reg, &mut vfs, &env, "rm", &["-f", "del.txt"], "");
        assert_eq!(result.exit_code, 0);
        assert!(vfs.read_file("/home/player", "del.txt").is_err());
    }

    #[test]
    fn rm_recursive_removes_directory() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("placeholder.txt", "");
        vfs.mkdir_p("/home/player", "tree", "player").unwrap();
        vfs.write_file("/home/player/tree", "leaf.txt", "x", false, "player")
            .unwrap();
        let result = run(&reg, &mut vfs, &env, "rm", &["-r", "tree"], "");
        assert_eq!(result.exit_code, 0);
        assert!(vfs.read_file("/home/player/tree", "leaf.txt").is_err());
    }

    #[test]
    fn awk_field_separator() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("t.txt", "");
        // Extract second colon-separated field
        let result = run(
            &reg,
            &mut vfs,
            &env,
            "awk",
            &["-F:", "{print $2}"],
            "a:b:c\n1:2:3",
        );
        let lines: Vec<&str> = result.stdout.trim().lines().collect();
        assert_eq!(lines, vec!["b", "2"]);
    }

    #[test]
    fn sort_numeric_flag() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("t.txt", "");
        let result = run(&reg, &mut vfs, &env, "sort", &["-n"], "10\n2\n1\n20");
        let lines: Vec<&str> = result.stdout.trim().lines().collect();
        assert_eq!(lines, vec!["1", "2", "10", "20"]);
    }

    #[test]
    fn diff_shows_changed_lines() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("/home/player/a.txt", "hello\nworld\n");
        vfs.write_file("/home/player", "b.txt", "hello\nearth\n", false, "player")
            .unwrap();
        let result = run(&reg, &mut vfs, &env, "diff", &["a.txt", "b.txt"], "");
        assert!(result.stdout.contains("< world"));
        assert!(result.stdout.contains("> earth"));
    }

    #[test]
    fn diff_identical_files_produces_no_output() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("/home/player/a.txt", "same\n");
        vfs.write_file("/home/player", "b.txt", "same\n", false, "player")
            .unwrap();
        let result = run(&reg, &mut vfs, &env, "diff", &["a.txt", "b.txt"], "");
        assert_eq!(result.stdout.trim(), "");
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn which_returns_bin_path() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("t.txt", "");
        let result = run(&reg, &mut vfs, &env, "which", &["grep"], "");
        assert_eq!(result.stdout.trim(), "/bin/grep");
    }

    #[test]
    fn chmod_changes_permission_bits() {
        let reg = super::default_registry();
        let (mut vfs, env) = ctx_with_file("/home/player/script.sh", "#!/bin/sh");
        let result = run(&reg, &mut vfs, &env, "chmod", &["755", "script.sh"], "");
        assert_eq!(result.exit_code, 0);
        let node = vfs.stat("/home/player", "script.sh").unwrap();
        assert_eq!(node.meta.perms.mode, 0o755);
    }
}
