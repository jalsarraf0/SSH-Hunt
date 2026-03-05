#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use protocol::ScriptRunResult;
use rhai::{Array, Dynamic, Engine, EvalAltResult, Scope};
use thiserror::Error;
use tokio::task;
use tokio::time::timeout;

#[derive(Debug, Clone)]
pub struct ScriptPolicy {
    pub max_script_size: usize,
    pub max_operations: u64,
    pub max_runtime: Duration,
    pub max_output_bytes: usize,
}

impl Default for ScriptPolicy {
    fn default() -> Self {
        Self {
            max_script_size: 8 * 1024,
            max_operations: 50_000,
            max_runtime: Duration::from_millis(600),
            max_output_bytes: 8 * 1024,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ScriptContext {
    pub visible_nodes: Vec<String>,
    pub virtual_files: BTreeMap<String, String>,
}

#[derive(Debug, Error)]
pub enum ScriptError {
    #[error("script exceeds maximum size")]
    TooLarge,
    #[error("script runtime exceeded limit")]
    Timeout,
    #[error("script output exceeded limit")]
    OutputLimit,
    #[error("script execution error: {0}")]
    Runtime(String),
}

#[derive(Clone)]
pub struct ScriptEngine {
    policy: ScriptPolicy,
}

impl ScriptEngine {
    pub fn new(policy: ScriptPolicy) -> Self {
        Self { policy }
    }

    pub async fn run(
        &self,
        source: &str,
        ctx: ScriptContext,
    ) -> std::result::Result<ScriptRunResult, ScriptError> {
        if source.len() > self.policy.max_script_size {
            return Err(ScriptError::TooLarge);
        }

        let policy = self.policy.clone();
        let src = source.to_owned();

        let fut = task::spawn_blocking(move || run_inner(src, ctx, policy));
        let timed = timeout(self.policy.max_runtime, fut)
            .await
            .map_err(|_| ScriptError::Timeout)?;

        match timed {
            Ok(inner) => inner,
            Err(join_err) => Err(ScriptError::Runtime(format!("join error: {join_err}"))),
        }
    }
}

fn run_inner(
    source: String,
    ctx: ScriptContext,
    policy: ScriptPolicy,
) -> std::result::Result<ScriptRunResult, ScriptError> {
    let output = Arc::new(Mutex::new(String::new()));
    let output_ref = Arc::clone(&output);

    let mut engine = Engine::new();
    engine.set_max_operations(policy.max_operations);
    engine.set_max_call_levels(32);
    engine.set_max_expr_depths(32, 16);
    engine.set_max_modules(8);

    let node_list = ctx.visible_nodes.clone();
    engine.register_fn("scan_nodes", move || -> Array {
        node_list
            .iter()
            .cloned()
            .map(Dynamic::from)
            .collect::<Array>()
    });

    let file_map = ctx.virtual_files.clone();
    engine.register_fn("read_virtual", move |path: &str| -> String {
        file_map.get(path).cloned().unwrap_or_default()
    });

    engine.register_fn("grep", |text: &str, needle: &str| -> String {
        text.lines()
            .filter(|line| line.contains(needle))
            .collect::<Vec<_>>()
            .join("\n")
    });

    engine.on_print(move |line| {
        if let Ok(mut guard) = output_ref.lock() {
            guard.push_str(line);
            guard.push('\n');
        }
    });

    let mut scope = Scope::new();
    let eval = engine.eval_with_scope::<Dynamic>(&mut scope, &source);
    let elapsed_ms = 0u64;

    match eval {
        Ok(value) => {
            let mut out = output
                .lock()
                .map_err(|_| ScriptError::Runtime("poisoned output lock".to_owned()))?
                .clone();
            if !value.is_unit() {
                out.push_str(&value.to_string());
                out.push('\n');
            }
            if out.len() > policy.max_output_bytes {
                return Err(ScriptError::OutputLimit);
            }
            Ok(ScriptRunResult {
                output: out,
                exit_code: 0,
                consumed_ops: policy.max_operations,
                elapsed_ms,
            })
        }
        Err(err) => Err(map_rhai_error(err)),
    }
}

fn map_rhai_error(err: Box<EvalAltResult>) -> ScriptError {
    ScriptError::Runtime(err.to_string())
}

pub async fn run_marketplace_script(
    engine: &ScriptEngine,
    source: &str,
    ctx: ScriptContext,
    cooldown_ready: bool,
) -> Result<ScriptRunResult> {
    if !cooldown_ready {
        return Err(anyhow!("script is on cooldown"));
    }
    let result = engine
        .run(source, ctx)
        .await
        .map_err(|e| anyhow!("script failed: {e}"))?;
    Ok(result)
}

pub async fn parse_and_grep(engine: &ScriptEngine, input: &str, needle: &str) -> Result<String> {
    let script = format!(r#"let x = grep(read_virtual(\"/tmp/input\"), \"{needle}\"); x"#);
    let mut files = BTreeMap::new();
    files.insert("/tmp/input".to_owned(), input.to_owned());
    let result = engine
        .run(
            &script,
            ScriptContext {
                visible_nodes: vec!["ghost-node".to_owned()],
                virtual_files: files,
            },
        )
        .await
        .context("grep script run failed")?;
    Ok(result.output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn script_size_limit() {
        let engine = ScriptEngine::new(ScriptPolicy {
            max_script_size: 4,
            ..ScriptPolicy::default()
        });
        let err = engine
            .run("print('12345')", ScriptContext::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ScriptError::TooLarge));
    }

    #[tokio::test]
    async fn output_limit() {
        let engine = ScriptEngine::new(ScriptPolicy {
            max_output_bytes: 10,
            ..ScriptPolicy::default()
        });
        let err = engine
            .run("\"this output is too long\"", ScriptContext::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ScriptError::OutputLimit));
    }

    #[tokio::test]
    async fn no_real_fs_api_available() {
        let engine = ScriptEngine::new(ScriptPolicy::default());
        let err = engine
            .run("import \"fs\" as fs;", ScriptContext::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ScriptError::Runtime(_)));
    }
}
