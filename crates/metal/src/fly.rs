use crate::{CreateServerRequest, MetalProvider, ProviderCapabilities, Server, ServerStatus};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::process::Command as StdCommand;
use tokio::process::Command;
use tokio::time::{sleep, timeout, Duration};
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct FlyProvider {
    token: Option<String>,
    org: Option<String>,
    default_image: String,
}

#[derive(Debug, Clone, Deserialize)]
struct FlyApp {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Status")]
    status: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct FlyMachine {
    id: String,
    state: Option<String>,
    region: Option<String>,
    private_ip: Option<String>,
    config: Option<FlyMachineConfig>,
}

#[derive(Debug, Clone, Deserialize)]
struct FlyMachineConfig {
    guest: Option<FlyMachineGuest>,
}

#[derive(Debug, Clone, Deserialize)]
struct FlyMachineGuest {
    cpu_kind: Option<String>,
    cpus: Option<u32>,
    memory_mb: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct FlyIp {
    #[serde(rename = "Address")]
    address: String,
    #[serde(rename = "Type")]
    ip_type: String,
}

impl FlyProvider {
    pub fn new(config: HashMap<String, String>) -> Result<Self> {
        let token = config
            .get("api_token")
            .cloned()
            .or_else(|| std::env::var("FLY_API_TOKEN").ok())
            .or_else(|| std::env::var("FLY_ACCESS_TOKEN").ok());
        let org = config
            .get("org")
            .cloned()
            .or_else(|| std::env::var("FLY_ORG").ok());
        let default_image = config
            .get("image")
            .cloned()
            .or_else(|| std::env::var("FLY_MACHINE_IMAGE").ok())
            .unwrap_or_else(|| "ubuntu:22.04".to_string());

        let probe = StdCommand::new("flyctl")
            .arg("version")
            .output()
            .context("Failed to execute flyctl. Install flyctl to use provider='fly'.")?;
        if !probe.status.success() {
            anyhow::bail!("flyctl is not available. Install flyctl to use provider='fly'.");
        }

        Ok(Self {
            token,
            org,
            default_image,
        })
    }

    async fn run_flyctl(&self, args: &[&str]) -> Result<std::process::Output> {
        let mut cmd = Command::new("flyctl");
        cmd.args(args);
        if let Some(token) = &self.token {
            cmd.env("FLY_ACCESS_TOKEN", token);
            cmd.env("FLY_API_TOKEN", token);
        }

        debug!("flyctl {}", args.join(" "));
        let output = timeout(Duration::from_secs(60), cmd.output())
            .await
            .context("flyctl command timed out")?
            .context("Failed to execute flyctl command")?;
        Ok(output)
    }

    async fn run_flyctl_json<T: for<'de> Deserialize<'de>>(&self, args: &[&str]) -> Result<T> {
        let out = self.run_flyctl(args).await?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!(
                "flyctl command failed ({}): {}",
                args.join(" "),
                stderr.trim()
            );
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        serde_json::from_str(&stdout)
            .with_context(|| format!("Failed to parse flyctl JSON for '{}'", args.join(" ")))
    }

    async fn ensure_app_exists(&self, app: &str) -> Result<()> {
        let apps = self.list_apps().await?;
        if apps.iter().any(|a| a.name == app) {
            return Ok(());
        }

        info!("Creating Fly app: {}", app);
        let mut args = vec!["apps", "create", app, "--yes"];
        if let Some(org) = &self.org {
            args.push("--org");
            args.push(org.as_str());
        }
        let out = self.run_flyctl(&args).await?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!("Failed to create Fly app '{}': {}", app, stderr.trim());
        }
        Ok(())
    }

    async fn list_apps(&self) -> Result<Vec<FlyApp>> {
        let mut args = vec!["apps", "list", "--json"];
        if let Some(org) = &self.org {
            args.push("--org");
            args.push(org.as_str());
        }
        self.run_flyctl_json(&args).await
    }

    async fn list_machines(&self, app: &str) -> Result<Vec<FlyMachine>> {
        self.run_flyctl_json(&["machine", "list", "--app", app, "--json"])
            .await
    }

    async fn list_ips(&self, app: &str) -> Result<Vec<FlyIp>> {
        self.run_flyctl_json(&["ips", "list", "--app", app, "--json"])
            .await
    }

    fn map_machine_status(state: Option<&str>) -> ServerStatus {
        match state.unwrap_or("").to_ascii_lowercase().as_str() {
            "created" | "starting" => ServerStatus::Creating,
            "started" | "running" => ServerStatus::Running,
            "stopping" | "stopped" | "suspended" => ServerStatus::Stopped,
            "destroying" | "destroyed" => ServerStatus::Deleting,
            _ => ServerStatus::Error,
        }
    }

    fn map_app_status(status: Option<&str>) -> ServerStatus {
        match status.unwrap_or("").to_ascii_lowercase().as_str() {
            "deployed" | "running" => ServerStatus::Running,
            "pending" | "deploying" => ServerStatus::Creating,
            "suspended" | "stopped" => ServerStatus::Stopped,
            "destroying" => ServerStatus::Deleting,
            _ => ServerStatus::Error,
        }
    }

