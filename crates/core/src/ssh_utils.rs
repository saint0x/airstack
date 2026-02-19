use airstack_config::ServerConfig;
use airstack_metal::{get_provider as get_metal_provider, Server};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Output};

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

fn shell_escape(arg: &str) -> String {
    if arg.is_empty() {
        return "''".to_string();
    }
    if arg
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || "-_./:".contains(ch))
    {
        return arg.to_string();
    }
    format!("'{}'", arg.replace('\'', "'\"'\"'"))
}

pub fn join_shell_command(parts: &[String]) -> String {
    parts
        .iter()
        .map(|part| shell_escape(part))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn parse_fly_server_id(id: &str) -> Option<(String, Option<String>)> {
    let rest = id.strip_prefix("fly:")?;
    let mut parts = rest.splitn(2, ':');
    let app = parts.next()?.to_string();
    if app.is_empty() {
        return None;
    }
    let machine = parts.next().map(|m| m.to_string());
    Some((app, machine))
}

pub async fn resolve_fly_target(server_cfg: &ServerConfig) -> Result<(String, Option<String>)> {
    let server = lookup_provider_server(server_cfg).await?;
    parse_fly_server_id(&server.id).with_context(|| {
        format!(
            "Fly server '{}' had unexpected id format '{}'",
            server_cfg.name, server.id
        )
    })
}

pub async fn execute_remote_command(
    server_cfg: &ServerConfig,
    command: &[String],
) -> Result<Output> {
    if server_cfg.provider == "fly" {
        let (app, machine) = resolve_fly_target(server_cfg).await?;
        let cmd_string = join_shell_command(command);

        let mut fly_cmd = Command::new("flyctl");
        fly_cmd.arg("ssh");
        fly_cmd.arg("console");
        fly_cmd.arg("--app");
        fly_cmd.arg(app);
        if let Some(machine) = machine {
            fly_cmd.arg("--machine");
            fly_cmd.arg(machine);
        }
        fly_cmd.arg("--command");
        fly_cmd.arg(cmd_string);

        return fly_cmd
            .output()
            .context("Failed to execute Fly SSH command");
    }

    let ip = resolve_server_public_ip(server_cfg).await?;
    let mut ssh_cmd = build_ssh_command(
        &server_cfg.ssh_key,
        &ip,
        &SshCommandOptions {
            user: "root",
            batch_mode: false,
            connect_timeout_secs: None,
            strict_host_key_checking: "no",
            user_known_hosts_file: Some("/dev/null"),
            log_level: "ERROR",
        },
    )?;
    ssh_cmd.args(command);
    ssh_cmd.output().context("Failed to execute SSH command")
}

pub async fn start_remote_session(server_cfg: &ServerConfig, command: &[String]) -> Result<i32> {
    if server_cfg.provider == "fly" {
        let (app, machine) = resolve_fly_target(server_cfg).await?;

        let mut fly_cmd = Command::new("flyctl");
        fly_cmd.arg("ssh");
        fly_cmd.arg("console");
        fly_cmd.arg("--app");
        fly_cmd.arg(app);
        if let Some(machine) = machine {
            fly_cmd.arg("--machine");
            fly_cmd.arg(machine);
        }
        if !command.is_empty() {
            fly_cmd.arg("--command");
            fly_cmd.arg(join_shell_command(command));
        }
        let status = fly_cmd
            .status()
            .context("Failed to start Fly SSH session")?;
        return Ok(status.code().unwrap_or(1));
    }

    let ip = resolve_server_public_ip(server_cfg).await?;
    let mut ssh_cmd = build_ssh_command(
        &server_cfg.ssh_key,
        &ip,
        &SshCommandOptions {
            user: "root",
            batch_mode: false,
            connect_timeout_secs: None,
            strict_host_key_checking: "no",
            user_known_hosts_file: Some("/dev/null"),
            log_level: "ERROR",
        },
    )?;
    ssh_cmd.args(command);
    let status = ssh_cmd.status().context("Failed to start SSH session")?;
    Ok(status.code().unwrap_or(1))
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

#[cfg(test)]
mod tests {
    use super::{
        build_ssh_command, join_shell_command, parse_fly_server_id, resolve_identity_path,
        SshCommandOptions,
    };
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_dir() -> std::path::PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        std::env::temp_dir().join(format!("airstack-ssh-utils-{now}"))
    }

    #[test]
    fn resolve_identity_path_ignores_non_path_keys() {
        let resolved = resolve_identity_path("my-key-name").expect("resolution should not fail");
        assert!(resolved.is_none());
    }

    #[test]
    fn resolve_identity_path_prefers_private_key_for_pub_path() {
        let dir = unique_dir();
        fs::create_dir_all(&dir).expect("temp dir creation should succeed");
        let private = dir.join("id_ed25519");
        let public = dir.join("id_ed25519.pub");
        fs::write(&private, "PRIVATE").expect("private key write should succeed");
        fs::write(&public, "PUBLIC").expect("public key write should succeed");

        let resolved = resolve_identity_path(&public.to_string_lossy())
            .expect("resolution should not fail")
            .expect("private key should be selected");
        assert_eq!(resolved, private);

        fs::remove_dir_all(&dir).expect("temp dir cleanup should succeed");
    }

    #[test]
    fn build_ssh_command_includes_target_and_options() {
        let cmd = build_ssh_command(
            "",
            "203.0.113.10",
            &SshCommandOptions {
                user: "root",
                batch_mode: true,
                connect_timeout_secs: Some(7),
                strict_host_key_checking: "accept-new",
                user_known_hosts_file: None,
                log_level: "ERROR",
            },
        )
        .expect("command build should succeed");

        let args: Vec<String> = cmd
            .get_args()
            .map(|v| v.to_string_lossy().to_string())
            .collect();

        assert!(args.contains(&"BatchMode=yes".to_string()));
        assert!(args.contains(&"ConnectTimeout=7".to_string()));
        assert!(args.contains(&"StrictHostKeyChecking=accept-new".to_string()));
        assert!(args.contains(&"LogLevel=ERROR".to_string()));
        assert!(
            args.last().is_some_and(|last| last == "root@203.0.113.10"),
            "expected target at end, args: {args:?}"
        );
    }

    #[test]
    fn join_shell_command_quotes_arguments() {
        let cmd = join_shell_command(&[
            "docker".to_string(),
            "exec".to_string(),
            "name with spaces".to_string(),
        ]);
        assert_eq!(cmd, "docker exec 'name with spaces'");
    }

    #[test]
    fn parse_fly_server_id_parses_app_and_machine() {
        let parsed = parse_fly_server_id("fly:my-app:abc123").expect("id should parse");
        assert_eq!(parsed.0, "my-app");
        assert_eq!(parsed.1.as_deref(), Some("abc123"));
    }
}
