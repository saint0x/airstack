use airstack_config::AirstackConfig;
use airstack_container::{get_provider as get_container_provider, RunServiceRequest};
use airstack_metal::{get_provider as get_metal_provider, CreateServerRequest};
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;
use tracing::{info, warn};

use crate::dependencies::deployment_order;
use crate::output;
use crate::retry::retry_with_backoff;
use crate::state::{LocalState, ServerState, ServiceState};

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
        for server in &infra.servers {
            info!("Planning server: {} ({})", server.name, server.server_type);

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
                    server.name, server.server_type, server.region
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
                output::line(format!(
                    "✅ Server already exists: {} ({})",
                    existing_server.name, existing_server.id
                ));
                server_records.push(UpServerRecord {
                    name: existing_server.name,
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
                    },
                );
                continue;
            }

            let request = CreateServerRequest {
                name: server.name.clone(),
                server_type: server.server_type.clone(),
                region: server.region.clone(),
                ssh_key: server.ssh_key.clone(),
                attach_floating_ip: server.floating_ip.unwrap_or(false),
            };

            match retry_with_backoff(
                3,
                Duration::from_millis(300),
                &format!("create server '{}'", server.name),
                |_| metal_provider.create_server(request.clone()),
            )
            .await
            {
                Ok(created_server) => {
                    let created_id = created_server.id.clone();
                    let created_ip = created_server.public_ip.clone();
                    output::line(format!(
                        "✅ Created server: {} ({})",
                        created_server.name, created_server.id
                    ));
                    if let Some(ip) = &created_server.public_ip {
                        output::line(format!("   Public IP: {}", ip));
                    }
                    server_records.push(UpServerRecord {
                        name: created_server.name,
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
                        },
                    );
                }
                Err(e) => {
                    warn!("Failed to create server {}: {}", server.name, e);
                    return Err(e);
                }
            }
        }
    }

    if let Some(services) = &config.services {
        let order = deployment_order(services, None)?;
        let container_provider =
            get_container_provider("docker").context("Failed to initialize Docker provider")?;

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

            let request = RunServiceRequest {
                name: service_name.clone(),
                image: service.image.clone(),
                ports: service.ports.clone(),
                env: service.env.clone(),
                volumes: service.volumes.clone(),
                restart_policy: Some("unless-stopped".to_string()),
            };

            let container = retry_with_backoff(
                3,
                Duration::from_millis(250),
                &format!("deploy service '{}'", service_name),
                |_| container_provider.run_service(request.clone()),
            )
            .await
            .with_context(|| format!("Failed to deploy service {}", service_name))?;

            output::line(format!(
                "✅ Deployed service: {} ({})",
                service_name, container.id
            ));
            service_records.push(UpServiceRecord {
                name: service_name.clone(),
                image: service.image.clone(),
                container_id: Some(container.id),
            });
            state.services.insert(
                service_name.clone(),
                ServiceState {
                    image: service.image.clone(),
                    replicas: 1,
                    containers: vec![service_name.clone()],
                },
            );
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
