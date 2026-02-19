use crate::output;
use crate::ssh_utils::execute_remote_command;
use airstack_config::AirstackConfig;
use anyhow::{Context, Result};
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Debug, Clone, Subcommand)]
pub enum BackupCommands {
    #[command(about = "Enable managed backups")]
    Enable {
        #[arg(long)]
        server: Option<String>,
        #[arg(long, default_value = "/var/backups/airstack")]
        remote_dir: String,
    },
    #[command(about = "Show backup status")]
    Status,
    #[command(about = "Restore from backup archive")]
    Restore {
        #[arg(long)]
        archive: String,
        #[arg(long)]
        destination: String,
    },
}

pub async fn run(config_path: &str, command: BackupCommands) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;

    match command {
        BackupCommands::Enable { server, remote_dir } => {
            let selected = select_server(&config, server)?;
            let cmd = vec![
                "sh".to_string(),
                "-lc".to_string(),
                format!("mkdir -p {}", shell_quote(&remote_dir)),
            ];
            let out = execute_remote_command(selected, &cmd).await?;
            if !out.status.success() {
                anyhow::bail!(
                    "Failed to initialize remote backup directory: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
            save_backup_profile(&config.project.name, &selected.name, &remote_dir)?;
            output::line(format!(
                "✅ backups enabled on {}:{}",
                selected.name, remote_dir
            ));
        }
        BackupCommands::Status => {
            let profile = load_backup_profile(&config.project.name)?
                .context("Backups are not enabled. Run 'airstack backup enable' first.")?;
            let server = config
                .infra
                .as_ref()
                .and_then(|i| i.servers.iter().find(|s| s.name == profile.server))
                .context("Backup profile server not found in current config")?;

            let cmd = vec![
                "sh".to_string(),
                "-lc".to_string(),
                format!(
                    "ls -1 {}/*.tar.gz 2>/dev/null | tail -n 20",
                    shell_quote(&profile.remote_dir)
                ),
            ];
            let out = execute_remote_command(server, &cmd).await?;
            let list = String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string())
                .collect::<Vec<_>>();

            if output::is_json() {
                output::emit_json(&serde_json::json!({
                    "enabled": true,
                    "server": profile.server,
                    "remote_dir": profile.remote_dir,
                    "archives": list,
                }))?;
            } else {
                output::line(format!("Backup server: {}", profile.server));
                output::line(format!("Remote dir: {}", profile.remote_dir));
                if list.is_empty() {
                    output::line("No archives found.");
                } else {
                    for item in list {
                        output::line(format!("- {}", item));
                    }
                }
            }
        }
        BackupCommands::Restore {
            archive,
            destination,
        } => {
            let profile = load_backup_profile(&config.project.name)?
                .context("Backups are not enabled. Run 'airstack backup enable' first.")?;
            let server = config
                .infra
                .as_ref()
                .and_then(|i| i.servers.iter().find(|s| s.name == profile.server))
                .context("Backup profile server not found in current config")?;

            let cmd = vec![
                "sh".to_string(),
                "-lc".to_string(),
                format!(
                    "mkdir -p {dest} && tar -xzf {archive} -C {dest}",
                    dest = shell_quote(&destination),
                    archive = shell_quote(&archive)
                ),
            ];
            let out = execute_remote_command(server, &cmd).await?;
            if !out.status.success() {
                anyhow::bail!(
                    "Restore failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }

            output::line(format!(
                "✅ restore completed on {} from {} -> {}",
                server.name, archive, destination
            ));
        }
    }

    Ok(())
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct BackupProfile {
    server: String,
    remote_dir: String,
}

fn profile_path(project: &str) -> Result<PathBuf> {
    let home = dirs::home_dir().context("Failed to resolve home directory")?;
    let dir = home.join(".airstack").join("backup");
    std::fs::create_dir_all(&dir).context("Failed to create backup profile directory")?;
    Ok(dir.join(format!("{}.toml", project)))
}

fn save_backup_profile(project: &str, server: &str, remote_dir: &str) -> Result<()> {
    let path = profile_path(project)?;
    let profile = BackupProfile {
        server: server.to_string(),
        remote_dir: remote_dir.to_string(),
    };
    std::fs::write(path, toml::to_string_pretty(&profile)?)
        .context("Failed to save backup profile")?;
    Ok(())
}

fn load_backup_profile(project: &str) -> Result<Option<BackupProfile>> {
    let path = profile_path(project)?;
    if !path.exists() {
        return Ok(None);
    }
    let profile: BackupProfile = toml::from_str(&std::fs::read_to_string(path)?)
        .context("Failed to parse backup profile")?;
    Ok(Some(profile))
}

fn select_server<'a>(
    config: &'a AirstackConfig,
    requested: Option<String>,
) -> Result<&'a airstack_config::ServerConfig> {
    let infra = config
        .infra
        .as_ref()
        .context("No infra.servers configured")?;

    if let Some(name) = requested {
        return infra
            .servers
            .iter()
            .find(|s| s.name == name)
            .with_context(|| format!("Server '{}' not found", name));
    }

    infra.servers.first().context("No infra.servers configured")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
