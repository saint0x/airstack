use crate::deploy_runtime::{
    deploy_service, existing_service_image, resolve_target, rollback_service, run_healthcheck,
};
use crate::output;
use airstack_config::AirstackConfig;
use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use std::process::Command;

#[derive(Debug, Clone, Args)]
pub struct ShipArgs {
    #[arg(help = "Service name")]
    pub service: String,
    #[arg(long, help = "Image tag (default: current git SHA)")]
    pub tag: Option<String>,
    #[arg(
        long,
        default_value_t = true,
        help = "Push image before deploy (required for remote hosts)"
    )]
    pub push: bool,
    #[arg(
        long,
        help = "Update service image in config file after successful ship"
    )]
    pub update_config: bool,
    #[arg(long, help = "Allow local deploys even when infra servers exist")]
    pub allow_local_deploy: bool,
}

#[derive(Debug, Serialize)]
struct ShipOutput {
    service: String,
    image: String,
    pushed: bool,
    deployed: bool,
    running: bool,
    healthy: Option<bool>,
    rolled_back: bool,
}

pub async fn run(config_path: &str, args: ShipArgs) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let services = config
        .services
        .as_ref()
        .context("No services defined in configuration")?;
    let service_cfg = services
        .get(&args.service)
        .with_context(|| format!("Service '{}' not found", args.service))?;

    let base_image = service_cfg
        .image
        .split(':')
        .next()
        .unwrap_or(&service_cfg.image);
    let tag = args.tag.clone().unwrap_or(git_sha()?);
    let final_image = format!("{}:{}", base_image, tag);

    // Build + push phase
    run_cmd("docker", &["build", "-t", &final_image, "."])?;
    if args.push {
        run_cmd("docker", &["push", &final_image])?;
    }

    // Deploy phase
    let target = resolve_target(&config, service_cfg, args.allow_local_deploy)?;
    let previous_image = existing_service_image(&target, &args.service).await?;
    let mut deploy_cfg = service_cfg.clone();
    deploy_cfg.image = final_image.clone();

    let mut rolled_back = false;
    let mut deployed = deploy_service(&target, &args.service, &deploy_cfg)
        .await
        .with_context(|| format!("Failed deploying ship image for '{}'", args.service))?;

    if let Some(hc) = &service_cfg.healthcheck {
        if let Err(err) = run_healthcheck(&target, &args.service, hc).await {
            deployed.healthy = Some(false);
            if let Some(prev) = &previous_image {
                let _ = rollback_service(&target, &args.service, prev, service_cfg).await;
                rolled_back = true;
            }
            return Err(err).with_context(|| {
                format!(
                    "Ship healthcheck failed for '{}' (rollback attempted={})",
                    args.service, rolled_back
                )
            });
        }
        deployed.healthy = Some(true);
    }

    if args.update_config {
        update_config_image(config_path, &args.service, &final_image)?;
    }

    if output::is_json() {
        output::emit_json(&ShipOutput {
            service: args.service,
            image: final_image,
            pushed: args.push,
            deployed: true,
            running: deployed.running,
            healthy: deployed.healthy,
            rolled_back,
        })?;
    } else {
        output::line(format!("✅ ship complete: {}", final_image));
        output::line(format!(
            "   running={} healthy={}",
            deployed.running,
            deployed
                .healthy
                .map(|v| v.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        ));
    }

    Ok(())
}

fn run_cmd(cmd: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(cmd)
        .args(args)
        .status()
        .with_context(|| format!("Failed to execute {}", cmd))?;
    if !status.success() {
        anyhow::bail!("Command failed: {} {}", cmd, args.join(" "));
    }
    Ok(())
}

fn git_sha() -> Result<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .context("Failed to execute git rev-parse")?;
    if !out.status.success() {
        anyhow::bail!("Failed to determine git SHA");
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn update_config_image(config_path: &str, service: &str, image: &str) -> Result<()> {
    let raw = std::fs::read_to_string(config_path)
        .with_context(|| format!("Failed to read config file {}", config_path))?;
    let mut value: toml::Value = toml::from_str(&raw).context("Failed to parse TOML")?;

    let services = value
        .get_mut("services")
        .and_then(|v| v.as_table_mut())
        .context("[services] table missing in config")?;
    let entry = services
        .get_mut(service)
        .and_then(|v| v.as_table_mut())
        .with_context(|| format!("Service '{}' not found in config", service))?;
    entry.insert("image".to_string(), toml::Value::String(image.to_string()));

    std::fs::write(config_path, toml::to_string_pretty(&value)?)
        .with_context(|| format!("Failed to write config file {}", config_path))?;
    Ok(())
}
