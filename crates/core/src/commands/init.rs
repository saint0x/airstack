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

pub async fn run(
    name: Option<String>,
    provider: Option<String>,
    preset: Option<String>,
    config_path: &str,
) -> Result<()> {
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
    let mut updated_content = content.replace("my-project", &project_name);
    if provider.as_deref() == Some("hetzner") {
        updated_content = updated_content
            .replace("region = \"nbg1\"", "region = \"ash\"")
            .replace("server_type = \"cx21\"", "server_type = \"cpx21\"");
    }
    if provider.as_deref() == Some("fly") {
        updated_content = updated_content
            .replace("provider = \"hetzner\"", "provider = \"fly\"")
            .replace("region = \"nbg1\"", "region = \"iad\"")
            .replace("server_type = \"cx21\"", "server_type = \"shared-cpu-1x\"");
    }

    if preset.as_deref() == Some("clickhouse") {
        updated_content.push_str(
            r#"

[services.clickhouse]
image = "clickhouse/clickhouse-server:24.8"
ports = [8123, 9000]
env = { CLICKHOUSE_DB = "analytics", CLICKHOUSE_DEFAULT_ACCESS_MANAGEMENT = "1" }
volumes = ["/opt/airstack/clickhouse/data:/var/lib/clickhouse"]
healthcheck = { http = { path = "/ping", port = 8123, expected_status = 200 }, interval_secs = 5, retries = 20, timeout_secs = 3 }
# Optional: ensure public interfaces are enabled in your remote config:
# /opt/airstack/clickhouse/config/config.d/network.xml with
#   <clickhouse><listen_host>0.0.0.0</listen_host></clickhouse>
"#,
        );
    }
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
        if let Some(provider) = provider {
            output::line(format!("🔌 Provider preset: {}", provider));
        }
        if let Some(preset) = preset {
            output::line(format!("📦 Service preset: {}", preset));
        }
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
