use crate::ssh_utils::{execute_remote_command, join_shell_command};
use airstack_config::{AirstackConfig, HealthcheckConfig, ServerConfig, ServiceConfig};
use anyhow::{Context, Result};
use std::process::Output;
use tokio::time::{sleep, Duration};

#[derive(Debug, Clone)]
pub enum RuntimeTarget {
    Local,
    Remote(ServerConfig),
}

#[derive(Debug, Clone)]
pub struct RuntimeDeployResult {
    pub id: String,
    pub status: String,
    pub ports: Vec<String>,
}

pub fn resolve_target(
    config: &AirstackConfig,
    service: &ServiceConfig,
    allow_local_deploy: bool,
) -> Result<RuntimeTarget> {
    let infra = config.infra.as_ref();
    let infra_present = infra.is_some_and(|i| !i.servers.is_empty());

    let deploy_mode = config
        .project
        .deploy_mode
        .as_deref()
        .unwrap_or(if infra_present { "remote" } else { "local" });

    match deploy_mode {
        "local" => {
            if infra_present && !allow_local_deploy {
                anyhow::bail!(
                    "Unsafe local deploy blocked: infra servers exist. Use remote deploy mode or pass --allow-local-deploy"
                );
            }
            Ok(RuntimeTarget::Local)
        }
        "remote" => {
            let infra =
                infra.context("Remote deploy mode selected but no infra.servers configured")?;
            let target_name = service
                .target_server
                .clone()
                .or_else(|| infra.servers.first().map(|s| s.name.clone()))
                .context("Remote deploy mode requires at least one infra server")?;
            let server = infra
                .servers
                .iter()
                .find(|s| s.name == target_name)
                .with_context(|| {
                    format!("target server '{}' not found in infra.servers", target_name)
                })?
                .clone();
            if server.provider == "fly" {
                anyhow::bail!(
                    "Remote service deploy to provider='fly' is not supported via docker runtime. Use Fly-native deploy flow"
                );
            }
            Ok(RuntimeTarget::Remote(server))
        }
        _ => anyhow::bail!(
            "Invalid deploy mode '{}'. Expected local|remote",
            deploy_mode
        ),
    }
}

pub async fn existing_service_image(target: &RuntimeTarget, name: &str) -> Result<Option<String>> {
    let output = run_shell(
        target,
        &format!("docker ps -a --filter name=^/{name}$ --format '{{{{.Image}}}}' | head -n 1"),
    )
    .await?;

    if !output.status.success() {
        return Ok(None);
    }

    let image = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if image.is_empty() {
        Ok(None)
    } else {
        Ok(Some(image))
    }
}

pub async fn deploy_service(
    target: &RuntimeTarget,
    name: &str,
    service: &ServiceConfig,
) -> Result<RuntimeDeployResult> {
    let mut run_parts = vec![
        "docker".to_string(),
        "run".to_string(),
        "-d".to_string(),
        "--name".to_string(),
        name.to_string(),
        "--restart".to_string(),
        "unless-stopped".to_string(),
    ];

    for port in &service.ports {
        run_parts.push("-p".to_string());
        run_parts.push(format!("{}:{}", port, port));
    }

    if let Some(env) = &service.env {
        for (key, value) in env {
            run_parts.push("-e".to_string());
            run_parts.push(format!("{}={}", key, value));
        }
    }

    if let Some(vols) = &service.volumes {
        for volume in vols {
            run_parts.push("-v".to_string());
            run_parts.push(volume.clone());
        }
    }

    run_parts.push(service.image.clone());

    let script = format!(
        "docker rm -f {name} >/dev/null 2>&1 || true; {}",
        join_shell_command(&run_parts)
    );

    let run_out = run_shell(target, &script).await?;
    if !run_out.status.success() {
        let stderr = String::from_utf8_lossy(&run_out.stderr);
        anyhow::bail!("Failed to deploy service '{}': {}", name, stderr.trim());
    }

    inspect_service(target, name).await
}

pub async fn rollback_service(
    target: &RuntimeTarget,
    name: &str,
    previous_image: &str,
    service: &ServiceConfig,
) -> Result<()> {
    let mut rollback_cfg = service.clone();
    rollback_cfg.image = previous_image.to_string();
    let _ = deploy_service(target, name, &rollback_cfg).await?;
    Ok(())
}

pub async fn run_healthcheck(
    target: &RuntimeTarget,
    name: &str,
    healthcheck: &HealthcheckConfig,
) -> Result<()> {
    let retries = healthcheck.retries.unwrap_or(10);
    let interval = Duration::from_secs(healthcheck.interval_secs.unwrap_or(5));

    let cmd = join_shell_command(&healthcheck.command);
    let check_script = format!("docker exec {} sh -lc {}", name, shell_quote(&cmd));

    let mut last_err = String::new();
    for _ in 0..retries {
        let out = run_shell(target, &check_script).await?;
        if out.status.success() {
            return Ok(());
        }
        last_err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        sleep(interval).await;
    }

    anyhow::bail!("Healthcheck failed for service '{}': {}", name, last_err)
}

async fn inspect_service(target: &RuntimeTarget, name: &str) -> Result<RuntimeDeployResult> {
    let inspect = run_shell(
        target,
        &format!(
            "docker ps -a --filter name=^/{name}$ --format '{{{{.ID}}}}|{{{{.Image}}}}|{{{{.Status}}}}|{{{{.Ports}}}}' | head -n 1"
        ),
    )
    .await?;

    if !inspect.status.success() {
        let stderr = String::from_utf8_lossy(&inspect.stderr);
        anyhow::bail!(
            "Failed to inspect deployed service '{}': {}",
            name,
            stderr.trim()
        );
    }

    let line = String::from_utf8_lossy(&inspect.stdout).trim().to_string();
    if line.is_empty() {
        anyhow::bail!("Deployed service '{}' was not found after deploy", name);
    }

    let parts: Vec<&str> = line.split('|').collect();
    let id = parts.first().copied().unwrap_or_default().to_string();
    let status = parts.get(2).copied().unwrap_or_default().to_string();
    let ports = parts
        .get(3)
        .map(|p| {
            p.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(RuntimeDeployResult {
        id,
        status,
        ports,
    })
}

async fn run_shell(target: &RuntimeTarget, script: &str) -> Result<Output> {
    match target {
        RuntimeTarget::Local => {
            let out = std::process::Command::new("sh")
                .arg("-lc")
                .arg(script)
                .output()
                .context("Failed to execute local shell command")?;
            Ok(out)
        }
        RuntimeTarget::Remote(server_cfg) => {
            execute_remote_command(
                server_cfg,
                &["sh".to_string(), "-lc".to_string(), script.to_string()],
            )
            .await
        }
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
