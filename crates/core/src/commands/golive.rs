use crate::commands::edge;
use crate::deploy_runtime::{
    preflight_image_access, resolve_target, run_healthcheck, run_http_health_probe,
};
use crate::output;
use airstack_config::AirstackConfig;
use airstack_metal::get_provider as get_metal_provider;
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Serialize)]
struct ReadinessCheck {
    name: String,
    ok: bool,
    detail: String,
}

#[derive(Debug, Serialize)]
struct GoLiveOutput {
    project: String,
    ok: bool,
    checks: Vec<ReadinessCheck>,
}

pub async fn run(config_path: &str) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let mut checks = Vec::new();

    infra_up_check(&config, &mut checks).await;
    image_pull_checks(&config, &mut checks).await;
    edge_checks(config_path, &config, &mut checks).await;
    app_health_checks(&config, &mut checks).await;

    let ok = checks.iter().all(|c| c.ok);
    let payload = GoLiveOutput {
        project: config.project.name.clone(),
        ok,
        checks,
    };

    if output::is_json() {
        output::emit_json(&payload)?;
    } else {
        output::line("🚀 Go-Live Readiness");
        for c in &payload.checks {
            let mark = if c.ok { "✅" } else { "❌" };
            output::line(format!("{} {}: {}", mark, c.name, c.detail));
        }
    }

    if !payload.ok {
        anyhow::bail!("Go-live readiness failed");
    }
    Ok(())
}

async fn infra_up_check(config: &AirstackConfig, checks: &mut Vec<ReadinessCheck>) {
    let Some(infra) = &config.infra else {
        checks.push(ReadinessCheck {
            name: "infra-up".to_string(),
            ok: false,
            detail: "no infra.servers configured".to_string(),
        });
        return;
    };

    let mut by_provider: HashMap<String, Vec<airstack_metal::Server>> = HashMap::new();
    for server in &infra.servers {
        if by_provider.contains_key(&server.provider) {
            continue;
        }
        match get_metal_provider(&server.provider, HashMap::new()) {
            Ok(provider) => match provider.list_servers().await {
                Ok(servers) => {
                    by_provider.insert(server.provider.clone(), servers);
                }
                Err(e) => {
                    checks.push(ReadinessCheck {
                        name: format!("infra-up:{}", server.provider),
                        ok: false,
                        detail: format!("provider list failed: {}", e),
                    });
                    return;
                }
            },
            Err(e) => {
                checks.push(ReadinessCheck {
                    name: format!("infra-up:{}", server.provider),
                    ok: false,
                    detail: format!("provider init failed: {}", e),
                });
                return;
            }
        }
    }

    let mut failures = Vec::new();
    for desired in &infra.servers {
        let Some(servers) = by_provider.get(&desired.provider) else {
            failures.push(format!("{} (provider lookup missing)", desired.name));
            continue;
        };
        let found = servers.iter().find(|s| s.name == desired.name);
        match found {
            Some(s) if matches!(s.status, airstack_metal::ServerStatus::Running) => {}
            Some(s) => failures.push(format!("{} ({:?})", desired.name, s.status)),
            None => failures.push(format!("{} (missing)", desired.name)),
        }
    }

    checks.push(ReadinessCheck {
        name: "infra-up".to_string(),
        ok: failures.is_empty(),
        detail: if failures.is_empty() {
            "all configured servers are running".to_string()
        } else {
            format!("non-ready servers: {}", failures.join(", "))
        },
    });
}

async fn image_pull_checks(config: &AirstackConfig, checks: &mut Vec<ReadinessCheck>) {
    let Some(services) = &config.services else {
        checks.push(ReadinessCheck {
            name: "image-pull".to_string(),
            ok: false,
            detail: "no services configured".to_string(),
        });
        return;
    };

    let mut failures = Vec::new();
    for (name, svc) in services {
        match resolve_target(config, svc, false) {
            Ok(target) => {
                if let Err(e) = preflight_image_access(&target, &svc.image).await {
                    failures.push(format!("{}: {}", name, e));
                }
            }
            Err(e) => {
                failures.push(format!("{}: target resolution failed ({})", name, e));
            }
        }
    }

    checks.push(ReadinessCheck {
        name: "image-pull".to_string(),
        ok: failures.is_empty(),
        detail: if failures.is_empty() {
            "all service images are pullable/available".to_string()
        } else {
            failures.join(" | ")
        },
    });
}

