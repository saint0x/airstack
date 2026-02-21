use crate::commands::edge;
use crate::commands::release;
use crate::deploy_runtime::{
    collect_container_diagnostics, deploy_service_with_strategy, evaluate_service_health,
    existing_service_image, resolve_target, rollback_service, DeployStrategy,
};
use crate::output;
use crate::state::{HealthState, LocalState, ServiceState};
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
    #[arg(
        long,
        help = "Deploy strategy: rolling|bluegreen|canary",
        default_value = "rolling"
    )]
    pub strategy: String,
    #[arg(
        long,
        help = "Canary observation window in seconds (strategy=canary)",
        default_value_t = 45
    )]
    pub canary_seconds: u64,
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
    let mut state = LocalState::load(&config.project.name)?;
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
    release::preflight_local_docker_available()?;
    run_cmd("docker", &["build", "-t", &final_image, "."])?;
    if args.push {
        run_cmd("docker", &["push", &final_image])?;
    }

    // Deploy phase
    let strategy = DeployStrategy::parse(&args.strategy)?;
    let target = resolve_target(&config, service_cfg, args.allow_local_deploy)?;
    let previous_image = existing_service_image(&target, &args.service).await?;
    let mut deploy_cfg = service_cfg.clone();
    deploy_cfg.image = final_image.clone();

    let mut rolled_back = false;
    let mut deployed = deploy_service_with_strategy(
        &target,
        &args.service,
        &deploy_cfg,
        service_cfg.healthcheck.as_ref(),
        strategy,
        args.canary_seconds,
    )
    .await
    .with_context(|| format!("Failed deploying ship image for '{}'", args.service))?;

    if service_cfg.healthcheck.is_some() {
        if let Err(err) =
            evaluate_service_health(&target, &args.service, service_cfg, false, 1, false)
                .await
                .and_then(|eval| {
                    if eval.ok {
                        Ok(())
                    } else {
                        anyhow::bail!("{}", eval.detail)
                    }
                })
        {
            deployed.healthy = Some(false);
            let diag = collect_container_diagnostics(&target, &args.service).await;
            if let Some(prev) = &previous_image {
                let _ = rollback_service(&target, &args.service, prev, service_cfg).await;
                rolled_back = true;
                output::line(format!(
                    "↩️ rollback target for {} -> image {}",
                    args.service, prev
                ));
            }
            return Err(err).with_context(|| {
                format!(
                    "Ship healthcheck failed for '{}' (rollback attempted={}). diagnostics: {}",
                    args.service, rolled_back, diag
                )
            });
        }
        deployed.healthy = Some(true);
    }

    if args.update_config {
        release::update_config_image(config_path, &args.service, &final_image)?;
    }

    let now = unix_now();
    let deploy_command = format!(
        "airstack ship {} --tag {}{}{}",
        args.service,
        tag,
        if args.push { " --push" } else { "" },
        if args.update_config {
            " --update-config"
        } else {
            ""
        }
    );
    state
        .services
        .entry(args.service.clone())
        .and_modify(|s| {
            s.image = final_image.clone();
            s.last_status = Some("Shipped".to_string());
            s.last_checked_unix = now;
            s.last_error = None;
            s.last_deploy_command = Some(deploy_command.clone());
            s.last_deploy_unix = Some(now);
            s.image_origin = Some(if args.push {
                "registry-pushed".to_string()
            } else {
                "local-build-only".to_string()
            });
        })
        .or_insert(ServiceState {
            image: final_image.clone(),
            replicas: 0,
            containers: Vec::new(),
            health: HealthState::Unknown,
            last_status: Some("Shipped".to_string()),
            last_checked_unix: now,
            last_error: None,
            last_deploy_command: Some(deploy_command.clone()),
            last_deploy_unix: Some(now),
            image_origin: Some(if args.push {
                "registry-pushed".to_string()
            } else {
                "local-build-only".to_string()
            }),
        });
    state.save()?;

    if args.service == "caddy" && config.edge.is_some() {
        edge::apply_from_config(&config)
            .await
            .with_context(|| "Failed to sync edge config during caddy ship")?;
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

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
