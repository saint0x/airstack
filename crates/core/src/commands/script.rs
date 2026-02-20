use crate::output;
use crate::ssh_utils::{execute_remote_command, join_shell_command};
use crate::state::{LocalState, ScriptRunState};
use airstack_config::{AirstackConfig, ScriptConfig};
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Subcommand)]
pub enum ScriptCommands {
    #[command(about = "List configured scripts")]
    List,
    #[command(about = "Plan script execution")]
    Plan(ScriptPlanArgs),
    #[command(about = "Run a named script")]
    Run(ScriptRunArgs),
}

#[derive(Debug, Clone, Args)]
pub struct ScriptPlanArgs {
    #[arg(help = "Script name (optional, defaults to all scripts)")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct ScriptRunArgs {
    #[arg(help = "Script name")]
    pub name: String,
    #[arg(long, help = "Override target with a specific server")]
    pub server: Option<String>,
    #[arg(long, help = "Run against all infra servers")]
    pub all_servers: bool,
    #[arg(long, help = "Show script command details")]
    pub explain: bool,
    #[arg(long, help = "Do not execute; show what would run")]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ScriptRunOptions {
    pub dry_run: bool,
    pub explain: bool,
}

#[derive(Debug, Serialize)]
struct ScriptListRow {
    name: String,
    target: String,
    file: String,
    idempotency: String,
}

#[derive(Debug, Serialize)]
struct ScriptPlanRow {
    script: String,
    server: String,
    action: String,
    reason: String,
}

#[derive(Debug, Serialize)]
struct ScriptRunRow {
    script: String,
    server: String,
    ok: bool,
    skipped: bool,
    detail: String,
}

pub async fn run(config_path: &str, command: ScriptCommands) -> Result<()> {
    match command {
        ScriptCommands::List => list(config_path).await,
        ScriptCommands::Plan(args) => plan(config_path, args).await,
        ScriptCommands::Run(args) => {
            run_named_script(config_path, args, ScriptRunOptions::default()).await
        }
    }
}

pub async fn run_hook_scripts(
    config_path: &str,
    script_names: &[String],
    options: ScriptRunOptions,
) -> Result<()> {
    for name in script_names {
        run_named_script(
            config_path,
            ScriptRunArgs {
                name: name.clone(),
                server: None,
                all_servers: false,
                explain: options.explain,
                dry_run: options.dry_run,
            },
            options.clone(),
        )
        .await?;
    }
    Ok(())
}

async fn list(config_path: &str) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let scripts = config.scripts.as_ref().context("No [scripts] configured")?;

    let mut rows = Vec::new();
    for (name, script) in scripts {
        rows.push(ScriptListRow {
            name: name.clone(),
            target: script.target.clone(),
            file: script.file.clone(),
            idempotency: script
                .idempotency
                .clone()
                .unwrap_or_else(|| "always".to_string()),
        });
    }

    if output::is_json() {
        output::emit_json(&serde_json::json!({ "scripts": rows }))?;
        return Ok(());
    }
    output::line("📜 Airstack Scripts");
    for row in rows {
        output::line(format!(
            "- {} target={} file={} idempotency={}",
            row.name, row.target, row.file, row.idempotency
        ));
    }
    Ok(())
}

async fn plan(config_path: &str, args: ScriptPlanArgs) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let scripts = config.scripts.as_ref().context("No [scripts] configured")?;
    let state = LocalState::load(&config.project.name)?;
    let mut rows = Vec::new();

    for (name, script) in scripts {
        if args.name.as_ref().is_some_and(|n| n != name) {
            continue;
        }
        let servers = resolve_target_servers(&config, script, None, false)?;
        let hash = load_script_hash(config_path, script)?;
        for server in servers {
            let key = script_state_key(name, &server.name);
            let prior = state.script_runs.get(&key).cloned().unwrap_or_default();
            let (action, reason) = planned_action(script, &hash, &prior);
            rows.push(ScriptPlanRow {
                script: name.clone(),
                server: server.name.clone(),
                action,
                reason,
            });
        }
    }
    if output::is_json() {
        output::emit_json(&serde_json::json!({ "plan": rows }))?;
    } else {
        output::line("🧭 Script Plan");
        for row in rows {
            output::line(format!(
                "- {} on {} -> {} ({})",
                row.script, row.server, row.action, row.reason
            ));
        }
    }
    Ok(())
}

