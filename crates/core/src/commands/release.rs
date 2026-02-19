use crate::output;
use airstack_config::AirstackConfig;
use anyhow::{Context, Result};
use clap::Args;
use std::process::Command;

#[derive(Debug, Clone, Args)]
pub struct ReleaseArgs {
    #[arg(help = "Service name")]
    pub service: String,
    #[arg(long, help = "Image tag (default: current git SHA)")]
    pub tag: Option<String>,
    #[arg(long, help = "Push image after build")]
    pub push: bool,
    #[arg(long, help = "Update service image in config file")]
    pub update_config: bool,
}

pub async fn run(config_path: &str, args: ReleaseArgs) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let services = config
        .services
        .as_ref()
        .context("No services defined in configuration")?;
    let svc = services
        .get(&args.service)
        .with_context(|| format!("Service '{}' not found", args.service))?;

    let base_image = svc.image.split(':').next().unwrap_or(&svc.image);
    let tag = match &args.tag {
        Some(t) => t.clone(),
        None => git_sha()?,
    };
    let final_image = format!("{}:{}", base_image, tag);

    run_cmd("docker", &["build", "-t", &final_image, "."])?;

    if args.push {
        run_cmd("docker", &["push", &final_image])?;
    }

    if args.update_config {
        update_config_image(config_path, &args.service, &final_image)?;
    }

    if output::is_json() {
        output::emit_json(&serde_json::json!({
            "service": args.service,
            "image": final_image,
            "pushed": args.push,
            "updated_config": args.update_config,
        }))?;
    } else {
        output::line(format!("✅ release built: {}", final_image));
        if args.push {
            output::line("✅ image pushed");
        }
        if args.update_config {
            output::line("✅ config image updated");
        }
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
