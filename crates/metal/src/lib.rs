use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod fly;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirewallSpec {
    pub name: String,
    pub rules: Vec<FirewallRuleSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirewallRuleSpec {
    pub protocol: String,
    pub port: Option<String>,
    pub source_ips: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub supports_public_ip: bool,
    pub supports_direct_ssh: bool,
    pub supports_provider_ssh: bool,
    pub supports_server_create: bool,
    pub supports_server_destroy: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRequestValidation {
    pub valid: bool,
    pub reason: Option<String>,
    pub valid_regions_for_type: Vec<String>,
    pub valid_server_types_for_region: Vec<String>,
    pub suggested_region: Option<String>,
    pub suggested_server_type: Option<String>,
    pub permanent: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CapacityResolveOptions {
    pub auto_fallback: bool,
    pub resolve_capacity: bool,
}

#[async_trait::async_trait]
pub trait MetalProvider: Send + Sync {
    fn capabilities(&self) -> ProviderCapabilities;
    async fn create_server(&self, request: CreateServerRequest) -> Result<Server>;
    async fn destroy_server(&self, id: &str) -> Result<()>;
    async fn get_server(&self, id: &str) -> Result<Server>;
    async fn list_servers(&self) -> Result<Vec<Server>>;
    async fn upload_ssh_key(&self, name: &str, public_key_path: &str) -> Result<String>;
    async fn attach_floating_ip(&self, server_id: &str) -> Result<String>;
    async fn ensure_firewall(&self, _spec: &FirewallSpec) -> Result<Option<String>> {
        Ok(None)
    }
    async fn attach_firewall_to_server(&self, _firewall_id: &str, _server_id: &str) -> Result<()> {
        Ok(())
    }
    async fn validate_create_request(
        &self,
        _request: &CreateServerRequest,
    ) -> Result<CreateRequestValidation> {
        Ok(CreateRequestValidation {
            valid: true,
            reason: None,
            valid_regions_for_type: Vec::new(),
            valid_server_types_for_region: Vec::new(),
            suggested_region: None,
            suggested_server_type: None,
            permanent: false,
        })
    }
    async fn resolve_create_request(
        &self,
        request: &CreateServerRequest,
        _opts: CapacityResolveOptions,
    ) -> Result<CreateServerRequest> {
        Ok(request.clone())
    }
}

pub fn get_provider(
    provider_name: &str,
    config: HashMap<String, String>,
) -> Result<Box<dyn MetalProvider>> {
    match provider_name {
        "hetzner" => Ok(Box::new(hetzner::HetznerProvider::new(config)?)),
        "fly" => Ok(Box::new(fly::FlyProvider::new(config)?)),
        _ => anyhow::bail!("Unsupported metal provider: {}", provider_name),
    }
}

#[cfg(test)]
mod tests {
    use super::get_provider;
    use std::collections::HashMap;

    #[test]
    fn rejects_unsupported_provider() {
        let err = match get_provider("nope", HashMap::new()) {
            Ok(_) => panic!("expected unsupported provider error"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("Unsupported metal provider"),
            "unexpected error: {err}"
        );
    }
}
