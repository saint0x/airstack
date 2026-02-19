use crate::output;
use crate::secrets_store;
use airstack_config::AirstackConfig;
use anyhow::{Context, Result};
use clap::Subcommand;

#[derive(Debug, Clone, Subcommand)]
pub enum SecretsCommands {
    #[command(about = "Set an encrypted secret")]
    Set { key: String, value: String },
    #[command(about = "Get a secret value")]
    Get { key: String },
    #[command(about = "List secret keys")]
    List,
    #[command(about = "Delete a secret")]
    Delete { key: String },
}

pub async fn run(config_path: &str, command: SecretsCommands) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let project = &config.project.name;

    match command {
        SecretsCommands::Set { key, value } => {
            secrets_store::set(project, &key, &value)?;
            if output::is_json() {
                output::emit_json(&serde_json::json!({"ok": true, "action": "set", "key": key}))?;
            } else {
                output::line(format!("✅ secret set: {}", key));
            }
        }
        SecretsCommands::Get { key } => match secrets_store::get(project, &key)? {
            Some(value) => {
                if output::is_json() {
                    output::emit_json(&serde_json::json!({"key": key, "value": value}))?;
                } else {
                    output::line(value);
                }
            }
            None => anyhow::bail!("Secret '{}' not found", key),
        },
        SecretsCommands::List => {
            let keys = secrets_store::list(project)?;
            if output::is_json() {
                output::emit_json(&serde_json::json!({"keys": keys}))?;
            } else if keys.is_empty() {
                output::line("No secrets set.");
            } else {
                for key in keys {
                    output::line(format!("- {}", key));
                }
            }
        }
        SecretsCommands::Delete { key } => {
            let deleted = secrets_store::delete(project, &key)?;
            if !deleted {
                anyhow::bail!("Secret '{}' not found", key);
            }
            if output::is_json() {
                output::emit_json(
                    &serde_json::json!({"ok": true, "action": "delete", "key": key}),
                )?;
            } else {
                output::line(format!("✅ secret deleted: {}", key));
            }
        }
    }

    Ok(())
}
