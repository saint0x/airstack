use airstack_config::AirstackConfig;
use airstack_container::get_provider as get_container_provider;
use airstack_metal::get_provider as get_metal_provider;
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::HashMap;
use tracing::{info, warn};

use crate::output;
use crate::state::{DriftReport, HealthState, LocalState, ServerState, ServiceState};

#[derive(Debug, Serialize)]
struct ServerStatusRecord {
    name: String,
    status: String,
    cached_health: Option<String>,
    cached_last_checked_unix: Option<u64>,
    public_ip: Option<String>,
    private_ip: Option<String>,
    server_type: Option<String>,
    region: Option<String>,
    note: Option<String>,
}

#[derive(Debug, Serialize)]
struct ServiceStatusRecord {
    name: String,
    status: String,
    cached_health: Option<String>,
    cached_last_checked_unix: Option<u64>,
    image: Option<String>,
    ports: Vec<String>,
    note: Option<String>,
}

#[derive(Debug, Serialize)]
struct StatusOutput {
    project: String,
    description: Option<String>,
    infrastructure: Vec<ServerStatusRecord>,
    services: Vec<ServiceStatusRecord>,
    drift: DriftReport,
}

pub async fn run(config_path: &str, detailed: bool) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let mut state = LocalState::load(&config.project.name)?;
    let drift = state.detect_drift(&config);

    info!("Checking status for project: {}", config.project.name);

    let mut infra_records = Vec::new();
    let mut service_records = Vec::new();

    if !output::is_json() {
        output::line("📊 Airstack Status Report");
        output::line(format!("Project: {}", config.project.name));
        if let Some(desc) = &config.project.description {
            output::line(format!("Description: {}", desc));
        }
        output::line("");
    }

    if let Some(infra) = &config.infra {
        if !output::is_json() {
            output::line("🏗️  Infrastructure Status:");
        }

        for server in &infra.servers {
            let provider_config = HashMap::new();
            match get_metal_provider(&server.provider, provider_config) {
                Ok(metal_provider) => match metal_provider.list_servers().await {
                    Ok(servers) => {
                        if let Some(found_server) = servers.iter().find(|s| s.name == server.name) {
                            let status_text = format!("{:?}", found_server.status);
                            let cached_health = map_server_health(found_server.status.clone());
                            let checked_at = unix_now();

                            state.servers.insert(
                                server.name.clone(),
                                ServerState {
                                    provider: server.provider.clone(),
                                    id: Some(found_server.id.clone()),
                                    public_ip: found_server.public_ip.clone(),
                                    health: cached_health,
                                    last_status: Some(status_text.clone()),
                                    last_checked_unix: checked_at,
                                    last_error: None,
                                },
                            );

                            if !output::is_json() {
                                let status_icon = match found_server.status {
                                    airstack_metal::ServerStatus::Running => "✅",
                                    airstack_metal::ServerStatus::Creating => "🔄",
                                    airstack_metal::ServerStatus::Stopped => "⏹️",
                                    airstack_metal::ServerStatus::Deleting => "🗑️",
                                    airstack_metal::ServerStatus::Error => "❌",
                                };
                                output::line(format!(
                                    "   {} {} ({})",
                                    status_icon, found_server.name, status_text
                                ));
                                if detailed {
                                    if let Some(ip) = &found_server.public_ip {
                                        output::line(format!("      Public IP: {}", ip));
                                    }
                                    if let Some(ip) = &found_server.private_ip {
                                        output::line(format!("      Private IP: {}", ip));
                                    }
                                    output::line(format!(
                                        "      Type: {}",
                                        found_server.server_type
                                    ));
                                    output::line(format!("      Region: {}", found_server.region));
                                    output::line(format!(
                                        "      Cached health: {} @ {}",
                                        cached_health.as_str(),
                                        checked_at
                                    ));
                                }
                            }

                            infra_records.push(ServerStatusRecord {
                                name: found_server.name.clone(),
                                status: status_text,
                                cached_health: Some(cached_health.as_str().to_string()),
                                cached_last_checked_unix: Some(checked_at),
                                public_ip: found_server.public_ip.clone(),
                                private_ip: found_server.private_ip.clone(),
                                server_type: Some(found_server.server_type.clone()),
                                region: Some(found_server.region.clone()),
                                note: None,
                            });
                        } else {
                            let checked_at = unix_now();
                            state.servers.insert(
                                server.name.clone(),
                                ServerState {
                                    provider: server.provider.clone(),
                                    id: None,
                                    public_ip: None,
                                    health: HealthState::Unhealthy,
                                    last_status: Some("NotFound".to_string()),
                                    last_checked_unix: checked_at,
                                    last_error: Some("not found in provider".to_string()),
                                },
                            );

                            if !output::is_json() {
                                output::line(format!("   ❓ {} (not found)", server.name));
                            }
                            infra_records.push(ServerStatusRecord {
                                name: server.name.clone(),
                                status: "NotFound".to_string(),
                                cached_health: Some(HealthState::Unhealthy.as_str().to_string()),
                                cached_last_checked_unix: Some(checked_at),
                                public_ip: None,
                                private_ip: None,
                                server_type: Some(server.server_type.clone()),
                                region: Some(server.region.clone()),
                                note: Some("not found in provider".to_string()),
                            });
                        }
                    }
                    Err(e) => {
                        warn!("Failed to check server {}: {}", server.name, e);
                        let checked_at = unix_now();
                        let prev_id = state.servers.get(&server.name).and_then(|s| s.id.clone());
                        let prev_ip = state
                            .servers
                            .get(&server.name)
                            .and_then(|s| s.public_ip.clone());
                        state.servers.insert(
                            server.name.clone(),
                            ServerState {
                                provider: server.provider.clone(),
                                id: prev_id,
                                public_ip: prev_ip,
                                health: HealthState::Unhealthy,
                                last_status: Some("Error".to_string()),
                                last_checked_unix: checked_at,
                                last_error: Some(format!("error checking status: {}", e)),
                            },
                        );

                        if !output::is_json() {
                            output::line(format!("   ❌ {} (error checking status)", server.name));
                        }
                        infra_records.push(ServerStatusRecord {
                            name: server.name.clone(),
                            status: "Error".to_string(),
                            cached_health: Some(HealthState::Unhealthy.as_str().to_string()),
                            cached_last_checked_unix: Some(checked_at),
                            public_ip: None,
                            private_ip: None,
                            server_type: Some(server.server_type.clone()),
                            region: Some(server.region.clone()),
                            note: Some(format!("error checking status: {}", e)),
                        });
                    }
                },
                Err(e) => {
                    warn!("Failed to initialize provider for {}: {}", server.name, e);
                    let checked_at = unix_now();
                    let prev_id = state.servers.get(&server.name).and_then(|s| s.id.clone());
                    let prev_ip = state
                        .servers
                        .get(&server.name)
                        .and_then(|s| s.public_ip.clone());
                    state.servers.insert(
                        server.name.clone(),
                        ServerState {
                            provider: server.provider.clone(),
                            id: prev_id,
                            public_ip: prev_ip,
                            health: HealthState::Unhealthy,
                            last_status: Some("ProviderError".to_string()),
                            last_checked_unix: checked_at,
                            last_error: Some(format!("provider error: {}", e)),
                        },
                    );

                    if !output::is_json() {
                        output::line(format!("   ❌ {} (provider error)", server.name));
                    }
                    infra_records.push(ServerStatusRecord {
                        name: server.name.clone(),
                        status: "ProviderError".to_string(),
                        cached_health: Some(HealthState::Unhealthy.as_str().to_string()),
                        cached_last_checked_unix: Some(checked_at),
                        public_ip: None,
                        private_ip: None,
                        server_type: Some(server.server_type.clone()),
                        region: Some(server.region.clone()),
                        note: Some(format!("provider error: {}", e)),
                    });
                }
            }
        }

        if !output::is_json() {
            output::line("");
        }
    }

    if let Some(services) = &config.services {
        if !output::is_json() {
            output::line("🚀 Services Status:");
        }

        match get_container_provider("docker") {
            Ok(container_provider) => {
                for (service_name, service_config) in services {
                    match container_provider.get_container(service_name).await {
                        Ok(container) => {
                            let status_text = format!("{:?}", container.status);
                            let cached_health = map_container_health(container.status.clone());
                            let checked_at = unix_now();
                            let replicas = state
                                .services
                                .get(service_name)
                                .map(|s| s.replicas)
                                .unwrap_or(1);
                            let containers = state
                                .services
                                .get(service_name)
                                .map(|s| s.containers.clone())
                                .unwrap_or_else(|| vec![service_name.clone()]);

                            state.services.insert(
                                service_name.clone(),
                                ServiceState {
                                    image: container.image.clone(),
                                    replicas,
                                    containers,
                                    health: cached_health,
                                    last_status: Some(status_text.clone()),
                                    last_checked_unix: checked_at,
                                    last_error: None,
                                },
                            );

                            if !output::is_json() {
                                let status_icon = match container.status {
                                    airstack_container::ContainerStatus::Running => "✅",
                                    airstack_container::ContainerStatus::Creating => "🔄",
                                    airstack_container::ContainerStatus::Stopped => "⏹️",
                                    airstack_container::ContainerStatus::Exited => "💀",
                                    airstack_container::ContainerStatus::Restarting => "🔄",
                                    _ => "❓",
                                };
                                output::line(format!(
                                    "   {} {} ({})",
                                    status_icon, service_name, status_text
                                ));

                                if detailed {
                                    output::line(format!("      Image: {}", container.image));
                                    output::line(format!(
                                        "      Cached health: {} @ {}",
                                        cached_health.as_str(),
                                        checked_at
                                    ));
                                    if !container.ports.is_empty() {
                                        output::line("      Ports:");
                                        for port in &container.ports {
                                            if let Some(host_port) = port.host_port {
                                                output::line(format!(
                                                    "        localhost:{} -> {}",
                                                    host_port, port.container_port
                                                ));
                                            }
                                        }
                                    }
                                }
                            }

                            let ports = container
                                .ports
                                .iter()
                                .filter_map(|port| {
                                    port.host_port.map(|host_port| {
                                        format!("localhost:{}->{}", host_port, port.container_port)
                                    })
                                })
                                .collect::<Vec<_>>();

                            service_records.push(ServiceStatusRecord {
                                name: service_name.clone(),
                                status: status_text,
                                cached_health: Some(cached_health.as_str().to_string()),
                                cached_last_checked_unix: Some(checked_at),
                                image: Some(container.image.clone()),
                                ports,
                                note: None,
                            });
                        }
                        Err(_) => {
                            let checked_at = unix_now();
                            let replicas = state
                                .services
                                .get(service_name)
                                .map(|s| s.replicas)
                                .unwrap_or(0);
                            let containers = state
                                .services
                                .get(service_name)
                                .map(|s| s.containers.clone())
                                .unwrap_or_default();
                            state.services.insert(
                                service_name.clone(),
                                ServiceState {
                                    image: service_config.image.clone(),
                                    replicas,
                                    containers,
                                    health: HealthState::Unhealthy,
                                    last_status: Some("NotDeployed".to_string()),
                                    last_checked_unix: checked_at,
                                    last_error: Some("container not found".to_string()),
                                },
                            );

                            if !output::is_json() {
                                output::line(format!("   ❓ {} (not deployed)", service_name));
                                if detailed {
                                    output::line(format!(
                                        "      Configured image: {}",
                                        service_config.image
                                    ));
                                    output::line(format!(
                                        "      Configured ports: {:?}",
                                        service_config.ports
                                    ));
                                }
                            }
                            service_records.push(ServiceStatusRecord {
                                name: service_name.clone(),
                                status: "NotDeployed".to_string(),
                                cached_health: Some(HealthState::Unhealthy.as_str().to_string()),
                                cached_last_checked_unix: Some(checked_at),
                                image: Some(service_config.image.clone()),
                                ports: Vec::new(),
                                note: Some("container not found".to_string()),
                            });
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Failed to initialize container provider: {}", e);
                if !output::is_json() {
                    output::line("   ❌ Unable to check container status");
                }
                service_records.push(ServiceStatusRecord {
                    name: "docker".to_string(),
                    status: "ProviderError".to_string(),
                    cached_health: Some(HealthState::Unhealthy.as_str().to_string()),
                    cached_last_checked_unix: Some(unix_now()),
                    image: None,
                    ports: Vec::new(),
                    note: Some(format!("container provider init failed: {}", e)),
                });
            }
        }
        if !output::is_json() {
            output::line("");
        }
    }

    state.save()?;

    if output::is_json() {
        output::emit_json(&StatusOutput {
            project: config.project.name,
            description: config.project.description,
            infrastructure: infra_records,
            services: service_records,
            drift,
        })?;
    } else {
        if !drift.missing_servers_in_cache.is_empty()
            || !drift.extra_servers_in_cache.is_empty()
            || !drift.missing_services_in_cache.is_empty()
            || !drift.extra_services_in_cache.is_empty()
        {
            output::line("🟢 State Drift (cache vs config):");
            if !drift.missing_servers_in_cache.is_empty() {
                output::line(format!(
                    "   Missing servers in cache: {:?}",
                    drift.missing_servers_in_cache
                ));
            }
            if !drift.extra_servers_in_cache.is_empty() {
                output::line(format!(
                    "   Extra servers in cache: {:?}",
                    drift.extra_servers_in_cache
                ));
            }
            if !drift.missing_services_in_cache.is_empty() {
                output::line(format!(
                    "   Missing services in cache: {:?}",
                    drift.missing_services_in_cache
                ));
            }
            if !drift.extra_services_in_cache.is_empty() {
                output::line(format!(
                    "   Extra services in cache: {:?}",
                    drift.extra_services_in_cache
                ));
            }
            output::line("");
        }
        output::line("Use 'airstack status --detailed' for more information");
    }

    Ok(())
}

fn map_server_health(status: airstack_metal::ServerStatus) -> HealthState {
    use airstack_metal::ServerStatus;

    match status {
        ServerStatus::Running => HealthState::Healthy,
        ServerStatus::Creating => HealthState::Degraded,
        ServerStatus::Stopped | ServerStatus::Deleting | ServerStatus::Error => {
            HealthState::Unhealthy
        }
    }
}

fn map_container_health(status: airstack_container::ContainerStatus) -> HealthState {
    use airstack_container::ContainerStatus;

    match status {
        ContainerStatus::Running => HealthState::Healthy,
        ContainerStatus::Creating | ContainerStatus::Restarting => HealthState::Degraded,
        ContainerStatus::Stopped
        | ContainerStatus::Paused
        | ContainerStatus::Removing
        | ContainerStatus::Dead
        | ContainerStatus::Exited => HealthState::Unhealthy,
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
