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
    reason: String,
    detail: String,
    remediation: Vec<String>,
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
                reason: "provider_managed".to_string(),
                detail: "provider=fly uses fly-managed image pull path".to_string(),
                remediation: Vec::new(),
            });
            continue;
        }

        let cmd = format!("docker pull {}", shell_quote(&args.image));
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
                reason: "ok".to_string(),
                detail: "pull succeeded".to_string(),
                remediation: Vec::new(),
            });
            continue;
        }

        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let detail = if stderr.is_empty() { stdout } else { stderr };
        let (reason, remediation) = classify_pull_failure(&args.image, &detail);
        rows.push(RegistryDoctorRecord {
            server: server.name.clone(),
            image: args.image.clone(),
            ok: false,
            reason,
            detail,
            remediation,
        });
    }

    if output::is_json() {
        output::emit_json(&serde_json::json!({ "results": rows }))?;
    } else {
        output::line("🔐 Registry Doctor");
        for row in &rows {
            let mark = if row.ok { "✅" } else { "❌" };
            output::line(format!(
                "{} {} image={} reason={} {}",
                mark, row.server, row.image, row.reason, row.detail
            ));
            for hint in &row.remediation {
                output::line(format!("   - {}", hint));
            }
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

fn classify_pull_failure(image: &str, stderr: &str) -> (String, Vec<String>) {
    let msg = stderr.to_ascii_lowercase();

    if msg.contains("unauthorized")
        || msg.contains("authentication required")
        || msg.contains("access denied")
        || msg.contains(": denied")
        || msg.contains("requested access to the resource is denied")
    {
        let mut remediation = vec![
            "Authenticate on target host: `echo \"$GHCR_TOKEN\" | docker login ghcr.io -u <github-username> --password-stdin`".to_string(),
        ];
        if image.starts_with("ghcr.io/") {
            remediation.push(
                "Token scopes required: `read:packages` (and `repo` if pulling private packages tied to private repos)".to_string(),
            );
        }
        return ("auth_denied".to_string(), remediation);
    }

    if msg.contains("manifest unknown")
        || msg.contains("not found")
        || msg.contains("tag")
        || msg.contains("no such image")
    {
        return (
            "tag_missing".to_string(),
            vec![
                "Verify image tag exists in registry and was pushed successfully".to_string(),
                format!("Expected image: {}", image),
            ],
        );
    }

    if msg.contains("i/o timeout")
        || msg.contains("timeout")
        || msg.contains("no such host")
        || msg.contains("temporary failure")
        || msg.contains("connection refused")
        || msg.contains("tls handshake timeout")
    {
        return (
            "network".to_string(),
            vec![
                "Check outbound network egress/DNS from target host to registry".to_string(),
                "Retry pull after connectivity stabilizes".to_string(),
            ],
        );
    }

    (
        "unknown".to_string(),
        vec!["Inspect raw docker pull stderr for exact root cause".to_string()],
    )
}
