use crate::output;
use crate::ssh_utils::resolve_server_public_ip;
use airstack_config::AirstackConfig;
use anyhow::{Context, Result};
use base64::Engine;
use clap::Args;
use serde_json::json;
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
    #[arg(
        long,
        help = "Build/push via remote Docker daemon on this infra server"
    )]
    pub remote_build: Option<String>,
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

    if let Some(server_name) = &args.remote_build {
        let infra = config
            .infra
            .as_ref()
            .context("remote build requires [infra] servers in config")?;
        let server = infra
            .servers
            .iter()
            .find(|s| &s.name == server_name)
            .with_context(|| format!("remote build server '{}' not found", server_name))?;
        if server.provider == "fly" {
            anyhow::bail!(
                "release --remote-build does not support provider='fly'; use Fly-native release flow"
            );
        }
        let ip = resolve_server_public_ip(server).await?;
        let ctx = format!("airstack-remote-{}-{}", server_name, unix_now());
        run_cmd(
            "docker",
            &[
                "context",
                "create",
                &ctx,
                "--docker",
                &format!("host=ssh://root@{}", ip),
            ],
        )?;
        let build_result = run_cmd(
            "docker",
            &["--context", &ctx, "build", "-t", &final_image, "."],
        )
        .and_then(|_| {
            if args.push {
                let temp_cfg = prepare_docker_auth_config_for_push(&final_image)?;
                let push = if let Some(cfg_dir) = temp_cfg.as_ref() {
                    let docker_config = cfg_dir.to_string_lossy().to_string();
                    run_cmd_with_env(
                        "docker",
                        &["--context", &ctx, "push", &final_image],
                        &[("DOCKER_CONFIG", docker_config.as_str())],
                    )
                } else {
                    run_cmd("docker", &["--context", &ctx, "push", &final_image])
                };
                if let Some(cfg_dir) = temp_cfg {
                    let _ = std::fs::remove_dir_all(cfg_dir);
                }
                push
            } else {
                Ok(())
            }
        });
        let cleanup_result = run_cmd("docker", &["context", "rm", "-f", &ctx]);
        if let Err(e) = build_result {
            return Err(e);
        }
        let _ = cleanup_result;
    } else {
        run_cmd("docker", &["build", "-t", &final_image, "."])?;
    }
    if args.push && args.remote_build.is_none() {
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
            "remote_build": args.remote_build,
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

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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

fn run_cmd_with_env(cmd: &str, args: &[&str], env: &[(&str, &str)]) -> Result<()> {
    let mut command = Command::new(cmd);
    command.args(args);
    for (k, v) in env {
        command.env(k, v);
    }
    let status = command
        .status()
        .with_context(|| format!("Failed to execute {}", cmd))?;
    if !status.success() {
        anyhow::bail!("Command failed: {} {}", cmd, args.join(" "));
    }
    Ok(())
}

fn prepare_docker_auth_config_for_push(image: &str) -> Result<Option<std::path::PathBuf>> {
    let registry = image.split('/').next().unwrap_or_default();
    if registry != "ghcr.io" {
        return Ok(None);
    }

    let token = std::env::var("GHCR_TOKEN")
        .or_else(|_| std::env::var("GITHUB_TOKEN"))
        .context("release --remote-build --push for ghcr.io requires GHCR_TOKEN or GITHUB_TOKEN")?;
    let username = std::env::var("GHCR_USERNAME")
        .or_else(|_| std::env::var("GITHUB_ACTOR"))
        .context(
            "release --remote-build --push for ghcr.io requires GHCR_USERNAME or GITHUB_ACTOR",
        )?;

    let dir = std::env::temp_dir().join(format!("airstack-docker-auth-{}", unix_now()));
    std::fs::create_dir_all(&dir).with_context(|| {
        format!(
            "Failed to create temporary docker auth dir {}",
            dir.display()
        )
    })?;
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("{username}:{token}"));
    let config = json!({
        "auths": {
            "ghcr.io": {
                "auth": auth
            }
        }
    });
    let cfg_path = dir.join("config.json");
    std::fs::write(&cfg_path, serde_json::to_vec_pretty(&config)?)
        .with_context(|| format!("Failed to write docker auth config {}", cfg_path.display()))?;
    Ok(Some(dir))
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
