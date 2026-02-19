use crate::output;
use crate::ssh_utils::execute_remote_command;
use airstack_config::{AirstackConfig, ServerConfig, ServiceConfig};
use anyhow::{Context, Result};
use serde::Serialize;
use tokio::process::Command;

#[derive(Debug, Serialize)]
struct ImageDriftRecord {
    service: String,
    desired_image: String,
    running_image: Option<String>,
    target_server: Option<String>,
    matches: bool,
}

#[derive(Debug, Serialize)]
struct DriftOutput {
    project: String,
    image_drift: Vec<ImageDriftRecord>,
}

pub async fn run(config_path: &str) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let services = config
        .services
        .as_ref()
        .context("No services configured for drift check")?;

    let mut records = Vec::new();
    for (name, svc) in services {
        let target = resolve_target_server(&config, svc);
        let running = match target.as_ref() {
            Some(server) => inspect_running_image(server, name).await?,
            None => None,
        };
        records.push(ImageDriftRecord {
            service: name.clone(),
            desired_image: svc.image.clone(),
            running_image: running.clone(),
            target_server: target.map(|s| s.name.clone()),
            matches: running.as_deref() == Some(svc.image.as_str()),
        });
    }

    let out = DriftOutput {
        project: config.project.name,
        image_drift: records,
    };

    if output::is_json() {
        output::emit_json(&out)?;
    } else {
        output::line("🧭 Image Drift");
        for row in &out.image_drift {
            let mark = if row.matches { "✅" } else { "⚠️" };
            output::line(format!(
                "{} {} desired={} running={} target={}",
                mark,
                row.service,
                row.desired_image,
                row.running_image
                    .clone()
                    .unwrap_or_else(|| "not-found".to_string()),
                row.target_server
                    .clone()
                    .unwrap_or_else(|| "none".to_string())
            ));
        }
    }

    Ok(())
}

fn resolve_target_server<'a>(
    config: &'a AirstackConfig,
    svc: &ServiceConfig,
) -> Option<&'a ServerConfig> {
    let infra = config.infra.as_ref()?;
    if let Some(name) = &svc.target_server {
        infra.servers.iter().find(|s| s.name == *name)
    } else {
        infra.servers.first()
    }
}

async fn inspect_running_image(server: &ServerConfig, service: &str) -> Result<Option<String>> {
    if server.provider == "fly" {
        let out = Command::new("flyctl")
            .args(["machine", "list", "--app", &server.name, "--json"])
            .output()
            .await
            .context("Failed to execute flyctl machine list")?;
        if !out.status.success() {
            return Ok(None);
        }
        let v: serde_json::Value =
            serde_json::from_slice(&out.stdout).context("Failed to parse fly machine list")?;
        let image = v
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|m| m.get("config"))
            .and_then(|c| c.get("image"))
            .and_then(|i| i.as_str())
            .map(|s| s.to_string());
        return Ok(image);
    }

    let out = execute_remote_command(
        server,
        &[
            "sh".to_string(),
            "-lc".to_string(),
            format!(
                "docker inspect -f '{{{{.Config.Image}}}}' {} 2>/dev/null || true",
                service
            ),
        ],
    )
    .await?;
    if !out.status.success() {
        return Ok(None);
    }
    let img = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if img.is_empty() {
        Ok(None)
    } else {
        Ok(Some(img))
    }
}
