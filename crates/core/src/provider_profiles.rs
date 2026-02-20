use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderProfileStore {
    #[serde(default)]
    pub active: BTreeMap<String, String>,
    #[serde(default)]
    pub providers: BTreeMap<String, BTreeMap<String, ProviderProfile>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderProfile {
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub description: Option<String>,
    pub updated_at_unix: u64,
}

pub fn apply_profiles_for_run(explicit: Option<&str>) -> Result<()> {
    let store = load_store()?;
    for (provider, profile) in &store.active {
        apply_profile_env(&store, provider, profile)?;
    }
    if let Some(selector) = explicit {
        let (provider, profile) = parse_profile_selector(selector)?;
        apply_profile_env(&store, provider.as_str(), profile.as_str())?;
    }
    Ok(())
}

pub fn parse_profile_selector(input: &str) -> Result<(String, String)> {
    let mut parts = input.splitn(2, ':');
    let provider = parts.next().unwrap_or_default().trim();
    let profile = parts.next().unwrap_or_default().trim();
    if provider.is_empty() || profile.is_empty() {
        anyhow::bail!(
            "Invalid provider profile selector '{}'. Expected format: <provider>:<profile>",
            input
        );
    }
    Ok((provider.to_string(), profile.to_string()))
}

pub fn load_store() -> Result<ProviderProfileStore> {
    let path = store_file()?;
    if !path.exists() {
        return Ok(ProviderProfileStore::default());
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read provider profile store: {}", path.display()))?;
    let store: ProviderProfileStore =
        serde_json::from_str(&raw).context("Failed to parse provider profile store JSON")?;
    Ok(store)
}

pub fn save_store(store: &ProviderProfileStore) -> Result<()> {
    let path = store_file()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create provider profile directory: {}",
                parent.display()
            )
        })?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(store)?)
        .with_context(|| format!("Failed to write provider profile store: {}", path.display()))?;
    Ok(())
}

pub fn upsert_profile(
    provider: &str,
    name: &str,
    env: BTreeMap<String, String>,
    description: Option<String>,
    activate: bool,
) -> Result<()> {
    let mut store = load_store()?;
    let entry = store.providers.entry(provider.to_string()).or_default();
    entry.insert(
        name.to_string(),
        ProviderProfile {
            env,
            description,
            updated_at_unix: unix_now(),
        },
    );
    if activate {
        store.active.insert(provider.to_string(), name.to_string());
    }
    save_store(&store)
}

pub fn remove_profile(provider: &str, name: &str) -> Result<()> {
    let mut store = load_store()?;
    if let Some(profiles) = store.providers.get_mut(provider) {
        profiles.remove(name);
        if profiles.is_empty() {
            store.providers.remove(provider);
        }
    }
    if store
        .active
        .get(provider)
        .is_some_and(|active| active == name)
    {
        store.active.remove(provider);
    }
    save_store(&store)
}

pub fn set_active_profile(provider: &str, name: &str) -> Result<()> {
    let mut store = load_store()?;
    let Some(profiles) = store.providers.get(provider) else {
        anyhow::bail!("Provider '{}' has no profiles", provider);
    };
    if !profiles.contains_key(name) {
        anyhow::bail!("Profile '{}' not found for provider '{}'", name, provider);
    }
    store.active.insert(provider.to_string(), name.to_string());
    save_store(&store)
}

pub fn get_profile(provider: &str, name: &str) -> Result<ProviderProfile> {
    let store = load_store()?;
    let profile = store
        .providers
        .get(provider)
        .and_then(|v| v.get(name))
        .cloned()
        .with_context(|| format!("Profile '{}' not found for provider '{}'", name, provider))?;
    Ok(profile)
}

pub fn list_provider_profiles(provider: &str) -> Result<Vec<String>> {
    let store = load_store()?;
    let names = store
        .providers
        .get(provider)
        .map(|m| m.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    Ok(names)
}

pub fn store_snapshot_dir(provider: &str, name: &str) -> Result<PathBuf> {
    let root = store_root()?;
    Ok(root
        .join("provider-profiles")
        .join(provider)
        .join(name)
        .join("config"))
}

pub fn apply_profile_env(store: &ProviderProfileStore, provider: &str, name: &str) -> Result<()> {
    let profile = store
        .providers
        .get(provider)
        .and_then(|m| m.get(name))
        .with_context(|| format!("Profile '{}' not found for provider '{}'", name, provider))?;
    for (key, value) in &profile.env {
        std::env::set_var(key, value);
    }
    Ok(())
}

pub fn copy_dir_recursive(source: &Path, dest: &Path) -> Result<()> {
    if !source.exists() {
        anyhow::bail!("Source path does not exist: {}", source.display());
    }
    if source.is_file() {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(source, dest)?;
        return Ok(());
    }
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn store_root() -> Result<PathBuf> {
    if let Ok(home) = std::env::var("AIRSTACK_HOME") {
        if !home.trim().is_empty() {
            return Ok(PathBuf::from(home));
        }
    }
    let home =
        dirs::home_dir().context("Could not resolve home directory for provider profiles")?;
    Ok(home.join(".airstack"))
}

fn store_file() -> Result<PathBuf> {
    Ok(store_root()?.join("provider_profiles.json"))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::parse_profile_selector;

    #[test]
    fn parse_selector_requires_provider_and_profile() {
        let ok = parse_profile_selector("fly:work").expect("selector should parse");
        assert_eq!(ok.0, "fly");
        assert_eq!(ok.1, "work");
        assert!(parse_profile_selector("fly").is_err());
    }
}
