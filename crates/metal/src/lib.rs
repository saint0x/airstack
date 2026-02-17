use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod hetzner;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Server {
    pub id: String,
    pub name: String,
    pub status: ServerStatus,
    pub public_ip: Option<String>,
    pub private_ip: Option<String>,
    pub server_type: String,
    pub region: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerStatus {
    Creating,
    Running,
    Stopped,
    Deleting,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateServerRequest {
    pub name: String,
    pub server_type: String,
    pub region: String,
    pub ssh_key: String,
    pub attach_floating_ip: bool,
}

#[async_trait::async_trait]
pub trait MetalProvider: Send + Sync {
    async fn create_server(&self, request: CreateServerRequest) -> Result<Server>;
    async fn destroy_server(&self, id: &str) -> Result<()>;
    async fn get_server(&self, id: &str) -> Result<Server>;
    async fn list_servers(&self) -> Result<Vec<Server>>;
    async fn upload_ssh_key(&self, name: &str, public_key_path: &str) -> Result<String>;
    async fn attach_floating_ip(&self, server_id: &str, region: &str) -> Result<String>;
}

pub fn get_provider(
    provider_name: &str,
    config: HashMap<String, String>,
) -> Result<Box<dyn MetalProvider>> {
    match provider_name {
        "hetzner" => Ok(Box::new(hetzner::HetznerProvider::new(config)?)),
        _ => anyhow::bail!("Unsupported metal provider: {}", provider_name),
    }
}
