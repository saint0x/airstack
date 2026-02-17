use crate::{Container, ContainerProvider, ContainerStatus, PortMapping, RunServiceRequest};
use anyhow::{Context, Result};
use bollard::container::{
    Config, CreateContainerOptions, ListContainersOptions, LogsOptions, RemoveContainerOptions,
    StartContainerOptions, StopContainerOptions,
};
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::image::BuildImageOptions;
use bollard::models::{ContainerSummary, HostConfig, PortBinding};
use bollard::Docker;
use std::collections::HashMap;
use std::default::Default;
use tokio_stream::StreamExt;
use tracing::{debug, info, warn};

pub struct DockerProvider {
    docker: Docker,
}

impl DockerProvider {
    pub fn new() -> Result<Self> {
        let docker =
            Docker::connect_with_socket_defaults().context("Failed to connect to Docker daemon")?;

        Ok(Self { docker })
    }

    fn convert_status(state: &str) -> ContainerStatus {
        match state {
            "created" => ContainerStatus::Creating,
            "running" => ContainerStatus::Running,
            "paused" => ContainerStatus::Paused,
            "restarting" => ContainerStatus::Restarting,
            "removing" => ContainerStatus::Removing,
            "exited" => ContainerStatus::Exited,
            "dead" => ContainerStatus::Dead,
            _ => ContainerStatus::Stopped,
        }
    }

    fn convert_container(summary: ContainerSummary) -> Container {
        let ports = summary
            .ports
            .unwrap_or_default()
            .into_iter()
            .map(|port| PortMapping {
                container_port: port.private_port,
                host_port: port.public_port,
                protocol: port
                    .typ
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "tcp".to_string()),
            })
            .collect();

        Container {
            id: summary.id.unwrap_or_default(),
            name: summary
                .names
                .unwrap_or_default()
                .first()
                .map(|n| n.trim_start_matches('/').to_string())
                .unwrap_or_default(),
            image: summary.image.unwrap_or_default(),
            status: Self::convert_status(&summary.state.unwrap_or_default()),
            ports,
        }
    }
}

#[async_trait::async_trait]
impl ContainerProvider for DockerProvider {
    async fn build_image(&self, path: &str, tag: &str) -> Result<()> {
        info!("Building Docker image: {} from {}", tag, path);

        let build_options = BuildImageOptions {
            dockerfile: "Dockerfile",
            t: tag,
            ..Default::default()
        };

        let tar_path = std::path::Path::new(path);
        if !tar_path.exists() {
            anyhow::bail!("Build path does not exist: {}", path);
        }

        // For simplicity, we'll expect a tar archive at the path
        let tar_bytes = tokio::fs::read(path)
            .await
            .with_context(|| format!("Failed to read build context tar: {}", path))?;

        let mut stream = self
            .docker
            .build_image(build_options, None, Some(tar_bytes.into()));

        while let Some(msg) = stream.next().await {
            match msg {
                Ok(output) => {
                    if let Some(stream) = output.stream {
                        debug!("Build output: {}", stream.trim());
                    }
                    if let Some(error) = output.error {
                        anyhow::bail!("Build failed: {}", error);
                    }
                }
                Err(e) => anyhow::bail!("Build stream error: {}", e),
            }
        }

        info!("Successfully built image: {}", tag);
        Ok(())
    }

