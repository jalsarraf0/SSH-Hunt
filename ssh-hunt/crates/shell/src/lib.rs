#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Arc;

use thiserror::Error;
use vfs::{normalize_path, Vfs};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ShellError {
    #[error("parse error: {0}")]
    Parse(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimpleCommand {
    pub program: String,
    pub args: Vec<String>,
    pub stdin: Option<String>,
    pub stdout: Option<(String, bool)>, // (path, append)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pipeline {
    pub commands: Vec<SimpleCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainOp {
    Always,
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainSegment {
    pub op: ChainOp,
    pub pipeline: Pipeline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedLine {
    pub segments: Vec<ChainSegment>,
}

#[derive(Debug, Clone)]
pub struct CommandResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl CommandResult {
    pub fn ok(stdout: impl Into<String>) -> Self {
        Self {
            stdout: stdout.into(),
            stderr: String::new(),
            exit_code: 0,
        }
    }

    pub fn err(stderr: impl Into<String>, exit_code: i32) -> Self {
        Self {
            stdout: String::new(),
            stderr: stderr.into(),
            exit_code,
        }
    }
}

type CommandFn = dyn Fn(&mut ExecutionContext<'_>, &[String], &str) -> CommandResult + Send + Sync;

#[derive(Default, Clone)]
pub struct BuiltinRegistry {
    handlers: HashMap<String, Arc<CommandFn>>,
}

impl BuiltinRegistry {
    pub fn register<F>(&mut self, name: &str, handler: F)
    where
        F: Fn(&mut ExecutionContext<'_>, &[String], &str) -> CommandResult + Send + Sync + 'static,
    {
        self.handlers.insert(name.to_owned(), Arc::new(handler));
    }

    pub fn get(&self, name: &str) -> Option<&Arc<CommandFn>> {
        self.handlers.get(name)
    }
}

pub struct ExecutionContext<'a> {
    pub vfs: &'a mut Vfs,
    pub cwd: String,
    pub user: String,
    pub node: String,
    pub env: HashMap<String, String>,
    pub last_exit: i32,
}

impl<'a> ExecutionContext<'a> {
    pub fn new(vfs: &'a mut Vfs, user: impl Into<String>, node: impl Into<String>) -> Self {
        let user = user.into();
        let node = node.into();
        let cwd = "/home".to_owned();
        let mut env = HashMap::new();
        env.insert("USER".to_owned(), user.clone());
        env.insert("HOME".to_owned(), "/home".to_owned());
        env.insert("PWD".to_owned(), cwd.clone());
        env.insert("PATH".to_owned(), "/bin:/usr/bin".to_owned());
        env.insert("NODE".to_owned(), node.clone());
        env.insert("?".to_owned(), "0".to_owned());
        Self {
            vfs,
            cwd,
            user,
            node,
            env,
            last_exit: 0,
        }
    }

    pub fn prompt(&self) -> String {
        format!("{}@{}:{}$ ", self.user, self.node, self.cwd)
    }
}

#[derive(Default)]
pub struct ShellEngine {
    registry: BuiltinRegistry,
}

impl ShellEngine {
    pub fn with_registry(registry: BuiltinRegistry) -> Self {
        Self { registry }
    }

    pub fn parse(
        &self,
        input: &str,
        env: &HashMap<String, String>,
    ) -> Result<ParsedLine, ShellError> {
        parse_line(input, env)
    }

    pub fn execute(
        &self,
        ctx: &mut ExecutionContext<'_>,
        input: &str,
    ) -> Result<CommandResult, ShellError> {
        let parsed = self.parse(input, &ctx.env)?;
        let mut result = CommandResult::ok(String::new());

        for segment in parsed.segments {
            let should_run = match segment.op {
                ChainOp::Always => true,
                ChainOp::And => result.exit_code == 0,
                ChainOp::Or => result.exit_code != 0,
            };

            if !should_run {
                continue;
            }

            result = self.execute_pipeline(ctx, &segment.pipeline);
            ctx.last_exit = result.exit_code;
            ctx.env.insert("?".to_owned(), ctx.last_exit.to_string());
        }

        Ok(result)
    }

    fn execute_pipeline(
        &self,
        ctx: &mut ExecutionContext<'_>,
        pipeline: &Pipeline,
    ) -> CommandResult {
        let mut stdin_buf = String::new();
        let mut last = CommandResult::ok(String::new());

        for command in &pipeline.commands {
            let mut effective_input = stdin_buf.clone();
            if let Some(in_path) = &command.stdin {
                match ctx.vfs.read_file(&ctx.cwd, in_path) {
                    Ok(content) => effective_input = content,
                    Err(err) => return CommandResult::err(format!("{err}\n"), 1),
                }
            }

            let handler = self.registry.get(&command.program);
            let mut out = match handler {
                Some(cmd) => cmd(ctx, &command.args, &effective_input),
                None => {
                    CommandResult::err(format!("{}: command not found\n", command.program), 127)
                }
            };

            if let Some((path, append)) = &command.stdout {
                let normalized = normalize_path(&ctx.cwd, path).unwrap_or_else(|_| path.clone());
                if let Err(err) =
                    ctx.vfs
                        .write_file("/", &normalized, &out.stdout, *append, &ctx.user)
                {
                    return CommandResult::err(format!("{err}\n"), 1);
                }
                out.stdout.clear();
            }

            stdin_buf = out.stdout.clone();
            last = out;
        }

        last
    }
}

fn parse_line(input: &str, env: &HashMap<String, String>) -> Result<ParsedLine, ShellError> {
    let tokens = tokenize(input, env)?;
    if tokens.is_empty() {
        return Ok(ParsedLine {
            segments: Vec::new(),
        });
    }

    let mut segments = Vec::new();
    let mut current_op = ChainOp::Always;
    let mut cur_pipeline = Vec::<SimpleCommand>::new();
    let mut cur_words = Vec::<String>::new();
    let mut stdin: Option<String> = None;
    let mut stdout: Option<(String, bool)> = None;

    let flush_command = |words: &mut Vec<String>,
                         stdin: &mut Option<String>,
                         stdout: &mut Option<(String, bool)>|
     -> Result<Option<SimpleCommand>, ShellError> {
        if words.is_empty() {
            return Ok(None);
        }
        let program = words.remove(0);
        let args = words.clone();
        words.clear();
        Ok(Some(SimpleCommand {
            program,
            args,
            stdin: stdin.take(),
            stdout: stdout.take(),
        }))
    };

    let mut i = 0usize;
    while i < tokens.len() {
        match tokens[i].as_str() {
            "|" => {
                if let Some(command) = flush_command(&mut cur_words, &mut stdin, &mut stdout)? {
                    cur_pipeline.push(command);
                } else {
                    return Err(ShellError::Parse("unexpected pipe".to_owned()));
                }
            }
            "&&" | "||" => {
                if let Some(command) = flush_command(&mut cur_words, &mut stdin, &mut stdout)? {
                    cur_pipeline.push(command);
                }
                if cur_pipeline.is_empty() {
                    return Err(ShellError::Parse("empty command chain segment".to_owned()));
                }
                segments.push(ChainSegment {
                    op: current_op,
                    pipeline: Pipeline {
                        commands: cur_pipeline.clone(),
                    },
                });
                cur_pipeline.clear();
                current_op = if tokens[i] == "&&" {
                    ChainOp::And
                } else {
                    ChainOp::Or
                };
            }
            ">" | ">>" | "<" => {
                i += 1;
                let next = tokens
                    .get(i)
                    .ok_or_else(|| ShellError::Parse("missing redirect target".to_owned()))?;
                match tokens[i - 1].as_str() {
                    "<" => stdin = Some(next.clone()),
                    ">" => stdout = Some((next.clone(), false)),
                    ">>" => stdout = Some((next.clone(), true)),
                    _ => {}
                }
            }
            other => cur_words.push(other.to_owned()),
        }
        i += 1;
    }

    if let Some(command) = flush_command(&mut cur_words, &mut stdin, &mut stdout)? {
        cur_pipeline.push(command);
    }

    if !cur_pipeline.is_empty() {
        segments.push(ChainSegment {
            op: current_op,
            pipeline: Pipeline {
                commands: cur_pipeline,
            },
        });
    }

    Ok(ParsedLine { segments })
}

fn tokenize(input: &str, env: &HashMap<String, String>) -> Result<Vec<String>, ShellError> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut chars = input.chars().peekable();
    let mut single_quote = false;
    let mut double_quote = false;

    while let Some(ch) = chars.next() {
        match ch {
            '\\' if !single_quote => {
                if let Some(next) = chars.next() {
                    cur.push(next);
                }
            }
            '\'' if !double_quote => single_quote = !single_quote,
            '"' if !single_quote => double_quote = !double_quote,
            '$' if !single_quote => {
                let mut name = String::new();
                while let Some(c) = chars.peek() {
                    if c.is_ascii_alphanumeric() || *c == '_' || *c == '?' {
                        name.push(*c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if name.is_empty() {
                    cur.push('$');
                } else {
                    cur.push_str(env.get(&name).map(String::as_str).unwrap_or(""));
                }
            }
            ' ' | '\t' if !single_quote && !double_quote => {
                if !cur.is_empty() {
                    tokens.push(cur.clone());
                    cur.clear();
                }
            }
            '|' | '&' | '>' | '<' if !single_quote && !double_quote => {
                if !cur.is_empty() {
                    tokens.push(cur.clone());
                    cur.clear();
                }

                let mut op = ch.to_string();
                if let Some(next) = chars.peek() {
                    let pair = format!("{ch}{next}");
                    if pair == "&&" || pair == "||" || pair == ">>" {
                        op = pair;
                        chars.next();
                    }
                }
                tokens.push(op);
            }
            _ => cur.push(ch),
        }
    }

    if single_quote || double_quote {
        return Err(ShellError::Parse("unterminated quote".to_owned()));
    }

    if !cur.is_empty() {
        tokens.push(cur);
    }

    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> HashMap<String, String> {
        HashMap::from([
            ("USER".to_owned(), "neo".to_owned()),
            ("HOME".to_owned(), "/home/neo".to_owned()),
            ("?".to_owned(), "0".to_owned()),
        ])
    }

    #[test]
    fn parse_quotes_escapes_and_env() {
        let line = parse_line(r#"echo "hello $USER" 'raw $HOME' escaped\ value"#, &env()).unwrap();
        let cmd = &line.segments[0].pipeline.commands[0];
        assert_eq!(cmd.program, "echo");
        assert_eq!(cmd.args[0], "hello neo");
        assert_eq!(cmd.args[1], "raw $HOME");
        assert_eq!(cmd.args[2], "escaped value");
    }

    #[test]
    fn parse_pipeline_and_redirection() {
        let line = parse_line("cat < in.txt | grep neon > out.txt", &env()).unwrap();
        let a = &line.segments[0].pipeline.commands[0];
        let b = &line.segments[0].pipeline.commands[1];
        assert_eq!(a.stdin.as_deref(), Some("in.txt"));
        assert_eq!(b.stdout.as_ref().map(|v| v.0.as_str()), Some("out.txt"));
    }

    #[test]
    fn parse_and_or() {
        let line = parse_line("false && echo a || echo b", &env()).unwrap();
        assert_eq!(line.segments.len(), 3);
        assert_eq!(line.segments[0].op, ChainOp::Always);
        assert_eq!(line.segments[1].op, ChainOp::And);
        assert_eq!(line.segments[2].op, ChainOp::Or);
    }

    #[test]
    fn execute_pipeline() {
        let mut vfs = Vfs::default();
        vfs.mkdir_p("/", "home", "neo").unwrap();
        let mut reg = BuiltinRegistry::default();
        reg.register("echo", |_, args, _| {
            CommandResult::ok(format!("{}\n", args.join(" ")))
        });
        reg.register("wc", |_, _, stdin| {
            let lines = stdin.lines().count();
            CommandResult::ok(format!("{lines}\n"))
        });
        let shell = ShellEngine::with_registry(reg);
        let mut ctx = ExecutionContext::new(&mut vfs, "neo", "train-node");
        let out = shell.execute(&mut ctx, "echo a\\nb | wc").unwrap();
        assert_eq!(out.stdout.trim(), "1");
        assert_eq!(out.exit_code, 0);
    }
}
