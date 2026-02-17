use crate::dependencies::deployment_order;
use crate::output;
use crate::retry::retry_with_backoff;
use crate::state::{HealthState, LocalState, ServiceState};
use airstack_config::AirstackConfig;
use airstack_container::{get_provider as get_container_provider, RunServiceRequest};
use anyhow::{Context, Result};
use serde::Serialize;
use std::time::Duration;
use tracing::info;

#[derive(Debug, Serialize)]
struct DeployRecord {
    service: String,
    container_id: String,
    status: String,
    ports: Vec<String>,
}

#[derive(Debug, Serialize)]
struct DeployOutput {
    requested: String,
    order: Vec<String>,
    deployed: Vec<DeployRecord>,
}

pub async fn run(config_path: &str, service_name: &str, _target: Option<String>) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let mut state = LocalState::load(&config.project.name)?;

    info!("Deploying service: {}", service_name);

    let services = config
        .services
        .context("No services defined in configuration")?;

    let order = if service_name == "all" {
        deployment_order(&services, None)?
    } else {
        deployment_order(&services, Some(service_name))?
    };

    let container_provider =
        get_container_provider("docker").context("Failed to initialize Docker provider")?;

    output::line(format!("🚀 Deploying request: {}", service_name));

    let mut deployed = Vec::new();

    for deploy_name in &order {
        let service = services
            .get(deploy_name.as_str())
            .with_context(|| format!("Service '{}' not found in configuration", deploy_name))?;

        output::line(format!(
            "   {} -> {} (ports: {:?})",
            deploy_name, service.image, service.ports
        ));

        let request = RunServiceRequest {
            name: deploy_name.to_string(),
            image: service.image.clone(),
            ports: service.ports.clone(),
            env: service.env.clone(),
            volumes: service.volumes.clone(),
            restart_policy: Some("unless-stopped".to_string()),
        };

        let container = retry_with_backoff(
            3,
            Duration::from_millis(250),
            &format!("deploy service '{}'", deploy_name),
            |_| container_provider.run_service(request.clone()),
        )
        .await
        .with_context(|| format!("Failed to deploy service {}", deploy_name))?;

        let ports = container
            .ports
            .iter()
            .filter_map(|port| {
                port.host_port
                    .map(|host_port| format!("localhost:{}->{}", host_port, port.container_port))
            })
            .collect::<Vec<_>>();

        output::line(format!(
            "✅ Successfully deployed service: {} ({})",
            deploy_name, container.id
        ));

        deployed.push(DeployRecord {
            service: deploy_name.to_string(),
            container_id: container.id.clone(),
            status: format!("{:?}", container.status),
            ports,
        });

        state.services.insert(
            deploy_name.to_string(),
            ServiceState {
                image: service.image.clone(),
                replicas: 1,
                containers: vec![deploy_name.to_string()],
                health: map_container_health(container.status.clone()),
                last_status: Some(format!("{:?}", container.status)),
                last_checked_unix: unix_now(),
                last_error: None,
            },
        );
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