    async fn run_service(&self, request: RunServiceRequest) -> Result<Container> {
        info!(
            "Running service: {} with image: {}",
            request.name, request.image
        );

        // Idempotent deploy: remove an existing container with the same name before create.
        let _ = self
            .docker
            .remove_container(
                &request.name,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await;

        let mut port_bindings = HashMap::new();
        for port in &request.ports {
            let container_port = format!("{}/tcp", port);
            port_bindings.insert(
                container_port,
                Some(vec![PortBinding {
                    host_ip: Some("0.0.0.0".to_string()),
                    host_port: Some(port.to_string()),
                }]),
            );
        }

        let host_config = HostConfig {
            port_bindings: Some(port_bindings),
            restart_policy: request.restart_policy.map(|policy| {
                use bollard::models::RestartPolicyNameEnum;
                let name = match policy.as_str() {
                    "always" => RestartPolicyNameEnum::ALWAYS,
                    "unless-stopped" => RestartPolicyNameEnum::UNLESS_STOPPED,
                    "on-failure" => RestartPolicyNameEnum::ON_FAILURE,
                    _ => RestartPolicyNameEnum::NO,
                };
                bollard::models::RestartPolicy {
                    name: Some(name),
                    maximum_retry_count: None,
                }
            }),
            binds: request.volumes,
            ..Default::default()
        };

        let env: Option<Vec<String>> = request.env.map(|env_map| {
            env_map
                .into_iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect()
        });

        let config = Config {
            image: Some(request.image.clone()),
            env,
            exposed_ports: Some(
                request
                    .ports
                    .iter()
                    .map(|port| (format!("{}/tcp", port), HashMap::new()))
                    .collect(),
            ),
            host_config: Some(host_config),
            ..Default::default()
        };

        let options = CreateContainerOptions {
            name: request.name.clone(),
            platform: None,
        };

        let container = self
            .docker
            .create_container(Some(options), config)
            .await
            .context("Failed to create container")?;

        self.docker
            .start_container(&container.id, None::<StartContainerOptions<String>>)
            .await
            .context("Failed to start container")?;

        info!(
            "Successfully started service: {} ({})",
            request.name, container.id
        );

        Ok(Container {
            id: container.id,
            name: request.name,
            image: request.image,
            status: ContainerStatus::Running,
            ports: request
                .ports
                .into_iter()
                .map(|port| PortMapping {
                    container_port: port,
                    host_port: Some(port),
                    protocol: "tcp".to_string(),
                })
                .collect(),
        })
    }

    async fn stop_service(&self, name: &str) -> Result<()> {
        info!("Stopping service: {}", name);

        let options = StopContainerOptions { t: 10 };

        self.docker
            .stop_container(name, Some(options))
            .await
            .with_context(|| format!("Failed to stop container: {}", name))?;

        self.docker
            .remove_container(name, None)
            .await
            .with_context(|| format!("Failed to remove container: {}", name))?;

        info!("Successfully stopped and removed service: {}", name);
        Ok(())
    }

    async fn get_container(&self, name: &str) -> Result<Container> {
        debug!("Getting container: {}", name);

        let options = ListContainersOptions::<String> {
            all: true,
            filters: [("name".to_string(), vec![name.to_string()])]
                .iter()
                .cloned()
                .collect(),
            ..Default::default()
        };

        let containers = self
            .docker
            .list_containers(Some(options))
            .await
            .context("Failed to list containers")?;

        let container = containers
            .into_iter()
            .find(|c| {
                c.names
                    .as_ref()
                    .unwrap_or(&vec![])
                    .iter()
                    .any(|n| n.trim_start_matches('/') == name)
            })
            .with_context(|| format!("Container not found: {}", name))?;

        Ok(Self::convert_container(container))
    }

    async fn list_containers(&self) -> Result<Vec<Container>> {
        debug!("Listing containers");

        let options = ListContainersOptions::<String> {
            all: true,
            ..Default::default()
        };

        let containers = self
            .docker
            .list_containers(Some(options))
            .await
            .context("Failed to list containers")?;

        Ok(containers
            .into_iter()
            .map(Self::convert_container)
            .collect())
    }

    async fn logs(&self, name: &str, follow: bool) -> Result<Vec<String>> {
        debug!("Getting logs for container: {}", name);

        let options = LogsOptions::<String> {
            follow,
            stdout: true,
            stderr: true,
            timestamps: true,
            ..Default::default()
        };

        let mut stream = self.docker.logs(name, Some(options));
        let mut logs = Vec::new();

        while let Some(msg) = stream.next().await {
            match msg {
                Ok(log_output) => {
                    logs.push(log_output.to_string());
                    if !follow && logs.len() > 1000 {
                        break;
                    }
                }
                Err(e) => {
                    warn!("Error reading logs: {}", e);
                    break;
                }
            }
        }

        Ok(logs)
    }

    async fn exec(&self, name: &str, command: Vec<String>) -> Result<String> {
        info!("Executing command in container {}: {:?}", name, command);

        let exec_options = CreateExecOptions {
            cmd: Some(command),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            ..Default::default()
        };

        let exec = self
            .docker
            .create_exec(name, exec_options)
            .await
            .context("Failed to create exec")?;

        let start_exec = self.docker.start_exec(&exec.id, None).await?;

        let mut result = String::new();
        if let StartExecResults::Attached { mut output, .. } = start_exec {
            while let Some(msg) = output.next().await {
                match msg {
                    Ok(log_output) => {
                        result.push_str(&log_output.to_string());
                    }
                    Err(e) => {
                        warn!("Error reading exec output: {}", e);
                        break;
                    }
                }
            }
        }

        Ok(result)
    }
}