async fn run_named_script(
    config_path: &str,
    args: ScriptRunArgs,
    options: ScriptRunOptions,
) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let scripts = config.scripts.as_ref().context("No [scripts] configured")?;
    let script = scripts
        .get(&args.name)
        .with_context(|| format!("Script '{}' not found", args.name))?;

    let servers =
        resolve_target_servers(&config, script, args.server.as_deref(), args.all_servers)?;
    let hash = load_script_hash(config_path, script)?;
    let script_content = load_script_content(config_path, script)?;
    let mut state = LocalState::load(&config.project.name)?;
    let mut rows = Vec::new();
    let explain = args.explain || options.explain;

    for server in servers {
        let key = script_state_key(&args.name, &server.name);
        let prior = state.script_runs.get(&key).cloned().unwrap_or_default();
        let (action, reason) = planned_action(script, &hash, &prior);
        if action == "skip" {
            rows.push(ScriptRunRow {
                script: args.name.clone(),
                server: server.name.clone(),
                ok: true,
                skipped: true,
                detail: reason,
            });
            continue;
        }
        if args.dry_run || options.dry_run {
            rows.push(ScriptRunRow {
                script: args.name.clone(),
                server: server.name.clone(),
                ok: true,
                skipped: false,
                detail: if explain {
                    format!("dry-run; would execute {}", script.file)
                } else {
                    "dry-run".to_string()
                },
            });
            continue;
        }

        let shell = script.shell.clone().unwrap_or_else(|| "bash".to_string());
        let attempts = script
            .retry
            .as_ref()
            .and_then(|r| r.max_attempts)
            .unwrap_or(1)
            .max(1);
        let transient_only = script
            .retry
            .as_ref()
            .and_then(|r| r.transient_only)
            .unwrap_or(false);

        let mut last_err = None;
        for attempt in 1..=attempts {
            let out =
                execute_script_remote(server, &args.name, script, &shell, &script_content).await;
            match out {
                Ok(detail) => {
                    state.script_runs.insert(
                        key.clone(),
                        ScriptRunState {
                            last_hash: Some(hash.clone()),
                            last_run_unix: now_unix(),
                        },
                    );
                    rows.push(ScriptRunRow {
                        script: args.name.clone(),
                        server: server.name.clone(),
                        ok: true,
                        skipped: false,
                        detail: if explain {
                            format!("{} ({detail})", script.file)
                        } else {
                            detail
                        },
                    });
                    last_err = None;
                    break;
                }
                Err(e) => {
                    let msg = e.to_string();
                    last_err = Some(msg.clone());
                    if !transient_only || is_transient_script_error(&msg) {
                        if attempt < attempts {
                            continue;
                        }
                    }
                    break;
                }
            }
        }

        if let Some(err) = last_err {
            rows.push(ScriptRunRow {
                script: args.name.clone(),
                server: server.name.clone(),
                ok: false,
                skipped: false,
                detail: err,
            });
        }
    }

    state.save()?;

    if output::is_json() {
        output::emit_json(&serde_json::json!({ "results": rows }))?;
    } else {
        output::line(format!("📜 Script Run: {}", args.name));
        for row in &rows {
            let mark = if row.ok { "✅" } else { "❌" };
            let mode = if row.skipped { "skip" } else { "run" };
            output::line(format!(
                "{} {} on {} [{}] {}",
                mark, row.script, row.server, mode, row.detail
            ));
        }
    }

    if rows.iter().any(|r| !r.ok) {
        anyhow::bail!("one or more script executions failed");
    }
    Ok(())
}

