use airstack_config::AirstackConfig;
use anyhow::{Context, Result};
use serde::Serialize;
use tokio::process::Command;
use tracing::info;

use crate::output;
use crate::ssh_utils::{
    execute_remote_command, join_shell_command, resolve_fly_target, start_remote_session,
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

pub struct ContainerExec {
    pub command: Vec<String>,
    pub cmd: Option<String>,
    pub script: Option<String>,
}

pub async fn run(
    config_path: &str,
    server: &str,
    container: &str,
    exec: ContainerExec,
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

    let command_modes = usize::from(!exec.command.is_empty())
        + usize::from(exec.cmd.is_some())
        + usize::from(exec.script.is_some());
    if command_modes > 1 {
        anyhow::bail!("Use only one execution mode: --cmd, --script, or -- <argv...>");
    }

    if server_cfg.provider == "fly" {
        return run_fly_container_exec(server, container, server_cfg, exec).await;
    }

    if command_modes == 0 {
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
    let requested_command = if let Some(cmd) = exec.cmd {
        remote_cmd.push("sh".to_string());
        remote_cmd.push("-lc".to_string());
        remote_cmd.push(cmd.clone());
        vec!["sh".to_string(), "-lc".to_string(), cmd]
    } else if let Some(script_path) = exec.script {
        let script = std::fs::read_to_string(&script_path)
            .with_context(|| format!("Failed to read script '{}'", script_path))?;
        remote_cmd.push("sh".to_string());
        remote_cmd.push("-lc".to_string());
        remote_cmd.push(script.clone());
        vec!["sh".to_string(), "-lc".to_string(), script]
    } else {
        remote_cmd.extend(exec.command.iter().cloned());
        exec.command.clone()
    };
    if !output::is_json() {
        output::line(format!("🔧 Executing: {}", join_shell_command(&remote_cmd)));
    }

    let result = execute_remote_command(server_cfg, &remote_cmd).await?;
    let stdout = String::from_utf8_lossy(&result.stdout).to_string();
    let stderr = String::from_utf8_lossy(&result.stderr).to_string();

    if output::is_json() {
        output::emit_json(&ContainerExecOutput {
            server: server.to_string(),
            container: container.to_string(),
            command: requested_command,
            exit_code: result.status.code().unwrap_or(1),
            stdout,
            stderr,
        })?;
    } else {
        if !stdout.is_empty() {
            print!("{stdout}");
            if !stdout.ends_with('\n') {
                println!();
            }
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

async fn run_fly_container_exec(
    server: &str,
    container: &str,
    server_cfg: &airstack_config::ServerConfig,
    exec: ContainerExec,
) -> Result<()> {
    let (app, machine) = resolve_fly_target(server_cfg).await?;
    let command_modes = usize::from(!exec.command.is_empty())
        + usize::from(exec.cmd.is_some())
        + usize::from(exec.script.is_some());
    if command_modes == 0 {
        if output::is_json() {
            anyhow::bail!(
                "Interactive container exec cannot be used with --json. Provide a command."
            );
        }
        let mut fly = Command::new("flyctl");
        fly.arg("ssh")
            .arg("console")
            .arg("--app")
            .arg(&app)
            .arg("--container")
            .arg(container);
        if let Some(machine) = machine {
            fly.arg("--machine").arg(machine);
        }
        let status = fly
            .status()
            .await
            .context("Failed to start Fly container shell")?;
        if !status.success() {
            anyhow::bail!(
                "Interactive Fly container shell failed with {:?}",
                status.code()
            );
        }
        return Ok(());
    }

    let requested_command = if let Some(cmd) = &exec.cmd {
        vec!["sh".to_string(), "-lc".to_string(), cmd.clone()]
    } else if let Some(script_path) = &exec.script {
        let script = std::fs::read_to_string(script_path)
            .with_context(|| format!("Failed to read script '{}'", script_path))?;
        vec!["sh".to_string(), "-lc".to_string(), script]
    } else {
        exec.command.clone()
    };
    let fly_command = join_shell_command(&requested_command);

    let mut fly = Command::new("flyctl");

    fly.arg("ssh")
        .arg("console")
        .arg("--app")
        .arg(&app)
        .arg("--container")
        .arg(container)
        .arg("--command")
        .arg(&fly_command);
    if !output::is_json() {
        output::line(format!(
            "🔧 Executing: flyctl ssh console --app {} --container {} --command {}",
            app, container, fly_command
        ));
    }
    if let Some(machine) = machine {
        fly.arg("--machine").arg(machine);
    }
    let result = fly
        .output()
        .await
        .context("Failed to execute Fly container command")?;
    let stdout = String::from_utf8_lossy(&result.stdout).to_string();
    let stderr = String::from_utf8_lossy(&result.stderr).to_string();

    if output::is_json() {
        output::emit_json(&ContainerExecOutput {
            server: server.to_string(),
            container: container.to_string(),
            command: requested_command,
            exit_code: result.status.code().unwrap_or(1),
            stdout,
            stderr,
        })?;
    } else {
        if !stdout.is_empty() {
            print!("{stdout}");
            if !stdout.ends_with('\n') {
                println!();
            }
        }
        if !stderr.is_empty() {
            output::error_line(stderr);
        }
    }

    if !result.status.success() {
        anyhow::bail!(
            "Fly container command failed with exit code {:?}",
            result.status.code()
        );
    }

    Ok(())
}
