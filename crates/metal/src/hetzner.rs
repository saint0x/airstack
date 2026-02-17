use crate::{CreateServerRequest, MetalProvider, Server, ServerStatus};
use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info};

#[derive(Debug)]
pub struct HetznerProvider {
    client: Client,
    api_token: String,
    base_url: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct HetznerServer {
    id: u64,
    name: String,
    status: String,
    public_net: HetznerPublicNet,
    private_net: Vec<HetznerPrivateNet>,
    server_type: HetznerServerType,
    datacenter: HetznerDatacenter,
}

#[derive(Debug, Serialize, Deserialize)]
struct HetznerPublicNet {
    ipv4: Option<HetznerIp>,
    floating_ips: Vec<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct HetznerIp {
    ip: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct HetznerPrivateNet {
    ip: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct HetznerServerType {
    name: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct HetznerDatacenter {
    location: HetznerLocation,
}

#[derive(Debug, Serialize, Deserialize)]
struct HetznerLocation {
    name: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct HetznerResponse<T> {
    servers: Option<Vec<T>>,
    server: Option<T>,
}

#[derive(Debug, Serialize)]
struct CreateServerPayload {
    name: String,
    server_type: String,
    location: String,
    ssh_keys: Vec<String>,
    public_net: CreateServerPublicNet,
}

#[derive(Debug, Serialize)]
struct CreateServerPublicNet {
    enable_ipv4: bool,
    enable_ipv6: bool,
}

impl HetznerProvider {
    pub fn new(config: HashMap<String, String>) -> Result<Self> {
        let api_token = if let Some(token) = config.get("api_token") {
            token.clone()
        } else if let Ok(token) = std::env::var("HETZNER_TOKEN") {
            token
        } else {
            anyhow::bail!("Hetzner API token not found in config or HETZNER_TOKEN env var");
        };

        let client = Client::builder()
            .user_agent("airstack/0.1.0")
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            client,
            api_token,
            base_url: "https://api.hetzner.cloud/v1".to_string(),
        })
    }

    fn convert_status(status: &str) -> ServerStatus {
        match status {
            "initializing" | "starting" => ServerStatus::Creating,
            "running" => ServerStatus::Running,
            "stopping" | "off" => ServerStatus::Stopped,
            "deleting" => ServerStatus::Deleting,
            _ => ServerStatus::Error,
        }
    }

    fn convert_server(hetzner_server: HetznerServer) -> Server {
        Server {
            id: hetzner_server.id.to_string(),
            name: hetzner_server.name,
            status: Self::convert_status(&hetzner_server.status),
            public_ip: hetzner_server.public_net.ipv4.map(|ip| ip.ip),
            private_ip: hetzner_server.private_net.first().map(|net| net.ip.clone()),
            server_type: hetzner_server.server_type.name,
            region: hetzner_server.datacenter.location.name,
        }
    }
}

#[async_trait::async_trait]
impl MetalProvider for HetznerProvider {
    async fn create_server(&self, request: CreateServerRequest) -> Result<Server> {
        info!("Creating Hetzner server: {}", request.name);

        let ssh_key_name = if request.ssh_key.starts_with("~") || request.ssh_key.starts_with("/") {
            let key_id = self
                .upload_ssh_key(&format!("{}-key", request.name), &request.ssh_key)
                .await?;
            key_id
        } else {
            request.ssh_key
        };

        let payload = CreateServerPayload {
            name: request.name.clone(),
            server_type: request.server_type,
            location: request.region,
            ssh_keys: vec![ssh_key_name],
            public_net: CreateServerPublicNet {
                enable_ipv4: true,
                enable_ipv6: false,
            },
        };

        let response = self
            .client
            .post(&format!("{}/servers", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_token))
            .json(&payload)
            .send()
            .await
            .context("Failed to send create server request")?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to create server: {}", error_text);
        }

        let result: HetznerResponse<HetznerServer> = response
            .json()
            .await
            .context("Failed to parse create server response")?;

        let server = result.server.context("No server in response")?;
        let mut converted_server = Self::convert_server(server);

        if request.attach_floating_ip {
            debug!("Attaching floating IP to server: {}", converted_server.id);
            let floating_ip = self.attach_floating_ip(&converted_server.id).await?;
            converted_server.public_ip = Some(floating_ip);
        }

        info!(
            "Successfully created server: {} ({})",
            request.name, converted_server.id
        );
        Ok(converted_server)
    }

    async fn destroy_server(&self, id: &str) -> Result<()> {
        info!("Destroying Hetzner server: {}", id);

        let response = self
            .client
            .delete(&format!("{}/servers/{}", self.base_url, id))
            .header("Authorization", format!("Bearer {}", self.api_token))
            .send()
            .await
            .context("Failed to send destroy server request")?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to destroy server: {}", error_text);
        }

        info!("Successfully destroyed server: {}", id);
        Ok(())
    }

    async fn get_server(&self, id: &str) -> Result<Server> {
        debug!("Getting Hetzner server: {}", id);

        let response = self
            .client
            .get(&format!("{}/servers/{}", self.base_url, id))
            .header("Authorization", format!("Bearer {}", self.api_token))
            .send()
            .await
            .context("Failed to send get server request")?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to get server: {}", error_text);
        }

        let result: HetznerResponse<HetznerServer> = response
            .json()
            .await
            .context("Failed to parse get server response")?;

        let server = result.server.context("No server in response")?;
        Ok(Self::convert_server(server))
    }

