use crate::{
    CapacityResolveOptions, CreateRequestValidation, CreateServerRequest, MetalProvider,
    ProviderCapabilities, Server, ServerStatus,
};
use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
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

#[derive(Debug, Serialize, Deserialize)]
struct HetznerSshKey {
    id: u64,
    name: String,
    public_key: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct HetznerSshKeysResponse {
    ssh_keys: Option<Vec<HetznerSshKey>>,
}

#[derive(Debug, Serialize)]
struct CreateServerPayload {
    name: String,
    server_type: String,
    location: String,
    image: String,
    ssh_keys: Vec<String>,
    public_net: CreateServerPublicNet,
}

#[derive(Debug, Serialize)]
struct CreateServerPublicNet {
    enable_ipv4: bool,
    enable_ipv6: bool,
}

impl HetznerProvider {
    const DEFAULT_REGION: &'static str = "ash";
    const PREFERRED_REGIONS: [&'static str; 5] = ["ash", "hel1", "nbg1", "fsn1", "hil"];

    pub fn new(config: HashMap<String, String>) -> Result<Self> {
        let api_token = if let Some(token) = config.get("api_token") {
            token.clone()
        } else if let Ok(token) = std::env::var("HETZNER_API_KEY") {
            token
        } else if let Ok(token) = std::env::var("HETZNER_API_TOKEN") {
            token
        } else if let Ok(token) = std::env::var("HETZNER_TOKEN") {
            token
        } else {
            anyhow::bail!(
                "Hetzner API token not found in config or env vars HETZNER_API_KEY/HETZNER_API_TOKEN/HETZNER_TOKEN"
            );
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

    async fn find_existing_ssh_key_id(
        &self,
        name: &str,
        public_key: &str,
    ) -> Result<Option<String>> {
        let response = self
            .client
            .get(format!("{}/ssh_keys", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_token))
            .send()
            .await
            .context("Failed to send list SSH keys request")?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to list SSH keys: {}", error_text);
        }

        let result: HetznerSshKeysResponse = response
            .json()
            .await
            .context("Failed to parse list SSH keys response")?;

        let found = result
            .ssh_keys
            .unwrap_or_default()
            .into_iter()
            .find(|k| k.name == name || k.public_key.trim() == public_key);

        Ok(found.map(|k| k.id.to_string()))
    }

    async fn fetch_type_region_matrix(
        &self,
    ) -> Result<(
        BTreeMap<String, BTreeSet<String>>,
        BTreeMap<String, BTreeSet<String>>,
    )> {
        let response = self
            .client
            .get(format!("{}/server_types", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_token))
            .send()
            .await
            .context("Failed to send server_types request")?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to query Hetzner server types: {}", error_text);
        }

        let value: serde_json::Value = response
            .json()
            .await
            .context("Failed to parse Hetzner server types response")?;
        let mut by_type: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut by_region: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

        let Some(server_types) = value.get("server_types").and_then(|v| v.as_array()) else {
            return Ok((by_type, by_region));
        };

        for item in server_types {
            let Some(type_name) = item.get("name").and_then(|v| v.as_str()) else {
                continue;
            };
            let entry = by_type.entry(type_name.to_string()).or_default();

            if let Some(prices) = item.get("prices").and_then(|v| v.as_array()) {
                for price in prices {
                    let loc = price.get("location");
                    let region = loc
                        .and_then(|l| l.get("name"))
                        .and_then(|v| v.as_str())
                        .or_else(|| loc.and_then(|v| v.as_str()));
                    if let Some(region) = region {
                        entry.insert(region.to_string());
                        by_region
                            .entry(region.to_string())
                            .or_default()
                            .insert(type_name.to_string());
                    }
                }
            }
        }

        Ok((by_type, by_region))
    }

    fn choose_preferred_region(available: &[String]) -> Option<String> {
        for preferred in Self::PREFERRED_REGIONS {
            if available.iter().any(|r| r == preferred) {
                return Some(preferred.to_string());
            }
        }
        available.first().cloned()
    }
}

#[async_trait::async_trait]
impl MetalProvider for HetznerProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_public_ip: true,
            supports_direct_ssh: true,
            supports_provider_ssh: false,
            supports_server_create: true,
            supports_server_destroy: true,
        }
    }

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
            // Hetzner API requires image in create payload.
            image: "ubuntu-24.04".to_string(),
            ssh_keys: vec![ssh_key_name],
            public_net: CreateServerPublicNet {
                enable_ipv4: true,
                enable_ipv6: false,
            },
        };

        let response = self
            .client
            .post(format!("{}/servers", self.base_url))
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

    async fn validate_create_request(
        &self,
        request: &CreateServerRequest,
    ) -> Result<CreateRequestValidation> {
        let (by_type, by_region) = self.fetch_type_region_matrix().await?;

        let type_regions = by_type
            .get(&request.server_type)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect::<Vec<_>>();
        let region_types = if request.region.is_empty() {
            Vec::new()
        } else {
            by_region
                .get(&request.region)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect::<Vec<_>>()
        };

        if !by_type.contains_key(&request.server_type) {
            let suggested_server_type = by_region
                .get(&request.region)
                .and_then(|types| types.iter().next().cloned())
                .or_else(|| by_type.keys().next().cloned());
            return Ok(CreateRequestValidation {
                valid: false,
                reason: Some(format!("unsupported server_type '{}'", request.server_type)),
                valid_regions_for_type: type_regions,
                valid_server_types_for_region: region_types,
                suggested_region: Some(Self::DEFAULT_REGION.to_string()),
                suggested_server_type,
                permanent: true,
            });
        }

        let region = if request.region.is_empty() {
            Self::DEFAULT_REGION.to_string()
        } else {
            request.region.clone()
        };
        let valid = by_type
            .get(&request.server_type)
            .is_some_and(|regions| regions.contains(&region));
        let suggested_region = if valid {
            None
        } else {
            Self::choose_preferred_region(&type_regions)
        };
        Ok(CreateRequestValidation {
            valid,
            reason: if valid {
                None
            } else {
                Some(format!(
                    "server_type '{}' is not available in region '{}'",
                    request.server_type, region
                ))
            },
            valid_regions_for_type: type_regions,
            valid_server_types_for_region: region_types,
            suggested_region,
            suggested_server_type: None,
            permanent: !valid,
        })
    }

    async fn resolve_create_request(
        &self,
        request: &CreateServerRequest,
        opts: CapacityResolveOptions,
    ) -> Result<CreateServerRequest> {
        let mut resolved = request.clone();
        if resolved.region.is_empty() {
            resolved.region = Self::DEFAULT_REGION.to_string();
        }

        if resolved.region == "auto" || opts.resolve_capacity {
            let validation = self
                .validate_create_request(&CreateServerRequest {
                    region: Self::DEFAULT_REGION.to_string(),
                    ..resolved.clone()
                })
                .await?;
            if let Some(region) = validation
                .suggested_region
                .or_else(|| Self::choose_preferred_region(&validation.valid_regions_for_type))
            {
                resolved.region = region;
            } else if resolved.region == "auto" {
                resolved.region = Self::DEFAULT_REGION.to_string();
            }
        }

        let validation = self.validate_create_request(&resolved).await?;
        if validation.valid {
            return Ok(resolved);
        }

        if opts.resolve_capacity || opts.auto_fallback {
            if let Some(region) = validation.suggested_region {
                resolved.region = region;
                return Ok(resolved);
            }
        }

        Ok(resolved)
    }

    async fn destroy_server(&self, id: &str) -> Result<()> {
        info!("Destroying Hetzner server: {}", id);

        let response = self
            .client
            .delete(format!("{}/servers/{}", self.base_url, id))
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
            .get(format!("{}/servers/{}", self.base_url, id))
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
            .get(format!("{}/servers", self.base_url))
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
            .post(format!("{}/ssh_keys", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_token))
            .json(&payload)
            .send()
            .await
            .context("Failed to send upload SSH key request")?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            if error_text.contains("uniqueness_error") {
                if let Some(existing_id) = self
                    .find_existing_ssh_key_id(name, public_key.trim())
                    .await?
                {
                    info!(
                        "SSH key already exists; reusing id {} for key {}",
                        existing_id, name
                    );
                    return Ok(existing_id);
                }
            }
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
            .post(format!("{}/floating_ips", self.base_url))
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
