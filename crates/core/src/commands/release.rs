use crate::output;
use crate::ssh_utils::{
    execute_remote_command, execute_remote_shell_command, resolve_identity_path,
    resolve_server_public_ip,
};
use crate::state::{HealthState, LocalState, ServiceState};
use airstack_config::{AirstackConfig, ServerConfig, ServiceConfig};
use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct RemotePushFailure {
    pub image: String,
    pub registry: String,
    pub last_error: String,
}

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
    #[arg(long, help = "Override the remote host used for build/push")]
    pub remote_build: Option<String>,
    #[arg(long, value_enum, default_value_t = ReleaseFrom::Build, help = "Start release at this phase (build or push)")]
    pub from: ReleaseFrom,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum ReleaseFrom {
    Build,
    Push,
}

pub async fn run(config_path: &str, args: ReleaseArgs, dry_run: bool) -> Result<()> {
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
    let remote_build_server =
        default_remote_build_server_name(&config, svc, args.remote_build.as_deref())?;
    let build_context_root = resolve_build_context_root(config_path)?;
    if dry_run {
        if args.from == ReleaseFrom::Build {
            if let Some(server_name) = &remote_build_server {
                let _ = resolve_remote_build_server(&config, server_name)?;
                output::line(format!(
                    "🧪 dry-run: would build '{}' on remote server '{}'",
                    final_image, server_name
                ));
                if args.push {
                    output::line(format!(
                        "🧪 dry-run: would push '{}' from remote server '{}'",
                        final_image, server_name
                    ));
                }
            } else {
                output::line(format!("🧪 dry-run: would build '{}'", final_image));
                if args.push {
                    output::line(format!("🧪 dry-run: would push '{}'", final_image));
                }
            }
        } else if args.push {
            output::line(format!(
                "🧪 dry-run: would resume push for '{}' from phase '{}'",
                final_image,
                format!("{:?}", args.from).to_ascii_lowercase()
            ));
        }
        if args.update_config {
            output::line(format!(
                "🧪 dry-run: would update config image for service '{}'",
                args.service
            ));
        }
        if output::is_json() {
            output::emit_json(&serde_json::json!({
                "service": args.service,
                "image": final_image,
                "pushed": args.push,
                "updated_config": args.update_config,
                "remote_build": remote_build_server,
                "from": format!("{:?}", args.from).to_ascii_lowercase(),
                "operation_id": operation_id,
                "dry_run": true,
                "phases": ["build", if args.push { "push" } else { "skip-push" }],
            }))?;
        } else {
            output::line(
                "🧪 dry-run complete; no build, push, config, or state changes were performed.",
            );
        }
        return Ok(());
    }

    if args.from == ReleaseFrom::Build {
        emit_phase(&operation_id, "build", "start");
            if let Some(server_name) = &remote_build_server {
                let server = resolve_remote_build_server(&config, server_name)?;
                run_remote_build(server, server_name, &final_image, &build_context_root).await?;
            } else {
                preflight_local_docker_available()?;
                run_cmd("docker", &["build", "-t", &final_image, "."])?;
        }
        emit_phase(&operation_id, "build", "ok");
    } else if args.push {
        if let Some(server_name) = &remote_build_server {
            let server = resolve_remote_build_server(&config, server_name)?;
            preflight_remote_push_requirements(server, &final_image).await?;
        } else {
            preflight_local_docker_available()?;
        }
        emit_phase(&operation_id, "build", "skipped");
    }

    if let Some(server_name) = &remote_build_server {
        let server = resolve_remote_build_server(&config, server_name)?;
        if args.push {
            emit_phase(&operation_id, "push", "start");
            if let Err(push_failure) = run_remote_push(server, &final_image).await {
                let recovery_hint = build_remote_push_recovery_hint(
                    &config,
                    svc,
                    server,
                    server_name,
                    &args.service,
                    &tag,
                    &final_image,
                )
                .await?;
                anyhow::bail!(
                    "Remote registry push failed on '{}' for '{}'. Airstack used remote daemon auth only. Ensure remote auth with `docker login {}` on the target host. Last error: {}{}",
                    server.name,
                    push_failure.image,
                    push_failure.registry,
                    push_failure.last_error,
                    recovery_hint
                        .map(|hint| format!("\n{}", hint))
                        .unwrap_or_default()
                );
            }
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

    let image_origin = if remote_build_server.is_some() && args.push {
        "registry-pushed-via-remote"
    } else if remote_build_server.is_some() {
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
        remote_build_server
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
            "remote_build": remote_build_server,
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
            remote_build_server
                .as_ref()
                .map(|s| format!(" --remote-build {s}"))
                .unwrap_or_default()
        ));
    }

    Ok(())
}

