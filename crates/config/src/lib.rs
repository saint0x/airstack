use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirstackConfig {
    pub project: ProjectConfig,
    pub infra: Option<InfraConfig>,
    pub services: Option<HashMap<String, ServiceConfig>>,
    pub edge: Option<EdgeConfig>,
    pub providers: Option<ProvidersConfig>,
    pub scripts: Option<HashMap<String, ScriptConfig>>,
    pub hooks: Option<HooksConfig>,
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
    pub firewall: Option<FirewallConfig>,
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
pub struct FirewallConfig {
    pub name: String,
    pub ingress: Vec<FirewallRuleConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirewallRuleConfig {
    pub protocol: String,
    pub port: Option<String>,
    pub source_ips: Vec<String>,
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
    pub extra_include_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeSiteConfig {
    pub host: String,
    pub upstream_service: String,
    pub upstream_port: u16,
    pub tls_email: Option<String>,
    pub redirect_http: Option<bool>,
    pub nearest: Option<EdgeNearestConfig>,
    #[serde(default, rename = "static")]
    pub static_routes: Vec<EdgeStaticRouteConfig>,
    #[serde(default)]
    pub routes: Vec<EdgeRouteConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeNearestConfig {
    pub method: Option<String>,
    pub header: Option<String>,
    pub default_upstream: Option<String>,
    #[serde(default)]
    pub regions: Vec<EdgeNearestRegionConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeNearestRegionConfig {
    pub name: String,
    #[serde(default)]
    pub countries: Vec<String>,
    pub upstream: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeStaticRouteConfig {
    pub path_prefix: String,
    pub root: String,
    pub browse: Option<bool>,
    pub headers: Option<HashMap<String, String>>,
    pub host_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeRouteConfig {
    pub path_prefix: String,
    pub upstream: Option<String>,
    pub strip_prefix: Option<bool>,
    pub headers: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvidersConfig {
    pub profiles: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptConfig {
    pub target: String,
    pub file: String,
    pub shell: Option<String>,
    pub args: Option<Vec<String>>,
    pub env: Option<HashMap<String, String>>,
    pub idempotency: Option<String>,
    pub timeout_secs: Option<u64>,
    pub retry: Option<ScriptRetryConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptRetryConfig {
    pub max_attempts: Option<usize>,
    pub transient_only: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HooksConfig {
    pub pre_provision: Option<Vec<String>>,
    pub post_provision: Option<Vec<String>>,
    pub post_deploy: Option<Vec<String>>,
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
            if let Some(fw) = &infra.firewall {
                if fw.name.trim().is_empty() {
                    anyhow::bail!("infra.firewall.name cannot be empty");
                }
                if fw.ingress.is_empty() {
                    anyhow::bail!("infra.firewall.ingress must contain at least one rule");
                }
                for rule in &fw.ingress {
                    if !matches!(rule.protocol.as_str(), "tcp" | "udp" | "icmp") {
                        anyhow::bail!(
                            "infra.firewall ingress protocol must be one of tcp|udp|icmp"
                        );
                    }
                    if rule.source_ips.is_empty() {
                        anyhow::bail!("infra.firewall rule source_ips cannot be empty");
                    }
                    if rule.protocol != "icmp"
                        && rule.port.as_ref().is_none_or(|p| p.trim().is_empty())
                    {
                        anyhow::bail!(
                            "infra.firewall rule for protocol '{}' requires non-empty port",
                            rule.protocol
                        );
                    }
                }
            }
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
                if let Some(nearest) = &site.nearest {
                    let method = nearest.method.as_deref().unwrap_or("geo-header");
                    if method != "geo-header" {
                        anyhow::bail!(
                            "edge.sites.nearest.method must be 'geo-header' (got '{}')",
                            method
                        );
                    }
                    let header = nearest.header.as_deref().unwrap_or("CF-IPCountry");
                    if header.trim().is_empty() {
                        anyhow::bail!("edge.sites.nearest.header cannot be empty when set");
                    }
                    if nearest.regions.is_empty() {
                        anyhow::bail!(
                            "edge.sites.nearest.regions must contain at least one region"
                        );
                    }

                    let mut seen_countries = HashSet::new();
                    for region in &nearest.regions {
                        if region.name.trim().is_empty() {
                            anyhow::bail!("edge.sites.nearest.regions.name cannot be empty");
                        }
                        if region.upstream.trim().is_empty() {
                            anyhow::bail!("edge.sites.nearest.regions.upstream cannot be empty");
                        }
                        if region.countries.is_empty() {
                            anyhow::bail!(
                                "edge.sites.nearest.regions.countries must contain at least one ISO country code"
                            );
                        }
                        for country in &region.countries {
                            let normalized = country.trim().to_ascii_uppercase();
                            if normalized.len() != 2
                                || !normalized.chars().all(|c| c.is_ascii_alphabetic())
                            {
                                anyhow::bail!(
                                    "edge.sites.nearest.regions.countries entries must be ISO-3166 alpha-2 codes (got '{}')",
                                    country
                                );
                            }
                            if !seen_countries.insert(normalized.clone()) {
                                anyhow::bail!(
                                    "edge.sites.nearest has duplicate country code '{}' across regions",
                                    normalized
                                );
                            }
                        }
                    }
                    if nearest
                        .default_upstream
                        .as_ref()
                        .is_some_and(|u| u.trim().is_empty())
                    {
                        anyhow::bail!(
                            "edge.sites.nearest.default_upstream cannot be empty when set"
                        );
                    }
                }
                for route in &site.static_routes {
                    if route.path_prefix.trim().is_empty() {
                        anyhow::bail!("edge.sites.static path_prefix cannot be empty");
                    }
                    if route.root.trim().is_empty() {
                        anyhow::bail!("edge.sites.static root cannot be empty");
                    }
                    if !route.path_prefix.starts_with('/') {
                        anyhow::bail!(
                            "edge.sites.static path_prefix must start with '/' (got '{}')",
                            route.path_prefix
                        );
                    }
                    if let Some(host_path) = &route.host_path {
                        if host_path.trim().is_empty() || !host_path.starts_with('/') {
                            anyhow::bail!(
                                "edge.sites.static host_path must be an absolute path when set"
                            );
                        }
                    }
                }
                for route in &site.routes {
                    if route.path_prefix.trim().is_empty() {
                        anyhow::bail!("edge.sites.routes path_prefix cannot be empty");
                    }
                    if !route.path_prefix.starts_with('/') {
                        anyhow::bail!(
                            "edge.sites.routes path_prefix must start with '/' (got '{}')",
                            route.path_prefix
                        );
                    }
                    if route.upstream.as_ref().is_some_and(|u| u.trim().is_empty()) {
                        anyhow::bail!("edge.sites.routes upstream cannot be empty when set");
                    }
                }
            }
            if let Some(include) = &edge.extra_include_file {
                if include.trim().is_empty() {
                    anyhow::bail!("edge.extra_include_file cannot be empty when set");
                }
            }
        }

        if let Some(providers) = &self.providers {
            if let Some(profile_pins) = &providers.profiles {
                for (provider, profile) in profile_pins {
                    if provider.trim().is_empty() || profile.trim().is_empty() {
                        anyhow::bail!("providers.profiles entries require non-empty provider and profile names");
                    }
                }
            }
        }

        if let Some(scripts) = &self.scripts {
            for (name, script) in scripts {
                if name.trim().is_empty() {
                    anyhow::bail!("Script name cannot be empty");
                }
                if script.target.trim().is_empty() {
                    anyhow::bail!("Script '{}' target cannot be empty", name);
                }
                if script.file.trim().is_empty() {
                    anyhow::bail!("Script '{}' file cannot be empty", name);
                }
                if let Some(mode) = &script.idempotency {
                    if mode != "always" && mode != "once" && mode != "on-change" {
                        anyhow::bail!(
                            "Script '{}' idempotency must be one of: always|once|on-change",
                            name
                        );
                    }
                }
            }
        }

        if let Some(hooks) = &self.hooks {
            if let Some(scripts) = &self.scripts {
                for (hook, names) in [
                    ("pre_provision", hooks.pre_provision.as_ref()),
                    ("post_provision", hooks.post_provision.as_ref()),
                    ("post_deploy", hooks.post_deploy.as_ref()),
                ] {
                    if let Some(names) = names {
                        for name in names {
                            if !scripts.contains_key(name) {
                                anyhow::bail!(
                                    "Hook '{}' references unknown script '{}'",
                                    hook,
                                    name
                                );
                            }
                        }
                    }
                }
            } else if hooks.pre_provision.is_some()
                || hooks.post_provision.is_some()
                || hooks.post_deploy.is_some()
            {
                anyhow::bail!("Hooks configured but no [scripts] defined");
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
                if infra.firewall.is_some() {
                    base_infra.firewall = infra.firewall.clone();
                }
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
                    firewall: infra.firewall,
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

        if let Some(providers) = overlay.providers {
            self.providers = Some(providers);
        }

        if let Some(scripts) = overlay.scripts {
            let base_scripts = self.scripts.get_or_insert_with(HashMap::new);
            for (name, script) in scripts {
                base_scripts.insert(name, script);
            }
        }

        if let Some(hooks) = overlay.hooks {
            self.hooks = Some(hooks);
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
extra_include_file = "/opt/aria/Caddy.extra"

[[edge.sites]]
host = "api.example.com"
upstream_service = "api"
upstream_port = 80
tls_email = "ops@example.com"
redirect_http = true

[[edge.sites.static]]
path_prefix = "/downloads"
root = "/srv/downloads"
browse = false

[[edge.sites.routes]]
path_prefix = "/internal"
upstream = "api:8080"
strip_prefix = true

[providers.profiles]
fly = "work"
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
    providers: Option<ProvidersConfig>,
    scripts: Option<HashMap<String, ScriptConfig>>,
    hooks: Option<HooksConfig>,
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
                firewall: None,
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
            providers: None,
            scripts: None,
            hooks: None,
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

    #[test]
    fn validate_rejects_unknown_hook_script_reference() {
        let mut cfg = base_config();
        cfg.scripts = Some(HashMap::from([(
            "bootstrap".to_string(),
            ScriptConfig {
                target: "all".to_string(),
                file: "scripts/bootstrap.sh".to_string(),
                shell: None,
                args: None,
                env: None,
                idempotency: Some("always".to_string()),
                timeout_secs: None,
                retry: None,
            },
        )]));
        cfg.hooks = Some(HooksConfig {
            pre_provision: Some(vec!["missing".to_string()]),
            post_provision: None,
            post_deploy: None,
        });

        let err = cfg.validate().expect_err("unknown hook script should fail");
        assert!(
            err.to_string()
                .contains("Hook 'pre_provision' references unknown script 'missing'"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_accepts_scripts_and_hooks() {
        let mut cfg = base_config();
        cfg.scripts = Some(HashMap::from([(
            "bootstrap".to_string(),
            ScriptConfig {
                target: "all".to_string(),
                file: "scripts/bootstrap.sh".to_string(),
                shell: Some("bash".to_string()),
                args: Some(vec!["--fast".to_string()]),
                env: Some(HashMap::from([(
                    "AIRSTACK_STAGE".to_string(),
                    "prod".to_string(),
                )])),
                idempotency: Some("on-change".to_string()),
                timeout_secs: Some(120),
                retry: Some(ScriptRetryConfig {
                    max_attempts: Some(2),
                    transient_only: Some(true),
                }),
            },
        )]));
        cfg.hooks = Some(HooksConfig {
            pre_provision: Some(vec!["bootstrap".to_string()]),
            post_provision: None,
            post_deploy: None,
        });

        cfg.validate().expect("valid scripts/hooks should pass");
    }

    #[test]
    fn validate_rejects_invalid_firewall_protocol() {
        let mut cfg = base_config();
        cfg.infra = Some(InfraConfig {
            servers: cfg.infra.as_ref().expect("infra exists").servers.clone(),
            firewall: Some(FirewallConfig {
                name: "web".to_string(),
                ingress: vec![FirewallRuleConfig {
                    protocol: "http".to_string(),
                    port: Some("80".to_string()),
                    source_ips: vec!["0.0.0.0/0".to_string()],
                }],
            }),
        });
        let err = cfg
            .validate()
            .expect_err("invalid firewall protocol should fail");
        assert!(
            err.to_string()
                .contains("protocol must be one of tcp|udp|icmp"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_edge_static_without_leading_slash() {
        let mut cfg = base_config();
        cfg.edge = Some(EdgeConfig {
            provider: "caddy".to_string(),
            extra_include_file: None,
            sites: vec![EdgeSiteConfig {
                host: "api.example.com".to_string(),
                upstream_service: "api".to_string(),
                upstream_port: 80,
                tls_email: None,
                redirect_http: Some(true),
                nearest: None,
                static_routes: vec![EdgeStaticRouteConfig {
                    path_prefix: "downloads".to_string(),
                    root: "/srv/downloads".to_string(),
                    browse: Some(false),
                    headers: None,
                    host_path: Some("/srv/downloads".to_string()),
                }],
                routes: vec![],
            }],
        });

        let err = cfg
            .validate()
            .expect_err("static path prefix must start with slash");
        assert!(
            err.to_string()
                .contains("edge.sites.static path_prefix must start with '/'"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_edge_static_non_absolute_host_path() {
        let mut cfg = base_config();
        cfg.edge = Some(EdgeConfig {
            provider: "caddy".to_string(),
            extra_include_file: None,
            sites: vec![EdgeSiteConfig {
                host: "api.example.com".to_string(),
                upstream_service: "api".to_string(),
                upstream_port: 80,
                tls_email: None,
                redirect_http: Some(true),
                nearest: None,
                static_routes: vec![EdgeStaticRouteConfig {
                    path_prefix: "/downloads".to_string(),
                    root: "/srv/downloads".to_string(),
                    browse: Some(false),
                    headers: None,
                    host_path: Some("srv/downloads".to_string()),
                }],
                routes: vec![],
            }],
        });

        let err = cfg
            .validate()
            .expect_err("static host_path must be absolute");
        assert!(
            err.to_string()
                .contains("edge.sites.static host_path must be an absolute path"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_accepts_geo_header_nearest_routing() {
        let mut cfg = base_config();
        cfg.edge = Some(EdgeConfig {
            provider: "caddy".to_string(),
            extra_include_file: None,
            sites: vec![EdgeSiteConfig {
                host: "backend.quilt.sh".to_string(),
                upstream_service: "api_us".to_string(),
                upstream_port: 8080,
                tls_email: Some("ops@quilt.sh".to_string()),
                redirect_http: Some(true),
                nearest: Some(EdgeNearestConfig {
                    method: Some("geo-header".to_string()),
                    header: Some("CF-IPCountry".to_string()),
                    default_upstream: Some("api_us:8080".to_string()),
                    regions: vec![EdgeNearestRegionConfig {
                        name: "eu".to_string(),
                        countries: vec!["GB".to_string(), "DE".to_string(), "FR".to_string()],
                        upstream: "api_eu:8080".to_string(),
                    }],
                }),
                static_routes: vec![],
                routes: vec![],
            }],
        });

        cfg.validate()
            .expect("valid geo-header nearest routing should pass");
    }

    #[test]
    fn validate_rejects_duplicate_country_in_nearest_regions() {
        let mut cfg = base_config();
        cfg.edge = Some(EdgeConfig {
            provider: "caddy".to_string(),
            extra_include_file: None,
            sites: vec![EdgeSiteConfig {
                host: "backend.quilt.sh".to_string(),
                upstream_service: "api_us".to_string(),
                upstream_port: 8080,
                tls_email: Some("ops@quilt.sh".to_string()),
                redirect_http: Some(true),
                nearest: Some(EdgeNearestConfig {
                    method: Some("geo-header".to_string()),
                    header: Some("CF-IPCountry".to_string()),
                    default_upstream: Some("api_us:8080".to_string()),
                    regions: vec![
                        EdgeNearestRegionConfig {
                            name: "eu".to_string(),
                            countries: vec!["GB".to_string(), "DE".to_string()],
                            upstream: "api_eu:8080".to_string(),
                        },
                        EdgeNearestRegionConfig {
                            name: "na".to_string(),
                            countries: vec!["US".to_string(), "DE".to_string()],
                            upstream: "api_us:8080".to_string(),
                        },
                    ],
                }),
                static_routes: vec![],
                routes: vec![],
            }],
        });

        let err = cfg
            .validate()
            .expect_err("duplicate country across nearest regions must fail");
        assert!(
            err.to_string()
                .contains("edge.sites.nearest has duplicate country code 'DE'"),
            "unexpected error: {err}"
        );
    }
}
