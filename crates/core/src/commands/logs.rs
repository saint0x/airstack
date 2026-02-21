use crate::output;
use crate::ssh_utils::{execute_remote_command, start_remote_session};
use airstack_config::{AirstackConfig, ServerConfig, ServiceConfig};
use airstack_container::get_provider as get_container_provider;
use anyhow::{Context, Result};
use serde::Serialize;
use tracing::info;

#[derive(Debug, Serialize)]
struct LogsOutput {
    service: String,
    container_id: String,
    status: String,
    source_mode: String,
    server: Option<String>,
    follow: bool,
    lines: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceMode {
    Auto,
    Ssh,
    ControlPlane,
}

impl SourceMode {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "auto" => Ok(Self::Auto),
            "ssh" => Ok(Self::Ssh),
            "control-plane" => Ok(Self::ControlPlane),
            _ => anyhow::bail!(
                "Invalid --source '{}'. Expected one of: auto|ssh|control-plane",
                value
            ),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Ssh => "ssh",
            Self::ControlPlane => "control-plane",
        }
    }
}

#[derive(Debug, Clone)]
struct RemoteContainerRecord {
    server: String,
    name: String,
    id: String,
    image: String,
    status: String,
}

pub async fn run(
    config_path: &str,
    service: &str,
    follow: bool,
    tail: Option<usize>,
    source: &str,
) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let source_mode = SourceMode::parse(source)?;

    info!("Getting logs for service: {}", service);

    let services = config
        .services
        .context("No services defined in configuration")?;

    if !services.contains_key(service) {
        anyhow::bail!("Service '{}' not found in configuration", service);
    }

    let service_cfg = services
        .get(service)
        .context("Service disappeared from configuration")?;

    if source_mode == SourceMode::Auto || source_mode == SourceMode::ControlPlane {
        if let Ok(container_provider) = get_container_provider("docker") {
            if let Ok(container) = container_provider.get_container(service).await {
                output::line(format!(
                    "📋 Logs for service: {} ({})",
                    service, container.id
                ));
                output::line(format!("   Status: {:?}", container.status));
                output::line("   Source: control-plane");
                output::line("");

                match container_provider.logs(service, follow).await {
                    Ok(logs) => {
                        let display_logs = if let Some(tail_count) = tail {
                            if logs.len() > tail_count {
                                logs.into_iter()
                                    .rev()
                                    .take(tail_count)
                                    .collect::<Vec<_>>()
                                    .into_iter()
                                    .rev()
                                    .collect()
                            } else {
                                logs
                            }
                        } else {
                            logs
                        };

                        if output::is_json() {
                            output::emit_json(&LogsOutput {
                                service: service.to_string(),
                                container_id: container.id.clone(),
                                status: format!("{:?}", container.status),
                                source_mode: source_mode.as_str().to_string(),
                                server: None,
                                follow,
                                lines: display_logs,
                            })?;
                        } else {
                            if display_logs.is_empty() {
                                output::line(format!("No logs available for service: {}", service));
                            } else {
                                for log_line in display_logs {
                                    print!("{}", log_line);
                                }
                            }

                            if follow {
                                output::line("\n👀 Following logs... Press Ctrl+C to exit");
                                // In a real implementation, we'd continue streaming logs here
                                // The bollard stream would handle the continuous output
                            }
                        }
                    }
                    Err(e) => {
                        anyhow::bail!("Failed to retrieve logs for service {}: {}", service, e);
                    }
                }
                return Ok(());
            }
        }
    }

    if source_mode == SourceMode::ControlPlane {
        anyhow::bail!(
            "Service '{}' was not found on the local runtime control-plane. Use '--source ssh' to fetch remote logs.",
            service
        );
    }

    let infra = config
        .infra
        .context("No infra servers defined; cannot inspect remote logs over SSH")?;

    let mut remote_containers = Vec::new();
    for server_cfg in &infra.servers {
        if let Ok(mut items) = inspect_remote_containers_for_server(server_cfg).await {
            remote_containers.append(&mut items);
        }
    }

    let remote = find_remote_for_service(service, service_cfg, &remote_containers).context(
        "Service was not found on local runtime or remote SSH inventory. It may not be deployed.",
    )?;

    if !output::is_json() {
        output::line(format!("📋 Logs for service: {} ({})", service, remote.id));
        output::line(format!("   Status: {}", remote.status));
        output::line(format!("   Source: ssh ({})", remote.server));
        output::line("");
    }

    if follow {
        let script = remote_log_script(&remote.name, true, tail);
        let status = start_remote_session(
            infra
                .servers
                .iter()
                .find(|s| s.name == remote.server)
                .context("Matched remote server configuration is missing")?,
            &["sh".to_string(), "-lc".to_string(), script],
        )
        .await?;
        if status != 0 {
            anyhow::bail!("remote log follow exited with status {}", status);
        }
        return Ok(());
    }

    let logs = fetch_remote_logs_once(
        infra
            .servers
            .iter()
            .find(|s| s.name == remote.server)
            .context("Matched remote server configuration is missing")?,
        &remote.name,
        tail,
    )
    .await?;

    if output::is_json() {
        output::emit_json(&LogsOutput {
            service: service.to_string(),
            container_id: remote.id.clone(),
            status: remote.status.clone(),
            source_mode: source_mode.as_str().to_string(),
            server: Some(remote.server.clone()),
            follow,
            lines: logs.clone(),
        })?;
    } else if logs.is_empty() {
        output::line(format!("No logs available for service: {}", service));
    } else {
        for line in logs {
            print!("{}", line);
        }
    }

    Ok(())
}

