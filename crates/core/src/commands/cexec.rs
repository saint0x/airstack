use airstack_config::{AirstackConfig, ServerConfig};
use anyhow::{Context, Result};
use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;
use tracing::info;

use crate::output;

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

    let ip = resolve_server_ip(server_cfg).await?;
    info!(
        "Executing command in remote container '{}' on {} ({})",
        container, server, ip
    );

    if command.is_empty() {
        if output::is_json() {
            anyhow::bail!(
                "Interactive container exec cannot be used with --json. Provide a command."
            );
        }
        let mut ssh_cmd = build_ssh_command(server_cfg, &ip)?;
        ssh_cmd.arg("docker");
        ssh_cmd.arg("exec");
        ssh_cmd.arg("-it");
        ssh_cmd.arg(container);
        ssh_cmd.arg("sh");
        let status = ssh_cmd
            .status()
            .context("Failed to start container shell")?;
        if !status.success() {
            anyhow::bail!(
                "Interactive container shell failed with {:?}",
                status.code()
            );
        }
        return Ok(());
    }

    let mut ssh_cmd = build_ssh_command(server_cfg, &ip)?;
    ssh_cmd.arg("docker");
    ssh_cmd.arg("exec");
    ssh_cmd.arg(container);
    for part in &command {
        ssh_cmd.arg(part);
    }
    let result = ssh_cmd
        .output()
        .context("Failed to execute remote container command")?;
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

async fn resolve_server_ip(server_cfg: &ServerConfig) -> Result<String> {
    use airstack_metal::get_provider as get_metal_provider;
    use std::collections::HashMap;

    let metal_provider = get_metal_provider(&server_cfg.provider, HashMap::new())
        .with_context(|| format!("Failed to initialize {} provider", server_cfg.provider))?;
    let servers = metal_provider
        .list_servers()
        .await
        .context("Failed to list servers from provider")?;
    let found = servers
        .iter()
        .find(|s| s.name == server_cfg.name)
        .with_context(|| format!("Server '{}' not found in provider", server_cfg.name))?;
    found
        .public_ip
        .clone()
        .context("Server has no public IP address")
}

fn build_ssh_command(server_cfg: &ServerConfig, ip: &str) -> Result<Command> {
    let mut ssh_cmd = Command::new("ssh");
    ssh_cmd.args(["-o", "BatchMode=yes"]);
    ssh_cmd.args(["-o", "ConnectTimeout=10"]);
    ssh_cmd.args(["-o", "StrictHostKeyChecking=accept-new"]);
    ssh_cmd.args(["-o", "LogLevel=ERROR"]);

    if let Some(identity_path) = resolve_identity_path(&server_cfg.ssh_key)? {
        ssh_cmd.args(["-i", &identity_path.to_string_lossy()]);
    }

    ssh_cmd.arg(format!("root@{ip}"));
    Ok(ssh_cmd)
}

fn resolve_identity_path(ssh_key: &str) -> Result<Option<PathBuf>> {
    if ssh_key.is_empty() {
        return Ok(None);
    }
    if !(ssh_key.starts_with("~") || ssh_key.starts_with("/")) {
        return Ok(None);
    }

    let path = if ssh_key.starts_with("~") {
        let home = dirs::home_dir().context("Could not resolve home directory")?;
        home.join(&ssh_key[2..])
    } else {
        PathBuf::from(ssh_key)
    };

    if path.extension().is_some_and(|ext| ext == "pub") {
        let mut private = path.clone();
        private.set_extension("");
        if private.exists() {
            return Ok(Some(private));
        }
    }
    if path.exists() {
        return Ok(Some(path));
    }
    Ok(None)
}
