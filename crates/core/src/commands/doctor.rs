use crate::deploy_runtime::{preflight_image_access, resolve_target};
use crate::infra_preflight::{check_ssh_key_path, format_validation_error, resolve_server_request};
use crate::output;
use airstack_config::AirstackConfig;
use airstack_metal::{get_provider as get_metal_provider, CapacityResolveOptions};
use anyhow::{Context, Result};
use std::collections::HashMap;

pub async fn run(config_path: &str) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let mut issues = Vec::new();
    let mut warnings = Vec::new();

    if config.infra.is_some() {
        if config.project.deploy_mode.as_deref().unwrap_or("remote") == "local" {
            issues.push("project.deploy_mode=local while infra.servers exists".to_string());
        }
    }

    if let Some(infra) = &config.infra {
        for server in &infra.servers {
            if let Err(e) = check_ssh_key_path(server) {
                issues.push(e.to_string());
            }
            if let Err(e) = get_metal_provider(&server.provider, HashMap::new()) {
                issues.push(format!(
                    "infra '{}': provider '{}' init failed (credential/token check): {}",
                    server.name, server.provider, e
                ));
                continue;
            }
            match resolve_server_request(
                server,
                CapacityResolveOptions {
                    auto_fallback: false,
                    resolve_capacity: false,
                },
            )
            .await
            {
                Ok(pre) => {
                    if !pre.validation.valid {
                        issues.push(format_validation_error(server, &pre));
                    }
                }
                Err(e) => issues.push(format!(
                    "infra '{}': provider preflight failed: {}",
                    server.name, e
                )),
            }
            warnings.push(format!(
                "infra '{}': quota preflight not supported for provider '{}'",
                server.name, server.provider
            ));
        }
    }

    if let Some(services) = &config.services {
        for (name, svc) in services {
            if svc.image.ends_with(":latest") {
                issues.push(format!("service '{}' uses mutable :latest image tag", name));
            }
            if svc.env.as_ref().is_some_and(|e| {
                e.keys()
                    .any(|k| k.contains("PASSWORD") || k.contains("TOKEN") || k.contains("SECRET"))
            }) {
                issues.push(format!(
                    "service '{}' has secret-like env keys in config; move to secrets store",
                    name
                ));
            }
            if svc.healthcheck.is_none() {
                issues.push(format!("service '{}' has no healthcheck configured", name));
            }
            match resolve_target(&config, svc, false) {
                Ok(target) => {
                    if let Err(e) = preflight_image_access(&target, &svc.image).await {
                        issues.push(format!(
                            "service '{}': image preflight failed for '{}': {}",
                            name, svc.image, e
                        ));
                    }
                }
                Err(e) => issues.push(format!(
                    "service '{}': target resolution failed: {}",
                    name, e
                )),
            }
        }
    }

    if let Some(edge) = &config.edge {
        if edge.provider == "caddy" {
            for site in &edge.sites {
                if site.tls_email.is_none() {
                    issues.push(format!(
                        "edge site '{}' has no tls_email set (cert ops visibility reduced)",
                        site.host
                    ));
                }
            }
        }
    }

    if output::is_json() {
        output::emit_json(&serde_json::json!({
            "ok": issues.is_empty(),
            "issues": issues,
            "warnings": warnings,
        }))?;
        return Ok(());
    }

    if issues.is_empty() {
        output::line("✅ doctor: no blocking issues found");
        return Ok(());
    }

    output::line("❌ doctor found issues:");
    for i in &issues {
        output::line(format!("- {}", i));
    }
    if !warnings.is_empty() {
        output::line("⚠️ doctor warnings:");
        for w in &warnings {
            output::line(format!("- {}", w));
        }
    }
    anyhow::bail!("doctor checks failed")
}
