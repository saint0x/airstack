use airstack_config::AirstackConfig;
use airstack_container::get_provider as get_container_provider;
use airstack_metal::get_provider as get_metal_provider;
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::HashMap;
use tracing::{info, warn};

use crate::output;
use crate::state::DriftReport;
use crate::state::LocalState;

#[derive(Debug, Serialize)]
struct ServerStatusRecord {
    name: String,
    status: String,
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
    let state = LocalState::load(&config.project.name)?;
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
                                }
                            }

                            infra_records.push(ServerStatusRecord {
                                name: found_server.name.clone(),
                                status: status_text,
                                public_ip: found_server.public_ip.clone(),
                                private_ip: found_server.private_ip.clone(),
                                server_type: Some(found_server.server_type.clone()),
                                region: Some(found_server.region.clone()),
                                note: None,
                            });
                        } else {
                            if !output::is_json() {
                                output::line(format!("   ❓ {} (not found)", server.name));
                            }
                            infra_records.push(ServerStatusRecord {
                                name: server.name.clone(),
                                status: "NotFound".to_string(),
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
                        if !output::is_json() {
                            output::line(format!("   ❌ {} (error checking status)", server.name));
                        }
                        infra_records.push(ServerStatusRecord {
                            name: server.name.clone(),
                            status: "Error".to_string(),
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
                    if !output::is_json() {
                        output::line(format!("   ❌ {} (provider error)", server.name));
                    }
                    infra_records.push(ServerStatusRecord {
                        name: server.name.clone(),
                        status: "ProviderError".to_string(),
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
                                image: Some(container.image.clone()),
                                ports,
                                note: None,
                            });
                        }
                        Err(_) => {
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
