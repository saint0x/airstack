use airstack_config::{AirstackConfig, InfraConfig, ServerConfig};
use airstack_container::get_provider as get_container_provider;
use airstack_metal::{get_provider as get_metal_provider, Server};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::process::Command;
use tokio::task::JoinSet;
use tracing::{info, warn};

use crate::output;
use crate::ssh_utils::execute_remote_command;
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

#[derive(Debug, Deserialize)]
struct FlyMachineStatusLine {
    id: String,
    name: Option<String>,
    state: Option<String>,
    config: Option<FlyMachineStatusConfig>,
}

#[derive(Debug, Deserialize)]
struct FlyMachineStatusConfig {
    image: Option<String>,
    services: Option<Vec<FlyMachineService>>,
}

#[derive(Debug, Deserialize)]
struct FlyMachineService {
    internal_port: Option<u16>,
    ports: Option<Vec<FlyMachinePort>>,
}

#[derive(Debug, Deserialize)]
struct FlyMachinePort {
    port: Option<u16>,
    handlers: Option<Vec<String>>,
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

        let provider_servers = fetch_provider_servers(infra).await;

        for server in &infra.servers {
            match provider_servers.get(&server.provider) {
                Some(Ok(servers)) => {
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
                                output::line(format!("      Type: {}", found_server.server_type));
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
                Some(Err(e)) => {
                    warn!(
                        "Failed to initialize or query provider {} for {}: {}",
                        server.provider, server.name, e
                    );
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
                            last_status: Some("ProviderError".to_string()),
                            last_checked_unix: checked_at,
                            last_error: Some(e.clone()),
                        },
                    );
                    infra_records.push(ServerStatusRecord {
                        name: server.name.clone(),
                        status: "ProviderError".to_string(),
                        cached_health: Some(HealthState::Unhealthy.as_str().to_string()),
                        cached_last_checked_unix: Some(checked_at),
                        public_ip: None,
                        private_ip: None,
                        server_type: Some(server.server_type.clone()),
                        region: Some(server.region.clone()),
                        note: Some(e.clone()),
                    });
                }
                None => {
                    let checked_at = unix_now();
                    let note = format!(
                        "provider '{}' was not scheduled for lookup",
                        server.provider
                    );
                    warn!(
                        "No provider lookup result available for {}: {}",
                        server.name, note
                    );
                    infra_records.push(ServerStatusRecord {
                        name: server.name.clone(),
                        status: "ProviderError".to_string(),
                        cached_health: Some(HealthState::Unhealthy.as_str().to_string()),
                        cached_last_checked_unix: Some(checked_at),
                        public_ip: None,
                        private_ip: None,
                        server_type: Some(server.server_type.clone()),
                        region: Some(server.region.clone()),
                        note: Some(note),
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
        let mut probe_set = JoinSet::new();
        for server_cfg in &infra.servers {
            let cfg = server_cfg.clone();
            probe_set.spawn(async move {
                let server_name = cfg.name.clone();
                let result = if cfg.provider == "fly" {
                    inspect_fly_workloads_for_server(&cfg).await
                } else {
                    inspect_remote_containers_for_server(&cfg).await
                };
                (server_name, result)
            });
        }

        let mut probe_results: HashMap<String, Result<Vec<RemoteContainerRecord>>> = HashMap::new();
        while let Some(joined) = probe_set.join_next().await {
            match joined {
                Ok((server_name, result)) => {
                    probe_results.insert(server_name, result);
                }
                Err(e) => {
                    warn!("Remote container probe task failed to join: {}", e);
                }
            }
        }

        // Preserve configured server order for stable output.
        for server_cfg in &infra.servers {
            if let Some(result) = probe_results.remove(&server_cfg.name) {
                match result {
                    Ok(mut containers) => remote_containers.append(&mut containers),
                    Err(e) => {
                        warn!(
                            "Remote container inventory failed for {}: {}",
                            server_cfg.name, e
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

async fn inspect_remote_containers_for_server(
    server_cfg: &ServerConfig,
) -> Result<Vec<RemoteContainerRecord>> {
    let scripts = [
        "docker ps --format '{{json .}}'",
        "sudo -n docker ps --format '{{json .}}'",
        "podman ps --format '{{json .}}'",
        "sudo -n podman ps --format '{{json .}}'",
    ];

    let mut last_err = String::new();
    for script in scripts {
        let out = execute_remote_command(
            server_cfg,
            &["sh".to_string(), "-lc".to_string(), script.to_string()],
        )
        .await?;

        if out.status.success() {
            return parse_remote_container_lines(server_cfg, &out.stdout);
        }

        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if !stderr.is_empty() {
            last_err = stderr;
        }
    }

    anyhow::bail!("remote container inventory failed: {}", last_err);
}

fn parse_remote_container_lines(
    server_cfg: &ServerConfig,
    stdout: &[u8],
) -> Result<Vec<RemoteContainerRecord>> {
    let stdout = String::from_utf8_lossy(stdout);
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
    Ok(items)
}

async fn inspect_fly_workloads_for_server(
    server_cfg: &ServerConfig,
) -> Result<Vec<RemoteContainerRecord>> {
    let mut cmd = Command::new("flyctl");
    cmd.arg("machine")
        .arg("list")
        .arg("--app")
        .arg(&server_cfg.name)
        .arg("--json");

    if let Ok(token) = std::env::var("FLY_API_TOKEN") {
        cmd.env("FLY_API_TOKEN", token.clone());
        cmd.env("FLY_ACCESS_TOKEN", token);
    } else if let Ok(token) = std::env::var("FLY_ACCESS_TOKEN") {
        cmd.env("FLY_API_TOKEN", token.clone());
        cmd.env("FLY_ACCESS_TOKEN", token);
    }

    let out = cmd
        .output()
        .await
        .context("Failed to execute flyctl machine list")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("fly machine list failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    let machines: Vec<FlyMachineStatusLine> =
        serde_json::from_str(&stdout).context("Failed to parse fly machine list JSON")?;

    let mut records = Vec::new();
    for machine in machines {
        let ports = machine
            .config
            .as_ref()
            .and_then(|c| c.services.as_ref())
            .map(|services| {
                let mut out = Vec::new();
                for svc in services {
                    let internal = svc.internal_port.unwrap_or_default();
                    if let Some(mapped) = &svc.ports {
                        for p in mapped {
                            let external = p.port.unwrap_or_default();
                            let handlers = p
                                .handlers
                                .as_ref()
                                .map(|h| h.join("+"))
                                .unwrap_or_else(|| "raw".to_string());
                            out.push(format!("{}->{} ({})", external, internal, handlers));
                        }
                    } else if internal > 0 {
                        out.push(format!("internal:{}", internal));
                    }
                }
                out
            })
            .unwrap_or_default();

        records.push(RemoteContainerRecord {
            server: server_cfg.name.clone(),
            name: machine.name.unwrap_or_else(|| machine.id.clone()),
            id: machine.id.clone(),
            image: machine
                .config
                .as_ref()
                .and_then(|c| c.image.clone())
                .unwrap_or_else(|| "fly-machine".to_string()),
            status: machine.state.unwrap_or_else(|| "unknown".to_string()),
            ports,
        });
    }

    Ok(records)
}

async fn fetch_provider_servers(
    infra: &InfraConfig,
) -> HashMap<String, Result<Vec<Server>, String>> {
    let mut lookup_set = JoinSet::new();
    let mut providers = std::collections::HashSet::new();

    for server in &infra.servers {
        if providers.insert(server.provider.clone()) {
            let provider = server.provider.clone();
            lookup_set.spawn(async move {
                let provider_config = HashMap::new();
                let result = match get_metal_provider(&provider, provider_config) {
                    Ok(metal_provider) => metal_provider
                        .list_servers()
                        .await
                        .map_err(|e| format!("error checking status: {}", e)),
                    Err(e) => Err(format!("provider error: {}", e)),
                };
                (provider, result)
            });
        }
    }

    let mut by_provider = HashMap::new();
    while let Some(joined) = lookup_set.join_next().await {
        match joined {
            Ok((provider, result)) => {
                by_provider.insert(provider, result);
            }
            Err(e) => {
                let err = format!("provider lookup task failed: {}", e);
                warn!("{}", err);
            }
        }
    }
    by_provider
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
