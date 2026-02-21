use airstack_config::AirstackConfig;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LocalState {
    pub project: String,
    pub updated_at_unix: u64,
    pub servers: BTreeMap<String, ServerState>,
    pub services: BTreeMap<String, ServiceState>,
    #[serde(default)]
    pub script_runs: BTreeMap<String, ScriptRunState>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum HealthState {
    Healthy,
    Degraded,
    Unhealthy,
    #[default]
    Unknown,
}

impl HealthState {
    pub fn as_str(self) -> &'static str {
        match self {
            HealthState::Healthy => "healthy",
            HealthState::Degraded => "degraded",
            HealthState::Unhealthy => "unhealthy",
            HealthState::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerState {
    pub provider: String,
    pub id: Option<String>,
    pub public_ip: Option<String>,
    #[serde(default)]
    pub health: HealthState,
    #[serde(default)]
    pub last_status: Option<String>,
    #[serde(default)]
    pub last_checked_unix: u64,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceState {
    pub image: String,
    pub replicas: usize,
    pub containers: Vec<String>,
    #[serde(default)]
    pub health: HealthState,
    #[serde(default)]
    pub last_status: Option<String>,
    #[serde(default)]
    pub last_checked_unix: u64,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub last_deploy_command: Option<String>,
    #[serde(default)]
    pub last_deploy_unix: Option<u64>,
    #[serde(default)]
    pub image_origin: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DriftReport {
    pub missing_servers_in_cache: Vec<String>,
    pub extra_servers_in_cache: Vec<String>,
    pub missing_services_in_cache: Vec<String>,
    pub extra_services_in_cache: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScriptRunState {
    pub last_hash: Option<String>,
    pub last_run_unix: u64,
}

impl LocalState {
    pub fn load(project_name: &str) -> Result<Self> {
        let path = state_file_path(project_name)?;
        if !path.exists() {
            return Ok(LocalState {
                project: project_name.to_string(),
                updated_at_unix: now_unix(),
                ..Default::default()
            });
        }

        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read local state file: {}", path.display()))?;
        let mut state: LocalState =
            serde_json::from_str(&content).context("Failed to parse local state JSON")?;
        if state.project.is_empty() {
            state.project = project_name.to_string();
        }
        Ok(state)
    }

    pub fn save(&mut self) -> Result<()> {
        self.updated_at_unix = now_unix();
        let path = state_file_path(&self.project)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create local state directory: {}",
                    parent.display()
                )
            })?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("Failed to write local state file: {}", path.display()))?;
        Ok(())
    }

    pub fn detect_drift(&self, config: &AirstackConfig) -> DriftReport {
        let desired_servers = config
            .infra
            .as_ref()
            .map(|i| {
                i.servers
                    .iter()
                    .map(|s| s.name.clone())
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let cached_servers = self.servers.keys().cloned().collect::<BTreeSet<_>>();

        let desired_services = config
            .services
            .as_ref()
            .map(|s| s.keys().cloned().collect::<BTreeSet<_>>())
            .unwrap_or_default();
        let cached_services = self.services.keys().cloned().collect::<BTreeSet<_>>();

        DriftReport {
            missing_servers_in_cache: desired_servers
                .difference(&cached_servers)
                .cloned()
                .collect(),
            extra_servers_in_cache: cached_servers
                .difference(&desired_servers)
                .cloned()
                .collect(),
            missing_services_in_cache: desired_services
                .difference(&cached_services)
                .cloned()
                .collect(),
            extra_services_in_cache: cached_services
                .difference(&desired_services)
                .cloned()
                .collect(),
        }
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn state_file_path(project_name: &str) -> Result<PathBuf> {
    let base = dirs::home_dir()
        .context("Could not resolve home directory for local state")?
        .join(".airstack")
        .join("state");
    let project_key = sanitize_project_key(project_name);
    Ok(base.join(format!("{}.json", project_key)))
}

fn sanitize_project_key(project_name: &str) -> String {
    let sanitized = project_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "default".to_string()
    } else {
        sanitized
    }
}