async fn inspect_remote_containers_for_server(
    server_cfg: &ServerConfig,
) -> Result<Vec<RemoteContainerRecord>> {
    let scripts = [
        "docker ps -a --format '{{.ID}}\t{{.Image}}\t{{.Names}}\t{{.Status}}'",
        "docker container ls -a --format '{{.ID}}\t{{.Image}}\t{{.Names}}\t{{.Status}}'",
        "sudo -n docker ps -a --format '{{.ID}}\t{{.Image}}\t{{.Names}}\t{{.Status}}'",
        "podman ps -a --format '{{.ID}}\t{{.Image}}\t{{.Names}}\t{{.Status}}'",
        "sudo -n podman ps -a --format '{{.ID}}\t{{.Image}}\t{{.Names}}\t{{.Status}}'",
    ];

    let mut last_err = String::new();
    for script in scripts {
        let out = execute_remote_command(
            server_cfg,
            &["sh".to_string(), "-lc".to_string(), script.to_string()],
        )
        .await?;

        if out.status.success() {
            return parse_remote_container_lines(server_cfg, &out.stdout);
        }

        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if !stderr.is_empty() {
            last_err = stderr;
        }
    }

    anyhow::bail!("remote container inventory failed: {}", last_err);
}

fn parse_remote_container_lines(
    server_cfg: &ServerConfig,
    stdout: &[u8],
) -> Result<Vec<RemoteContainerRecord>> {
    let stdout = String::from_utf8_lossy(stdout);
    let mut items = Vec::new();
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        let mut parts = line.splitn(4, '\t').collect::<Vec<_>>();
        if parts.len() < 4 {
            parts = line.splitn(4, "\\t").collect::<Vec<_>>();
        }
        if parts.len() < 4 {
            continue;
        }
        items.push(RemoteContainerRecord {
            server: server_cfg.name.clone(),
            id: parts[0].trim().to_string(),
            image: parts[1].trim().to_string(),
            name: parts[2].trim().to_string(),
            status: parts[3].trim().to_string(),
        });
    }
    Ok(items)
}