async fn edge_checks(config_path: &str, config: &AirstackConfig, checks: &mut Vec<ReadinessCheck>) {
    if config.edge.is_none() {
        checks.push(ReadinessCheck {
            name: "edge-dns-tls".to_string(),
            ok: true,
            detail: "edge config not present (skipped)".to_string(),
        });
        return;
    }
    match edge::run(config_path, edge::EdgeCommands::Diagnose).await {
        Ok(_) => checks.push(ReadinessCheck {
            name: "edge-dns-tls".to_string(),
            ok: true,
            detail: "edge DNS/TLS checks passed".to_string(),
        }),
        Err(e) => checks.push(ReadinessCheck {
            name: "edge-dns-tls".to_string(),
            ok: false,
            detail: format!("{}", e),
        }),
    }
}

async fn app_health_checks(config: &AirstackConfig, checks: &mut Vec<ReadinessCheck>) {
    let Some(services) = &config.services else {
        checks.push(ReadinessCheck {
            name: "app-health".to_string(),
            ok: false,
            detail: "no services configured".to_string(),
        });
        return;
    };

    let mut failures = Vec::new();
    let mut passed = Vec::new();
    let mut missing_hc = BTreeMap::new();
    for (name, svc) in services {
        let Some(hc) = &svc.healthcheck else {
            missing_hc.insert(name.clone(), "missing healthcheck".to_string());
            continue;
        };
        match resolve_target(config, svc, false) {
            Ok(target) => {
                match run_healthcheck(&target, name, hc).await {
                    Ok(_) => passed.push(name.clone()),
                    Err(e) => {
                        if let Some(port) = svc.ports.first().copied() {
                            match run_http_health_probe(&target, port, hc.timeout_secs).await {
                                Ok(_) => {
                                    passed.push(format!("{name} (via /health fallback on :{port})"))
                                }
                                Err(http_err) => {
                                    failures.push(format!(
                                        "{}: {}; fallback probe failed: {}",
                                        name, e, http_err
                                    ));
                                }
                            }
                        } else {
                            failures.push(format!("{}: {}", name, e));
                        }
                    }
                };
            }
            Err(e) => failures.push(format!("{}: target resolution failed ({})", name, e)),
        }
    }

    checks.push(build_app_health_check(passed, missing_hc, failures));
}

fn build_app_health_check(
    passed: Vec<String>,
    missing_hc: BTreeMap<String, String>,
    failures: Vec<String>,
) -> ReadinessCheck {
    let ok = failures.is_empty();
    let mut detail_parts = Vec::new();
    if !passed.is_empty() {
        detail_parts.push(format!(
            "passed: {}",
            passed
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !missing_hc.is_empty() {
        detail_parts.push(format!(
            "skipped (no healthcheck): {}",
            missing_hc
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !failures.is_empty() {
        detail_parts.push(format!("failed: {}", failures.join(" | ")));
    }
    ReadinessCheck {
        name: "app-health".to_string(),
        ok,
        detail: if detail_parts.is_empty() {
            "no service healthchecks configured (skipped)".to_string()
        } else {
            detail_parts.join(" ; ")
        },
    }
}

#[cfg(test)]
mod tests {
    use super::build_app_health_check;
    use std::collections::BTreeMap;

    #[test]
    fn app_health_missing_checks_are_skipped_not_failed() {
        let mut missing = BTreeMap::new();
        missing.insert("database".to_string(), "missing healthcheck".to_string());
        let check = build_app_health_check(vec!["api".to_string()], missing, vec![]);
        assert!(check.ok);
        assert!(check.detail.contains("passed: api"));
        assert!(check.detail.contains("skipped (no healthcheck): database"));
    }

    #[test]
    fn app_health_real_failures_fail_check() {
        let check = build_app_health_check(
            vec![],
            BTreeMap::new(),
            vec!["api: status code 500".to_string()],
        );
        assert!(!check.ok);
        assert!(check.detail.contains("failed: api: status code 500"));
    }
}
