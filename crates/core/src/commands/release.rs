use crate::output;
use crate::ssh_utils::{execute_remote_command, resolve_server_public_ip};
use crate::state::{HealthState, LocalState, ServiceState};
use airstack_config::{AirstackConfig, ServerConfig};
use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
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
    #[arg(long, value_enum, default_value_t = ReleaseFrom::Build, help = "Start release at this phase (build or push)")]
    pub from: ReleaseFrom,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum ReleaseFrom {
    Build,
    Push,
}

pub async fn run(config_path: &str, args: ReleaseArgs) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let mut state = LocalState::load(&config.project.name)?;
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

    let operation_id = format!("rel-{}-{}", args.service, unix_now());
    if args.from == ReleaseFrom::Build {
        emit_phase(&operation_id, "build", "start");
        if let Some(server_name) = &args.remote_build {
            let server = resolve_remote_build_server(&config, server_name)?;
            if args.push {
                preflight_remote_push_requirements(server, &final_image).await?;
            }
            run_remote_build(server, server_name, &final_image).await?;
        } else {
            preflight_local_docker_available()?;
            run_cmd("docker", &["build", "-t", &final_image, "."])?;
        }
        emit_phase(&operation_id, "build", "ok");
    } else if args.push {
        if let Some(server_name) = &args.remote_build {
            let server = resolve_remote_build_server(&config, server_name)?;
            preflight_remote_push_requirements(server, &final_image).await?;
        } else {
            preflight_local_docker_available()?;
        }
        emit_phase(&operation_id, "build", "skipped");
    }

    if let Some(server_name) = &args.remote_build {
        let server = resolve_remote_build_server(&config, server_name)?;
        if args.push {
            emit_phase(&operation_id, "push", "start");
            run_remote_push(server, &final_image).await?;
            emit_phase(&operation_id, "push", "ok");
        }
    } else {
        if args.push {
            emit_phase(&operation_id, "push", "start");
            run_cmd("docker", &["push", &final_image])?;
            emit_phase(&operation_id, "push", "ok");
        }
    }

    if args.update_config {
        update_config_image(config_path, &args.service, &final_image)?;
    }

    let image_origin = if args.remote_build.is_some() && args.push {
        "registry-pushed-via-remote"
    } else if args.remote_build.is_some() {
        "remote-host-local-only"
    } else if args.push {
        "registry-pushed"
    } else {
        "local-build-only"
    };
    let now = unix_now();
    let deploy_command = format!(
        "airstack release {} --tag {}{}{}{}",
        args.service,
        tag,
        if args.push { " --push" } else { "" },
        if args.update_config {
            " --update-config"
        } else {
            ""
        },
        args.remote_build
            .as_ref()
            .map(|s| format!(" --remote-build {s}"))
            .unwrap_or_default()
    );
    state
        .services
        .entry(args.service.clone())
        .and_modify(|s| {
            s.image = final_image.clone();
            s.last_status = Some("Released".to_string());
            s.last_checked_unix = now;
            s.last_error = None;
            s.last_deploy_command = Some(deploy_command.clone());
            s.last_deploy_unix = Some(now);
            s.image_origin = Some(image_origin.to_string());
        })
        .or_insert(ServiceState {
            image: final_image.clone(),
            replicas: 0,
            containers: Vec::new(),
            health: HealthState::Unknown,
            last_status: Some("Released".to_string()),
            last_checked_unix: now,
            last_error: None,
            last_deploy_command: Some(deploy_command.clone()),
            last_deploy_unix: Some(now),
            image_origin: Some(image_origin.to_string()),
        });
    state.save()?;

    if output::is_json() {
        output::emit_json(&serde_json::json!({
            "service": args.service,
            "image": final_image,
            "pushed": args.push,
            "updated_config": args.update_config,
            "remote_build": args.remote_build,
            "from": format!("{:?}", args.from).to_ascii_lowercase(),
            "operation_id": operation_id,
            "phases": ["build", if args.push { "push" } else { "skip-push" }],
        }))?;
    } else {
        output::line(format!("✅ release built: {}", final_image));
        if args.push {
            output::line("✅ image pushed");
        }
        if args.update_config {
            output::line("✅ config image updated");
        }
        output::line(format!(
            "🧩 operation id: {} (resume push without rebuild: airstack release {} --tag {} --push{} --from push)",
            operation_id,
            args.service,
            tag,
            args.remote_build
                .as_ref()
                .map(|s| format!(" --remote-build {s}"))
                .unwrap_or_default()
        ));
    }

    Ok(())
}

pub fn resolve_remote_build_server<'a>(
    config: &'a AirstackConfig,
    server_name: &str,
) -> Result<&'a ServerConfig> {
    let infra = config
        .infra
        .as_ref()
        .context("remote build requires [infra] servers in config")?;
    let server = infra
        .servers
        .iter()
        .find(|s| s.name == server_name)
        .with_context(|| format!("remote build server '{}' not found", server_name))?;
    if server.provider == "fly" {
        anyhow::bail!(
            "release --remote-build does not support provider='fly'; use Fly-native release flow"
        );
    }
    Ok(server)
}

