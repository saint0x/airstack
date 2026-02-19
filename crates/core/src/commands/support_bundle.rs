use airstack_config::AirstackConfig;
use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use std::fs;
use std::process::Command;

#[derive(Debug, Clone, Args)]
pub struct SupportBundleArgs {
    #[arg(long, help = "Output directory for bundle")]
    pub out_dir: Option<String>,
}

#[derive(Debug, Serialize)]
struct BundleRun {
    name: String,
    command: Vec<String>,
    exit_code: Option<i32>,
    ok: bool,
    stdout_file: String,
    stderr_file: String,
}

#[derive(Debug, Serialize)]
struct BundleManifest {
    project: String,
    created_unix: u64,
    runs: Vec<BundleRun>,
}

pub async fn run(config_path: &str, args: SupportBundleArgs) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let bundle_dir = args
        .out_dir
        .unwrap_or_else(|| format!("support-bundle-{}", unix_now()));
    fs::create_dir_all(&bundle_dir)
        .with_context(|| format!("Failed to create bundle dir {}", bundle_dir))?;

    let mut runs = Vec::new();
    runs.push(run_capture(
        "status",
        &bundle_dir,
        &[
            "--config",
            config_path,
            "--json",
            "status",
            "--detailed",
            "--source",
            "auto",
            "--probe",
        ],
    )?);
    runs.push(run_capture(
        "go-live",
        &bundle_dir,
        &[
            "--config",
            config_path,
            "--json",
            "go-live",
            "--explain",
            "--stability",
            "1",
        ],
    )?);
    runs.push(run_capture(
        "edge-diagnose",
        &bundle_dir,
        &["--config", config_path, "--json", "edge", "diagnose"],
    )?);

    let probe_image = config
        .services
        .as_ref()
        .and_then(|s| s.values().next().map(|v| v.image.clone()))
        .unwrap_or_else(|| "ghcr.io/OWNER/REPO:TAG".to_string());
    runs.push(run_capture(
        "registry-doctor",
        &bundle_dir,
        &[
            "--config",
            config_path,
            "--json",
            "registry",
            "doctor",
            "--image",
            &probe_image,
        ],
    )?);

    if let Some(services) = &config.services {
        for service in services.keys() {
            runs.push(run_capture(
                &format!("logs-{}", service),
                &bundle_dir,
                &["--config", config_path, "logs", service, "--tail", "200"],
            )?);
        }
    }

    let manifest = BundleManifest {
        project: config.project.name,
        created_unix: unix_now(),
        runs,
    };
    fs::write(
        format!("{}/manifest.json", bundle_dir),
        serde_json::to_string_pretty(&manifest)?,
    )
    .with_context(|| format!("Failed to write manifest in {}", bundle_dir))?;

    println!("✅ support bundle created at {}", bundle_dir);
    Ok(())
}

fn run_capture(name: &str, bundle_dir: &str, args: &[&str]) -> Result<BundleRun> {
    let exe = std::env::current_exe().context("Failed to resolve current executable")?;
    let out = Command::new(exe)
        .args(args)
        .output()
        .with_context(|| format!("Failed to run {}", name))?;

    let stdout_file = format!("{}/{}.stdout.log", bundle_dir, sanitize(name));
    let stderr_file = format!("{}/{}.stderr.log", bundle_dir, sanitize(name));
    fs::write(&stdout_file, &out.stdout)?;
    fs::write(&stderr_file, &out.stderr)?;

    Ok(BundleRun {
        name: name.to_string(),
        command: args.iter().map(|v| v.to_string()).collect(),
        exit_code: out.status.code(),
        ok: out.status.success(),
        stdout_file,
        stderr_file,
    })
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
