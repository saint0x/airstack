use crate::output;
use crate::ssh_utils::execute_remote_command;
use airstack_config::{AirstackConfig, EdgeSiteConfig};
use anyhow::{Context, Result};
use clap::Subcommand;
use serde::Serialize;
use std::net::ToSocketAddrs;

#[derive(Debug, Clone, Subcommand)]
pub enum EdgeCommands {
    #[command(about = "Preview reverse-proxy config and actions")]
    Plan,
    #[command(about = "Apply reverse-proxy config")]
    Apply,
    #[command(about = "Validate DNS and edge prerequisites")]
    Validate,
    #[command(about = "Show edge status")]
    Status,
}

#[derive(Debug, Serialize)]
struct EdgeStatus {
    provider: String,
    sites: Vec<EdgeSiteStatus>,
}

#[derive(Debug, Serialize)]
struct EdgeSiteStatus {
    host: String,
    dns_resolved: bool,
    upstream_service: String,
    upstream_port: u16,
}

pub async fn run(config_path: &str, command: EdgeCommands) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let edge = config.edge.as_ref().context("No [edge] config defined")?;

    match command {
        EdgeCommands::Plan => plan(edge),
        EdgeCommands::Validate => validate(edge),
        EdgeCommands::Status => status(edge),
        EdgeCommands::Apply => apply(&config).await,
    }
}

fn plan(edge: &airstack_config::EdgeConfig) -> Result<()> {
    let rendered = render_caddyfile(&edge.sites);
    output::line("🧩 Edge Plan");
    output::line(format!("Provider: {}", edge.provider));
    output::line("Generated Caddyfile:");
    output::line(rendered);
    Ok(())
}

fn validate(edge: &airstack_config::EdgeConfig) -> Result<()> {
    let mut failures = Vec::new();
    for site in &edge.sites {
        let ok = (site.host.as_str(), 443)
            .to_socket_addrs()
            .map(|mut a| a.next().is_some())
            .unwrap_or(false);
        if !ok {
            failures.push(format!("{} does not resolve for :443", site.host));
        }
    }

    if failures.is_empty() {
        output::line("✅ edge validate: DNS prerequisites look good");
        return Ok(());
    }

    output::line("❌ edge validate failed:");
    for f in &failures {
        output::line(format!("- {}", f));
    }
    anyhow::bail!("edge validation failed")
}

fn status(edge: &airstack_config::EdgeConfig) -> Result<()> {
    let sites = edge
        .sites
        .iter()
        .map(|s| EdgeSiteStatus {
            host: s.host.clone(),
            dns_resolved: (s.host.as_str(), 443)
                .to_socket_addrs()
                .map(|mut a| a.next().is_some())
                .unwrap_or(false),
            upstream_service: s.upstream_service.clone(),
            upstream_port: s.upstream_port,
        })
        .collect::<Vec<_>>();

    let payload = EdgeStatus {
        provider: edge.provider.clone(),
        sites,
    };

    if output::is_json() {
        output::emit_json(&payload)?;
    } else {
        output::line("🌐 Edge Status");
        output::line(format!("Provider: {}", payload.provider));
        for s in payload.sites {
            output::line(format!(
                "- {} -> {}:{} (dns={})",
                s.host, s.upstream_service, s.upstream_port, s.dns_resolved
            ));
        }
    }

    Ok(())
}

async fn apply(config: &AirstackConfig) -> Result<()> {
    let edge = config.edge.as_ref().context("No [edge] config defined")?;
    if edge.provider != "caddy" {
        anyhow::bail!("Only edge.provider='caddy' is currently supported");
    }

    let infra = config
        .infra
        .as_ref()
        .context("Edge apply requires infra.servers")?;
    let server = infra
        .servers
        .first()
        .context("Edge apply requires at least one server")?;

    let caddyfile = render_caddyfile(&edge.sites);
    let upload_script = format!(
        "cat > /etc/caddy/Caddyfile <<'CADDY'\n{}\nCADDY\n && caddy validate --config /etc/caddy/Caddyfile && systemctl reload caddy",
        caddyfile
    );

    let out = execute_remote_command(
        server,
        &["sh".to_string(), "-lc".to_string(), upload_script],
    )
    .await?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("Edge apply failed: {}", stderr.trim());
    }

    output::line("✅ edge apply: caddy config updated and reloaded");
    Ok(())
}

fn render_caddyfile(sites: &[EdgeSiteConfig]) -> String {
    let mut lines = Vec::new();
    for site in sites {
        lines.push(format!("{} {{", site.host));
        if site.redirect_http.unwrap_or(true) {
            lines.push("  @http protocol http".to_string());
            lines.push("  redir @http https://{host}{uri} 308".to_string());
        }
        if let Some(email) = &site.tls_email {
            lines.push(format!("  tls {}", email));
        }
        lines.push(format!(
            "  reverse_proxy {}:{}",
            site.upstream_service, site.upstream_port
        ));
        lines.push("}".to_string());
        lines.push(String::new());
    }
    lines.join("\n")
}
