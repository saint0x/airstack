use crate::output;
use airstack_config::AirstackConfig;
use airstack_metal::get_provider as get_metal_provider;
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{self, Write};
use tracing::{info, warn};

#[derive(Debug, Serialize)]
struct DestroyOutput {
    project: String,
    destroyed: Vec<String>,
    not_found: Vec<String>,
    failed: Vec<String>,
}

pub async fn run(config_path: &str, _target: Option<String>, force: bool) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;

    info!(
        "Planning destruction of infrastructure for project: {}",
        config.project.name
    );
    let mut destroyed = Vec::new();
    let mut not_found = Vec::new();
    let mut failed = Vec::new();

    if let Some(infra) = &config.infra {
        output::line("⚠️  The following servers will be DESTROYED:");
        for server in &infra.servers {
            output::line(format!(
                "   • {} ({} in {})",
                server.name, server.server_type, server.region
            ));
        }
        output::line("");

        if !force {
            print!("Are you sure you want to destroy this infrastructure? (y/N): ");
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;

            if !input.trim().to_lowercase().starts_with('y') {
                output::line("Aborted.");
                return Ok(());
            }
        }

        for server in &infra.servers {
            info!("🗑️  Destroying server: {}", server.name);

            let provider_config = HashMap::new();

            let metal_provider = get_metal_provider(&server.provider, provider_config)
                .with_context(|| format!("Failed to initialize {} provider", server.provider))?;

            // First, we need to list servers to find the ID
            match metal_provider.list_servers().await {
                Ok(servers) => {
                    if let Some(found_server) = servers.iter().find(|s| s.name == server.name) {
                        match metal_provider.destroy_server(&found_server.id).await {
                            Ok(_) => {
                                output::line(format!("✅ Destroyed server: {}", server.name));
                                destroyed.push(server.name.clone());
                            }
                            Err(e) => {
                                warn!("❌ Failed to destroy server {}: {}", server.name, e);
                                failed.push(server.name.clone());
                            }
                        }
                    } else {
                        warn!(
                            "⚠️  Server not found: {} (may have been already deleted)",
                            server.name
                        );
                        not_found.push(server.name.clone());
                    }
                }
                Err(e) => {
                    warn!("❌ Failed to list servers: {}", e);
                    failed.push(server.name.clone());
                }
            }
        }
    } else {
        output::line("No infrastructure defined in configuration.");
    }

    if output::is_json() {
        output::emit_json(&DestroyOutput {
            project: config.project.name,
            destroyed,
            not_found,
            failed,
        })?;
    } else {
        output::line("🧹 Infrastructure destruction completed!");
    }

    Ok(())
}
