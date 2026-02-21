use crate::commands::status;
use crate::output;
use crate::provider_profiles;
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Subcommand)]
pub enum ProviderCommands {
    #[command(about = "Manage provider profiles")]
    Profile {
        #[command(subcommand)]
        command: ProviderProfileCommands,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum ProviderProfileCommands {
    #[command(about = "List provider profiles")]
    List(ProviderProfileListArgs),
    #[command(about = "Show profile details")]
    Show(ProviderProfileShowArgs),
    #[command(about = "Create or update a profile")]
    Set(ProviderProfileSetArgs),
    #[command(about = "Activate a profile for a provider")]
    Use(ProviderProfileUseArgs),
    #[command(about = "Remove a profile")]
    Remove(ProviderProfileRemoveArgs),
    #[command(about = "Snapshot a local config directory into a profile")]
    Snapshot(ProviderProfileSnapshotArgs),
    #[command(about = "Run status across profiles for a provider")]
    Status(ProviderProfileStatusArgs),
}

#[derive(Debug, Clone, Args)]
pub struct ProviderProfileListArgs {
    #[arg(long, help = "Filter by provider name")]
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct ProviderProfileShowArgs {
    pub provider: String,
    pub name: String,
}

#[derive(Debug, Clone, Args)]
pub struct ProviderProfileSetArgs {
    pub provider: String,
    pub name: String,
    #[arg(
        long = "env",
        value_name = "KEY=VALUE",
        help = "Set environment key-value"
    )]
    pub env: Vec<String>,
    #[arg(
        long = "from-env",
        value_name = "KEY",
        help = "Import value from current environment"
    )]
    pub from_env: Vec<String>,
    #[arg(long, help = "Optional profile description")]
    pub description: Option<String>,
    #[arg(long, help = "Activate profile after set")]
    pub activate: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ProviderProfileUseArgs {
    pub provider: String,
    pub name: String,
}

#[derive(Debug, Clone, Args)]
pub struct ProviderProfileRemoveArgs {
    pub provider: String,
    pub name: String,
}

#[derive(Debug, Clone, Args)]
pub struct ProviderProfileSnapshotArgs {
    pub provider: String,
    pub name: String,
    #[arg(long, help = "Source config directory to snapshot (e.g. ~/.fly)")]
    pub source: String,
    #[arg(
        long,
        help = "Environment variable to set to snapshot path (e.g. FLY_CONFIG_DIR)"
    )]
    pub config_env: Option<String>,
    #[arg(
        long = "env",
        value_name = "KEY=VALUE",
        help = "Set environment key-value"
    )]
    pub env: Vec<String>,
    #[arg(
        long = "from-env",
        value_name = "KEY",
        help = "Import value from current environment"
    )]
    pub from_env: Vec<String>,
    #[arg(long, help = "Activate profile after snapshot")]
    pub activate: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ProviderProfileStatusArgs {
    pub provider: String,
    #[arg(long, help = "Show detailed status")]
    pub detailed: bool,
    #[arg(long, help = "Run active probes")]
    pub probe: bool,
    #[arg(long, default_value = "auto", help = "Status source mode")]
    pub source: String,
    #[arg(
        long = "profile",
        help = "Specific profile(s) to run (repeatable). Defaults to all profiles for provider."
    )]
    pub profiles: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ProfileRow {
    provider: String,
    name: String,
    active: bool,
    env_keys: Vec<String>,
}

pub async fn run(config_path: &str, command: ProviderCommands) -> Result<()> {
    match command {
        ProviderCommands::Profile { command } => run_profile(config_path, command).await,
    }
}

async fn run_profile(config_path: &str, command: ProviderProfileCommands) -> Result<()> {
    match command {
        ProviderProfileCommands::List(args) => list_profiles(args),
        ProviderProfileCommands::Show(args) => show_profile(args),
        ProviderProfileCommands::Set(args) => set_profile(args),
        ProviderProfileCommands::Use(args) => use_profile(args),
        ProviderProfileCommands::Remove(args) => remove_profile(args),
        ProviderProfileCommands::Snapshot(args) => snapshot_profile(args),
        ProviderProfileCommands::Status(args) => status_profiles(config_path, args).await,
    }
}

fn list_profiles(args: ProviderProfileListArgs) -> Result<()> {
    let store = provider_profiles::load_store()?;
    let mut rows = Vec::new();
    for (provider, profiles) in &store.providers {
        if args.provider.as_ref().is_some_and(|p| p != provider) {
            continue;
        }
        for (name, profile) in profiles {
            rows.push(ProfileRow {
                provider: provider.clone(),
                name: name.clone(),
                active: store.active.get(provider).is_some_and(|v| v == name),
                env_keys: profile.env.keys().cloned().collect(),
            });
        }
    }

    if output::is_json() {
        output::emit_json(&serde_json::json!({ "profiles": rows }))?;
        return Ok(());
    }

    output::line("🔌 Provider Profiles");
    if rows.is_empty() {
        output::line("(none)");
        return Ok(());
    }
    for row in rows {
        let mark = if row.active { "✅" } else { "  " };
        output::line(format!(
            "{} {}:{} env=[{}]",
            mark,
            row.provider,
            row.name,
            row.env_keys.join(",")
        ));
    }
    Ok(())
}

