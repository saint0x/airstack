use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirstackConfig {
    pub project: ProjectConfig,
    pub infra: Option<InfraConfig>,
    pub services: Option<HashMap<String, ServiceConfig>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfraConfig {
    pub servers: Vec<ServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub name: String,
    pub provider: String,
    pub region: String,
    pub server_type: String,
    pub ssh_key: String,
    pub floating_ip: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    pub image: String,
    pub ports: Vec<u16>,
    pub env: Option<HashMap<String, String>>,
    pub volumes: Option<Vec<String>>,
    pub depends_on: Option<Vec<String>>,
}

impl AirstackConfig {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file: {:?}", path.as_ref()))?;

        let config: AirstackConfig =
            toml::from_str(&content).with_context(|| "Failed to parse TOML configuration")?;

        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.project.name.is_empty() {
            anyhow::bail!("Project name cannot be empty");
        }

        if let Some(infra) = &self.infra {
            for server in &infra.servers {
                if server.name.is_empty() {
                    anyhow::bail!("Server name cannot be empty");
                }
                if server.provider.is_empty() {
                    anyhow::bail!("Server provider cannot be empty");
                }
            }
        }

        if let Some(services) = &self.services {
            for (name, service) in services {
                if name.is_empty() {
                    anyhow::bail!("Service name cannot be empty");
                }
                if service.image.is_empty() {
                    anyhow::bail!("Service image cannot be empty for service: {}", name);
                }
            }
        }

        Ok(())
    }

    pub fn get_config_path() -> Result<std::path::PathBuf> {
        let current_dir = std::env::current_dir().context("Failed to get current directory")?;

        let config_path = current_dir.join("airstack.toml");
        if config_path.exists() {
            return Ok(config_path);
        }

        anyhow::bail!("No airstack.toml found in current directory");
    }

    pub fn init_example<P: AsRef<Path>>(path: P) -> Result<()> {
        let example_config = r#"[project]
name = "my-project"
description = "Example Airstack project"

[[infra.servers]]
name = "web-server"
provider = "hetzner"
region = "nbg1"
server_type = "cx21"
ssh_key = "~/.ssh/id_ed25519.pub"
floating_ip = true

[services.api]
image = "nginx:latest"
ports = [80, 443]
env = { ENVIRONMENT = "production" }

[services.database]
image = "postgres:15"
ports = [5432]
env = { POSTGRES_DB = "myapp", POSTGRES_USER = "user", POSTGRES_PASSWORD = "password" }
volumes = ["./data:/var/lib/postgresql/data"]
"#;

        std::fs::write(&path, example_config)
            .with_context(|| format!("Failed to write config file: {:?}", path.as_ref()))?;

        Ok(())
    }
}