fn resolve_target_servers<'a>(
    config: &'a AirstackConfig,
    script: &ScriptConfig,
    override_server: Option<&str>,
    all_servers: bool,
) -> Result<Vec<&'a airstack_config::ServerConfig>> {
    let infra = config
        .infra
        .as_ref()
        .context("Script execution requires infra.servers")?;
    if all_servers {
        return Ok(infra.servers.iter().collect());
    }
    if let Some(name) = override_server {
        let server = infra
            .servers
            .iter()
            .find(|s| s.name == name)
            .with_context(|| format!("Server '{}' not found", name))?;
        return Ok(vec![server]);
    }
    if script.target == "all" {
        return Ok(infra.servers.iter().collect());
    }
    if let Some(name) = script.target.strip_prefix("server:") {
        let server = infra
            .servers
            .iter()
            .find(|s| s.name == name)
            .with_context(|| format!("Target server '{}' not found", name))?;
        return Ok(vec![server]);
    }
    anyhow::bail!(
        "Unsupported script target '{}'. Use 'all' or 'server:<name>'",
        script.target
    )
}

fn script_path(config_path: &str, script: &ScriptConfig) -> Result<PathBuf> {
    let cfg = Path::new(config_path);
    let base = cfg.parent().unwrap_or_else(|| Path::new("."));
    Ok(base.join(&script.file))
}

fn load_script_content(config_path: &str, script: &ScriptConfig) -> Result<String> {
    let path = script_path(config_path, script)?;
    fs::read_to_string(&path)
        .with_context(|| format!("Failed to read script file '{}'", path.display()))
}

fn load_script_hash(config_path: &str, script: &ScriptConfig) -> Result<String> {
    let content = load_script_content(config_path, script)?;
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    Ok(format!("{:x}", hasher.finalize()))
}

fn script_state_key(script_name: &str, server: &str) -> String {
    format!("{script_name}@{server}")
}

fn planned_action(script: &ScriptConfig, hash: &str, prior: &ScriptRunState) -> (String, String) {
    let mode = script
        .idempotency
        .as_deref()
        .unwrap_or("always")
        .to_string();
    match mode.as_str() {
        "once" if prior.last_run_unix > 0 => ("skip".to_string(), "already ran once".to_string()),
        "on-change" => {
            if prior.last_hash.as_deref() == Some(hash) {
                ("skip".to_string(), "script content unchanged".to_string())
            } else {
                ("run".to_string(), "content changed".to_string())
            }
        }
        _ => ("run".to_string(), format!("idempotency={mode}")),
    }
}

async fn execute_script_remote(
    server: &airstack_config::ServerConfig,
    script_name: &str,
    script: &ScriptConfig,
    shell: &str,
    content: &str,
) -> Result<String> {
    let marker = format!(
        "AIRSTACK_SCRIPT_{}_{}",
        script_name.replace('-', "_"),
        Uuid::new_v4().simple()
    );
    let remote_path = format!("/tmp/airstack-{}-{}.sh", script_name, now_unix());

    let mut exec_parts = vec!["env".to_string()];
    if let Some(env) = &script.env {
        let mut sorted = BTreeMap::new();
        for (k, v) in env {
            sorted.insert(k.clone(), v.clone());
        }
        for (k, v) in sorted {
            exec_parts.push(format!("{k}={v}"));
        }
    }
    exec_parts.push(shell.to_string());
    exec_parts.push(remote_path.clone());
    if let Some(args) = &script.args {
        exec_parts.extend(args.clone());
    }
    let exec_cmd = join_shell_command(&exec_parts);
    let run_cmd = if let Some(timeout) = script.timeout_secs {
        format!(
            "if command -v timeout >/dev/null 2>&1; then timeout {timeout} {exec_cmd}; else {exec_cmd}; fi"
        )
    } else {
        exec_cmd
    };

    let script_block = format!(
        "tmp={path}\ntrap 'rm -f \"$tmp\"' EXIT\ncat > \"$tmp\" <<'{marker}'\n{content}\n{marker}\nchmod +x \"$tmp\"\n{run_cmd}",
        path = remote_path,
        marker = marker,
        content = content,
        run_cmd = run_cmd
    );

    let out = execute_remote_command(server, &["sh".to_string(), "-lc".to_string(), script_block])
        .await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let detail = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            format!("exit={:?}", out.status.code())
        };
        anyhow::bail!("remote script failed: {}", detail);
    }
    Ok("ok".to_string())
}

