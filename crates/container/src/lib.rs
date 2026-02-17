use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod docker;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: ContainerStatus,
    pub ports: Vec<PortMapping>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContainerStatus {
    Creating,
    Running,
    Stopped,
    Paused,
    Restarting,
    Removing,
    Dead,
    Exited,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortMapping {
    pub container_port: u16,
    pub host_port: Option<u16>,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunServiceRequest {
    pub name: String,
    pub image: String,
    pub ports: Vec<u16>,
    pub env: Option<HashMap<String, String>>,
    pub volumes: Option<Vec<String>>,
    pub restart_policy: Option<String>,
}

#[async_trait::async_trait]
pub trait ContainerProvider: Send + Sync {
    async fn build_image(&self, path: &str, tag: &str) -> Result<()>;
    async fn run_service(&self, request: RunServiceRequest) -> Result<Container>;
    async fn stop_service(&self, name: &str) -> Result<()>;
    async fn get_container(&self, name: &str) -> Result<Container>;
    async fn list_containers(&self) -> Result<Vec<Container>>;
    async fn logs(&self, name: &str, follow: bool) -> Result<Vec<String>>;
    async fn exec(&self, name: &str, command: Vec<String>) -> Result<String>;
}

pub fn get_provider(provider_name: &str) -> Result<Box<dyn ContainerProvider>> {
    match provider_name {
        "docker" => Ok(Box::new(docker::DockerProvider::new()?)),
        _ => anyhow::bail!("Unsupported container provider: {}", provider_name),
    }
}
