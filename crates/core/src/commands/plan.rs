use crate::output;
use airstack_config::AirstackConfig;
use airstack_metal::get_provider as get_metal_provider;
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Serialize)]
struct PlanAction {
    resource_type: String,
    resource: String,
    action: String,
    reason: String,
}

#[derive(Debug, Serialize)]
struct PlanOutput {
    project: String,
    actions: Vec<PlanAction>,
}

pub async fn run(config_path: &str, include_destroy: bool) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let mut actions = Vec::new();

    if let Some(infra) = &config.infra {
        let mut by_provider: HashMap<String, Vec<String>> = HashMap::new();
        for server in &infra.servers {
            by_provider
                .entry(server.provider.clone())
                .or_default()
                .push(server.name.clone());
        }

        for (provider, desired_names) in by_provider {
            let desired: HashSet<String> = desired_names.into_iter().collect();
            let remote = get_metal_provider(&provider, HashMap::new())
                .with_context(|| format!("Failed to initialize provider {}", provider))?
                .list_servers()
                .await
                .unwrap_or_default();
            let remote_names: HashSet<String> = remote.into_iter().map(|s| s.name).collect();

            for name in desired.difference(&remote_names) {
                actions.push(PlanAction {
                    resource_type: "server".to_string(),
                    resource: name.clone(),
                    action: "create".to_string(),
                    reason: format!("missing in provider {}", provider),
                });
            }

            for name in desired.intersection(&remote_names) {
                actions.push(PlanAction {
                    resource_type: "server".to_string(),
                    resource: name.clone(),
                    action: "noop".to_string(),
                    reason: format!("already exists in provider {}", provider),
                });
            }

            if include_destroy {
                for name in remote_names.difference(&desired) {
                    actions.push(PlanAction {
                        resource_type: "server".to_string(),
                        resource: name.clone(),
                        action: "destroy".to_string(),
                        reason: format!("exists in provider {} but not in config", provider),
                    });
                }
            }
        }
    }

    if let Some(services) = &config.services {
        for (name, svc) in services {
            actions.push(PlanAction {
                resource_type: "service".to_string(),
                resource: name.clone(),
                action: "deploy".to_string(),
                reason: format!("ensure image {} is active", svc.image),
            });
        }
    }

    if output::is_json() {
        output::emit_json(&PlanOutput {
            project: config.project.name,
            actions,
        })?;
        return Ok(());
    }

    output::line("🧭 Airstack Plan");
    if actions.is_empty() {
        output::line("No actions.");
        return Ok(());
    }

    for action in &actions {
        output::line(format!(
            "- [{}] {} {} ({})",
            action.resource_type, action.action, action.resource, action.reason
        ));
    }

    Ok(())
}