fn is_transient_script_error(message: &str) -> bool {
    let msg = message.to_ascii_lowercase();
    msg.contains("timeout")
        || msg.contains("connection reset")
        || msg.contains("temporarily unavailable")
        || msg.contains("broken pipe")
        || msg.contains("network")
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{planned_action, resolve_target_servers};
    use crate::state::ScriptRunState;
    use airstack_config::{AirstackConfig, InfraConfig, ProjectConfig, ScriptConfig, ServerConfig};

    fn test_config() -> AirstackConfig {
        AirstackConfig {
            project: ProjectConfig {
                name: "demo".to_string(),
                description: None,
                deploy_mode: Some("remote".to_string()),
            },
            infra: Some(InfraConfig {
                servers: vec![
                    ServerConfig {
                        name: "web-1".to_string(),
                        provider: "hetzner".to_string(),
                        region: "hel1".to_string(),
                        server_type: "cpx21".to_string(),
                        ssh_key: "~/.ssh/id_ed25519.pub".to_string(),
                        floating_ip: Some(false),
                    },
                    ServerConfig {
                        name: "web-2".to_string(),
                        provider: "hetzner".to_string(),
                        region: "hel1".to_string(),
                        server_type: "cpx21".to_string(),
                        ssh_key: "~/.ssh/id_ed25519.pub".to_string(),
                        floating_ip: Some(false),
                    },
                ],
                firewall: None,
            }),
            services: None,
            edge: None,
            scripts: None,
            hooks: None,
        }
    }

    #[test]
    fn resolve_target_servers_all_and_specific() {
        let cfg = test_config();
        let all_script = ScriptConfig {
            target: "all".to_string(),
            file: "scripts/bootstrap.sh".to_string(),
            shell: None,
            args: None,
            env: None,
            idempotency: None,
            timeout_secs: None,
            retry: None,
        };
        let one_script = ScriptConfig {
            target: "server:web-2".to_string(),
            ..all_script.clone()
        };

        let all =
            resolve_target_servers(&cfg, &all_script, None, false).expect("all should resolve");
        assert_eq!(all.len(), 2);
        let one = resolve_target_servers(&cfg, &one_script, None, false)
            .expect("specific server should resolve");
        assert_eq!(one[0].name, "web-2");
    }

    #[test]
    fn planned_action_respects_idempotency_modes() {
        let mut script = ScriptConfig {
            target: "all".to_string(),
            file: "scripts/bootstrap.sh".to_string(),
            shell: None,
            args: None,
            env: None,
            idempotency: Some("once".to_string()),
            timeout_secs: None,
            retry: None,
        };
        let prior = ScriptRunState {
            last_hash: Some("abc".to_string()),
            last_run_unix: 123,
        };

        let (action_once, _) = planned_action(&script, "abc", &prior);
        assert_eq!(action_once, "skip");

        script.idempotency = Some("on-change".to_string());
        let (same_action, _) = planned_action(&script, "abc", &prior);
        assert_eq!(same_action, "skip");
        let (changed_action, _) = planned_action(&script, "def", &prior);
        assert_eq!(changed_action, "run");

        script.idempotency = Some("always".to_string());
        let (always_action, _) = planned_action(&script, "abc", &prior);
        assert_eq!(always_action, "run");
    }
}