    fn server_type_for_machine(machine: &FlyMachine) -> String {
        let Some(guest) = machine.config.as_ref().and_then(|cfg| cfg.guest.as_ref()) else {
            return "fly-machine".to_string();
        };
        let kind = guest
            .cpu_kind
            .clone()
            .unwrap_or_else(|| "shared".to_string());
        let cpus = guest.cpus.unwrap_or(1);
        let mem = guest.memory_mb.unwrap_or(256);
        format!("{}-{}x{}mb", kind, cpus, mem)
    }

    fn parse_server_id(id: &str) -> Result<(String, Option<String>)> {
        if let Some(rest) = id.strip_prefix("fly:") {
            let mut parts = rest.splitn(2, ':');
            let app = parts.next().unwrap_or_default();
            let machine = parts.next().map(|s| s.to_string());
            if app.is_empty() {
                anyhow::bail!("Invalid Fly server id '{}'", id);
            }
            return Ok((app.to_string(), machine));
        }

        if id.is_empty() {
            anyhow::bail!("Invalid empty Fly server id");
        }
        Ok((id.to_string(), None))
    }

    fn app_public_ip(ips: &[FlyIp]) -> Option<String> {
        ips.iter()
            .find(|ip| ip.ip_type == "shared_v4" || ip.ip_type == "v4")
            .map(|ip| ip.address.clone())
            .or_else(|| ips.first().map(|ip| ip.address.clone()))
    }

