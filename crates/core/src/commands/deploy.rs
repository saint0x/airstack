use crate::commands::edge;
use crate::commands::release;
use crate::dependencies::deployment_order;
use crate::deploy_runtime::{
    collect_container_diagnostics, deploy_service_with_strategy, evaluate_service_health,
    existing_service_image, resolve_target, rollback_service, DeployStrategy,
};
use crate::output;
use crate::state::{HealthState, LocalState, ServiceState};
use airstack_config::AirstackConfig;
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::process::Command;
use tracing::info;

#[derive(Debug, Serialize)]
struct DeployRecord {
    service: String,
    container_id: String,
    status: String,
    ports: Vec<String>,
    deployed: bool,
    running: bool,
    healthy: Option<bool>,
    discoverable: bool,
    detected_by: String,
}

#[derive(Debug, Serialize)]
struct DeployOutput {
    requested: String,
    order: Vec<String>,
    deployed: Vec<DeployRecord>,
}

pub async fn run(
    config_path: &str,
    service_name: &str,
    _target: Option<String>,
    allow_local_deploy: bool,
    latest_code: bool,
    push: bool,
    tag: Option<String>,
    strategy: String,
    canary_seconds: u64,
) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let mut state = LocalState::load(&config.project.name)?;

    info!("Deploying service: {}", service_name);

    let services = config
        .services
        .as_ref()
        .context("No services defined in configuration")?;

    let order = if service_name == "all" {
        deployment_order(services, None)?
    } else {
        deployment_order(services, Some(service_name))?
    };

    let mut image_overrides: HashMap<String, String> = HashMap::new();
    if latest_code {
        if service_name == "all" {
            anyhow::bail!("--latest-code requires an explicit single service, not 'all'");
        }
        let svc = services
            .get(service_name)
            .with_context(|| format!("Service '{}' not found in configuration", service_name))?;
        let base_image = svc.image.split(':').next().unwrap_or(&svc.image);
        let resolved_tag = tag.unwrap_or(git_sha()?);
        let built_image = format!("{}:{}", base_image, resolved_tag);
        let remote_mode = is_remote_deploy_mode(&config);
        let local_docker_ok = local_docker_available();

        if !local_docker_ok && remote_mode {
            if !push {
                anyhow::bail!(
                    "Local Docker daemon unavailable and deploy mode is remote. --latest-code in remote mode requires --push so remote hosts can pull the built image."
                );
            }
            let remote_server = svc
                .target_server
                .clone()
                .or_else(|| {
                    config
                        .infra
                        .as_ref()
                        .and_then(|i| i.servers.first().map(|s| s.name.clone()))
                })
                .context("Remote deploy mode selected but no infra server was found for --latest-code fallback")?;
            output::line(format!(
                "ℹ️ local Docker unavailable; using remote build fallback on '{}'",
                remote_server
            ));
            release::run(
                config_path,
                release::ReleaseArgs {
                    service: service_name.to_string(),
                    tag: Some(resolved_tag.clone()),
                    push: true,
                    update_config: false,
                    remote_build: Some(remote_server),
                    from: release::ReleaseFrom::Build,
                },
            )
            .await?;
        } else {
            release::preflight_local_docker_available()?;
            run_cmd("docker", &["build", "-t", &built_image, "."])?;
            if push {
                run_cmd("docker", &["push", &built_image])?;
            }
        }
        image_overrides.insert(service_name.to_string(), built_image);
    } else if let Some(tag) = tag {
        if service_name == "all" {
            anyhow::bail!("--tag requires an explicit single service, not 'all'");
        }
        let svc = services
            .get(service_name)
            .with_context(|| format!("Service '{}' not found in configuration", service_name))?;
        let base_image = svc.image.split(':').next().unwrap_or(&svc.image);
        let override_image = format!("{}:{}", base_image, tag);
        image_overrides.insert(service_name.to_string(), override_image);
    }

    output::line(format!("🚀 Deploying request: {}", service_name));

    let mut deployed = Vec::new();
    let strategy = DeployStrategy::parse(&strategy)?;

    for deploy_name in &order {
        let mut service = services
            .get(deploy_name.as_str())
            .with_context(|| format!("Service '{}' not found in configuration", deploy_name))?;
        let mut service_override = service.clone();
        if let Some(image) = image_overrides.get(deploy_name) {
            service_override.image = image.clone();
            service = &service_override;
        }

        output::line(format!(
            "   {} -> {} (ports: {:?})",
            deploy_name, service.image, service.ports
        ));

        let runtime_target = resolve_target(&config, service, allow_local_deploy)?;
        let previous_image = existing_service_image(&runtime_target, deploy_name).await?;

        let mut container = deploy_service_with_strategy(
            &runtime_target,
            deploy_name,
            service,
            service.healthcheck.as_ref(),
            strategy,
            canary_seconds,
        )
        .await
        .with_context(|| format!("Failed to deploy service {}", deploy_name))?;

        if service.healthcheck.is_some() {
            if let Err(err) =
                evaluate_service_health(&runtime_target, deploy_name, service, false, 1, false)
                    .await
                    .and_then(|eval| {
                        if eval.ok {
                            Ok(())
                        } else {
                            anyhow::bail!("{}", eval.detail)
                        }
                    })
            {
                container.healthy = Some(false);
                let diag = collect_container_diagnostics(&runtime_target, deploy_name).await;
                if let Some(prev) = &previous_image {
                    let _ = rollback_service(&runtime_target, deploy_name, prev, service).await;
                    output::line(format!(
                        "↩️ rollback target for {} -> image {}",
                        deploy_name, prev
                    ));
                }
                return Err(err).with_context(|| {
                    format!(
                        "Healthcheck gate failed for service '{}' (rolled back if possible). diagnostics: {}",
                        deploy_name, diag
                    )
                });
            }
            container.healthy = Some(true);
        } else {
            container.healthy = None;
        }

        output::line(format!(
            "✅ Successfully deployed service: {} ({})",
            deploy_name, container.id
        ));

        deployed.push(DeployRecord {
            service: deploy_name.to_string(),
            container_id: container.id.clone(),
            status: container.status.clone(),
            ports: container.ports.clone(),
            deployed: true,
            running: container.running,
            healthy: container.healthy,
            discoverable: container.discoverable,
            detected_by: container.detected_by.clone(),
        });

        state.services.insert(
            deploy_name.to_string(),
            ServiceState {
                image: service.image.clone(),
                replicas: 1,
                containers: vec![deploy_name.to_string()],
                health: map_container_health_text(&container.status),
                last_status: Some(container.status.clone()),
                last_checked_unix: unix_now(),
                last_error: None,
                last_deploy_command: Some(format!("airstack deploy {}", deploy_name)),
                last_deploy_unix: Some(unix_now()),
                image_origin: Some(if latest_code && push {
                    "registry-pushed".to_string()
                } else if latest_code {
                    "local-build-only".to_string()
                } else {
                    "config-declared".to_string()
                }),
            },
        );

        if deploy_name == "caddy" && config.edge.is_some() {
            edge::apply_from_config(&config)
                .await
                .with_context(|| "Failed to sync edge config during caddy deploy")?;
            output::line("✅ edge config reconciled during caddy deploy");
        }
    }

    state.save()?;

    if output::is_json() {
        let payload = DeployOutput {
            requested: service_name.to_string(),
            order,
            deployed,
        };
        output::emit_json(&payload)?;
    } else if deployed.is_empty() {
        output::line("No services were deployed.");
    } else {
        output::line("🎯 Deploy operation completed.");
    }

    Ok(())
}

fn is_remote_deploy_mode(config: &AirstackConfig) -> bool {
    if let Some(mode) = config.project.deploy_mode.as_deref() {
        return mode == "remote";
    }
    config.infra.as_ref().is_some_and(|i| !i.servers.is_empty())
}

fn local_docker_available() -> bool {
    std::process::Command::new("docker")
        .arg("info")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn run_cmd(cmd: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(cmd)
        .args(args)
        .status()
        .with_context(|| format!("Failed to execute {}", cmd))?;
    if !status.success() {
        anyhow::bail!("Command failed: {} {}", cmd, args.join(" "));
    }
    Ok(())
}

fn git_sha() -> Result<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .context("Failed to execute git rev-parse")?;
    if !out.status.success() {
        anyhow::bail!("Failed to determine git SHA");
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn map_container_health_text(status: &str) -> HealthState {
    let s = status.to_ascii_lowercase();
    if s.contains("up") || s.contains("running") {
        HealthState::Healthy
    } else if s.contains("restart") || s.contains("start") {
        HealthState::Degraded
    } else {
        HealthState::Unhealthy
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
