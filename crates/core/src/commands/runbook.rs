use crate::output;
use airstack_config::AirstackConfig;
use anyhow::{Context, Result};

pub async fn run(config_path: &str) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;

    output::line(format!("📘 Runbook: {}", config.project.name));
    output::line("1. Check drift and health");
    output::line("   airstack status --detailed");
    output::line("2. Validate policy and config safety");
    output::line("   airstack doctor");
    output::line("3. Preview changes");
    output::line("   airstack plan");
    output::line("4. Apply changes");
    output::line("   airstack apply");
    output::line("5. Build and publish release image");
    output::line("   airstack release <service> --push --update-config");
    output::line("6. Service troubleshooting");
    output::line("   airstack logs <service> --follow");
    output::line("   airstack ssh <server>");
    output::line("7. Secrets and backup operations");
    output::line("   airstack secrets list");
    output::line("   airstack backup status");

    if config.edge.is_some() {
        output::line("8. Edge checks");
        output::line("   airstack edge validate");
        output::line("   airstack edge status");
    }

    Ok(())
}
