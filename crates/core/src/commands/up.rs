use airstack_config::AirstackConfig;
use airstack_metal::{
    get_provider as get_metal_provider, CreateServerRequest, FirewallRuleSpec, FirewallSpec,
    ServerStatus,
};
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;
use tracing::{info, warn};

use crate::commands::edge;
use crate::commands::script::{run_hook_scripts, ScriptRunOptions};
use crate::dependencies::deployment_order;
use crate::deploy_runtime::{
    collect_container_diagnostics, deploy_service, evaluate_service_health, existing_service_image,
    resolve_target, rollback_service,
};
use crate::infra_preflight::{
    check_ssh_key_path, format_validation_error, is_permanent_provider_error,
    resolve_server_request,
};
use crate::output;
use crate::retry::{retry_with_backoff_classified, RetryDecision};
use crate::ssh_utils::execute_remote_command;
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
    force_local: bool,
    bootstrap_runtime: bool,
    auto_fallback: bool,
    resolve_capacity: bool,
) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let mut deploy_config = config.clone();
    if force_local {
        deploy_config.project.deploy_mode = Some("local".to_string());
    }
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

    if force_local && !output::is_json() {
        output::line(
            "ℹ️ local mode enabled: skipping infra provisioning and deploying services locally",
        );
    }

    if !force_local {
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
            let mut firewall_ids: HashMap<String, String> = HashMap::new();
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
                    .with_context(|| {
                        format!("Failed to initialize {} provider", server.provider)
                    })?;

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
                    if let Some(firewall) = &infra.firewall {
                        let spec = to_firewall_spec(firewall);
                        if let Some(fw_id) = ensure_firewall_attached(
                            &*metal_provider,
                            &server.provider,
                            &existing_server.id,
                            &spec,
                            &mut firewall_ids,
                        )
                        .await?
                        {
                            output::line(format!(
                                "🛡️ Firewall '{}' attached to {}",
                                fw_id, server.name
                            ));
                        }
                    }
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
                        if let Some(firewall) = &infra.firewall {
                            let spec = to_firewall_spec(firewall);
                            if let Some(fw_id) = ensure_firewall_attached(
                                &*metal_provider,
                                &server.provider,
                                &created_server.id,
                                &spec,
                                &mut firewall_ids,
                            )
                            .await?
                            {
                                output::line(format!(
                                    "🛡️ Firewall '{}' attached to {}",
                                    fw_id, server.name
                                ));
                            }
                        }
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

        if bootstrap_runtime && !dry_run {
            if let Some(infra) = &config.infra {
                output::line("🧰 bootstrapping runtime dependencies (docker)");
                for server in &infra.servers {
                    ensure_runtime_bootstrap(server).await.with_context(|| {
                        format!(
                            "runtime bootstrap failed for server '{}'; retry with 'airstack ssh {} -- <cmd>'",
                            server.name, server.name
                        )
                    })?;
                }
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

            let runtime_target =
                resolve_target(&deploy_config, service, allow_local_deploy || force_local)?;
            let previous_image = existing_service_image(&runtime_target, &service_name).await?;
            let deployed = match deploy_service(&runtime_target, &service_name, service).await {
                Ok(v) => v,
                Err(e) => {
                    let diag = collect_container_diagnostics(&runtime_target, &service_name).await;
                    return Err(e).with_context(|| {
                        format!(
                            "Failed to deploy service {}. diagnostics: {}",
                            service_name, diag
                        )
                    });
                }
            };

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
                    let diag = collect_container_diagnostics(&runtime_target, &service_name).await;
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
                            "Healthcheck gate failed for service '{}' (rolled back if possible). diagnostics: {}",
                            service_name, diag
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

        if !force_local {
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

fn to_firewall_spec(cfg: &airstack_config::FirewallConfig) -> FirewallSpec {
    FirewallSpec {
        name: cfg.name.clone(),
        rules: cfg
            .ingress
            .iter()
            .map(|r| FirewallRuleSpec {
                protocol: r.protocol.clone(),
                port: r.port.clone(),
                source_ips: r.source_ips.clone(),
            })
            .collect(),
    }
}

async fn ensure_firewall_attached(
    provider: &dyn airstack_metal::MetalProvider,
    provider_name: &str,
    server_id: &str,
    spec: &FirewallSpec,
    cache: &mut HashMap<String, String>,
) -> Result<Option<String>> {
    let key = format!("{provider_name}:{}", spec.name);
    let fw_id = if let Some(existing) = cache.get(&key) {
        existing.clone()
    } else {
        let Some(created) = provider.ensure_firewall(spec).await? else {
            return Ok(None);
        };
        cache.insert(key, created.clone());
        created
    };
    provider
        .attach_firewall_to_server(&fw_id, server_id)
        .await?;
    Ok(Some(fw_id))
}

async fn ensure_runtime_bootstrap(server: &airstack_config::ServerConfig) -> Result<()> {
    let script = r#"
if command -v docker >/dev/null 2>&1; then
  exit 0
fi
if command -v apt-get >/dev/null 2>&1; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -y >/dev/null 2>&1 || apt-get update -y
  apt-get install -y docker.io >/dev/null 2>&1 || apt-get install -y docker.io
  systemctl enable --now docker >/dev/null 2>&1 || true
  exit 0
fi
if command -v dnf >/dev/null 2>&1; then
  dnf -y install docker >/dev/null 2>&1 || dnf -y install docker
  systemctl enable --now docker >/dev/null 2>&1 || true
  exit 0
fi
if command -v yum >/dev/null 2>&1; then
  yum -y install docker >/dev/null 2>&1 || yum -y install docker
  systemctl enable --now docker >/dev/null 2>&1 || true
  exit 0
fi
echo \"unsupported package manager for docker bootstrap\" 1>&2
exit 1
"#;
    let out = execute_remote_command(
        server,
        &["sh".to_string(), "-lc".to_string(), script.to_string()],
    )
    .await?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        anyhow::bail!(
            "docker bootstrap failed on '{}': {}",
            server.name,
            if err.is_empty() {
                "unknown error".to_string()
            } else {
                err
            }
        );
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
