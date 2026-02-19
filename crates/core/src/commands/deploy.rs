use crate::dependencies::deployment_order;
use crate::deploy_runtime::{
    deploy_service, existing_service_image, resolve_target, rollback_service, run_healthcheck,
};
use crate::output;
use crate::state::{HealthState, LocalState, ServiceState};
use airstack_config::AirstackConfig;
use anyhow::{Context, Result};
use serde::Serialize;
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

pub async fn run(
    config_path: &str,
    service_name: &str,
    _target: Option<String>,
    allow_local_deploy: bool,
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

        let runtime_target = resolve_target(&config, service, allow_local_deploy)?;
        let previous_image = existing_service_image(&runtime_target, deploy_name).await?;

        let container = deploy_service(&runtime_target, deploy_name, service)
            .await
            .with_context(|| format!("Failed to deploy service {}", deploy_name))?;

        if let Some(hc) = &service.healthcheck {
            if let Err(err) = run_healthcheck(&runtime_target, deploy_name, hc).await {
                if let Some(prev) = &previous_image {
                    let _ = rollback_service(&runtime_target, deploy_name, prev, service).await;
                }
                return Err(err).with_context(|| {
                    format!(
                        "Healthcheck gate failed for service '{}' (rolled back if possible)",
                        deploy_name
                    )
                });
            }
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
