use airstack_config::{AirstackConfig, ServerConfig};
use airstack_container::get_provider as get_container_provider;
use airstack_metal::get_provider as get_metal_provider;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn};

use crate::output;
use crate::ssh_utils::{build_ssh_command, SshCommandOptions};
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

#[derive(Debug, Clone, Serialize)]
struct RemoteContainerRecord {
    server: String,
    name: String,
    id: String,
    image: String,
    status: String,
    ports: Vec<String>,
}

#[derive(Debug, Serialize)]
struct StatusOutput {
    project: String,
    description: Option<String>,
    infrastructure: Vec<ServerStatusRecord>,
    services: Vec<ServiceStatusRecord>,
    remote_containers: Vec<RemoteContainerRecord>,
    drift: DriftReport,
}

#[derive(Debug, Deserialize)]
struct DockerPsLine {
    #[serde(rename = "ID")]
    id: Option<String>,
    #[serde(rename = "Image")]
    image: Option<String>,
    #[serde(rename = "Names")]
    names: Option<String>,
    #[serde(rename = "Status")]
    status: Option<String>,
    #[serde(rename = "Ports")]
    ports: Option<String>,
}

pub async fn run(config_path: &str, detailed: bool) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let mut state = LocalState::load(&config.project.name)?;
    let drift = state.detect_drift(&config);

    info!("Checking status for project: {}", config.project.name);

    let mut infra_records = Vec::new();
    let mut service_records = Vec::new();
    let mut server_ips: HashMap<String, String> = HashMap::new();

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

                            if let Some(ip) = &found_server.public_ip {
                                server_ips.insert(server.name.clone(), ip.clone());
                            }

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
                        state.servers.insert(
                            server.name.clone(),
                            ServerState {
                                provider: server.provider.clone(),
                                id: state.servers.get(&server.name).and_then(|s| s.id.clone()),
                                public_ip: state
                                    .servers
                                    .get(&server.name)
                                    .and_then(|s| s.public_ip.clone()),
                                health: HealthState::Unhealthy,
                                last_status: Some("Error".to_string()),
                                last_checked_unix: checked_at,
                                last_error: Some(format!("error checking status: {}", e)),
                            },
                        );

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

    let mut remote_containers = Vec::new();
    if let Some(infra) = &config.infra {
        for server_cfg in &infra.servers {
            if let Some(ip) = server_ips.get(&server_cfg.name) {
                match inspect_remote_containers_for_server(server_cfg, ip) {
                    Ok(mut containers) => remote_containers.append(&mut containers),
                    Err(e) => {
                        warn!(
                            "Remote container inventory failed for {} ({}): {}",
                            server_cfg.name, ip, e
                        );
                    }
                }
            }
        }
    }

    if let Some(services) = &config.services {
        if !output::is_json() {
            output::line("🚀 Services Status:");
        }

        let local_container_provider = get_container_provider("docker").ok();

        for (service_name, service_config) in services {
            if let Some(remote) = remote_containers.iter().find(|c| c.name == *service_name) {
                let checked_at = unix_now();
                let health = map_remote_container_health(&remote.status);
                state.services.insert(
                    service_name.clone(),
                    ServiceState {
                        image: remote.image.clone(),
                        replicas: 1,
                        containers: vec![remote.name.clone()],
                        health,
                        last_status: Some(remote.status.clone()),
                        last_checked_unix: checked_at,
                        last_error: None,
                    },
                );

                if !output::is_json() {
                    output::line(format!(
                        "   ✅ {} (remote: {} on {})",
                        service_name, remote.status, remote.server
                    ));
                    if detailed {
                        output::line(format!("      Image: {}", remote.image));
                        if !remote.ports.is_empty() {
                            output::line(format!("      Ports: {}", remote.ports.join(", ")));
                        }
                    }
                }

                service_records.push(ServiceStatusRecord {
                    name: service_name.clone(),
                    status: remote.status.clone(),
                    cached_health: Some(health.as_str().to_string()),
                    cached_last_checked_unix: Some(checked_at),
                    image: Some(remote.image.clone()),
                    ports: remote.ports.clone(),
                    note: Some(format!("remote container on {}", remote.server)),
                });
                continue;
            }

            if let Some(container_provider) = &local_container_provider {
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

                        service_records.push(ServiceStatusRecord {
                            name: service_name.clone(),
                            status: status_text,
                            cached_health: Some(cached_health.as_str().to_string()),
                            cached_last_checked_unix: Some(checked_at),
                            image: Some(container.image.clone()),
                            ports: container
                                .ports
                                .iter()
                                .filter_map(|port| {
                                    port.host_port.map(|host_port| {
                                        format!("localhost:{}->{}", host_port, port.container_port)
                                    })
                                })
                                .collect(),
                            note: Some("local docker daemon".to_string()),
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
            } else {
                let checked_at = unix_now();
                service_records.push(ServiceStatusRecord {
                    name: service_name.clone(),
                    status: "ProviderError".to_string(),
                    cached_health: Some(HealthState::Unhealthy.as_str().to_string()),
                    cached_last_checked_unix: Some(checked_at),
                    image: Some(service_config.image.clone()),
                    ports: Vec::new(),
                    note: Some("container provider init failed".to_string()),
                });
            }
        }

        if !output::is_json() {
            output::line("");
        }
    }

    if detailed && !output::is_json() {
        output::line("🧱 Remote Container Inventory:");
        if remote_containers.is_empty() {
            output::line("   (none detected over SSH)");
        } else {
            for c in &remote_containers {
                output::line(format!(
                    "   • {} :: {} ({}) [{}]",
                    c.server, c.name, c.image, c.status
                ));
                if !c.ports.is_empty() {
                    output::line(format!("      Ports: {}", c.ports.join(", ")));
                }
            }
        }
        output::line("");
    }

    state.save()?;

    if output::is_json() {
        output::emit_json(&StatusOutput {
            project: config.project.name,
            description: config.project.description,
            infrastructure: infra_records,
            services: service_records,
            remote_containers,
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

fn inspect_remote_containers_for_server(
    server_cfg: &ServerConfig,
    ip: &str,
) -> Result<Vec<RemoteContainerRecord>> {
    let users = ["root", "ubuntu"];
    let mut last_err = None;

    for user in users {
        let mut ssh_cmd = build_ssh_command(
            &server_cfg.ssh_key,
            ip,
            &SshCommandOptions {
                user,
                batch_mode: true,
                connect_timeout_secs: Some(8),
                strict_host_key_checking: "accept-new",
                user_known_hosts_file: None,
                log_level: "ERROR",
            },
        )?;
        ssh_cmd.arg("docker");
        ssh_cmd.arg("ps");
        ssh_cmd.arg("--format");
        ssh_cmd.arg("'{{json .}}'");

        match ssh_cmd.output() {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let mut items = Vec::new();
                for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
                    let parsed: DockerPsLine = serde_json::from_str(line).with_context(|| {
                        format!(
                            "Failed to parse docker ps JSON for server {}: {}",
                            server_cfg.name, line
                        )
                    })?;
                    items.push(RemoteContainerRecord {
                        server: server_cfg.name.clone(),
                        name: parsed.names.unwrap_or_default(),
                        id: parsed.id.unwrap_or_default(),
                        image: parsed.image.unwrap_or_default(),
                        status: parsed.status.unwrap_or_else(|| "Unknown".to_string()),
                        ports: parsed
                            .ports
                            .filter(|p| !p.is_empty())
                            .map(|p| vec![p])
                            .unwrap_or_default(),
                    });
                }
                return Ok(items);
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                last_err = Some(anyhow::anyhow!(
                    "SSH/docker command failed as {}@{}: {}",
                    user,
                    ip,
                    stderr.trim()
                ));
            }
            Err(e) => {
                last_err = Some(
                    anyhow::Error::new(e)
                        .context(format!("Failed to execute SSH command as {}@{}", user, ip)),
                );
            }
        }
    }

    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Unable to inspect remote containers")))
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

fn map_remote_container_health(status: &str) -> HealthState {
    let s = status.to_ascii_lowercase();
    if s.starts_with("up") {
        HealthState::Healthy
    } else if s.contains("restart") {
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
