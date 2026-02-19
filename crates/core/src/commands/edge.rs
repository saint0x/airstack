use crate::output;
use crate::ssh_utils::{execute_remote_command, lookup_provider_server};
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
        EdgeCommands::Diagnose => diagnose(&config).await,
        EdgeCommands::Apply => apply_from_config(&config).await,
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
    dns_ttl_secs: Option<u32>,
    nameservers: Vec<String>,
    tls_handshake_ok: bool,
    remediation: Vec<String>,
}

async fn diagnose(config: &AirstackConfig) -> Result<()> {
    let edge = config.edge.as_ref().context("No [edge] config defined")?;
    let expected_edge_ip = resolve_edge_server_ip(config).await;

    let mut rows = Vec::new();
    for site in &edge.sites {
        let resolved = (site.host.as_str(), 443)
            .to_socket_addrs()
            .map(|iter| iter.map(|a| a.ip().to_string()).collect::<Vec<_>>())
            .unwrap_or_default();
        let dns_ok = !resolved.is_empty();
        let dns_ttl_secs = query_dns_ttl(&site.host).await;
        let nameservers = query_nameservers(&site.host).await;
        let dns_target_matches = expected_edge_ip
            .as_ref()
            .map(|ip| resolved.iter().any(|r| r == ip))
            .unwrap_or(true);

        let mut tls_ok = false;
        let mut remediation = Vec::new();
        if !dns_ok {
            remediation.push(format!(
                "DNS: ensure A/AAAA for '{}' points to edge host before ACME issuance",
                site.host
            ));
            if nameservers.is_empty() {
                remediation.push("NS visibility: no nameservers discovered for host domain; verify delegation at registrar".to_string());
            }
        } else if !dns_target_matches {
            remediation.push(format!(
                "DNS mismatch: '{}' resolves to [{}], expected edge IP {}",
                site.host,
                resolved.join(", "),
                expected_edge_ip
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string())
            ));
            remediation
                .push("Update DNS A/AAAA to the expected edge IP before ACME issuance".to_string());
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
            dns_ttl_secs,
            nameservers,
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
                "{} {} dns={} ttl={}ns={} tls={}",
                mark,
                row.host,
                row.dns_resolved,
                row.dns_ttl_secs
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".to_string()),
                if row.nameservers.is_empty() {
                    "?".to_string()
                } else {
                    row.nameservers.join(",")
                },
                row.tls_handshake_ok
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

pub async fn apply_from_config(config: &AirstackConfig) -> Result<()> {
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
        r#"set -e
tmp="$(mktemp /tmp/airstack-caddy.XXXXXX)"
cat > "$tmp" <<'CADDY'
{caddy}
CADDY

container_id=""
if command -v docker >/dev/null 2>&1; then
  container_id="$(docker ps -aqf 'name=^/caddy$' | head -n1 || true)"
fi

target=""
if [ -n "$container_id" ]; then
  mount_source="$(docker inspect -f '{{{{range .Mounts}}}}{{{{if eq .Destination "/etc/caddy/Caddyfile"}}}}{{{{.Source}}}}{{{{end}}}}{{{{end}}}}' caddy 2>/dev/null || true)"
  if [ -n "$mount_source" ]; then
    target="$mount_source"
  fi
fi

if [ -z "$target" ]; then
  for p in /opt/aria/Caddyfile /etc/caddy/Caddyfile; do
    if [ -e "$p" ]; then
      target="$p"
      break
    fi
  done
fi

host_write_ok=0
if [ -n "$target" ]; then
  mkdir -p "$(dirname "$target")" 2>/dev/null || true
  if cp "$tmp" "$target" 2>/dev/null; then
    host_write_ok=1
  fi
fi

if [ "$host_write_ok" -eq 0 ] && [ -n "$container_id" ]; then
  docker cp "$tmp" caddy:/etc/caddy/Caddyfile
  docker exec caddy sh -lc 'caddy validate --config /etc/caddy/Caddyfile' || true
  docker restart caddy >/dev/null 2>&1 || true
  echo "container:/etc/caddy/Caddyfile"
  rm -f "$tmp"
  exit 0
fi

if [ "$host_write_ok" -eq 1 ]; then
  if [ -n "$container_id" ]; then
    docker restart caddy >/dev/null 2>&1 || true
  elif command -v caddy >/dev/null 2>&1; then
    caddy validate --config "$target"
    if command -v systemctl >/dev/null 2>&1; then
      systemctl reload caddy || true
    fi
  fi
  echo "$target"
  rm -f "$tmp"
  exit 0
fi

echo "failed to write Caddyfile (host path not writable and no caddy container fallback)" >&2
rm -f "$tmp"
exit 1
"#,
        caddy = caddyfile
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

    let applied = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .last()
        .unwrap_or_default()
        .to_string();
    if !applied.is_empty() {
        output::line(format!("✅ edge apply: caddy config synced at {}", applied));
    } else {
        output::line("✅ edge apply: caddy config synced");
    }
    Ok(())
}

async fn resolve_edge_server_ip(config: &AirstackConfig) -> Option<String> {
    let infra = config.infra.as_ref()?;
    let server = infra.servers.first()?;
    let provider_server = lookup_provider_server(server).await.ok()?;
    provider_server.public_ip
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

async fn query_dns_ttl(host: &str) -> Option<u32> {
    let out = Command::new("sh")
        .arg("-lc")
        .arg(format!(
            "dig +noall +answer A {} 2>/dev/null | head -n 1",
            host
        ))
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if line.is_empty() {
        return None;
    }
    let parts = line.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 2 {
        return None;
    }
    parts.get(1)?.parse::<u32>().ok()
}

async fn query_nameservers(host: &str) -> Vec<String> {
    let apex = derive_apex(host);
    let cmd = format!(
        "dig +short NS {host} 2>/dev/null; dig +short NS {apex} 2>/dev/null",
        host = host,
        apex = apex
    );
    let out = Command::new("sh").arg("-lc").arg(cmd).output().await;
    let Ok(out) = out else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let mut uniq = std::collections::BTreeSet::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let ns = line.trim().trim_end_matches('.');
        if !ns.is_empty() {
            uniq.insert(ns.to_string());
        }
    }
    uniq.into_iter().collect()
}

fn derive_apex(host: &str) -> String {
    let parts = host.split('.').collect::<Vec<_>>();
    if parts.len() >= 2 {
        format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1])
    } else {
        host.to_string()
    }
}
