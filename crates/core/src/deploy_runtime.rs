use crate::ssh_utils::{execute_remote_command, join_shell_command};
use airstack_config::{
    AirstackConfig, HealthcheckConfig, HttpHealthcheckConfig, ServerConfig, ServiceConfig,
    TcpHealthcheckConfig,
};
use anyhow::{Context, Result};
use serde::Serialize;
use std::process::Output;
use tokio::time::{sleep, Duration};

#[derive(Debug, Clone)]
pub enum RuntimeTarget {
    Local,
    Remote(ServerConfig),
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeDeployResult {
    pub id: String,
    pub status: String,
    pub ports: Vec<String>,
    pub running: bool,
    pub discoverable: bool,
    pub detected_by: String,
    pub healthy: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthProbeRecord {
    pub profile: String,
    pub command: String,
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthEvaluation {
    pub ok: bool,
    pub detail: String,
    pub records: Vec<HealthProbeRecord>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub enum DeployStrategy {
    Rolling,
    BlueGreen,
    Canary,
}

impl DeployStrategy {
    pub fn parse(input: &str) -> Result<Self> {
        match input {
            "rolling" => Ok(Self::Rolling),
            "bluegreen" => Ok(Self::BlueGreen),
            "canary" => Ok(Self::Canary),
            _ => anyhow::bail!(
                "Invalid deploy strategy '{}'. Expected one of: rolling|bluegreen|canary",
                input
            ),
        }
    }
}

pub fn resolve_target(
    config: &AirstackConfig,
    service: &ServiceConfig,
    allow_local_deploy: bool,
) -> Result<RuntimeTarget> {
    let infra = config.infra.as_ref();
    let infra_present = infra.is_some_and(|i| !i.servers.is_empty());

    let deploy_mode = config
        .project
        .deploy_mode
        .as_deref()
        .unwrap_or(if infra_present { "remote" } else { "local" });

    match deploy_mode {
        "local" => {
            if infra_present && !allow_local_deploy {
                anyhow::bail!(
                    "Unsafe local deploy blocked: infra servers exist. Use remote deploy mode or pass --allow-local-deploy"
                );
            }
            Ok(RuntimeTarget::Local)
        }
        "remote" => {
            let infra =
                infra.context("Remote deploy mode selected but no infra.servers configured")?;
            let target_name = service
                .target_server
                .clone()
                .or_else(|| infra.servers.first().map(|s| s.name.clone()))
                .context("Remote deploy mode requires at least one infra server")?;
            let server = infra
                .servers
                .iter()
                .find(|s| s.name == target_name)
                .with_context(|| {
                    format!("target server '{}' not found in infra.servers", target_name)
                })?
                .clone();
            if server.provider == "fly" {
                anyhow::bail!(
                    "Remote service deploy to provider='fly' is not supported via docker runtime. Use Fly-native deploy flow"
                );
            }
            Ok(RuntimeTarget::Remote(server))
        }
        _ => anyhow::bail!(
            "Invalid deploy mode '{}'. Expected local|remote",
            deploy_mode
        ),
    }
}

pub async fn existing_service_image(target: &RuntimeTarget, name: &str) -> Result<Option<String>> {
    let output = run_shell(
        target,
        &format!("docker inspect -f '{{{{.Config.Image}}}}' {name} 2>/dev/null || true"),
    )
    .await?;

    if !output.status.success() {
        return Ok(None);
    }

    let image = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if image.is_empty() {
        Ok(None)
    } else {
        Ok(Some(image))
    }
}

pub async fn deploy_service(
    target: &RuntimeTarget,
    name: &str,
    service: &ServiceConfig,
) -> Result<RuntimeDeployResult> {
    preflight_image_access(target, &service.image).await?;

    let mut run_parts = vec![
        "docker".to_string(),
        "run".to_string(),
        "-d".to_string(),
        "--name".to_string(),
        name.to_string(),
        "--restart".to_string(),
        "unless-stopped".to_string(),
    ];

    for port in &service.ports {
        run_parts.push("-p".to_string());
        run_parts.push(format!("{}:{}", port, port));
    }

    if let Some(env) = &service.env {
        for (key, value) in env {
            run_parts.push("-e".to_string());
            run_parts.push(format!("{}={}", key, value));
        }
    }

    if let Some(vols) = &service.volumes {
        for volume in vols {
            run_parts.push("-v".to_string());
            run_parts.push(volume.clone());
        }
    }

    run_parts.push(service.image.clone());

    let script = format!(
        "docker rm -f {name} >/dev/null 2>&1 || true; \
         for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do \
           docker container inspect {name} >/dev/null 2>&1 || break; \
           docker rm -f {name} >/dev/null 2>&1 || true; \
           sleep 0.2; \
         done; \
         {}",
        join_shell_command(&run_parts)
    );

    let run_out = run_shell(target, &script).await?;
    if !run_out.status.success() {
        let stderr = String::from_utf8_lossy(&run_out.stderr);
        anyhow::bail!("Failed to deploy service '{}': {}", name, stderr.trim());
    }

    let launched_id = String::from_utf8_lossy(&run_out.stdout).trim().to_string();
    inspect_service(target, name, Some(launched_id)).await
}

pub async fn deploy_service_with_strategy(
    target: &RuntimeTarget,
    name: &str,
    service: &ServiceConfig,
    healthcheck: Option<&HealthcheckConfig>,
    strategy: DeployStrategy,
    canary_seconds: u64,
) -> Result<RuntimeDeployResult> {
    match strategy {
        DeployStrategy::Rolling => deploy_service(target, name, service).await,
        DeployStrategy::BlueGreen | DeployStrategy::Canary => {
            // Candidate runs without host port bindings to avoid conflicts while validating the new image.
            let candidate_name = format!("{}__candidate", name);
            let mut candidate = service.clone();
            candidate.ports = Vec::new();

            let _ = deploy_service(target, &candidate_name, &candidate).await?;

            if let Some(hc) = healthcheck {
                let mut health_service = service.clone();
                health_service.healthcheck = Some(hc.clone());
                if let Err(err) = evaluate_service_health(
                    target,
                    &candidate_name,
                    &health_service,
                    false,
                    1,
                    false,
                )
                .await
                .and_then(|eval| {
                    if eval.ok {
                        Ok(())
                    } else {
                        anyhow::bail!("{}", eval.detail)
                    }
                }) {
                    let _ = run_shell(
                        target,
                        &format!("docker rm -f {} >/dev/null 2>&1 || true", candidate_name),
                    )
                    .await;
                    return Err(err).with_context(|| {
                        format!(
                            "Candidate validation failed for '{}' with strategy {:?}",
                            name, strategy
                        )
                    });
                }
            }

            if strategy == DeployStrategy::Canary && canary_seconds > 0 {
                sleep(Duration::from_secs(canary_seconds)).await;
            }

            let promoted = match deploy_service(target, name, service).await {
                Ok(v) => v,
                Err(e) => {
                    let _ = run_shell(
                        target,
                        &format!("docker rm -f {} >/dev/null 2>&1 || true", candidate_name),
                    )
                    .await;
                    return Err(e);
                }
            };

            let _ = run_shell(
                target,
                &format!("docker rm -f {} >/dev/null 2>&1 || true", candidate_name),
            )
            .await;

            Ok(promoted)
        }
    }
}

pub async fn rollback_service(
    target: &RuntimeTarget,
    name: &str,
    previous_image: &str,
    service: &ServiceConfig,
) -> Result<()> {
    let mut rollback_cfg = service.clone();
    rollback_cfg.image = previous_image.to_string();
    let _ = deploy_service(target, name, &rollback_cfg).await?;
    Ok(())
}

pub async fn run_healthcheck(
    target: &RuntimeTarget,
    name: &str,
    healthcheck: &HealthcheckConfig,
) -> Result<()> {
    let service = ServiceConfig {
        image: String::new(),
        ports: Vec::new(),
        env: None,
        volumes: None,
        depends_on: None,
        target_server: None,
        healthcheck: Some(healthcheck.clone()),
        profile: None,
    };
    let evaluation = evaluate_service_health(target, name, &service, false, 1, false).await?;
    if evaluation.ok {
        Ok(())
    } else {
        anyhow::bail!("{}", evaluation.detail)
    }
}

pub async fn run_http_health_probe(
    target: &RuntimeTarget,
    port: u16,
    timeout_secs: Option<u64>,
) -> Result<()> {
    let timeout = timeout_secs.unwrap_or(5);
    let url = format!("http://127.0.0.1:{port}/health");
    let script = format!(
        "(curl -fsS --max-time {timeout} {url} >/dev/null 2>&1) || (wget -q --timeout={timeout} -O- {url} >/dev/null 2>&1)"
    );
    let out = run_shell(target, &script).await?;
    if out.status.success() {
        return Ok(());
    }
    anyhow::bail!(
        "HTTP /health probe failed for {}: {}",
        url,
        summarize_process_failure(&out)
    );
}

pub async fn evaluate_service_health(
    target: &RuntimeTarget,
    service_name: &str,
    service: &ServiceConfig,
    explain: bool,
    stability_runs: u32,
    jitter: bool,
) -> Result<HealthEvaluation> {
    let Some(healthcheck) = &service.healthcheck else {
        return Ok(HealthEvaluation {
            ok: true,
            detail: "missing healthcheck (skipped)".to_string(),
            records: Vec::new(),
        });
    };

    let runs = stability_runs.max(1);
    let mut run_summaries = Vec::new();
    let mut all_records = Vec::new();
    let mut overall_ok = true;

    for run_idx in 1..=runs {
        let mut records = Vec::new();
        let ok = evaluate_profile(
            target,
            service_name,
            service,
            healthcheck,
            "root",
            &mut records,
        )
        .await?;
        if !ok {
            overall_ok = false;
        }
        run_summaries.push(format!("run#{run_idx}:{}", if ok { "ok" } else { "fail" }));
        if explain {
            all_records.extend(records);
        }
        if jitter && run_idx < runs {
            let pause_ms = ((run_idx as u64 * 137) % 400) + 100;
            sleep(Duration::from_millis(pause_ms)).await;
        }
    }

    let detail = if overall_ok {
        format!("Healthcheck passed for service '{service_name}'")
    } else {
        format!(
            "Healthcheck failed for service '{service_name}': {}",
            run_summaries.join(", ")
        )
    };

    Ok(HealthEvaluation {
        ok: overall_ok,
        detail,
        records: all_records,
    })
}

fn evaluate_profile<'a>(
    target: &'a RuntimeTarget,
    service_name: &'a str,
    service: &'a ServiceConfig,
    hc: &'a HealthcheckConfig,
    profile_name: &'a str,
    records: &'a mut Vec<HealthProbeRecord>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool>> + Send + 'a>> {
    Box::pin(async move {
        if let Some(all_profiles) = &hc.all {
            let mut ok = true;
            for (idx, child) in all_profiles.iter().enumerate() {
                let child_name = format!("{profile_name}.all[{idx}]");
                if !evaluate_profile(target, service_name, service, child, &child_name, records)
                    .await?
                {
                    ok = false;
                }
            }
            return Ok(ok);
        }

        if let Some(any_profiles) = &hc.any {
            let mut ok = false;
            for (idx, child) in any_profiles.iter().enumerate() {
                let child_name = format!("{profile_name}.any[{idx}]");
                if evaluate_profile(target, service_name, service, child, &child_name, records)
                    .await?
                {
                    ok = true;
                }
            }
            return Ok(ok);
        }

        let retries = hc.retries.unwrap_or(10);
        let interval = Duration::from_secs(hc.interval_secs.unwrap_or(5));
        let mut last_record = None;

        for _ in 0..retries {
            let record = if !hc.command.is_empty() {
                execute_command_probe(target, service_name, &hc.command, profile_name).await?
            } else if let Some(http) = &hc.http {
                execute_http_probe(target, service_name, service, hc, http, profile_name).await?
            } else if let Some(tcp) = &hc.tcp {
                execute_tcp_probe(target, hc, tcp, profile_name).await?
            } else {
                anyhow::bail!(
                    "No executable health profile for service '{}'",
                    service_name
                );
            };
            let ok = record.ok;
            last_record = Some(record.clone());
            records.push(record);
            if ok {
                return Ok(true);
            }
            sleep(interval).await;
        }

        if let Some(last) = last_record {
            records.push(last);
        }
        Ok(false)
    })
}

async fn execute_command_probe(
    target: &RuntimeTarget,
    service_name: &str,
    command: &[String],
    profile_name: &str,
) -> Result<HealthProbeRecord> {
    let mut parts = vec![
        "docker".to_string(),
        "exec".to_string(),
        service_name.to_string(),
    ];
    parts.extend(command.to_vec());
    let script = join_shell_command(&parts);
    let out = run_shell(target, &script).await?;
    Ok(to_probe_record(profile_name, script, out))
}

async fn execute_http_probe(
    target: &RuntimeTarget,
    service_name: &str,
    service: &ServiceConfig,
    hc: &HealthcheckConfig,
    http: &HttpHealthcheckConfig,
    profile_name: &str,
) -> Result<HealthProbeRecord> {
    let timeout = http.timeout_secs.or(hc.timeout_secs).unwrap_or(5);
    let expected = http.expected_status.unwrap_or(200);
    let url = if let Some(url) = &http.url {
        url.clone()
    } else {
        let port = http
            .port
            .or_else(|| service.ports.first().copied())
            .context("http healthcheck requires `http.port` or service ports")?;
        let path = http.path.clone().unwrap_or_else(|| "/health".to_string());
        format!("http://127.0.0.1:{port}{path}")
    };

    let script = format!(
        "code=$(curl -sS -o /dev/null -w '%{{http_code}}' --max-time {timeout} {url} || true); [ \"$code\" = \"{expected}\" ]"
    );
    let out = run_shell(target, &script).await?;
    Ok(to_probe_record(
        profile_name,
        format!("probe[{service_name}] {script}"),
        out,
    ))
}

async fn execute_tcp_probe(
    target: &RuntimeTarget,
    hc: &HealthcheckConfig,
    tcp: &TcpHealthcheckConfig,
    profile_name: &str,
) -> Result<HealthProbeRecord> {
    let timeout = tcp.timeout_secs.or(hc.timeout_secs).unwrap_or(5);
    let host = tcp.host.clone().unwrap_or_else(|| "127.0.0.1".to_string());
    let script = format!(
        "nc -z -w {timeout} {host} {port}",
        timeout = timeout,
        host = shell_quote(&host),
        port = tcp.port
    );
    let out = run_shell(target, &script).await?;
    Ok(to_probe_record(profile_name, script, out))
}

fn to_probe_record(profile_name: &str, command: String, out: Output) -> HealthProbeRecord {
    HealthProbeRecord {
        profile: profile_name.to_string(),
        command,
        ok: out.status.success(),
        exit_code: out.status.code(),
        stdout: limit_output(String::from_utf8_lossy(&out.stdout).trim()),
        stderr: limit_output(String::from_utf8_lossy(&out.stderr).trim()),
    }
}

pub async fn preflight_image_access(target: &RuntimeTarget, image: &str) -> Result<()> {
    let script = format!(
        "docker image inspect {img} >/dev/null 2>&1 || docker pull {img}",
        img = shell_quote(image)
    );
    let out = run_shell(target, &script).await?;
    if out.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let mut hint = String::new();
    if image.starts_with("ghcr.io/") {
        hint =
            " Hint: ensure remote host has GHCR credentials (`docker login ghcr.io`) with read:packages scope."
                .to_string();
    }
    anyhow::bail!(
        "Image preflight failed for '{}': {}.{}",
        image,
        stderr,
        hint
    );
}

async fn inspect_service(
    target: &RuntimeTarget,
    name: &str,
    launched_id: Option<String>,
) -> Result<RuntimeDeployResult> {
    let inspect_id = launched_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .unwrap_or(name)
        .to_string();

    // Use docker inspect as the source of truth for discovery/existence.
    let inspect = run_shell(
        target,
        &format!(
            "docker inspect -f '{{{{.Id}}}}|{{{{.Config.Image}}}}|{{{{.State.Status}}}}' {inspect_id} 2>/dev/null || true"
        ),
    )
    .await?;
    let mut line = String::from_utf8_lossy(&inspect.stdout).trim().to_string();
    let mut detected_by = "id";

    if line.is_empty() {
        let by_name = run_shell(
            target,
            &format!(
                "docker inspect -f '{{{{.Id}}}}|{{{{.Config.Image}}}}|{{{{.State.Status}}}}' {name} 2>/dev/null || true"
            ),
        )
        .await?;
        line = String::from_utf8_lossy(&by_name.stdout).trim().to_string();
        detected_by = "name";
    }

    if line.is_empty() {
        anyhow::bail!("Deployed service '{}' was not found after deploy", name);
    }

    let mut result = parse_inspect_line(&line, detected_by)?;
    let ports_out = run_shell(
        target,
        &format!("docker ps -a --filter name=^/{name}$ --format '{{{{.Ports}}}}' | head -n 1"),
    )
    .await?;
    if ports_out.status.success() {
        let ports_line = String::from_utf8_lossy(&ports_out.stdout)
            .trim()
            .to_string();
        if !ports_line.is_empty() {
            result.ports = ports_line
                .split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect();
        }
    }
    Ok(result)
}

fn parse_inspect_line(line: &str, detected_by: &str) -> Result<RuntimeDeployResult> {
    let parts: Vec<&str> = line.split('|').collect();
    let id = parts.first().copied().unwrap_or_default().to_string();
    let status = parts.get(2).copied().unwrap_or_default().to_string();
    let ports = Vec::new();

    let s = status.to_ascii_lowercase();
    let running = s.starts_with("up") || s.contains("running") || s.contains("started");

    Ok(RuntimeDeployResult {
        id,
        status,
        ports,
        running,
        discoverable: true,
        detected_by: detected_by.to_string(),
        healthy: None,
    })
}

async fn run_shell(target: &RuntimeTarget, script: &str) -> Result<Output> {
    match target {
        RuntimeTarget::Local => {
            let out = std::process::Command::new("sh")
                .arg("-lc")
                .arg(script)
                .output()
                .context("Failed to execute local shell command")?;
            Ok(out)
        }
        RuntimeTarget::Remote(server_cfg) => {
            execute_remote_command(
                server_cfg,
                &["sh".to_string(), "-lc".to_string(), script.to_string()],
            )
            .await
        }
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn summarize_process_failure(output: &Output) -> String {
    let code = output
        .status
        .code()
        .map_or_else(|| "signal".to_string(), |c| c.to_string());
    let stderr = limit_output(String::from_utf8_lossy(&output.stderr).trim());
    let stdout = limit_output(String::from_utf8_lossy(&output.stdout).trim());
    match (stderr.is_empty(), stdout.is_empty()) {
        (true, true) => format!("exit={code}"),
        (false, true) => format!("exit={code} stderr={stderr}"),
        (true, false) => format!("exit={code} stdout={stdout}"),
        (false, false) => format!("exit={code} stderr={stderr} stdout={stdout}"),
    }
}

fn limit_output(value: &str) -> String {
    const MAX: usize = 300;
    if value.chars().count() <= MAX {
        return value.to_string();
    }
    let truncated: String = value.chars().take(MAX).collect();
    format!("{truncated}...")
}

#[cfg(test)]
mod tests {
    use super::summarize_process_failure;
    use std::process::Command;

    #[test]
    fn summarize_failure_includes_stderr_when_present() {
        let out = Command::new("sh")
            .arg("-lc")
            .arg("echo boom >&2; exit 7")
            .output()
            .expect("command should run");
        let summary = summarize_process_failure(&out);
        assert!(summary.contains("exit=7"));
        assert!(summary.contains("stderr=boom"));
    }

    #[test]
    fn summarize_failure_uses_stdout_when_stderr_empty() {
        let out = Command::new("sh")
            .arg("-lc")
            .arg("echo nope; exit 3")
            .output()
            .expect("command should run");
        let summary = summarize_process_failure(&out);
        assert!(summary.contains("exit=3"));
        assert!(summary.contains("stdout=nope"));
    }
}
