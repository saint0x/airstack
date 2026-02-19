use crate::output;
use crate::ssh_utils::{execute_remote_command, start_remote_session};
use airstack_config::AirstackConfig;
use anyhow::{Context, Result};
use serde::Serialize;
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

pub async fn run(config_path: &str, target: &str, command: Vec<String>) -> Result<()> {
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

    // Add command if specified
    if !command.is_empty() {
        output::line(format!("🔧 Executing: {}", command.join(" ")));
        let output = execute_remote_command(server_config, &command).await?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output::is_json() {
            output::emit_json(&SshOutput {
                target: target.to_string(),
                endpoint: endpoint.clone(),
                command: command.clone(),
                exit_code: output.status.code().unwrap_or(1),
                stdout,
                stderr,
            })?;
        } else {
            if !stdout.is_empty() {
                println!("{}", stdout);
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
