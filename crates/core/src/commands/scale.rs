use airstack_config::AirstackConfig;
use airstack_container::{
    get_provider as get_container_provider, Container, ContainerStatus, RunServiceRequest,
};
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use tracing::info;

use crate::output;
use crate::state::{LocalState, ServiceState};

#[derive(Debug, Serialize)]
struct ScaleOutput {
    service: String,
    previous_replicas: usize,
    target_replicas: usize,
    started: Vec<String>,
    restarted: Vec<String>,
    removed: Vec<String>,
}

pub async fn run(config_path: &str, service_name: &str, replicas: usize) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let mut state = LocalState::load(&config.project.name)?;

    if replicas == 0 {
        anyhow::bail!("Replica count must be at least 1");
    }

    let services = config
        .services
        .context("No services defined in configuration")?;

    let service = services
        .get(service_name)
        .with_context(|| format!("Service '{}' not found in configuration", service_name))?;

    info!(
        "Scaling service '{}' to {} replica(s)",
        service_name, replicas
    );

    let container_provider =
        get_container_provider("docker").context("Failed to initialize Docker provider")?;

    let containers = container_provider
        .list_containers()
        .await
        .context("Failed to list current containers")?;

    let existing = detect_service_replicas(service_name, &containers);
    let current_count = existing.len();

    output::line(format!(
        "📈 Scaling service '{}' from {} to {} replica(s)",
        service_name, current_count, replicas
    ));

    let mut started = Vec::new();
    let mut restarted = Vec::new();
    let mut removed = Vec::new();

    for replica in 1..=replicas {
        let container_name = replica_name(service_name, replica);
        let exists_running = existing
            .get(&replica)
            .map(|c| matches!(c.status, ContainerStatus::Running))
            .unwrap_or(false);

        if exists_running {
            continue;
        }

        let request = RunServiceRequest {
            name: container_name.clone(),
            image: service.image.clone(),
            ports: remap_ports(&service.ports, replica)?,
            env: service.env.clone(),
            volumes: service.volumes.clone(),
            restart_policy: Some("unless-stopped".to_string()),
        };

        container_provider
            .run_service(request)
            .await
            .with_context(|| format!("Failed to start replica {}", container_name))?;

        if existing.contains_key(&replica) {
            restarted.push(container_name.clone());
            output::line(format!("🔄 Recreated replica: {}", container_name));
        } else {
            started.push(container_name.clone());
            output::line(format!("✅ Started replica: {}", container_name));
        }
    }

    for (&replica, container) in existing.iter().rev() {
        if replica > replicas {
            container_provider
                .stop_service(&container.name)
                .await
                .with_context(|| format!("Failed to stop replica {}", container.name))?;
            removed.push(container.name.clone());
            output::line(format!("🗑️  Removed replica: {}", container.name));
        }
    }

    if output::is_json() {
        output::emit_json(&ScaleOutput {
            service: service_name.to_string(),
            previous_replicas: current_count,
            target_replicas: replicas,
            started,
            restarted,
            removed,
        })?;
    } else {
        output::line("🎯 Scale operation completed.");
    }

    state.services.insert(
        service_name.to_string(),
        ServiceState {
            image: service.image.clone(),
            replicas,
            containers: (1..=replicas)
                .map(|r| replica_name(service_name, r))
                .collect(),
        },
    );
    state.save()?;

    Ok(())
}

fn detect_service_replicas(
    service_name: &str,
    containers: &[Container],
) -> BTreeMap<usize, Container> {
    let mut replicas = BTreeMap::new();

    for container in containers {
        if container.name == service_name {
            replicas.insert(1, container.clone());
            continue;
        }

        if let Some(replica_index) = parse_replica_index(service_name, &container.name) {
            replicas.insert(replica_index, container.clone());
        }
    }

    replicas
}

fn parse_replica_index(service_name: &str, container_name: &str) -> Option<usize> {
    let prefix = format!("{}-", service_name);
    if !container_name.starts_with(&prefix) {
        return None;
    }

    let suffix = &container_name[prefix.len()..];
    suffix.parse::<usize>().ok().filter(|n| *n >= 1)
}

fn replica_name(service_name: &str, replica: usize) -> String {
    if replica == 1 {
        service_name.to_string()
    } else {
        format!("{}-{}", service_name, replica)
    }
}

fn remap_ports(base_ports: &[u16], replica: usize) -> Result<Vec<u16>> {
    if replica == 1 {
        return Ok(base_ports.to_vec());
    }

    let mut ports = Vec::with_capacity(base_ports.len());
    let mut seen = HashSet::new();
    let offset = replica.saturating_sub(1) as u32;

    for port in base_ports {
        let mapped = u32::from(*port) + offset;
        if mapped > u32::from(u16::MAX) {
            anyhow::bail!(
                "Port remap overflow for base port {} at replica {}",
                port,
                replica
            );
        }
        let mapped_u16 = mapped as u16;
        if !seen.insert(mapped_u16) {
            anyhow::bail!(
                "Port remap collision detected for replica {} on port {}",
                replica,
                mapped_u16
            );
        }
        ports.push(mapped_u16);
    }

    Ok(ports)
}

#[cfg(test)]
mod tests {
    use super::{parse_replica_index, remap_ports, replica_name};

    #[test]
    fn replica_name_uses_legacy_single_name() {
        assert_eq!(replica_name("api", 1), "api");
        assert_eq!(replica_name("api", 2), "api-2");
    }

    #[test]
    fn parse_replica_suffix() {
        assert_eq!(parse_replica_index("api", "api-3"), Some(3));
        assert_eq!(parse_replica_index("api", "api"), None);
        assert_eq!(parse_replica_index("api", "worker-2"), None);
        assert_eq!(parse_replica_index("api", "api-a"), None);
    }

    #[test]
    fn remap_ports_keeps_first_replica() {
        assert_eq!(remap_ports(&[80, 443], 1).unwrap(), vec![80, 443]);
    }

    #[test]
    fn remap_ports_offsets_subsequent_replicas() {
        assert_eq!(remap_ports(&[80, 443], 3).unwrap(), vec![82, 445]);
    }
}
