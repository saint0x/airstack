use crate::output;
use airstack_config::AirstackConfig;
use anyhow::{Context, Result};
use serde::Serialize;
use std::path::Path;
use tracing::info;

#[derive(Debug, Serialize)]
struct InitOutput {
    project: String,
    config_path: String,
    created: bool,
}

pub async fn run(name: Option<String>, config_path: &str) -> Result<()> {
    let project_name = name.unwrap_or_else(|| {
        std::env::current_dir()
            .ok()
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().to_string())
            })
            .unwrap_or_else(|| "my-project".to_string())
    });

    let config_file = Path::new(config_path);

    if config_file.exists() {
        anyhow::bail!("Configuration file already exists: {}", config_path);
    }

    info!("Initializing new Airstack project: {}", project_name);

    AirstackConfig::init_example(config_file).context("Failed to create example configuration")?;

    // Replace the project name in the generated config
    let content = std::fs::read_to_string(config_file)?;
    let updated_content = content.replace("my-project", &project_name);
    std::fs::write(config_file, updated_content)?;

    if output::is_json() {
        output::emit_json(&InitOutput {
            project: project_name,
            config_path: config_path.to_string(),
            created: true,
        })?;
    } else {
        output::line(format!("✅ Initialized Airstack project: {}", project_name));
        output::line(format!("📝 Configuration created: {}", config_path));
        output::line("");
        output::line("Next steps:");
        output::line(format!(
            "  1. Edit {} to configure your infrastructure",
            config_path
        ));
        output::line(
            "  2. Set up provider credentials in global AirStack env (~/.airstack/.env), e.g. HETZNER_API_KEY",
        );
        output::line("  3. Run 'airstack up' to provision your infrastructure");
    }

    Ok(())
}
