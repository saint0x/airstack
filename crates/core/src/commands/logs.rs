use crate::output;
use airstack_config::AirstackConfig;
use airstack_container::get_provider as get_container_provider;
use anyhow::{Context, Result};
use serde::Serialize;
use tracing::info;

#[derive(Debug, Serialize)]
struct LogsOutput {
    service: String,
    container_id: String,
    status: String,
    follow: bool,
    lines: Vec<String>,
}

pub async fn run(
    config_path: &str,
    service: &str,
    follow: bool,
    tail: Option<usize>,
) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;

    info!("Getting logs for service: {}", service);

    let services = config
        .services
        .context("No services defined in configuration")?;

    if !services.contains_key(service) {
        anyhow::bail!("Service '{}' not found in configuration", service);
    }

    let container_provider =
        get_container_provider("docker").context("Failed to initialize Docker provider")?;

    // Check if the service is running
    match container_provider.get_container(service).await {
        Ok(container) => {
            output::line(format!(
                "📋 Logs for service: {} ({})",
                service, container.id
            ));
            output::line(format!("   Status: {:?}", container.status));
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
        }
        Err(_) => {
            anyhow::bail!(
                "Service '{}' is not currently running. Deploy it first with 'airstack deploy {}'",
                service,
                service
            );
        }
    }

    Ok(())
}
