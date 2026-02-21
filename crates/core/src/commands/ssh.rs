use crate::output;
use crate::ssh_utils::{
    execute_remote_command, execute_remote_shell_command, join_shell_command, start_remote_session,
};
use airstack_config::AirstackConfig;
use anyhow::{Context, Result};
use serde::Serialize;
use std::path::Path;
use tracing::info;

#[derive(Debug, Serialize)]
struct SshOutput {
    target: String,
    endpoint: String,
    command: Vec<String>,
    exit_code: i32,
    stdout: String,
    stderr: String,
}

pub struct SshExec {
    pub command: Vec<String>,
    pub cmd: Option<String>,
    pub script: Option<String>,
}

pub async fn run(config_path: &str, target: &str, exec: SshExec) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;

    info!("Connecting to server: {}", target);

    let infra = config
        .infra
        .context("No infrastructure defined in configuration")?;

    let server_config = infra
        .servers
        .iter()
        .find(|s| s.name == target)
        .with_context(|| format!("Server '{}' not found in configuration", target))?;

    let endpoint = if server_config.provider == "fly" {
        "flyctl-ssh".to_string()
    } else {
        "ssh".to_string()
    };

    output::line(format!("🔌 Connecting to {} via {}", target, endpoint));

    let command_modes = usize::from(!exec.command.is_empty())
        + usize::from(exec.cmd.is_some())
        + usize::from(exec.script.is_some());
    if command_modes > 1 {
        anyhow::bail!("Use only one execution mode: --cmd, --script, or -- <argv...>");
    }

    // Add command if specified
    if command_modes == 1 {
        let (exec_display, output, output_command) = if let Some(cmd) = exec.cmd {
            let display = format!("sh -lc {}", shell_quote(&cmd));
            (
                display.clone(),
                execute_remote_shell_command(server_config, &display).await?,
                vec!["sh".to_string(), "-lc".to_string(), cmd],
            )
        } else if let Some(script_path) = exec.script {
            let script = std::fs::read_to_string(&script_path)
                .with_context(|| format!("Failed to read script '{}'", script_path))?;
            let script_name = Path::new(&script_path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("script");
            let wrapped = format!(
                "cat <<'AIRSTACK_SCRIPT' >/tmp/{script_name}.airstack.sh\n{script}\nAIRSTACK_SCRIPT\nchmod +x /tmp/{script_name}.airstack.sh\nsh /tmp/{script_name}.airstack.sh"
            );
            let display = format!("sh -lc {}", shell_quote(&wrapped));
            (
                display.clone(),
                execute_remote_shell_command(server_config, &display).await?,
                vec!["sh".to_string(), "-lc".to_string(), wrapped],
            )
        } else {
            let display = join_shell_command(&exec.command);
            (
                display,
                execute_remote_command(server_config, &exec.command).await?,
                exec.command.clone(),
            )
        };

        output::line(format!("🔧 Executing: {}", exec_display));
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output::is_json() {
            output::emit_json(&SshOutput {
                target: target.to_string(),
                endpoint: endpoint.clone(),
                command: output_command,
                exit_code: output.status.code().unwrap_or(1),
                stdout,
                stderr,
            })?;
        } else {
            if !stdout.is_empty() {
                print!("{}", stdout);
                if !stdout.ends_with('\n') {
                    println!();
                }
            }
            if !stderr.is_empty() {
                output::error_line(stderr);
            }
        }

        if !output.status.success() {
            anyhow::bail!(
                "SSH command failed with exit code: {:?}",
                output.status.code()
            );
        }
    } else {
        if output::is_json() {
            anyhow::bail!("Interactive SSH cannot be used with --json. Pass a command to execute.");
        }
        // Interactive SSH session
        output::line("🖥️  Starting interactive SSH session...");
        let code = start_remote_session(server_config, &[]).await?;

        if code != 0 {
            anyhow::bail!("SSH session failed with exit code: {}", code);
        }
    }

    Ok(())
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || "-_./:".contains(ch))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
