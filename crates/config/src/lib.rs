use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirstackConfig {
    pub project: ProjectConfig,
    pub infra: Option<InfraConfig>,
    pub services: Option<HashMap<String, ServiceConfig>>,
    pub edge: Option<EdgeConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub name: String,
    pub description: Option<String>,
    pub deploy_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfraConfig {
    pub servers: Vec<ServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub name: String,
    pub provider: String,
    #[serde(default)]
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
    pub target_server: Option<String>,
    pub healthcheck: Option<HealthcheckConfig>,
    pub profile: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthcheckConfig {
    #[serde(default)]
    pub command: Vec<String>,
    pub interval_secs: Option<u64>,
    pub retries: Option<u32>,
    pub timeout_secs: Option<u64>,
    pub http: Option<HttpHealthcheckConfig>,
    pub tcp: Option<TcpHealthcheckConfig>,
    pub any: Option<Vec<HealthcheckConfig>>,
    pub all: Option<Vec<HealthcheckConfig>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpHealthcheckConfig {
    pub url: Option<String>,
    pub path: Option<String>,
    pub port: Option<u16>,
    pub expected_status: Option<u16>,
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TcpHealthcheckConfig {
    pub host: Option<String>,
    pub port: u16,
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeConfig {
    pub provider: String,
    pub sites: Vec<EdgeSiteConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeSiteConfig {
    pub host: String,
    pub upstream_service: String,
    pub upstream_port: u16,
    pub tls_email: Option<String>,
    pub redirect_http: Option<bool>,
}

impl AirstackConfig {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file: {:?}", path.as_ref()))?;

        let mut config: AirstackConfig = match toml::from_str(&content) {
            Ok(v) => v,
            Err(err) => {
                anyhow::bail!("Failed to parse TOML configuration: {}", err);
            }
        };

        if let Ok(env_name) = std::env::var("AIRSTACK_ENV") {
            if !env_name.is_empty() {
                let base = path.as_ref();
                let parent = base.parent().unwrap_or_else(|| Path::new("."));
                let stem = base
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("airstack");
                let overlay_path = parent.join(format!("{}.{}.toml", stem, env_name));
                if overlay_path.exists() {
                    let overlay_content =
                        std::fs::read_to_string(&overlay_path).with_context(|| {
                            format!("Failed to read overlay config file: {:?}", overlay_path)
                        })?;
                    let overlay: OverlayConfig = toml::from_str(&overlay_content)
                        .with_context(|| "Failed to parse overlay TOML configuration")?;
                    config.apply_overlay(overlay);
                }
            }
        }

        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.project.name.is_empty() {
            anyhow::bail!("Project name cannot be empty");
        }

        if let Some(mode) = &self.project.deploy_mode {
            if mode != "local" && mode != "remote" {
                anyhow::bail!("project.deploy_mode must be 'local' or 'remote'");
            }
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
                if let Some(hc) = &service.healthcheck {
                    let has_cmd = !hc.command.is_empty();
                    let has_http = hc.http.is_some();
                    let has_tcp = hc.tcp.is_some();
                    let has_any = hc.any.as_ref().is_some_and(|v| !v.is_empty());
                    let has_all = hc.all.as_ref().is_some_and(|v| !v.is_empty());
                    if !(has_cmd || has_http || has_tcp || has_any || has_all) {
                        anyhow::bail!(
                            "Healthcheck for service '{}' must include one of: command/http/tcp/any/all",
                            name
                        );
                    }
                }
            }
        }

        if let Some(edge) = &self.edge {
            if edge.provider.is_empty() {
                anyhow::bail!("Edge provider cannot be empty");
            }
            for site in &edge.sites {
                if site.host.is_empty() {
                    anyhow::bail!("Edge site host cannot be empty");
                }
                if site.upstream_service.is_empty() {
                    anyhow::bail!("Edge upstream_service cannot be empty");
                }
                if site.upstream_port == 0 {
                    anyhow::bail!("Edge upstream_port must be > 0");
                }
            }
        }

        Ok(())
    }

    fn apply_overlay(&mut self, overlay: OverlayConfig) {
        if let Some(project) = overlay.project {
            if let Some(name) = project.name {
                self.project.name = name;
            }
            if project.description.is_some() {
                self.project.description = project.description;
            }
            if project.deploy_mode.is_some() {
                self.project.deploy_mode = project.deploy_mode;
            }
        }

        if let Some(infra) = overlay.infra {
            if let Some(base_infra) = &mut self.infra {
                for overlay_server in infra.servers {
                    if let Some(existing) = base_infra
                        .servers
                        .iter_mut()
                        .find(|s| s.name == overlay_server.name)
                    {
                        *existing = overlay_server;
                    } else {
                        base_infra.servers.push(overlay_server);
                    }
                }
            } else {
                self.infra = Some(InfraConfig {
                    servers: infra.servers,
                });
            }
        }

        if let Some(services) = overlay.services {
            let base_services = self.services.get_or_insert_with(HashMap::new);
            for (name, svc) in services {
                base_services.insert(name, svc);
            }
        }

        if let Some(edge) = overlay.edge {
            self.edge = Some(edge);
        }
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
deploy_mode = "remote"

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
healthcheck = { command = ["sh", "-lc", "wget -qO- http://127.0.0.1:80 >/dev/null"], interval_secs = 5, retries = 10, timeout_secs = 3 }

[services.database]
image = "postgres:15"
ports = [5432]
env = { POSTGRES_DB = "myapp", POSTGRES_USER = "user", POSTGRES_PASSWORD = "password" }
volumes = ["./data:/var/lib/postgresql/data"]

[edge]
provider = "caddy"

[[edge.sites]]
host = "api.example.com"
upstream_service = "api"
upstream_port = 80
tls_email = "ops@example.com"
redirect_http = true
"#;

        std::fs::write(&path, example_config)
            .with_context(|| format!("Failed to write config file: {:?}", path.as_ref()))?;

        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
struct OverlayConfig {
    project: Option<OverlayProjectConfig>,
    infra: Option<InfraConfig>,
    services: Option<HashMap<String, ServiceConfig>>,
    edge: Option<EdgeConfig>,
}

#[derive(Debug, Clone, Deserialize)]
struct OverlayProjectConfig {
    name: Option<String>,
    description: Option<String>,
    deploy_mode: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_path(filename: &str) -> std::path::PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        std::env::temp_dir().join(format!("airstack-config-tests-{now}-{filename}"))
    }

    fn base_config() -> AirstackConfig {
        AirstackConfig {
            project: ProjectConfig {
                name: "demo".to_string(),
                description: None,
                deploy_mode: Some("remote".to_string()),
            },
            infra: Some(InfraConfig {
                servers: vec![ServerConfig {
                    name: "web".to_string(),
                    provider: "hetzner".to_string(),
                    region: "nbg1".to_string(),
                    server_type: "cx21".to_string(),
                    ssh_key: "~/.ssh/id_ed25519.pub".to_string(),
                    floating_ip: Some(false),
                }],
            }),
            services: Some(HashMap::from([(
                "api".to_string(),
                ServiceConfig {
                    image: "nginx:latest".to_string(),
                    ports: vec![80],
                    env: None,
                    volumes: None,
                    depends_on: None,
                    target_server: None,
                    healthcheck: None,
                    profile: None,
                },
            )])),
            edge: None,
        }
    }

    #[test]
    fn validate_rejects_empty_project_name() {
        let mut cfg = base_config();
        cfg.project.name.clear();
        let err = cfg.validate().expect_err("expected validation error");
        assert!(
            err.to_string().contains("Project name cannot be empty"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_empty_server_provider() {
        let mut cfg = base_config();
        cfg.infra
            .as_mut()
            .expect("infra should exist")
            .servers
            .first_mut()
            .expect("one server expected")
            .provider
            .clear();
        let err = cfg.validate().expect_err("expected validation error");
        assert!(
            err.to_string().contains("Server provider cannot be empty"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_empty_service_image() {
        let mut cfg = base_config();
        cfg.services
            .as_mut()
            .expect("services should exist")
            .get_mut("api")
            .expect("api service should exist")
            .image
            .clear();
        let err = cfg.validate().expect_err("expected validation error");
        assert!(
            err.to_string().contains("Service image cannot be empty"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn init_example_writes_loadable_config() {
        let path = unique_path("example.toml");
        AirstackConfig::init_example(&path).expect("example config should write");
        let loaded = AirstackConfig::load(&path).expect("example config should parse");
        assert_eq!(loaded.project.name, "my-project");
        fs::remove_file(&path).expect("cleanup should succeed");
    }

    #[test]
    fn load_allows_missing_region_and_defaults_empty() {
        let path = unique_path("missing-region.toml");
        let raw = r#"
[project]
name = "demo"

[[infra.servers]]
name = "web"
provider = "hetzner"
server_type = "cpx21"
ssh_key = "~/.ssh/id_ed25519.pub"
"#;
        fs::write(&path, raw).expect("config write should succeed");
        let loaded = AirstackConfig::load(&path).expect("config should parse");
        assert_eq!(loaded.infra.unwrap().servers[0].region, "");
        fs::remove_file(&path).expect("cleanup should succeed");
    }

    #[test]
    fn load_fails_on_duplicate_key() {
        let path = unique_path("duplicate-key.toml");
        let raw = r#"
[project]
name = "demo"

[[infra.servers]]
name = "web"
provider = "hetzner"
region = "ash"
region = "hel1"
server_type = "cpx21"
ssh_key = "~/.ssh/id_ed25519.pub"
"#;
        fs::write(&path, raw).expect("config write should succeed");
        let err = AirstackConfig::load(&path).expect_err("duplicate key should fail");
        let msg = err.to_string();
        assert!(
            msg.contains("duplicate")
                && (msg.contains("line") || msg.contains("column") || msg.contains(":")),
            "unexpected error: {msg}"
        );
        fs::remove_file(&path).expect("cleanup should succeed");
    }
}