fn find_remote_for_service<'a>(
    service_name: &str,
    service_cfg: &ServiceConfig,
    remote_containers: &'a [RemoteContainerRecord],
) -> Option<&'a RemoteContainerRecord> {
    if let Some(exact) = remote_containers.iter().find(|c| c.name == service_name) {
        return Some(exact);
    }

    if let Some(prefix) = remote_containers.iter().find(|c| {
        c.name == format!("{service_name}-1")
            || c.name.starts_with(&format!("{service_name}_"))
            || c.name.starts_with(&format!("{service_name}-"))
    }) {
        return Some(prefix);
    }

    let desired_repo = service_cfg
        .image
        .split(':')
        .next()
        .unwrap_or(&service_cfg.image);
    remote_containers.iter().find(|c| {
        let running_repo = c.image.split(':').next().unwrap_or(&c.image);
        running_repo == desired_repo
    })
}

async fn fetch_remote_logs_once(
    server_cfg: &ServerConfig,
    container_name: &str,
    tail: Option<usize>,
) -> Result<Vec<String>> {
    let tail_arg = tail
        .map(|n| format!("--tail {}", n))
        .unwrap_or_else(|| "--tail 200".to_string());
    let quoted_name = shell_quote(container_name);
    let scripts = [
        format!("docker logs {tail_arg} {quoted_name} 2>&1"),
        format!("sudo -n docker logs {tail_arg} {quoted_name} 2>&1"),
        format!("podman logs {tail_arg} {quoted_name} 2>&1"),
        format!("sudo -n podman logs {tail_arg} {quoted_name} 2>&1"),
    ];

    let mut last_err = String::new();
    for script in scripts {
        let out =
            execute_remote_command(server_cfg, &["sh".to_string(), "-lc".to_string(), script])
                .await?;

        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|line| format!("{line}\n"))
                .collect());
        }

        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if !stderr.is_empty() {
            last_err = stderr;
        }
    }

    anyhow::bail!("remote logs command failed: {}", last_err);
}

fn remote_log_script(container_name: &str, follow: bool, tail: Option<usize>) -> String {
    let follow_arg = if follow { "-f " } else { "" };
    let tail_arg = tail
        .map(|n| format!("--tail {}", n))
        .unwrap_or_else(|| "--tail 200".to_string());
    let name = shell_quote(container_name);
    format!(
        "if command -v docker >/dev/null 2>&1; then docker logs {follow_arg}{tail_arg} {name}; \
         elif command -v podman >/dev/null 2>&1; then podman logs {follow_arg}{tail_arg} {name}; \
         elif command -v sudo >/dev/null 2>&1 && sudo -n docker info >/dev/null 2>&1; then sudo -n docker logs {follow_arg}{tail_arg} {name}; \
         elif command -v sudo >/dev/null 2>&1 && sudo -n podman info >/dev/null 2>&1; then sudo -n podman logs {follow_arg}{tail_arg} {name}; \
         else echo 'no supported container runtime found' >&2; exit 1; fi"
    )
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

#[cfg(test)]
mod tests {
    use super::{find_remote_for_service, RemoteContainerRecord};
    use airstack_config::ServiceConfig;
    use std::collections::HashMap;

    fn svc(image: &str) -> ServiceConfig {
        ServiceConfig {
            image: image.to_string(),
            ports: vec![],
            env: Some(HashMap::new()),
            volumes: None,
            depends_on: None,
            target_server: None,
            healthcheck: None,
            profile: None,
        }
    }

    #[test]
    fn find_remote_matches_prefix_name() {
        let records = vec![RemoteContainerRecord {
            server: "node-a".to_string(),
            name: "api-1".to_string(),
            id: "abc".to_string(),
            image: "repo/api:latest".to_string(),
            status: "Up 2 minutes".to_string(),
        }];
        let found = find_remote_for_service("api", &svc("repo/api:latest"), &records)
            .expect("prefix match should find container");
        assert_eq!(found.name, "api-1");
    }

    #[test]
    fn find_remote_matches_by_repo_when_name_differs() {
        let records = vec![RemoteContainerRecord {
            server: "node-a".to_string(),
            name: "generated-container".to_string(),
            id: "abc".to_string(),
            image: "repo/api:v2".to_string(),
            status: "Up 2 minutes".to_string(),
        }];
        let found = find_remote_for_service("api", &svc("repo/api:latest"), &records)
            .expect("repo match should find container");
        assert_eq!(found.name, "generated-container");
    }
}