pub fn default_remote_build_server_name(
    config: &AirstackConfig,
    service_cfg: &ServiceConfig,
    requested: Option<&str>,
) -> Result<Option<String>> {
    if let Some(name) = requested {
        return Ok(Some(name.to_string()));
    }

    if !prefer_remote_build(config) {
        return Ok(None);
    }

    let name = service_cfg.target_server.clone().or_else(|| {
        config
            .infra
            .as_ref()
            .and_then(|infra| infra.servers.first().map(|s| s.name.clone()))
    });

    name.map(Some).with_context(|| {
        "Remote build was selected, but no infra server or service target_server was configured"
    })
}

pub fn prefer_remote_build(config: &AirstackConfig) -> bool {
    if let Some(mode) = config.project.deploy_mode.as_deref() {
        return mode == "remote";
    }
    config
        .infra
        .as_ref()
        .is_some_and(|infra| !infra.servers.is_empty())
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

pub async fn run_remote_build(
    server: &ServerConfig,
    server_name: &str,
    image: &str,
    context_root: &Path,
) -> Result<()> {
    preflight_remote_push_requirements(server, image).await?;

    let archive_path = create_remote_build_archive(context_root)?;
    let archive_str = archive_path.to_string_lossy().to_string();
    let ip = resolve_server_public_ip(server).await?;
    let remote_root = format!("/tmp/airstack-remote-build-{}-{}", server_name, unix_now());
    let remote_archive = format!("{remote_root}.tgz");

    let upload_result = scp_local_file(server, &ip, &archive_str, &remote_archive);
    if let Err(err) = std::fs::remove_file(&archive_path) {
        output::line(format!(
            "⚠️ unable to remove local remote-build archive '{}': {}",
            archive_path.display(),
            err
        ));
    }
    upload_result?;

    let script = format!(
        "set -euo pipefail; cleanup() {{ rm -f {archive}; rm -rf {root}; }}; trap cleanup EXIT; rm -rf {root}; mkdir -p {root}; tar -xzf {archive} -C {root}; if docker info >/dev/null 2>&1; then docker build -t {image} {root}; elif sudo -n docker info >/dev/null 2>&1; then sudo -n docker build -t {image} {root}; else echo 'docker runtime unavailable on remote host' >&2; exit 1; fi",
        archive = shell_quote(&remote_archive),
        root = shell_quote(&remote_root),
        image = shell_quote(image),
    );
    let out = execute_remote_shell_command(server, &script).await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        anyhow::bail!(
            "Remote build failed on '{}': {}",
            server.name,
            if detail.is_empty() {
                "docker build exited unsuccessfully".to_string()
            } else {
                detail
            }
        );
    }
    Ok(())
}

