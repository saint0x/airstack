use crate::output;
use crate::ssh_utils::{resolve_identity_path, resolve_server_public_ip};
use airstack_config::AirstackConfig;
use anyhow::{Context, Result};
use serde::Serialize;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct UploadArgs {
    pub target: String,
    pub source: String,
    pub destination: String,
    pub checksum: Option<String>,
    pub mode: Option<String>,
    pub no_atomic: bool,
}

#[derive(Debug, Serialize)]
struct UploadOutput {
    target: String,
    source: String,
    destination: String,
    checksum: String,
    atomic: bool,
}

pub async fn run(config_path: &str, args: UploadArgs) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let infra = config
        .infra
        .as_ref()
        .context("No infrastructure defined in configuration")?;
    let server = infra
        .servers
        .iter()
        .find(|s| s.name == args.target)
        .with_context(|| format!("Server '{}' not found in configuration", args.target))?;

    let source = Path::new(&args.source);
    if !source.exists() {
        anyhow::bail!("Source file not found: {}", source.display());
    }
    if !source.is_file() {
        anyhow::bail!("Source path must be a file: {}", source.display());
    }

    if server.provider == "fly" {
        anyhow::bail!(
            "airstack upload/cp is not yet supported for provider=fly. Use `airstack ssh {} --cmd \"cat > {}\" < file` or provider-native transfer tooling.",
            args.target,
            args.destination
        );
    }

    let checksum = if let Some(c) = &args.checksum {
        c.to_ascii_lowercase()
    } else {
        compute_sha256(source)?
    };

    let server_ip = resolve_server_public_ip(server).await?;
    let remote_tmp = format!(
        "{}.airstack-upload-{}",
        args.destination,
        std::process::id()
    );

    run_remote_shell(
        server,
        &format!(
            "set -e; mkdir -p \"$(dirname {dest})\"",
            dest = shell_quote(&args.destination)
        ),
    )
    .await?;

    scp_file(server, &server_ip, &args.source, &remote_tmp)?;

    let verify_cmd = format!(
        "set -e; actual=''; if command -v sha256sum >/dev/null 2>&1; then actual=$(sha256sum {tmp} | awk '{{print $1}}'); else actual=$(shasum -a 256 {tmp} | awk '{{print $1}}'); fi; test \"$actual\" = \"{expected}\"",
        tmp = shell_quote(&remote_tmp),
        expected = checksum
    );
    run_remote_shell(server, &verify_cmd)
        .await
        .with_context(|| {
            format!(
                "Checksum verification failed for uploaded file '{}'.",
                remote_tmp
            )
        })?;

    let mode_clause = args
        .mode
        .as_ref()
        .map(|m| {
            format!(
                "; chmod {} {target}",
                shell_quote(m),
                target = shell_quote(&args.destination)
            )
        })
        .unwrap_or_default();

    if args.no_atomic {
        run_remote_shell(
            server,
            &format!(
                "set -e; cp {tmp} {dest}; rm -f {tmp}{mode}",
                tmp = shell_quote(&remote_tmp),
                dest = shell_quote(&args.destination),
                mode = mode_clause,
            ),
        )
        .await?;
    } else {
        run_remote_shell(
            server,
            &format!(
                "set -e; mv -f {tmp} {dest}{mode}",
                tmp = shell_quote(&remote_tmp),
                dest = shell_quote(&args.destination),
                mode = mode_clause,
            ),
        )
        .await?;
    }

    if output::is_json() {
        output::emit_json(&UploadOutput {
            target: args.target,
            source: args.source,
            destination: args.destination,
            checksum,
            atomic: !args.no_atomic,
        })?;
        return Ok(());
    }

    output::line(format!(
        "✅ uploaded {} -> {}:{} (sha256={}, atomic={})",
        source.display(),
        server.name,
        args.destination,
        checksum,
        !args.no_atomic
    ));

    Ok(())
}

fn scp_file(
    server: &airstack_config::ServerConfig,
    ip: &str,
    source: &str,
    remote_path: &str,
) -> Result<()> {
    let mut cmd = Command::new("scp");
    cmd.args([
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=no",
        "-o",
        "UserKnownHostsFile=/dev/null",
        "-o",
        "LogLevel=ERROR",
    ]);

    if let Some(identity_path) = resolve_identity_path(&server.ssh_key)? {
        cmd.args(["-i", &identity_path.to_string_lossy()]);
    }

    cmd.arg(source);
    cmd.arg(format!("root@{}:{}", ip, remote_path));

    let status = cmd.status().context("Failed to run scp")?;
    if !status.success() {
        anyhow::bail!("scp failed uploading '{}'", source);
    }
    Ok(())
}

async fn run_remote_shell(server: &airstack_config::ServerConfig, script: &str) -> Result<()> {
    let out = crate::ssh_utils::execute_remote_shell_command(server, script).await?;
    if out.status.success() {
        return Ok(());
    }
    anyhow::bail!(
        "Remote command failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    )
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || "-_./:".contains(ch))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn compute_sha256(path: &Path) -> Result<String> {
    let quoted = shell_quote(path.to_str().unwrap_or_default());
    let mut cmd = Command::new("sh");
    cmd.arg("-lc").arg(format!(
        "if command -v sha256sum >/dev/null 2>&1; then sha256sum {p} | awk '{{print $1}}'; else shasum -a 256 {p} | awk '{{print $1}}'; fi",
        p = quoted
    ));
    let out = cmd.output().context("Failed to compute local sha256")?;
    if !out.status.success() {
        anyhow::bail!(
            "Failed to compute local sha256: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .trim()
        .to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::shell_quote;

    #[test]
    fn shell_quote_handles_spaces() {
        assert_eq!(shell_quote("a b"), "'a b'");
    }
}