    async fn build_server_records_for_app(&self, app: &FlyApp) -> Vec<Server> {
        let ips = match self.list_ips(&app.name).await {
            Ok(v) => v,
            Err(e) => {
                warn!("Failed to list Fly IPs for app {}: {}", app.name, e);
                Vec::new()
            }
        };
        let public_ip = Self::app_public_ip(&ips);

        let machines = match self.list_machines(&app.name).await {
            Ok(v) => v,
            Err(e) => {
                warn!("Failed to list Fly machines for app {}: {}", app.name, e);
                return vec![Server {
                    id: format!("fly:{}", app.name),
                    name: app.name.clone(),
                    status: Self::map_app_status(app.status.as_deref()),
                    public_ip,
                    private_ip: None,
                    server_type: "fly-app".to_string(),
                    region: "global".to_string(),
                }];
            }
        };

        if machines.is_empty() {
            return vec![Server {
                id: format!("fly:{}", app.name),
                name: app.name.clone(),
                status: Self::map_app_status(app.status.as_deref()),
                public_ip,
                private_ip: None,
                server_type: "fly-app/0-machines".to_string(),
                region: "global".to_string(),
            }];
        }

        let mut region = "global".to_string();
        let mut private_ip = None;
        let mut status = ServerStatus::Stopped;
        if let Some(first) = machines.first() {
            region = first.region.clone().unwrap_or_else(|| "global".to_string());
            private_ip = first.private_ip.clone();
            status = Self::map_machine_status(first.state.as_deref());
        }
        if machines.iter().any(|m| {
            matches!(
                Self::map_machine_status(m.state.as_deref()),
                ServerStatus::Running
            )
        }) {
            status = ServerStatus::Running;
        } else if machines.iter().any(|m| {
            matches!(
                Self::map_machine_status(m.state.as_deref()),
                ServerStatus::Creating
            )
        }) {
            status = ServerStatus::Creating;
        }

        let machine_count = machines.len();
        vec![Server {
            id: format!("fly:{}", app.name),
            name: app.name.clone(),
            status,
            public_ip,
            private_ip,
            server_type: format!("fly-app/{}-machines", machine_count),
            region,
        }]
    }
}

#[async_trait::async_trait]
impl MetalProvider for FlyProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_public_ip: false,
            supports_direct_ssh: false,
            supports_provider_ssh: true,
            supports_server_create: true,
            supports_server_destroy: true,
        }
    }

    async fn create_server(&self, request: CreateServerRequest) -> Result<Server> {
        info!("Creating Fly machine/app: {}", request.name);
        self.ensure_app_exists(&request.name).await?;

        let existing = self.list_machines(&request.name).await.unwrap_or_default();
        if let Some(existing_machine) = existing.first() {
            let app_name = request.name.clone();
            return Ok(Server {
                id: format!("fly:{}", app_name),
                name: app_name.clone(),
                status: Self::map_machine_status(existing_machine.state.as_deref()),
                public_ip: Self::app_public_ip(&self.list_ips(&app_name).await.unwrap_or_default()),
                private_ip: existing_machine.private_ip.clone(),
                server_type: Self::server_type_for_machine(existing_machine),
                region: existing_machine
                    .region
                    .clone()
                    .unwrap_or_else(|| "global".to_string()),
            });
        }

        let image = self.default_image.clone();
        let out = self
            .run_flyctl(&[
                "machine",
                "run",
                image.as_str(),
                "--app",
                request.name.as_str(),
                "--name",
                request.name.as_str(),
                "--region",
                request.region.as_str(),
                "--vm-size",
                request.server_type.as_str(),
                "--detach",
            ])
            .await?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!(
                "Failed to create Fly machine for app '{}': {}",
                request.name,
                stderr.trim()
            );
        }

        let mut found: Option<FlyMachine> = None;
        for _ in 0..8u8 {
            let machines = self.list_machines(&request.name).await.unwrap_or_default();
            if let Some(machine) = machines.first() {
                found = Some(machine.clone());
                break;
            }
            sleep(Duration::from_millis(500)).await;
        }

        let machine = found.context("Fly machine was not visible after creation")?;
        if request.attach_floating_ip {
            let _ = self
                .attach_floating_ip(&format!("fly:{}", request.name))
                .await;
        }

        Ok(Server {
            id: format!("fly:{}", request.name),
            name: request.name.clone(),
            status: Self::map_machine_status(machine.state.as_deref()),
            public_ip: Self::app_public_ip(&self.list_ips(&request.name).await.unwrap_or_default()),
            private_ip: machine.private_ip.clone(),
            server_type: Self::server_type_for_machine(&machine),
            region: machine.region.unwrap_or_else(|| "global".to_string()),
        })
    }

    async fn destroy_server(&self, id: &str) -> Result<()> {
        let (app, machine_id) = Self::parse_server_id(id)?;
        info!("Destroying Fly server id={} app={}", id, app);

        let targets = if let Some(machine_id) = machine_id {
            vec![machine_id]
        } else {
            self.list_machines(&app)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|m| m.id)
                .collect()
        };

        if targets.is_empty() {
            return Ok(());
        }

        for machine in targets {
            let out = self
                .run_flyctl(&[
                    "machine",
                    "destroy",
                    "--app",
                    app.as_str(),
                    "--force",
                    machine.as_str(),
                ])
                .await?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                anyhow::bail!(
                    "Failed to destroy Fly machine '{}' in app '{}': {}",
                    machine,
                    app,
                    stderr.trim()
                );
            }
        }
        Ok(())
    }

    async fn get_server(&self, id: &str) -> Result<Server> {
        let (app, machine_opt) = Self::parse_server_id(id)?;
        let machines = self.list_machines(&app).await?;
        let machine = if let Some(machine_id) = machine_opt {
            machines
                .into_iter()
                .find(|m| m.id == machine_id)
                .with_context(|| {
                    format!("Fly machine '{}' not found in app '{}'", machine_id, app)
                })?
        } else {
            machines
                .into_iter()
                .next()
                .with_context(|| format!("No Fly machines found for app '{}'", app))?
        };

        Ok(Server {
            id: format!("fly:{}", app),
            name: app.clone(),
            status: Self::map_machine_status(machine.state.as_deref()),
            public_ip: Self::app_public_ip(&self.list_ips(&app).await.unwrap_or_default()),
            private_ip: machine.private_ip.clone(),
            server_type: Self::server_type_for_machine(&machine),
            region: machine.region.unwrap_or_else(|| "global".to_string()),
        })
    }

    async fn list_servers(&self) -> Result<Vec<Server>> {
        debug!("Listing Fly app/machine inventory");
        let apps = self.list_apps().await?;
        let mut servers = Vec::new();
        for app in apps {
            servers.extend(self.build_server_records_for_app(&app).await);
        }
        Ok(servers)
    }

    async fn upload_ssh_key(&self, name: &str, _public_key_path: &str) -> Result<String> {
        info!(
            "Fly provider uses flyctl-managed SSH certificates; skipping SSH key upload for {}",
            name
        );
        Ok(name.to_string())
    }

    async fn attach_floating_ip(&self, server_id: &str) -> Result<String> {
        let (app, _) = Self::parse_server_id(server_id)?;
        let existing = self.list_ips(&app).await.unwrap_or_default();
        if let Some(ip) = Self::app_public_ip(&existing) {
            return Ok(ip);
        }

        let out = self
            .run_flyctl(&["ips", "allocate-v4", "--app", app.as_str(), "--yes"])
            .await?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!(
                "Failed to allocate v4 IP for Fly app '{}': {}",
                app,
                stderr.trim()
            );
        }

        let refreshed = self.list_ips(&app).await?;
        Self::app_public_ip(&refreshed).with_context(|| {
            format!(
                "Fly app '{}' has no public IP after allocate-v4 completed",
                app
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::FlyProvider;

    #[test]
    fn parse_server_id_supports_app_and_machine() {
        let parsed = FlyProvider::parse_server_id("fly:demo:abc123").expect("id should parse");
        assert_eq!(parsed.0, "demo");
        assert_eq!(parsed.1.as_deref(), Some("abc123"));
    }

    #[test]
    fn parse_server_id_supports_app_only() {
        let parsed = FlyProvider::parse_server_id("fly:demo").expect("id should parse");
        assert_eq!(parsed.0, "demo");
        assert!(parsed.1.is_none());
    }

    #[test]
    fn map_machine_status_handles_started_and_stopped() {
        assert!(matches!(
            FlyProvider::map_machine_status(Some("started")),
            crate::ServerStatus::Running
        ));
        assert!(matches!(
            FlyProvider::map_machine_status(Some("stopped")),
            crate::ServerStatus::Stopped
        ));
    }
}