    async fn list_servers(&self) -> Result<Vec<Server>> {
        debug!("Listing Hetzner servers");

        let response = self
            .client
            .get(&format!("{}/servers", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_token))
            .send()
            .await
            .context("Failed to send list servers request")?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to list servers: {}", error_text);
        }

        let result: HetznerResponse<HetznerServer> = response
            .json()
            .await
            .context("Failed to parse list servers response")?;

        let servers = result.servers.unwrap_or_default();
        Ok(servers.into_iter().map(Self::convert_server).collect())
    }

    async fn upload_ssh_key(&self, name: &str, public_key_path: &str) -> Result<String> {
        info!("Uploading SSH key: {}", name);

        let expanded_path = if public_key_path.starts_with("~") {
            let home = dirs::home_dir().context("Could not find home directory")?;
            home.join(&public_key_path[2..])
        } else {
            public_key_path.into()
        };

        let public_key = std::fs::read_to_string(&expanded_path)
            .with_context(|| format!("Failed to read SSH public key: {:?}", expanded_path))?;

        let payload = serde_json::json!({
            "name": name,
            "public_key": public_key.trim()
        });

        let response = self
            .client
            .post(&format!("{}/ssh_keys", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_token))
            .json(&payload)
            .send()
            .await
            .context("Failed to send upload SSH key request")?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to upload SSH key: {}", error_text);
        }

        let result: serde_json::Value = response
            .json()
            .await
            .context("Failed to parse upload SSH key response")?;

        let ssh_key_id = result["ssh_key"]["id"]
            .as_u64()
            .context("No SSH key ID in response")?
            .to_string();

        info!("Successfully uploaded SSH key: {} ({})", name, ssh_key_id);
        Ok(ssh_key_id)
    }

    async fn attach_floating_ip(&self, server_id: &str) -> Result<String> {
        info!(
            "Creating and attaching floating IP to server: {}",
            server_id
        );

        let payload = serde_json::json!({
            "type": "assign",
            "assignee": server_id
        });

        let response = self
            .client
            .post(&format!("{}/floating_ips", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_token))
            .json(&payload)
            .send()
            .await
            .context("Failed to send create floating IP request")?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to create floating IP: {}", error_text);
        }

        let result: serde_json::Value = response
            .json()
            .await
            .context("Failed to parse create floating IP response")?;

        let floating_ip = result["floating_ip"]["ip"]
            .as_str()
            .context("No floating IP in response")?
            .to_string();

        info!("Successfully attached floating IP: {}", floating_ip);
        Ok(floating_ip)
    }
}
