use crate::output;
use airstack_config::AirstackConfig;
use anyhow::{Context, Result};

pub async fn run(config_path: &str) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let mut issues = Vec::new();

    if config.infra.is_some() {
        if config.project.deploy_mode.as_deref().unwrap_or("remote") == "local" {
            issues.push("project.deploy_mode=local while infra.servers exists".to_string());
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
    anyhow::bail!("doctor checks failed")
}
