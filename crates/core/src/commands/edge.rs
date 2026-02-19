use crate::output;
use crate::ssh_utils::execute_remote_command;
use airstack_config::{AirstackConfig, EdgeSiteConfig};
use anyhow::{Context, Result};
use clap::Subcommand;
use serde::Serialize;
use std::net::ToSocketAddrs;
use tokio::process::Command;

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
    #[command(about = "Diagnose TLS/ACME edge issues with remediation hints")]
    Diagnose,
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
        EdgeCommands::Diagnose => diagnose(edge).await,
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

#[derive(Debug, Serialize)]
struct EdgeDiagnosis {
    host: String,
    dns_resolved: bool,
    tls_handshake_ok: bool,
    remediation: Vec<String>,
}

async fn diagnose(edge: &airstack_config::EdgeConfig) -> Result<()> {
    let mut rows = Vec::new();
    for site in &edge.sites {
        let dns_ok = (site.host.as_str(), 443)
            .to_socket_addrs()
            .map(|mut a| a.next().is_some())
            .unwrap_or(false);

        let mut tls_ok = false;
        let mut remediation = Vec::new();
        if !dns_ok {
            remediation.push(format!(
                "DNS: ensure A/AAAA for '{}' points to edge host before ACME issuance",
                site.host
            ));
        } else {
            let out = Command::new("sh")
                .arg("-lc")
                .arg(format!(
                    "echo | openssl s_client -connect {h}:443 -servername {h} -brief 2>/dev/null",
                    h = site.host
                ))
                .output()
                .await
                .context("Failed to run openssl for edge diagnosis")?;
            tls_ok = out.status.success();
            if !tls_ok {
                remediation.push(format!(
                    "TLS: verify port 443 open and Caddy running, then run `airstack edge apply`"
                ));
                remediation.push(format!(
                    "ACME: check Caddy logs (`journalctl -u caddy -n 200`) for challenge failure details"
                ));
            }
        }

        if site.tls_email.is_none() {
            remediation.push(format!(
                "Config: set tls_email for '{}' to improve ACME ops visibility",
                site.host
            ));
        }

        rows.push(EdgeDiagnosis {
            host: site.host.clone(),
            dns_resolved: dns_ok,
            tls_handshake_ok: tls_ok,
            remediation,
        });
    }

    if output::is_json() {
        output::emit_json(&serde_json::json!({ "diagnosis": rows }))?;
    } else {
        output::line("🩺 Edge Diagnose");
        for row in &rows {
            let ok = row.dns_resolved && row.tls_handshake_ok;
            let mark = if ok { "✅" } else { "❌" };
            output::line(format!(
                "{} {} dns={} tls={}",
                mark, row.host, row.dns_resolved, row.tls_handshake_ok
            ));
            for hint in &row.remediation {
                output::line(format!("   - {}", hint));
            }
        }
    }

    if rows.iter().any(|r| !r.dns_resolved || !r.tls_handshake_ok) {
        anyhow::bail!("edge diagnose found actionable issues")
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
