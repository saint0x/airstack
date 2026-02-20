use crate::ssh_utils::resolve_identity_path;
use airstack_config::ServerConfig;
use airstack_metal::{
    get_provider as get_metal_provider, CapacityResolveOptions, CreateRequestValidation,
    CreateServerRequest,
};
use anyhow::{Context, Result};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ServerPreflight {
    pub request: CreateServerRequest,
    pub validation: CreateRequestValidation,
}

pub async fn resolve_server_request(
    server: &ServerConfig,
    opts: CapacityResolveOptions,
) -> Result<ServerPreflight> {
    let provider = get_metal_provider(&server.provider, HashMap::new())
        .with_context(|| format!("Failed to initialize provider '{}'", server.provider))?;
    let request = CreateServerRequest {
        name: server.name.clone(),
        server_type: server.server_type.clone(),
        region: server.region.clone(),
        ssh_key: server.ssh_key.clone(),
        attach_floating_ip: server.floating_ip.unwrap_or(false),
    };
    let resolved = provider.resolve_create_request(&request, opts).await?;
    let validation = provider.validate_create_request(&resolved).await?;
    Ok(ServerPreflight {
        request: resolved,
        validation,
    })
}

pub fn format_validation_error(server: &ServerConfig, pre: &ServerPreflight) -> String {
    let mut parts = Vec::new();
    parts.push(format!(
        "infra '{}': {}",
        server.name,
        pre.validation
            .reason
            .clone()
            .unwrap_or_else(|| "invalid provider request".to_string())
    ));
    if !pre.validation.valid_regions_for_type.is_empty() {
        parts.push(format!(
            "valid regions for server_type '{}': {}",
            pre.request.server_type,
            pre.validation.valid_regions_for_type.join(", ")
        ));
    }
    if !pre.validation.valid_server_types_for_region.is_empty() {
        parts.push(format!(
            "valid server types for region '{}': {}",
            pre.request.region,
            pre.validation.valid_server_types_for_region.join(", ")
        ));
    }
    if let Some(suggested) = &pre.validation.suggested_region {
        parts.push(format!("suggested patch: region={}", suggested));
    }
    if let Some(suggested) = &pre.validation.suggested_server_type {
        parts.push(format!("suggested patch: server_type={}", suggested));
    }
    parts.join(" | ")
}

pub fn check_ssh_key_path(server: &ServerConfig) -> Result<()> {
    if resolve_identity_path(&server.ssh_key)?.is_none() {
        anyhow::bail!(
            "infra '{}': ssh_key path '{}' not found",
            server.name,
            server.ssh_key
        );
    }
    Ok(())
}

pub fn is_permanent_provider_error(err: &anyhow::Error) -> bool {
    let msg = err.to_string().to_ascii_lowercase();
    msg.contains("invalid_input")
        || msg.contains("unsupported")
        || msg.contains("not available")
        || msg.contains("unknown server type")
        || msg.contains("invalid location")
        || msg.contains("forbidden")
        || msg.contains("unauthorized")
        || msg.contains("authentication")
}
