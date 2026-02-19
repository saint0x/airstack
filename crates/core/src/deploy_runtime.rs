use crate::ssh_utils::{execute_remote_command, join_shell_command};
use airstack_config::{AirstackConfig, HealthcheckConfig, ServerConfig, ServiceConfig};
use anyhow::{Context, Result};
use serde::Serialize;
use std::process::Output;
use tokio::time::{sleep, Duration};

#[derive(Debug, Clone)]
pub enum RuntimeTarget {
    Local,
    Remote(ServerConfig),
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeDeployResult {
    pub id: String,
    pub status: String,
    pub ports: Vec<String>,
    pub running: bool,
    pub discoverable: bool,
    pub detected_by: String,
    pub healthy: Option<bool>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub enum DeployStrategy {
    Rolling,
    BlueGreen,
    Canary,
}

impl DeployStrategy {
    pub fn parse(input: &str) -> Result<Self> {
        match input {
            "rolling" => Ok(Self::Rolling),
            "bluegreen" => Ok(Self::BlueGreen),
            "canary" => Ok(Self::Canary),
            _ => anyhow::bail!(
                "Invalid deploy strategy '{}'. Expected one of: rolling|bluegreen|canary",
                input
            ),
        }
    }
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
        &format!("docker inspect -f '{{{{.Config.Image}}}}' {name} 2>/dev/null || true"),
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
    preflight_image_access(target, &service.image).await?;

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
        "docker rm -f {name} >/dev/null 2>&1 || true; \
         for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do \
           docker container inspect {name} >/dev/null 2>&1 || break; \
           docker rm -f {name} >/dev/null 2>&1 || true; \
           sleep 0.2; \
         done; \
         {}",
        join_shell_command(&run_parts)
    );

    let run_out = run_shell(target, &script).await?;
    if !run_out.status.success() {
        let stderr = String::from_utf8_lossy(&run_out.stderr);
        anyhow::bail!("Failed to deploy service '{}': {}", name, stderr.trim());
    }

    let launched_id = String::from_utf8_lossy(&run_out.stdout).trim().to_string();
    inspect_service(target, name, Some(launched_id)).await
}

pub async fn deploy_service_with_strategy(
    target: &RuntimeTarget,
    name: &str,
    service: &ServiceConfig,
    healthcheck: Option<&HealthcheckConfig>,
    strategy: DeployStrategy,
    canary_seconds: u64,
) -> Result<RuntimeDeployResult> {
    match strategy {
        DeployStrategy::Rolling => deploy_service(target, name, service).await,
        DeployStrategy::BlueGreen | DeployStrategy::Canary => {
            // Candidate runs without host port bindings to avoid conflicts while validating the new image.
            let candidate_name = format!("{}__candidate", name);
            let mut candidate = service.clone();
            candidate.ports = Vec::new();

            let _ = deploy_service(target, &candidate_name, &candidate).await?;

            if let Some(hc) = healthcheck {
                if let Err(err) = run_healthcheck(target, &candidate_name, hc).await {
                    let _ = run_shell(
                        target,
                        &format!("docker rm -f {} >/dev/null 2>&1 || true", candidate_name),
                    )
                    .await;
                    return Err(err).with_context(|| {
                        format!(
                            "Candidate validation failed for '{}' with strategy {:?}",
                            name, strategy
                        )
                    });
                }
            }

            if strategy == DeployStrategy::Canary && canary_seconds > 0 {
                sleep(Duration::from_secs(canary_seconds)).await;
            }

            let promoted = match deploy_service(target, name, service).await {
                Ok(v) => v,
                Err(e) => {
                    let _ = run_shell(
                        target,
                        &format!("docker rm -f {} >/dev/null 2>&1 || true", candidate_name),
                    )
                    .await;
                    return Err(e);
                }
            };

            let _ = run_shell(
                target,
                &format!("docker rm -f {} >/dev/null 2>&1 || true", candidate_name),
            )
            .await;

            Ok(promoted)
        }
    }
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

pub async fn preflight_image_access(target: &RuntimeTarget, image: &str) -> Result<()> {
    let script = format!(
        "docker image inspect {img} >/dev/null 2>&1 || docker pull {img}",
        img = shell_quote(image)
    );
    let out = run_shell(target, &script).await?;
    if out.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let mut hint = String::new();
    if image.starts_with("ghcr.io/") {
        hint =
            " Hint: ensure remote host has GHCR credentials (`docker login ghcr.io`) with read:packages scope."
                .to_string();
    }
    anyhow::bail!(
        "Image preflight failed for '{}': {}.{}",
        image,
        stderr,
        hint
    );
}

async fn inspect_service(
    target: &RuntimeTarget,
    name: &str,
    launched_id: Option<String>,
) -> Result<RuntimeDeployResult> {
    let inspect_id = launched_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .unwrap_or(name)
        .to_string();

    // Use docker inspect as the source of truth for discovery/existence.
    let inspect = run_shell(
        target,
        &format!(
            "docker inspect -f '{{{{.Id}}}}|{{{{.Config.Image}}}}|{{{{.State.Status}}}}' {inspect_id} 2>/dev/null || true"
        ),
    )
    .await?;
    let mut line = String::from_utf8_lossy(&inspect.stdout).trim().to_string();
    let mut detected_by = "id";

    if line.is_empty() {
        let by_name = run_shell(
            target,
            &format!(
                "docker inspect -f '{{{{.Id}}}}|{{{{.Config.Image}}}}|{{{{.State.Status}}}}' {name} 2>/dev/null || true"
            ),
        )
        .await?;
        line = String::from_utf8_lossy(&by_name.stdout).trim().to_string();
        detected_by = "name";
    }

    if line.is_empty() {
        anyhow::bail!("Deployed service '{}' was not found after deploy", name);
    }

    let mut result = parse_inspect_line(&line, detected_by)?;
    let ports_out = run_shell(
        target,
        &format!("docker ps -a --filter name=^/{name}$ --format '{{{{.Ports}}}}' | head -n 1"),
    )
    .await?;
    if ports_out.status.success() {
        let ports_line = String::from_utf8_lossy(&ports_out.stdout)
            .trim()
            .to_string();
        if !ports_line.is_empty() {
            result.ports = ports_line
                .split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect();
        }
    }
    Ok(result)
}

fn parse_inspect_line(line: &str, detected_by: &str) -> Result<RuntimeDeployResult> {
    let parts: Vec<&str> = line.split('|').collect();
    let id = parts.first().copied().unwrap_or_default().to_string();
    let status = parts.get(2).copied().unwrap_or_default().to_string();
    let ports = Vec::new();

    let s = status.to_ascii_lowercase();
    let running = s.starts_with("up") || s.contains("running") || s.contains("started");

    Ok(RuntimeDeployResult {
        id,
        status,
        ports,
        running,
        discoverable: true,
        detected_by: detected_by.to_string(),
        healthy: None,
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
