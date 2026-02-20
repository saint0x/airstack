use airstack_config::AirstackConfig;
use airstack_metal::{get_provider as get_metal_provider, CreateServerRequest, ServerStatus};
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;
use tracing::{info, warn};

use crate::commands::edge;
use crate::commands::script::{run_hook_scripts, ScriptRunOptions};
use crate::dependencies::deployment_order;
use crate::deploy_runtime::{
    deploy_service, evaluate_service_health, existing_service_image, resolve_target,
    rollback_service,
};
use crate::infra_preflight::{
    check_ssh_key_path, format_validation_error, is_permanent_provider_error,
    resolve_server_request,
};
use crate::output;
use crate::retry::{retry_with_backoff_classified, RetryDecision};
use crate::state::{HealthState, LocalState, ServerState, ServiceState};
use airstack_metal::CapacityResolveOptions;

#[derive(Debug, Serialize)]
struct UpServerRecord {
    name: String,
    provider: String,
    action: String,
    id: Option<String>,
    public_ip: Option<String>,
}

#[derive(Debug, Serialize)]
struct UpServiceRecord {
    name: String,
    image: String,
    container_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct UpOutput {
    project: String,
    dry_run: bool,
    servers: Vec<UpServerRecord>,
    services: Vec<UpServiceRecord>,
}

pub async fn run(
    config_path: &str,
    _target: Option<String>,
    _provider: Option<String>,
    dry_run: bool,
    allow_local_deploy: bool,
    auto_fallback: bool,
    resolve_capacity: bool,
) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let mut state = LocalState::load(&config.project.name)?;

    info!(
        "Provisioning infrastructure for project: {}",
        config.project.name
    );

    if dry_run {
        info!("Dry run enabled - no changes will be made");
    }

    let mut server_records = Vec::new();
    let mut service_records = Vec::new();

    if let Some(infra) = &config.infra {
        if let Some(hooks) = &config.hooks {
            if let Some(pre_provision) = &hooks.pre_provision {
                output::line("🔧 running pre_provision hooks");
                run_hook_scripts(
                    config_path,
                    pre_provision,
                    ScriptRunOptions {
                        dry_run,
                        explain: false,
                    },
                )
                .await
                .context("pre_provision hook execution failed")?;
            }
        }
        for server in &infra.servers {
            info!("Planning server: {} ({})", server.name, server.server_type);
            check_ssh_key_path(server)?;
            let preflight = resolve_server_request(
                server,
                CapacityResolveOptions {
                    auto_fallback,
                    resolve_capacity,
                },
            )
            .await?;
            if !preflight.validation.valid {
                anyhow::bail!("{}", format_validation_error(server, &preflight));
            }

            if dry_run {
                server_records.push(UpServerRecord {
                    name: server.name.clone(),
                    provider: server.provider.clone(),
                    action: "plan-create".to_string(),
                    id: None,
                    public_ip: None,
                });
                output::line(format!(
                    "Would create server {} ({}, {})",
                    server.name, server.server_type, preflight.request.region
                ));
                continue;
            }

            let provider_config = HashMap::new();
            let metal_provider = get_metal_provider(&server.provider, provider_config)
                .with_context(|| format!("Failed to initialize {} provider", server.provider))?;

            let existing = metal_provider
                .list_servers()
                .await
                .unwrap_or_default()
                .into_iter()
                .find(|s| s.name == server.name);

            if let Some(existing_server) = existing {
                let existing_id = existing_server.id.clone();
                let existing_ip = existing_server.public_ip.clone();
                let existing_status = existing_server.status.clone();
                output::line(format!(
                    "✅ Server already exists: {} ({})",
                    existing_server.name, existing_server.id
                ));
                server_records.push(UpServerRecord {
                    name: existing_server.name.clone(),
                    provider: server.provider.clone(),
                    action: "unchanged".to_string(),
                    id: Some(existing_id.clone()),
                    public_ip: existing_ip.clone(),
                });
                state.servers.insert(
                    server.name.clone(),
                    ServerState {
                        provider: server.provider.clone(),
                        id: Some(existing_id),
                        public_ip: existing_ip,
                        health: map_server_health(existing_status.clone()),
                        last_status: Some(format!("{:?}", existing_status)),
                        last_checked_unix: unix_now(),
                        last_error: None,
                    },
                );
                continue;
            }

            let request = CreateServerRequest {
                name: server.name.clone(),
                server_type: server.server_type.clone(),
                region: preflight.request.region.clone(),
                ssh_key: server.ssh_key.clone(),
                attach_floating_ip: server.floating_ip.unwrap_or(false),
            };

            match retry_with_backoff_classified(
                3,
                Duration::from_millis(300),
                &format!("create server '{}'", server.name),
                |err| {
                    if is_permanent_provider_error(err) {
                        RetryDecision::Stop
                    } else {
                        RetryDecision::Retry
                    }
                },
                |_| metal_provider.create_server(request.clone()),
            )
            .await
            {
                Ok(created_server) => {
                    let created_id = created_server.id.clone();
                    let created_ip = created_server.public_ip.clone();
                    let created_status = created_server.status.clone();
                    output::line(format!(
                        "✅ Created server: {} ({})",
                        created_server.name, created_server.id
                    ));
                    if let Some(ip) = &created_server.public_ip {
                        output::line(format!("   Public IP: {}", ip));
                    }
                    server_records.push(UpServerRecord {
                        name: created_server.name.clone(),
                        provider: server.provider.clone(),
                        action: "created".to_string(),
                        id: Some(created_id.clone()),
                        public_ip: created_ip.clone(),
                    });
                    state.servers.insert(
                        server.name.clone(),
                        ServerState {
                            provider: server.provider.clone(),
                            id: Some(created_id),
                            public_ip: created_ip,
                            health: map_server_health(created_status.clone()),
                            last_status: Some(format!("{:?}", created_status)),
                            last_checked_unix: unix_now(),
                            last_error: None,
                        },
                    );
                }
                Err(e) => {
                    warn!("Failed to create server {}: {}", server.name, e);
                    return Err(e);
                }
            }
        }

        if let Some(hooks) = &config.hooks {
            if let Some(post_provision) = &hooks.post_provision {
                output::line("🔧 running post_provision hooks");
                run_hook_scripts(
                    config_path,
                    post_provision,
                    ScriptRunOptions {
                        dry_run,
                        explain: false,
                    },
                )
                .await
                .context("post_provision hook execution failed")?;
            }
        }
    }

