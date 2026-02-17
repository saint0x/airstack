use crate::output;
use airstack_config::AirstackConfig;
use airstack_metal::get_provider as get_metal_provider;
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::process::Command;
use tracing::info;

#[derive(Debug, Serialize)]
struct SshOutput {
    target: String,
    ip: String,
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

    let provider_config = HashMap::new();

    let metal_provider = get_metal_provider(&server_config.provider, provider_config)
        .with_context(|| format!("Failed to initialize {} provider", server_config.provider))?;

    // Find the server to get its IP address
    let servers = metal_provider
        .list_servers()
        .await
        .context("Failed to list servers")?;

    let server = servers
        .iter()
        .find(|s| s.name == target)
        .with_context(|| format!("Server '{}' not found in provider", target))?;

    let ip = server
        .public_ip
        .as_ref()
        .context("Server has no public IP address")?;

    output::line(format!("🔌 Connecting to {} ({})", target, ip));

    // Prepare SSH command
    let mut ssh_cmd = Command::new("ssh");
    ssh_cmd.args(&["-o", "StrictHostKeyChecking=no"]);
    ssh_cmd.args(&["-o", "UserKnownHostsFile=/dev/null"]);
    ssh_cmd.args(&["-o", "LogLevel=ERROR"]);

    // Add SSH key if specified
    if !server_config.ssh_key.is_empty()
        && (server_config.ssh_key.starts_with("~") || server_config.ssh_key.starts_with("/"))
    {
        let key_path = if server_config.ssh_key.starts_with("~") {
            let home = dirs::home_dir().context("Could not find home directory")?;
            home.join(&server_config.ssh_key[2..])
        } else {
            server_config.ssh_key.clone().into()
        };

        ssh_cmd.args(&["-i", &key_path.to_string_lossy()]);
    }

    // Default to root user for most cloud providers
    let ssh_target = format!("root@{}", ip);
    ssh_cmd.arg(&ssh_target);

    // Allocate a TTY for interactive sessions so tools like sudo and htop work correctly
    if command.is_empty() {
        ssh_cmd.arg("-t");
    }

    // Add command if specified
    if !command.is_empty() {
        if output::is_json() {
            // continue; JSON payload emitted after execution
        }
        ssh_cmd.args(&command);

        output::line(format!("🔧 Executing: {}", command.join(" ")));

        let output = ssh_cmd.output().context("Failed to execute SSH command")?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output::is_json() {
            output::emit_json(&SshOutput {
                target: target.to_string(),
                ip: ip.to_string(),
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

        let status = ssh_cmd.status().context("Failed to start SSH session")?;

        if !status.success() {
            anyhow::bail!("SSH session failed with exit code: {:?}", status.code());
        }
    }

    Ok(())
}
