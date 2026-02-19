use crate::output;
use crate::ssh_utils::execute_remote_command;
use airstack_config::AirstackConfig;
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde::Serialize;

#[derive(Debug, Clone, Subcommand)]
pub enum RegistryCommands {
    #[command(about = "Verify remote registry credentials/image pull permissions")]
    Doctor(RegistryDoctorArgs),
}

#[derive(Debug, Clone, Args)]
pub struct RegistryDoctorArgs {
    #[arg(long, help = "Server name (default: all non-fly servers)")]
    pub server: Option<String>,
    #[arg(
        long,
        help = "Image to verify pull access for",
        default_value = "ghcr.io/OWNER/REPO:TAG"
    )]
    pub image: String,
}

#[derive(Debug, Serialize)]
struct RegistryDoctorRecord {
    server: String,
    image: String,
    ok: bool,
    detail: String,
}

pub async fn run(config_path: &str, command: RegistryCommands) -> Result<()> {
    match command {
        RegistryCommands::Doctor(args) => doctor(config_path, args).await,
    }
}

async fn doctor(config_path: &str, args: RegistryDoctorArgs) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let infra = config
        .infra
        .as_ref()
        .context("No infra.servers configured")?;

    let targets = infra
        .servers
        .iter()
        .filter(|s| args.server.as_ref().is_none_or(|name| &s.name == name))
        .collect::<Vec<_>>();
    if targets.is_empty() {
        anyhow::bail!("No matching servers for registry doctor");
    }

    let mut rows = Vec::new();
    for server in targets {
        if server.provider == "fly" {
            rows.push(RegistryDoctorRecord {
                server: server.name.clone(),
                image: args.image.clone(),
                ok: true,
                detail: "provider=fly uses fly-managed image pull path".to_string(),
            });
            continue;
        }

        let cmd = format!("docker pull {} >/dev/null 2>&1", shell_quote(&args.image));
        let out = execute_remote_command(
            server,
            &["sh".to_string(), "-lc".to_string(), cmd.to_string()],
        )
        .await?;

        if out.status.success() {
            rows.push(RegistryDoctorRecord {
                server: server.name.clone(),
                image: args.image.clone(),
                ok: true,
                detail: "pull succeeded".to_string(),
            });
            continue;
        }

        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let mut detail = stderr.clone();
        if args.image.starts_with("ghcr.io/")
            && (stderr.contains("unauthorized")
                || stderr.contains("denied")
                || stderr.contains("authentication required"))
        {
            detail.push_str(
                " | remediation: docker login ghcr.io on this host with token scope read:packages",
            );
        }
        rows.push(RegistryDoctorRecord {
            server: server.name.clone(),
            image: args.image.clone(),
            ok: false,
            detail,
        });
    }

    if output::is_json() {
        output::emit_json(&serde_json::json!({ "results": rows }))?;
    } else {
        output::line("🔐 Registry Doctor");
        for row in &rows {
            let mark = if row.ok { "✅" } else { "❌" };
            output::line(format!(
                "{} {} image={} {}",
                mark, row.server, row.image, row.detail
            ));
        }
    }

    if rows.iter().any(|r| !r.ok) {
        anyhow::bail!("Registry doctor failed on one or more hosts");
    }

    Ok(())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
