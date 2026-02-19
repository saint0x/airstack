use airstack_config::AirstackConfig;
use anyhow::{Context, Result};
use serde::Serialize;
use tracing::info;

use crate::output;
use crate::ssh_utils::{execute_remote_command, start_remote_session};

#[derive(Debug, Serialize)]
struct ContainerExecOutput {
    server: String,
    container: String,
    command: Vec<String>,
    exit_code: i32,
    stdout: String,
    stderr: String,
}

pub async fn run(
    config_path: &str,
    server: &str,
    container: &str,
    command: Vec<String>,
) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let infra = config
        .infra
        .context("No infrastructure defined in configuration")?;

    let server_cfg = infra
        .servers
        .iter()
        .find(|s| s.name == server)
        .with_context(|| format!("Server '{}' not found in configuration", server))?;

    info!(
        "Executing command in remote container '{}' on {} via {}",
        container, server, server_cfg.provider
    );

    if command.is_empty() {
        if output::is_json() {
            anyhow::bail!(
                "Interactive container exec cannot be used with --json. Provide a command."
            );
        }
        let shell_cmd = vec![
            "docker".to_string(),
            "exec".to_string(),
            "-it".to_string(),
            container.to_string(),
            "sh".to_string(),
        ];
        let code = start_remote_session(server_cfg, &shell_cmd).await?;
        if code != 0 {
            anyhow::bail!("Interactive container shell failed with {}", code);
        }
        return Ok(());
    }

    let mut remote_cmd = vec![
        "docker".to_string(),
        "exec".to_string(),
        container.to_string(),
    ];
    remote_cmd.extend(command.iter().cloned());
    let result = execute_remote_command(server_cfg, &remote_cmd).await?;
    let stdout = String::from_utf8_lossy(&result.stdout).to_string();
    let stderr = String::from_utf8_lossy(&result.stderr).to_string();

    if output::is_json() {
        output::emit_json(&ContainerExecOutput {
            server: server.to_string(),
            container: container.to_string(),
            command,
            exit_code: result.status.code().unwrap_or(1),
            stdout,
            stderr,
        })?;
    } else {
        if !stdout.is_empty() {
            print!("{stdout}");
        }
        if !stderr.is_empty() {
            output::error_line(stderr);
        }
    }

    if !result.status.success() {
        anyhow::bail!(
            "Remote container command failed with exit code {:?}",
            result.status.code()
        );
    }

    Ok(())
}