pub async fn run_remote_build(server: &ServerConfig, server_name: &str, image: &str) -> Result<()> {
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
    let build_result = run_cmd("docker", &["--context", &ctx, "build", "-t", image, "."]);
    let cleanup_result = run_cmd("docker", &["context", "rm", "-f", &ctx]);
    if let Err(e) = build_result {
        return Err(e);
    }
    let _ = cleanup_result;
    Ok(())
}

pub async fn preflight_remote_push_requirements(server: &ServerConfig, image: &str) -> Result<()> {
    let Some(registry_host) = explicit_registry_host(image) else {
        anyhow::bail!(
            "Remote push requires an explicit registry host in image name. Example: ghcr.io/<org>/<image>:<tag>. Got '{}'",
            image
        );
    };

    let checks = [
        "command -v docker >/dev/null 2>&1",
        "docker info >/dev/null 2>&1 || sudo -n docker info >/dev/null 2>&1",
    ];

    for check in checks {
        let out = execute_remote_command(
            server,
            &["sh".to_string(), "-lc".to_string(), check.to_string()],
        )
        .await?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let reason = if !stderr.is_empty() { stderr } else { stdout };
            anyhow::bail!(
                "Remote preflight failed on '{}': Docker runtime unavailable before build/push. Install/start Docker on host and verify with `airstack ssh {} --cmd \"docker info\"`. Details: {}",
                server.name,
                server.name,
                if reason.is_empty() {
                    "docker check failed".to_string()
                } else {
                    reason
                }
            );
        }
    }

    let auth_hint = format!("docker login {}", registry_host);
    output::line(format!(
        "ℹ️ remote push target registry: {} (ensure auth with `{}` on {})",
        registry_host, auth_hint, server.name
    ));

    Ok(())
}

fn emit_phase(operation_id: &str, phase: &str, status: &str) {
    if !output::is_json() {
        output::line(format!(
            "phase={} status={} operation_id={}",
            phase, status, operation_id
        ));
    }
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

pub fn preflight_local_docker_available() -> Result<()> {
    let out = Command::new("docker")
        .args(["info"])
        .output()
        .context("Failed to execute docker info")?;
    if !out.status.success() {
        anyhow::bail!(
            "Local Docker daemon unavailable. For remote mode, use airstack release <service> --push --remote-build <server> (or airstack deploy <service> --latest-code --push in remote mode to auto-fallback)."
        );
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

async fn run_remote_push(server: &ServerConfig, image: &str) -> Result<()> {
    let registry = registry_host_for_login(image).unwrap_or_else(|| "docker.io".to_string());
    let quoted = shell_quote(image);
    let scripts = [
        format!("docker push {quoted} 2>&1"),
        format!("sudo -n docker push {quoted} 2>&1"),
    ];

    let mut last_err = String::new();
    for script in scripts {
        let out = execute_remote_command(
            server,
            &["sh".to_string(), "-lc".to_string(), script.to_string()],
        )
        .await?;
        if out.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let merged = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            "unknown remote push failure".to_string()
        };
        last_err = merged;
    }

    anyhow::bail!(
        "Remote registry push failed on '{}' for '{}'. Airstack used remote daemon auth (not local Docker credential helpers). Ensure remote auth with 'docker login {}' on the target host. Last error: {}",
        server.name,
        image,
        registry,
        last_err
    );
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || "-_./:".contains(ch))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn explicit_registry_host(image: &str) -> Option<String> {
    if !image.contains('/') {
        return None;
    }
    let first = image.split('/').next()?;
    if first.contains('.') || first.contains(':') || first == "localhost" {
        Some(first.to_string())
    } else {
        None
    }
}

fn registry_host_for_login(image: &str) -> Option<String> {
    explicit_registry_host(image).or_else(|| Some("docker.io".to_string()))
}

pub fn update_config_image(config_path: &str, service: &str, image: &str) -> Result<()> {
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

    let reloaded = AirstackConfig::load(config_path)
        .with_context(|| format!("Failed to re-load config file {} after update", config_path))?;
    let saved = reloaded
        .services
        .as_ref()
        .and_then(|s| s.get(service))
        .map(|s| s.image.clone())
        .with_context(|| format!("Service '{}' missing after config update", service))?;
    if saved != image {
        anyhow::bail!(
            "Config update verification failed for service '{}': expected image '{}' but found '{}'.",
            service,
            image,
            saved
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{explicit_registry_host, registry_host_for_login};

    #[test]
    fn explicit_registry_host_requires_host_prefix() {
        assert_eq!(
            explicit_registry_host("ghcr.io/org/app:abc").as_deref(),
            Some("ghcr.io")
        );
        assert_eq!(
            explicit_registry_host("registry.example.com:5000/org/app:abc").as_deref(),
            Some("registry.example.com:5000")
        );
        assert!(explicit_registry_host("org/app:abc").is_none());
        assert!(explicit_registry_host("app:abc").is_none());
    }

    #[test]
    fn registry_host_for_login_defaults_to_docker_hub() {
        assert_eq!(
            registry_host_for_login("ghcr.io/org/app:abc").as_deref(),
            Some("ghcr.io")
        );
        assert_eq!(
            registry_host_for_login("org/app:abc").as_deref(),
            Some("docker.io")
        );
        assert_eq!(
            registry_host_for_login("app:abc").as_deref(),
            Some("docker.io")
        );
    }
}
