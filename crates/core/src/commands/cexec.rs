use airstack_config::AirstackConfig;
use anyhow::{Context, Result};
use serde::Serialize;
use tracing::info;

use crate::output;
use crate::ssh_utils::{
    build_ssh_command as build_base_ssh_command, resolve_server_public_ip, SshCommandOptions,
};

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

    let ip = resolve_server_public_ip(server_cfg).await?;
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
        let mut ssh_cmd = build_container_ssh_command(&server_cfg.ssh_key, &ip)?;
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

    let mut ssh_cmd = build_container_ssh_command(&server_cfg.ssh_key, &ip)?;
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

fn build_container_ssh_command(ssh_key: &str, ip: &str) -> Result<std::process::Command> {
    build_base_ssh_command(
        ssh_key,
        ip,
        &SshCommandOptions {
            user: "root",
            batch_mode: true,
            connect_timeout_secs: Some(10),
            strict_host_key_checking: "accept-new",
            user_known_hosts_file: None,
            log_level: "ERROR",
        },
    )
}
