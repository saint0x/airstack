use crate::output;
use crate::provider_profiles;
use airstack_config::AirstackConfig;
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, Subcommand)]
pub enum ProviderCommands {
    #[command(about = "Manage provider profiles")]
    Profile {
        #[command(subcommand)]
        command: ProviderProfileCommands,
    },
    #[command(about = "List provider resources by profile")]
    Inventory(ProviderInventoryArgs),
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
        help = "Environment variable to set to snapshot path (e.g. FLYCTL_CONFIG_DIR)"
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
    #[arg(
        long,
        default_value = "config",
        help = "Scope for status: config|inventory"
    )]
    pub scope: String,
    #[arg(
        long,
        help = "Emit one JSON object per profile line for stable text pipelines"
    )]
    pub ndjson: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ProviderInventoryArgs {
    pub provider: String,
    #[arg(
        long = "profile",
        help = "Specific profile(s) to run (repeatable). Defaults to active profile."
    )]
    pub profiles: Vec<String>,
    #[arg(long, help = "Run inventory for all profiles")]
    pub all_profiles: bool,
}

#[derive(Debug, Serialize)]
struct ProfileRow {
    provider: String,
    name: String,
    active: bool,
    env_keys: Vec<String>,
    auth_ok: bool,
    account: Option<String>,
    organization: Option<String>,
    note: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProfileStatusRow {
    provider: String,
    profile: String,
    scope: String,
    source_mode: String,
    auth_ok: bool,
    account: Option<String>,
    organization: Option<String>,
    config_scope_warning: Option<String>,
    ok: bool,
    status: Option<serde_json::Value>,
    inventory: Option<FlyInventoryProfile>,
    error: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
struct FlyInventoryProfile {
    profile: String,
    identity: provider_profiles::ProfileIdentity,
    apps: Vec<FlyAppInventory>,
    error: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
struct FlyAppInventory {
    name: String,
    organization: Option<String>,
    status: Option<String>,
    platform_version: Option<String>,
    machines: Vec<serde_json::Value>,
}

pub async fn run(config_path: &str, command: ProviderCommands) -> Result<()> {
    match command {
        ProviderCommands::Profile { command } => run_profile(config_path, command).await,
        ProviderCommands::Inventory(args) => inventory(config_path, args).await,
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
            let identity = provider_profiles::resolve_profile_identity(&store, provider, name);
            rows.push(ProfileRow {
                provider: provider.clone(),
                name: name.clone(),
                active: store.active.get(provider).is_some_and(|v| v == name),
                env_keys: profile.env.keys().cloned().collect(),
                auth_ok: identity.auth_ok,
                account: identity.account,
                organization: identity.organization,
                note: identity.note,
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
            "{} {}:{} env=[{}] auth={} account={} org={}",
            mark,
            row.provider,
            row.name,
            row.env_keys.join(","),
            row.auth_ok,
            row.account.unwrap_or_else(|| "unknown".to_string()),
            row.organization.unwrap_or_else(|| "unknown".to_string())
        ));
        if let Some(note) = row.note {
            output::line(format!("   note: {}", note));
        }
    }
    Ok(())
}

fn show_profile(args: ProviderProfileShowArgs) -> Result<()> {
    let profile = provider_profiles::get_profile(&args.provider, &args.name)?;
    let store = provider_profiles::load_store()?;
    let identity = provider_profiles::resolve_profile_identity(&store, &args.provider, &args.name);
    if output::is_json() {
        output::emit_json(
            &serde_json::json!({ "provider": args.provider, "name": args.name, "profile": profile, "identity": identity }),
        )?;
        return Ok(());
    }
    output::line(format!("Profile {}:{}", args.provider, args.name));
    if let Some(desc) = profile.description {
        output::line(format!("description: {}", desc));
    }
    output::line(format!(
        "identity: auth_ok={} account={} org={}",
        identity.auth_ok,
        identity.account.unwrap_or_else(|| "unknown".to_string()),
        identity
            .organization
            .unwrap_or_else(|| "unknown".to_string())
    ));
    if let Some(note) = identity.note {
        output::line(format!("identity note: {}", note));
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
    if args.provider == "fly" {
        env.remove("FLY_CONFIG_DIR");
        let target_path = target.to_string_lossy().to_string();
        env.insert("FLYCTL_CONFIG_DIR".to_string(), target_path);
    } else if let Some(var) = args.config_env {
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
    let scope = parse_scope(&args.scope)?;
    let names = resolve_profile_names(&args.provider, &args.profiles, false)?;
    if names.is_empty() {
        anyhow::bail!("No profiles found for provider '{}'", args.provider);
    }

    let config_scope_warning = config_scope_warning(config_path, &args.provider)
        .ok()
        .flatten();
    let mut rows = Vec::new();

    for name in names {
        let selector = format!("{}:{}", args.provider, name);
        let store = provider_profiles::load_store()?;
        let identity = provider_profiles::resolve_profile_identity(&store, &args.provider, &name);

        let mut row = ProfileStatusRow {
            provider: args.provider.clone(),
            profile: name.clone(),
            scope: scope.to_string(),
            source_mode: args.source.clone(),
            auth_ok: identity.auth_ok,
            account: identity.account,
            organization: identity.organization,
            config_scope_warning: config_scope_warning.clone(),
            ok: false,
            status: None,
            inventory: None,
            error: None,
        };

        match scope {
            ScopeMode::Config => match run_status_subprocess(config_path, &selector, &args) {
                Ok(status_json) => {
                    row.ok = true;
                    row.status = Some(status_json);
                }
                Err(err) => row.error = Some(err.to_string()),
            },
            ScopeMode::Inventory => match collect_provider_inventory_profile(&args.provider, &name)
            {
                Ok(inv) => {
                    row.ok = true;
                    row.inventory = Some(inv);
                }
                Err(err) => row.error = Some(err.to_string()),
            },
        }
        rows.push(row);
    }

    if output::is_json() {
        output::emit_json(&serde_json::json!({
            "provider": args.provider,
            "scope": scope,
            "results": rows
        }))?;
    } else if args.ndjson {
        for row in &rows {
            println!("{}", serde_json::to_string(row)?);
        }
    } else {
        output::line(format!("Provider profile status (scope={scope})"));
        if let Some(warning) = &config_scope_warning {
            output::line(format!("warning: {}", warning));
        }
        for row in &rows {
            output::line(format!(
                "- {}:{} ok={} auth={} account={} org={}",
                row.provider,
                row.profile,
                row.ok,
                row.auth_ok,
                row.account.as_deref().unwrap_or("unknown"),
                row.organization.as_deref().unwrap_or("unknown")
            ));
            if let Some(err) = &row.error {
                output::line(format!("    error: {}", err));
            }
            if let Some(inv) = &row.inventory {
                output::line(format!("    apps={}", inv.apps.len()));
            }
            if let Some(status) = &row.status {
                let infra = status
                    .get("infrastructure")
                    .and_then(|v| v.as_array())
                    .map(|v| v.len())
                    .unwrap_or(0);
                let services = status
                    .get("services")
                    .and_then(|v| v.as_array())
                    .map(|v| v.len())
                    .unwrap_or(0);
                output::line(format!("    status infra={} services={}", infra, services));
            }
        }
    }

    if rows.iter().any(|r| !r.ok) {
        anyhow::bail!("one or more profile status checks failed")
    }
    Ok(())
}

async fn inventory(_config_path: &str, args: ProviderInventoryArgs) -> Result<()> {
    let names = resolve_profile_names(&args.provider, &args.profiles, args.all_profiles)?;
    if names.is_empty() {
        anyhow::bail!("No profiles found for provider '{}'", args.provider);
    }

    let mut results = Vec::new();
    let mut failures = Vec::new();
    for name in names {
        match collect_provider_inventory_profile(&args.provider, &name) {
            Ok(inv) => results.push(inv),
            Err(err) => failures.push(format!("{}:{} -> {}", args.provider, name, err)),
        }
    }

    if output::is_json() {
        output::emit_json(&serde_json::json!({
            "provider": args.provider,
            "scope": "inventory",
            "profiles": results,
            "errors": failures,
        }))?;
    } else {
        output::line(format!("Provider inventory for {}", args.provider));
        for inv in &results {
            output::line(format!(
                "- {} auth={} account={} org={} apps={}",
                inv.profile,
                inv.identity.auth_ok,
                inv.identity.account.as_deref().unwrap_or("unknown"),
                inv.identity.organization.as_deref().unwrap_or("unknown"),
                inv.apps.len()
            ));
            for app in &inv.apps {
                output::line(format!(
                    "    app={} org={} status={} machines={}",
                    app.name,
                    app.organization.as_deref().unwrap_or("unknown"),
                    app.status.as_deref().unwrap_or("unknown"),
                    app.machines.len()
                ));
            }
            if let Some(err) = &inv.error {
                output::line(format!("    error: {}", err));
            }
        }
        for err in &failures {
            output::line(format!("error: {}", err));
        }
    }

    if !failures.is_empty() {
        anyhow::bail!("one or more inventory checks failed")
    }
    Ok(())
}

fn resolve_profile_names(
    provider: &str,
    explicit: &[String],
    all_profiles: bool,
) -> Result<Vec<String>> {
    let mut names = if !explicit.is_empty() {
        explicit.to_vec()
    } else if all_profiles {
        provider_profiles::list_provider_profiles(provider)?
    } else if let Some(active) = provider_profiles::active_profile(provider)? {
        vec![active]
    } else {
        provider_profiles::list_provider_profiles(provider)?
    };
    names.sort();
    names.dedup();
    Ok(names)
}

fn run_status_subprocess(
    config_path: &str,
    selector: &str,
    args: &ProviderProfileStatusArgs,
) -> Result<serde_json::Value> {
    let exe = std::env::current_exe().context("Failed to locate current executable")?;
    let mut cmd = Command::new(exe);
    cmd.args([
        "--json",
        "--config",
        config_path,
        "--provider-profile",
        selector,
        "status",
        "--source",
        &args.source,
    ]);
    if args.detailed {
        cmd.arg("--detailed");
    }
    if args.probe {
        cmd.arg("--probe");
    }

    let out = cmd
        .output()
        .context("Failed to execute status subprocess")?;
    if !out.status.success() {
        anyhow::bail!(
            "status command failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let raw = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(raw.trim()).context("Failed parsing status JSON output")?;
    Ok(parsed)
}

fn collect_provider_inventory_profile(
    provider: &str,
    profile: &str,
) -> Result<FlyInventoryProfile> {
    let store = provider_profiles::load_store()?;
    let identity = provider_profiles::resolve_profile_identity(&store, provider, profile);
    provider_profiles::apply_profile_env(&store, provider, profile)?;

    if provider != "fly" {
        return Ok(FlyInventoryProfile {
            profile: profile.to_string(),
            identity,
            apps: Vec::new(),
            error: Some("inventory not implemented for provider".to_string()),
        });
    }

    let apps_out = Command::new("sh")
        .arg("-lc")
        .arg("flyctl apps list --json")
        .output()
        .context("Failed to execute flyctl apps list")?;

    if !apps_out.status.success() {
        return Ok(FlyInventoryProfile {
            profile: profile.to_string(),
            identity,
            apps: Vec::new(),
            error: Some(String::from_utf8_lossy(&apps_out.stderr).trim().to_string()),
        });
    }

    let apps_json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&apps_out.stdout).trim())
            .context("Failed to parse flyctl apps list JSON")?;
    let app_rows = apps_json.as_array().cloned().unwrap_or_default();
    let mut apps = Vec::new();

    for row in app_rows {
        let name = row
            .get("Name")
            .or_else(|| row.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let organization = row
            .get("Organization")
            .or_else(|| row.get("organization"))
            .and_then(|v| v.as_str())
            .map(|v| v.to_string());
        let status = row
            .get("Status")
            .or_else(|| row.get("status"))
            .and_then(|v| v.as_str())
            .map(|v| v.to_string());
        let platform_version = row
            .get("PlatformVersion")
            .or_else(|| row.get("platformVersion"))
            .and_then(|v| v.as_str())
            .map(|v| v.to_string());

        let machine_cmd = format!("flyctl machine list -a {} --json", shell_quote(&name));
        let machines_out = Command::new("sh").arg("-lc").arg(machine_cmd).output();
        let machines = match machines_out {
            Ok(out) if out.status.success() => serde_json::from_str::<serde_json::Value>(
                String::from_utf8_lossy(&out.stdout).trim(),
            )
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default(),
            _ => Vec::new(),
        };

        apps.push(FlyAppInventory {
            name,
            organization,
            status,
            platform_version,
            machines,
        });
    }

    Ok(FlyInventoryProfile {
        profile: profile.to_string(),
        identity,
        apps,
        error: None,
    })
}

fn config_scope_warning(config_path: &str, provider: &str) -> Result<Option<String>> {
    let config = AirstackConfig::load(config_path)?;
    let has_targets = config
        .infra
        .as_ref()
        .map(|infra| infra.servers.iter().any(|s| s.provider == provider))
        .unwrap_or(false);
    if has_targets {
        return Ok(None);
    }
    Ok(Some(format!(
        "Current config has no '{}' provider targets. Config-scoped status may not reflect provider inventory.",
        provider
    )))
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
enum ScopeMode {
    Config,
    Inventory,
}

impl std::fmt::Display for ScopeMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config => write!(f, "config"),
            Self::Inventory => write!(f, "inventory"),
        }
    }
}

fn parse_scope(raw: &str) -> Result<ScopeMode> {
    match raw {
        "config" => Ok(ScopeMode::Config),
        "inventory" => Ok(ScopeMode::Inventory),
        _ => anyhow::bail!("Invalid --scope '{}'. Expected config|inventory", raw),
    }
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

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || "-_./:".contains(ch))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::{parse_env_pair, parse_scope, ScopeMode};

    #[test]
    fn parse_env_pair_works() {
        let (k, v) = parse_env_pair("A=B").expect("pair should parse");
        assert_eq!(k, "A");
        assert_eq!(v, "B");
    }

    #[test]
    fn parse_scope_works() {
        assert!(matches!(
            parse_scope("config").expect("scope"),
            ScopeMode::Config
        ));
        assert!(matches!(
            parse_scope("inventory").expect("scope"),
            ScopeMode::Inventory
        ));
        assert!(parse_scope("bad").is_err());
    }
}