fn show_profile(args: ProviderProfileShowArgs) -> Result<()> {
    let profile = provider_profiles::get_profile(&args.provider, &args.name)?;
    if output::is_json() {
        output::emit_json(
            &serde_json::json!({ "provider": args.provider, "name": args.name, "profile": profile }),
        )?;
        return Ok(());
    }
    output::line(format!("Profile {}:{}", args.provider, args.name));
    if let Some(desc) = profile.description {
        output::line(format!("description: {}", desc));
    }
    for (k, v) in profile.env {
        output::line(format!("env {}={}", k, redact_if_secret(&k, &v)));
    }
    Ok(())
}

fn set_profile(args: ProviderProfileSetArgs) -> Result<()> {
    let env = build_env_map(&args.env, &args.from_env)?;
    provider_profiles::upsert_profile(
        &args.provider,
        &args.name,
        env,
        args.description,
        args.activate,
    )?;
    if !output::is_json() {
        output::line(format!("✅ profile saved: {}:{}", args.provider, args.name));
    }
    Ok(())
}

fn use_profile(args: ProviderProfileUseArgs) -> Result<()> {
    provider_profiles::set_active_profile(&args.provider, &args.name)?;
    if !output::is_json() {
        output::line(format!(
            "✅ active profile set: {}:{}",
            args.provider, args.name
        ));
    }
    Ok(())
}

fn remove_profile(args: ProviderProfileRemoveArgs) -> Result<()> {
    provider_profiles::remove_profile(&args.provider, &args.name)?;
    if !output::is_json() {
        output::line(format!(
            "✅ profile removed: {}:{}",
            args.provider, args.name
        ));
    }
    Ok(())
}

fn snapshot_profile(args: ProviderProfileSnapshotArgs) -> Result<()> {
    let source = expand_path(&args.source)?;
    let target = provider_profiles::store_snapshot_dir(&args.provider, &args.name)?;
    if target.exists() {
        std::fs::remove_dir_all(&target)
            .with_context(|| format!("Failed to clear snapshot path {}", target.display()))?;
    }
    provider_profiles::copy_dir_recursive(&source, &target)?;

    let mut env = build_env_map(&args.env, &args.from_env)?;
    if let Some(var) = args.config_env {
        env.insert(var, target.to_string_lossy().to_string());
    }
    provider_profiles::upsert_profile(&args.provider, &args.name, env, None, args.activate)?;

    if output::is_json() {
        output::emit_json(&serde_json::json!({
            "provider": args.provider,
            "name": args.name,
            "snapshot": target,
            "activate": args.activate
        }))?;
        return Ok(());
    }
    output::line(format!(
        "✅ snapshot profile saved: {}:{} -> {}",
        args.provider,
        args.name,
        target.display()
    ));
    Ok(())
}

async fn status_profiles(config_path: &str, args: ProviderProfileStatusArgs) -> Result<()> {
    if output::is_json() {
        anyhow::bail!("provider profile status does not support --json yet");
    }
    let mut names = if args.profiles.is_empty() {
        provider_profiles::list_provider_profiles(&args.provider)?
    } else {
        args.profiles.clone()
    };
    names.sort();
    if names.is_empty() {
        anyhow::bail!("No profiles found for provider '{}'", args.provider);
    }

    let mut failures = Vec::new();
    for name in names {
        let selector = format!("{}:{}", args.provider, name);
        let (provider, profile) = provider_profiles::parse_profile_selector(&selector)?;
        let store = provider_profiles::load_store()?;
        provider_profiles::apply_profile_env(&store, &provider, &profile)?;

        output::line("");
        output::line(format!("=== profile {} ===", selector));
        if let Err(e) =
            status::run(config_path, args.detailed, args.probe, false, &args.source).await
        {
            failures.push(format!("{} -> {}", selector, e));
        }
    }

    if !failures.is_empty() {
        anyhow::bail!(
            "one or more profile status checks failed: {}",
            failures.join(" | ")
        );
    }
    Ok(())
}

fn build_env_map(kv_pairs: &[String], from_env: &[String]) -> Result<BTreeMap<String, String>> {
    let mut env = BTreeMap::new();
    for pair in kv_pairs {
        let (key, value) = parse_env_pair(pair)?;
        env.insert(key, value);
    }
    for key in from_env {
        let value = std::env::var(key)
            .with_context(|| format!("Environment variable '{}' is not set", key))?;
        env.insert(key.clone(), value);
    }
    Ok(env)
}

fn parse_env_pair(raw: &str) -> Result<(String, String)> {
    let mut parts = raw.splitn(2, '=');
    let key = parts.next().unwrap_or_default().trim();
    let value = parts.next().unwrap_or_default().to_string();
    if key.is_empty() {
        anyhow::bail!("Invalid env pair '{}', expected KEY=VALUE", raw);
    }
    Ok((key.to_string(), value))
}

fn expand_path(raw: &str) -> Result<PathBuf> {
    if let Some(rest) = raw.strip_prefix("~/") {
        let home = dirs::home_dir().context("Could not resolve home directory")?;
        return Ok(home.join(rest));
    }
    Ok(PathBuf::from(raw))
}

fn redact_if_secret(key: &str, value: &str) -> String {
    let lower = key.to_ascii_lowercase();
    if lower.contains("token") || lower.contains("secret") || lower.contains("key") {
        "****".to_string()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::parse_env_pair;

    #[test]
    fn parse_env_pair_works() {
        let (k, v) = parse_env_pair("A=B").expect("pair should parse");
        assert_eq!(k, "A");
        assert_eq!(v, "B");
    }
}