pub async fn preflight_remote_push_requirements(server: &ServerConfig, image: &str) -> Result<()> {
    let registry_host = registry_host_for_login(image).unwrap_or_else(|| "docker.io".to_string());

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

pub fn resolve_build_context_root(config_path: &str) -> Result<PathBuf> {
    let cfg_path = Path::new(config_path);
    let root = if cfg_path.is_dir() {
        cfg_path.to_path_buf()
    } else {
        cfg_path
            .parent()
            .map(Path::to_path_buf)
            .context("Config path has no parent directory for build context")?
    };
    root.canonicalize().with_context(|| {
        format!(
            "Failed to resolve build context root from config path '{}'",
            config_path
        )
    })
}

fn create_remote_build_archive(context_root: &Path) -> Result<PathBuf> {
    let archive_path = std::env::temp_dir().join(format!(
        "airstack-remote-build-{}.tgz",
        uuid::Uuid::new_v4()
    ));
    let archive_str = archive_path.to_string_lossy().to_string();
    let excludes = [".git", "target", "node_modules", ".fozzy", ".DS_Store"];

    let mut cmd = Command::new("tar");
    cmd.current_dir(context_root);
    cmd.env("COPYFILE_DISABLE", "1");
    for exclude in excludes {
        cmd.arg(format!("--exclude={exclude}"));
    }
    cmd.args(["-czf", &archive_str, "."]);

    let output = cmd
        .output()
        .context("Failed to create remote build archive with tar")?;
    if !output.status.success() {
        anyhow::bail!(
            "Failed to create remote build archive: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(archive_path)
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
            "Local Docker daemon unavailable. In remote mode, Airstack now builds on the host automatically. In local-only mode, install/start Docker locally before running release."
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

fn scp_local_file(server: &ServerConfig, ip: &str, source: &str, remote_path: &str) -> Result<()> {
    let mut cmd = Command::new("scp");
    cmd.args([
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=no",
        "-o",
        "UserKnownHostsFile=/dev/null",
        "-o",
        "LogLevel=ERROR",
    ]);

    if let Some(identity_path) = resolve_identity_path(&server.ssh_key)? {
        cmd.args(["-i", &identity_path.to_string_lossy()]);
    }

    cmd.arg(source);
    cmd.arg(format!("root@{}:{}", ip, remote_path));

    let status = cmd.status().context("Failed to run scp for remote build")?;
    if !status.success() {
        anyhow::bail!(
            "scp failed uploading remote build archive '{}'",
            Path::new(source).display()
        );
    }
    Ok(())
}

pub async fn run_remote_push(
    server: &ServerConfig,
    image: &str,
) -> std::result::Result<(), RemotePushFailure> {
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
        .await
        .map_err(|err| RemotePushFailure {
            image: image.to_string(),
            registry: registry.clone(),
            last_error: err.to_string(),
        })?;
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

    Err(RemotePushFailure {
        image: image.to_string(),
        registry,
        last_error: last_err,
    })
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

async fn build_remote_push_recovery_hint(
    config: &AirstackConfig,
    service_cfg: &airstack_config::ServiceConfig,
    build_server: &ServerConfig,
    build_server_name: &str,
    service_name: &str,
    tag: &str,
    image: &str,
) -> Result<Option<String>> {
    if !remote_image_present(build_server, image).await? {
        return Ok(None);
    }

    let target_server_name = service_cfg
        .target_server
        .clone()
        .or_else(|| {
            config
                .infra
                .as_ref()
                .and_then(|infra| infra.servers.first().map(|s| s.name.clone()))
        })
        .unwrap_or_default();

    if target_server_name == build_server_name {
        return Ok(Some(format!(
            "Recovery path: image '{}' is already present on deploy target '{}'. You can continue without a registry push using `airstack deploy {} --tag {}`.",
            image, build_server_name, service_name, tag
        )));
    }

    Ok(Some(format!(
        "Recovery note: image '{}' exists on remote build host '{}', but the deploy target is '{}'. Registry push is still required unless you retag/rebuild on the deploy target.",
        image,
        build_server_name,
        if target_server_name.is_empty() {
            "unknown"
        } else {
            &target_server_name
        }
    )))
}

pub async fn remote_image_present(server: &ServerConfig, image: &str) -> Result<bool> {
    let out = execute_remote_command(
        server,
        &[
            "sh".to_string(),
            "-lc".to_string(),
            format!(
                "docker image inspect {} >/dev/null 2>&1 || sudo -n docker image inspect {} >/dev/null 2>&1",
                shell_quote(image),
                shell_quote(image)
            ),
        ],
    )
    .await?;
    Ok(out.status.success())
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
    use super::{
        default_remote_build_server_name, explicit_registry_host, prefer_remote_build,
        registry_host_for_login, resolve_build_context_root,
    };
    use airstack_config::{
        AirstackConfig, InfraConfig, ProjectConfig, ServerConfig, ServiceConfig,
    };
    use std::collections::HashMap;
    use std::fs;

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

    #[test]
    fn prefer_remote_build_when_remote_mode_or_infra_exists() {
        let config = AirstackConfig {
            project: ProjectConfig {
                name: "demo".to_string(),
                description: None,
                deploy_mode: Some("remote".to_string()),
            },
            infra: None,
            services: None,
            edge: None,
            providers: None,
            scripts: None,
            hooks: None,
        };
        assert!(prefer_remote_build(&config));

        let config2 = AirstackConfig {
            project: ProjectConfig {
                name: "demo".to_string(),
                description: None,
                deploy_mode: None,
            },
            infra: Some(InfraConfig {
                servers: vec![ServerConfig {
                    name: "aria".to_string(),
                    provider: "hetzner".to_string(),
                    region: "ash".to_string(),
                    ssh_key: "~/.ssh/id_ed25519.pub".to_string(),
                    server_type: "cpx21".to_string(),
                    floating_ip: None,
                }],
                firewall: None,
            }),
            services: None,
            edge: None,
            providers: None,
            scripts: None,
            hooks: None,
        };
        assert!(prefer_remote_build(&config2));
    }

    #[test]
    fn default_remote_build_prefers_service_target_then_first_infra() {
        let config = AirstackConfig {
            project: ProjectConfig {
                name: "demo".to_string(),
                description: None,
                deploy_mode: Some("remote".to_string()),
            },
            infra: Some(InfraConfig {
                servers: vec![ServerConfig {
                    name: "default-server".to_string(),
                    provider: "hetzner".to_string(),
                    region: "ash".to_string(),
                    ssh_key: "~/.ssh/id_ed25519.pub".to_string(),
                    server_type: "cpx21".to_string(),
                    floating_ip: None,
                }],
                firewall: None,
            }),
            services: None,
            edge: None,
            providers: None,
            scripts: None,
            hooks: None,
        };

        let svc_targeted = ServiceConfig {
            image: "ghcr.io/demo/app:latest".to_string(),
            ports: vec![8080],
            env: Some(HashMap::new()),
            volumes: Some(Vec::new()),
            depends_on: Some(Vec::new()),
            target_server: Some("target-server".to_string()),
            healthcheck: None,
            profile: None,
        };
        assert_eq!(
            default_remote_build_server_name(&config, &svc_targeted, None)
                .expect("remote build server should resolve"),
            Some("target-server".to_string())
        );

        let svc_default = ServiceConfig {
            image: "ghcr.io/demo/app:latest".to_string(),
            ports: vec![8080],
            env: Some(HashMap::new()),
            volumes: Some(Vec::new()),
            depends_on: Some(Vec::new()),
            target_server: None,
            healthcheck: None,
            profile: None,
        };
        assert_eq!(
            default_remote_build_server_name(&config, &svc_default, None)
                .expect("default server should resolve"),
            Some("default-server".to_string())
        );
    }

    #[test]
    fn resolve_build_context_root_uses_config_parent_directory() {
        let root = std::env::temp_dir().join(format!("airstack-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("create temp root");
        let config_path = root.join("airstack.toml");
        fs::write(&config_path, "[project]\nname='demo'\n").expect("write config");

        let resolved = resolve_build_context_root(config_path.to_string_lossy().as_ref())
            .expect("resolve build root");
        assert_eq!(resolved, root.canonicalize().expect("canonical temp root"));

        let _ = fs::remove_file(&config_path);
        let _ = fs::remove_dir_all(&root);
    }
}
