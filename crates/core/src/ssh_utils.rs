use airstack_config::ServerConfig;
use airstack_metal::{get_provider as get_metal_provider, Server};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct SshCommandOptions<'a> {
    pub user: &'a str,
    pub batch_mode: bool,
    pub connect_timeout_secs: Option<u64>,
    pub strict_host_key_checking: &'a str,
    pub user_known_hosts_file: Option<&'a str>,
    pub log_level: &'a str,
}

pub fn build_ssh_command(
    ssh_key: &str,
    ip: &str,
    options: &SshCommandOptions<'_>,
) -> Result<Command> {
    let mut ssh_cmd = Command::new("ssh");

    if options.batch_mode {
        ssh_cmd.args(["-o", "BatchMode=yes"]);
    }
    if let Some(timeout) = options.connect_timeout_secs {
        ssh_cmd.args(["-o", &format!("ConnectTimeout={timeout}")]);
    }
    ssh_cmd.args([
        "-o",
        &format!("StrictHostKeyChecking={}", options.strict_host_key_checking),
    ]);
    if let Some(path) = options.user_known_hosts_file {
        ssh_cmd.args(["-o", &format!("UserKnownHostsFile={path}")]);
    }
    ssh_cmd.args(["-o", &format!("LogLevel={}", options.log_level)]);

    if let Some(identity_path) = resolve_identity_path(ssh_key)? {
        ssh_cmd.args(["-i", &identity_path.to_string_lossy()]);
    }

    ssh_cmd.arg(format!("{}@{}", options.user, ip));
    Ok(ssh_cmd)
}

pub async fn lookup_provider_server(server_cfg: &ServerConfig) -> Result<Server> {
    let metal_provider = get_metal_provider(&server_cfg.provider, HashMap::new())
        .with_context(|| format!("Failed to initialize {} provider", server_cfg.provider))?;
    let servers = metal_provider
        .list_servers()
        .await
        .context("Failed to list servers from provider")?;
    servers
        .into_iter()
        .find(|s| s.name == server_cfg.name)
        .with_context(|| format!("Server '{}' not found in provider", server_cfg.name))
}

pub async fn resolve_server_public_ip(server_cfg: &ServerConfig) -> Result<String> {
    lookup_provider_server(server_cfg)
        .await?
        .public_ip
        .context("Server has no public IP address")
}

pub fn resolve_identity_path(ssh_key: &str) -> Result<Option<PathBuf>> {
    if ssh_key.is_empty() {
        return Ok(None);
    }

    if !(ssh_key.starts_with("~") || ssh_key.starts_with("/")) {
        return Ok(None);
    }

    let path = if ssh_key.starts_with("~") {
        let home = dirs::home_dir().context("Could not resolve home directory")?;
        home.join(&ssh_key[2..])
    } else {
        PathBuf::from(ssh_key)
    };

    if path.extension().is_some_and(|ext| ext == "pub") {
        let mut private = path.clone();
        private.set_extension("");
        if private.exists() {
            return Ok(Some(private));
        }
    }

    if path.exists() {
        return Ok(Some(path));
    }

    Ok(None)
}