    if let Some(services) = &config.services {
        let order = deployment_order(services, None)?;

        for service_name in order {
            let service = services.get(&service_name).with_context(|| {
                format!("Service '{}' not found in configuration", service_name)
            })?;

            if dry_run {
                output::line(format!(
                    "Would deploy service {} -> {}",
                    service_name, service.image
                ));
                service_records.push(UpServiceRecord {
                    name: service_name,
                    image: service.image.clone(),
                    container_id: None,
                });
                continue;
            }

            let runtime_target = resolve_target(&config, service, allow_local_deploy)?;
            let previous_image = existing_service_image(&runtime_target, &service_name).await?;
            let deployed = deploy_service(&runtime_target, &service_name, service)
                .await
                .with_context(|| format!("Failed to deploy service {}", service_name))?;

            if service.healthcheck.is_some() {
                if let Err(err) = evaluate_service_health(
                    &runtime_target,
                    &service_name,
                    service,
                    false,
                    1,
                    false,
                )
                .await
                .and_then(|eval| {
                    if eval.ok {
                        Ok(())
                    } else {
                        anyhow::bail!("{}", eval.detail)
                    }
                }) {
                    if let Some(prev) = &previous_image {
                        let _ =
                            rollback_service(&runtime_target, &service_name, prev, service).await;
                        output::line(format!(
                            "↩️ rollback target for {} -> image {}",
                            service_name, prev
                        ));
                    }
                    return Err(err).with_context(|| {
                        format!(
                            "Healthcheck gate failed for service '{}' (rolled back if possible)",
                            service_name
                        )
                    });
                }
            }

            output::line(format!(
                "✅ Deployed service: {} ({})",
                service_name, deployed.id
            ));
            service_records.push(UpServiceRecord {
                name: service_name.clone(),
                image: service.image.clone(),
                container_id: Some(deployed.id.clone()),
            });
            state.services.insert(
                service_name.clone(),
                ServiceState {
                    image: service.image.clone(),
                    replicas: 1,
                    containers: vec![service_name.clone()],
                    health: map_container_health_text(&deployed.status),
                    last_status: Some(deployed.status),
                    last_checked_unix: unix_now(),
                    last_error: None,
                },
            );

            if service_name == "caddy" && config.edge.is_some() {
                edge::apply_from_config(&config)
                    .await
                    .with_context(|| "Failed to sync edge config during caddy deploy")?;
                output::line("✅ edge config reconciled during caddy deploy");
            }
        }

        if let Some(hooks) = &config.hooks {
            if let Some(post_deploy) = &hooks.post_deploy {
                output::line("🔧 running post_deploy hooks");
                run_hook_scripts(
                    config_path,
                    post_deploy,
                    ScriptRunOptions {
                        dry_run,
                        explain: false,
                    },
                )
                .await
                .context("post_deploy hook execution failed")?;
            }
        }
    }

    if !dry_run {
        state.save()?;
    }

    if output::is_json() {
        output::emit_json(&UpOutput {
            project: config.project.name,
            dry_run,
            servers: server_records,
            services: service_records,
        })?;
    } else {
        output::line("🎉 Up operation completed.");
    }

    Ok(())
}

fn map_server_health(status: ServerStatus) -> HealthState {
    match status {
        ServerStatus::Running => HealthState::Healthy,
        ServerStatus::Creating => HealthState::Degraded,
        ServerStatus::Stopped | ServerStatus::Deleting | ServerStatus::Error => {
            HealthState::Unhealthy
        }
    }
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
