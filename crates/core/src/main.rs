use airstack_config::AirstackConfig;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

mod commands;
mod dependencies;
mod deploy_runtime;
mod env_loader;
mod infra_preflight;
mod output;
mod provider_profiles;
mod retry;
mod secrets_store;
mod ssh_utils;
mod state;
mod theme;

const AIRSTACK_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("AIRSTACK_GIT_SHA"),
    ", built ",
    env!("AIRSTACK_BUILD_TIMESTAMP"),
    ")"
);

#[derive(Parser)]
#[command(name = "airstack")]
#[command(about = "Modular, type-safe infrastructure SDK and CLI")]
#[command(version = AIRSTACK_VERSION)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, global = true, help = "Enable verbose output")]
    verbose: bool,

    #[arg(
        long,
        global = true,
        help = "Configuration file path (default: nearest ./airstack.toml in current or parent directories)"
    )]
    config: Option<String>,

    #[arg(long, global = true, help = "Perform a dry run without making changes")]
    dry_run: bool,

    #[arg(
        long,
        short = 'y',
        global = true,
        help = "Automatically answer yes to prompts"
    )]
    yes: bool,

    #[arg(long, global = true, help = "Output machine-readable JSON")]
    json: bool,

    #[arg(long, global = true, help = "Suppress human-readable output")]
    quiet: bool,

    #[arg(
        long,
        global = true,
        help = "Environment overlay (loads airstack.<env>.toml)"
    )]
    env: Option<String>,

    #[arg(
        long,
        global = true,
        help = "Allow local deploys even when infra servers exist"
    )]
    allow_local_deploy: bool,

    #[arg(
        long,
        global = true,
        help = "Provider profile override for this run (<provider>:<profile>)"
    )]
    provider_profile: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Initialize a new Airstack project")]
    Init {
        #[arg(help = "Project name")]
        name: Option<String>,
        #[arg(long, help = "Provider template (e.g., hetzner, fly)")]
        provider: Option<String>,
        #[arg(long, help = "Preset template (e.g., clickhouse)")]
        preset: Option<String>,
    },
    #[command(about = "Provision infrastructure and deploy services")]
    Up {
        #[arg(long, help = "Target environment")]
        target: Option<String>,
        #[arg(long, help = "Infrastructure provider")]
        provider: Option<String>,
        #[arg(
            long,
            help = "Deploy services locally and skip infrastructure provisioning"
        )]
        local: bool,
        #[arg(
            long,
            help = "Bootstrap runtime dependencies (Docker) on remote servers"
        )]
        bootstrap_runtime: bool,
        #[arg(long, help = "Allow provider-aware fallback to default valid region")]
        auto_fallback: bool,
        #[arg(long, help = "Resolve server region/type capacity automatically")]
        resolve_capacity: bool,
        #[arg(
            long,
            help = "Auto-create missing remote bind-mount host paths during preflight"
        )]
        ensure_host_paths: bool,
    },
    #[command(about = "Destroy infrastructure")]
    Destroy {
        #[arg(long, help = "Target environment")]
        target: Option<String>,
        #[arg(long, help = "Force destruction without confirmation")]
        force: bool,
    },
    #[command(about = "Deploy a specific service")]
    Deploy {
        #[arg(help = "Service name")]
        service: String,
        #[arg(long, help = "Target server")]
        target: Option<String>,
        #[arg(long, help = "Build latest local code into image before deploy")]
        latest_code: bool,
        #[arg(
            long,
            default_value_t = true,
            help = "Push image when using --latest-code"
        )]
        push: bool,
        #[arg(long, help = "Tag override for --latest-code")]
        tag: Option<String>,
        #[arg(
            long,
            help = "Deploy strategy: rolling|bluegreen|canary",
            default_value = "rolling"
        )]
        strategy: String,
        #[arg(
            long,
            help = "Canary observation window in seconds (strategy=canary)",
            default_value_t = 45
        )]
        canary_seconds: u64,
        #[arg(
            long,
            help = "Auto-create missing remote bind-mount host paths during preflight"
        )]
        ensure_host_paths: bool,
    },
    #[command(about = "Execute a command inside a container on a remote server")]
    #[command(
        after_help = "Example: airstack cexec <server> <container> -- <command>\nExample: airstack cexec <server> --container <container> -- <command>"
    )]
    Cexec {
        #[arg(help = "Server name")]
        server: String,
        #[arg(help = "Container name")]
        container: Option<String>,
        #[arg(
            long = "container",
            help = "Container name (named form to avoid positional ordering mistakes)"
        )]
        container_name: Option<String>,
        #[arg(help = "Command to execute in container", last = true)]
        command: Vec<String>,
        #[arg(long, help = "Execute this shell command string in the container")]
        cmd: Option<String>,
        #[arg(long, help = "Run a local script file in the container via shell")]
        script: Option<String>,
    },
    #[command(
        about = "Legacy build command (deprecated; use release/ship)",
        hide = true
    )]
    Build {
        #[arg(help = "Legacy mode (for example: remote/local)")]
        mode: Option<String>,
        #[arg(help = "Service name")]
        service: Option<String>,
    },
    #[command(about = "Scale a service to a target replica count")]
    Scale {
        #[arg(help = "Service name")]
        service: String,
        #[arg(help = "Target number of replicas")]
        replicas: usize,
    },
    #[command(about = "Launch lightweight interactive CLI menus")]
    Cli,
    #[command(about = "Launch the FrankenTUI-powered Airstack interface")]
    Tui {
        #[arg(
            long,
            help = "Start in a specific Airstack view (Dashboard, Servers, Services, etc.)"
        )]
        view: Option<String>,
    },
    #[command(about = "Run configured remote scripts and lifecycle hooks")]
    Script {
        #[command(subcommand)]
        command: commands::script::ScriptCommands,
    },
    #[command(about = "Show status of infrastructure and services")]
    Status {
        #[arg(long, help = "Show detailed status")]
        detailed: bool,
        #[arg(long, help = "Run active health probes for services")]
        probe: bool,
        #[arg(long, help = "Include image/deploy provenance fields in status output")]
        provenance: bool,
        #[arg(
            long,
            help = "Status source-of-truth mode: auto|provider|ssh|control-plane",
            default_value = "auto"
        )]
        source: String,
    },
    #[command(about = "SSH into a server")]
    Ssh {
        #[arg(help = "Server name")]
        target: String,
        #[arg(help = "Command to execute", last = true)]
        command: Vec<String>,
        #[arg(long, help = "Execute this shell command string on the remote host")]
        cmd: Option<String>,
        #[arg(long, help = "Run a local script file on the remote host via shell")]
        script: Option<String>,
    },
    #[command(about = "Show logs for a service")]
    Logs {
        #[arg(help = "Service name")]
        service: String,
        #[arg(long, short = 'f', help = "Follow log output")]
        follow: bool,
        #[arg(long, help = "Number of lines to show")]
        tail: Option<usize>,
        #[arg(
            long,
            help = "Logs source-of-truth mode: auto|ssh|control-plane",
            default_value = "auto"
        )]
        source: String,
    },
    #[command(about = "Preview planned infra/service actions")]
    Plan {
        #[arg(long, help = "Include destroy actions for unmanaged resources")]
        include_destroy: bool,
        #[arg(long, help = "Allow provider-aware fallback to default valid region")]
        auto_fallback: bool,
        #[arg(long, help = "Resolve server region/type capacity automatically")]
        resolve_capacity: bool,
    },
    #[command(about = "Apply desired infrastructure and services")]
    Apply {
        #[arg(
            long,
            help = "Auto-create missing remote bind-mount host paths during preflight"
        )]
        ensure_host_paths: bool,
    },
    #[command(about = "Edge reverse-proxy workflows")]
    Edge {
        #[command(subcommand)]
        command: commands::edge::EdgeCommands,
    },
    #[command(about = "Run production safety checks")]
    Doctor,
    #[command(about = "Validate full go-live readiness across infra/image/edge/health")]
    GoLive(commands::golive::GoLiveArgs),
    #[command(about = "Check image drift between config and running runtime")]
    Drift,
    #[command(about = "Registry credential diagnostics")]
    Registry {
        #[command(subcommand)]
        command: commands::registry::RegistryCommands,
    },
    #[command(about = "Converge runtime state to desired TOML state")]
    Reconcile(commands::reconcile::ReconcileArgs),
    #[command(about = "Print operational runbook for this stack")]
    Runbook,
    #[command(about = "Manage encrypted project secrets")]
    Secrets {
        #[command(subcommand)]
        command: commands::secrets::SecretsCommands,
    },
    #[command(about = "Managed backup lifecycle commands")]
    Backup {
        #[command(subcommand)]
        command: commands::backup::BackupCommands,
    },
    #[command(about = "Provider profile and multi-context workflows")]
    Provider {
        #[command(subcommand)]
        command: commands::provider::ProviderCommands,
    },
    #[command(about = "Build/publish release image for a service")]
    Release(commands::release::ReleaseArgs),
    #[command(about = "Atomic latest-code ship (build/push/deploy with rollback)")]
    Ship(commands::ship::ShipArgs),
    #[command(about = "Collect status/log/diagnostic artifacts for bug reports")]
    SupportBundle(commands::support_bundle::SupportBundleArgs),
    #[command(about = "Upload a local file to a remote server with checksum verification")]
    Upload {
        #[arg(help = "Server name")]
        target: String,
        #[arg(help = "Local source file path")]
        source: String,
        #[arg(help = "Remote destination file path")]
        destination: String,
        #[arg(
            long,
            help = "Expected sha256 checksum (defaults to local file sha256)"
        )]
        checksum: Option<String>,
        #[arg(long, help = "chmod mode to apply after upload (for example 0644)")]
        mode: Option<String>,
        #[arg(long, help = "Disable atomic move and write destination directly")]
        no_atomic: bool,
    },
    #[command(about = "Alias for `airstack upload`")]
    Cp {
        #[arg(help = "Server name")]
        target: String,
        #[arg(help = "Local source file path")]
        source: String,
        #[arg(help = "Remote destination file path")]
        destination: String,
        #[arg(
            long,
            help = "Expected sha256 checksum (defaults to local file sha256)"
        )]
        checksum: Option<String>,
        #[arg(long, help = "chmod mode to apply after upload (for example 0644)")]
        mode: Option<String>,
        #[arg(long, help = "Disable atomic move and write destination directly")]
        no_atomic: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = cleanup_stale_repo_binary();
    env_loader::load_airstack_env();

    let cli = Cli::parse();
    if let Some(env_name) = &cli.env {
        std::env::set_var("AIRSTACK_ENV", env_name);
    }
    provider_profiles::apply_profiles_for_run(cli.provider_profile.as_deref())?;
    output::configure(cli.json, cli.quiet);

    let level = if cli.verbose {
        Level::DEBUG
    } else if cli.json || cli.quiet {
        Level::ERROR
    } else {
        Level::WARN
    };

    let subscriber = FmtSubscriber::builder()
        .with_max_level(level)
        .with_target(false)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_file(false)
        .with_line_number(false)
        .compact()
        .finish();

    tracing::subscriber::set_global_default(subscriber)?;

    info!("Airstack CLI v{}", env!("CARGO_PKG_VERSION"));

    let (config_path, has_resolved_config) = match (&cli.command, &cli.config) {
        (Commands::Init { .. }, Some(path)) => (path.clone(), true),
        (Commands::Init { .. }, None) => ("airstack.toml".to_string(), true),
        (_, Some(path)) => (path.clone(), true),
        (_, None) => match AirstackConfig::get_config_path() {
            Ok(path) => (path.to_string_lossy().to_string(), true),
            Err(err) => {
                if matches!(cli.command, Commands::Status { .. }) {
                    (String::new(), false)
                } else {
                    return Err(err);
                }
            }
        },
    };
    if has_resolved_config {
        env_loader::load_airstack_env_for_config(&config_path);
        if is_mutating_command(&cli.command) {
            enforce_provider_mutation_guard(&config_path, cli.yes)?;
        }
    }

    match cli.command {
        Commands::Init {
            name,
            provider,
            preset,
        } => commands::init::run(name, provider, preset, &config_path).await,
        Commands::Up {
            target,
            provider,
            local,
            bootstrap_runtime,
            auto_fallback,
            resolve_capacity,
            ensure_host_paths,
        } => {
            commands::up::run(
                &config_path,
                target,
                provider,
                cli.dry_run,
                cli.allow_local_deploy,
                local,
                bootstrap_runtime,
                auto_fallback,
                resolve_capacity,
                ensure_host_paths,
            )
            .await
        }
        Commands::Destroy { target, force } => {
            commands::destroy::run(&config_path, target, force || cli.yes).await
        }
        Commands::Deploy {
            service,
            target,
            latest_code,
            push,
            tag,
            strategy,
            canary_seconds,
            ensure_host_paths,
        } => {
            commands::deploy::run(
                &config_path,
                &service,
                target,
                cli.allow_local_deploy,
                latest_code,
                push,
                tag,
                strategy,
                canary_seconds,
                ensure_host_paths,
            )
            .await
        }
        Commands::Cexec {
            server,
            container,
            container_name,
            command,
            cmd,
            script,
        } => {
            let resolved_container = container_name
                .or(container)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Missing container name. Usage: airstack cexec <server> <container> -- <command>\nOr: airstack cexec <server> --container <container> -- <command>"
                    )
                })?;
            commands::cexec::run(
                &config_path,
                &server,
                &resolved_container,
                commands::cexec::ContainerExec {
                    command,
                    cmd,
                    script,
                },
            )
            .await
        }
        Commands::Scale { service, replicas } => {
            commands::scale::run(&config_path, &service, replicas).await
        }
        Commands::Cli => commands::cli::run(&config_path).await,
        Commands::Tui { view } => commands::tui::run(&config_path, view).await,
        Commands::Script { command } => commands::script::run(&config_path, command).await,
        Commands::Status {
            detailed,
            probe,
            provenance,
            source,
        } => {
            if has_resolved_config {
                commands::status::run(&config_path, detailed, probe, provenance, &source).await
            } else {
                commands::status::run_auto_discover(detailed, probe, provenance, &source).await
            }
        }
        Commands::Ssh {
            target,
            command,
            cmd,
            script,
        } => {
            commands::ssh::run(
                &config_path,
                &target,
                commands::ssh::SshExec {
                    command,
                    cmd,
                    script,
                },
            )
            .await
        }
        Commands::Logs {
            service,
            follow,
            tail,
            source,
        } => commands::logs::run(&config_path, &service, follow, tail, &source).await,
        Commands::Plan {
            include_destroy,
            auto_fallback,
            resolve_capacity,
        } => {
            commands::plan::run(
                &config_path,
                include_destroy,
                auto_fallback,
                resolve_capacity,
            )
            .await
        }
        Commands::Apply { ensure_host_paths } => {
            commands::apply::run(&config_path, cli.allow_local_deploy, ensure_host_paths).await
        }
        Commands::Edge { command } => commands::edge::run(&config_path, command).await,
        Commands::Doctor => commands::doctor::run(&config_path).await,
        Commands::GoLive(args) => commands::golive::run(&config_path, args).await,
        Commands::Drift => commands::drift::run(&config_path).await,
        Commands::Registry { command } => commands::registry::run(&config_path, command).await,
        Commands::Reconcile(mut args) => {
            args.allow_local_deploy = cli.allow_local_deploy;
            commands::reconcile::run(&config_path, args).await
        }
        Commands::Runbook => commands::runbook::run(&config_path).await,
        Commands::Secrets { command } => commands::secrets::run(&config_path, command).await,
        Commands::Backup { command } => commands::backup::run(&config_path, command).await,
        Commands::Provider { command } => commands::provider::run(&config_path, command).await,
        Commands::Release(args) => commands::release::run(&config_path, args).await,
        Commands::Ship(mut args) => {
            args.allow_local_deploy = cli.allow_local_deploy;
            commands::ship::run(&config_path, args).await
        }
        Commands::Build { mode, service } => {
            let migration = match (mode.as_deref(), service.as_deref()) {
                (Some("remote"), Some(svc)) => format!(
                    "Legacy 'build remote' was replaced by:\n  airstack release {svc} --push --update-config --remote-build <server>\nOr atomic flow:\n  airstack ship {svc} --push --update-config"
                ),
                (_, Some(svc)) => format!(
                    "Legacy 'build' was replaced by:\n  airstack release {svc} --push --update-config\nOr atomic flow:\n  airstack ship {svc} --push --update-config"
                ),
                _ => "Legacy 'build' was replaced by 'release' / 'ship'.\nTry:\n  airstack release <service> --push --update-config\n  airstack ship <service> --push --update-config".to_string(),
            };
            anyhow::bail!("{migration}");
        }
        Commands::SupportBundle(args) => commands::support_bundle::run(&config_path, args).await,
        Commands::Upload {
            target,
            source,
            destination,
            checksum,
            mode,
            no_atomic,
        }
        | Commands::Cp {
            target,
            source,
            destination,
            checksum,
            mode,
            no_atomic,
        } => {
            commands::upload::run(
                &config_path,
                commands::upload::UploadArgs {
                    target,
                    source,
                    destination,
                    checksum,
                    mode,
                    no_atomic,
                },
            )
            .await
        }
    }
}

fn cleanup_stale_repo_binary() -> Result<()> {
    let exe = std::env::current_exe().context("Failed to resolve current executable path")?;
    let exe_canon = exe.canonicalize().unwrap_or(exe.clone());
    let exe_text = exe_canon.to_string_lossy();

    // Only auto-clean when running from cargo target output.
    if !exe_text.contains("/target/") {
        return Ok(());
    }

    let Some(root) = find_workspace_root(&exe_canon) else {
        return Ok(());
    };
    let bin_path = root.join("bin").join("airstack");
    if !bin_path.exists() {
        return Ok(());
    }

    let same = bin_path
        .canonicalize()
        .map(|p| p == exe_canon)
        .unwrap_or(false);
    if same {
        return Ok(());
    }

    let _ = fs::remove_file(&bin_path);
    #[cfg(unix)]
    {
        let _ = std::os::unix::fs::symlink(&exe_canon, &bin_path);
    }
    Ok(())
}

fn find_workspace_root(path: &Path) -> Option<PathBuf> {
    for anc in path.ancestors() {
        let cargo_toml = anc.join("Cargo.toml");
        let crates_dir = anc.join("crates");
        if cargo_toml.exists() && crates_dir.is_dir() {
            return Some(anc.to_path_buf());
        }
    }
    None
}

fn is_mutating_command(command: &Commands) -> bool {
    matches!(
        command,
        Commands::Up { .. }
            | Commands::Deploy { .. }
            | Commands::Destroy { .. }
            | Commands::Apply { .. }
            | Commands::Ship(_)
    )
}

fn enforce_provider_mutation_guard(config_path: &str, assume_yes: bool) -> Result<()> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let providers = config
        .infra
        .as_ref()
        .map(|infra| {
            infra
                .servers
                .iter()
                .map(|s| s.provider.clone())
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    if providers.is_empty() {
        return Ok(());
    }

    let store = provider_profiles::load_store()?;
    let pins = config
        .providers
        .as_ref()
        .and_then(|p| p.profiles.clone())
        .unwrap_or_default();

    let mut rows = Vec::new();
    for provider in providers {
        let active = store.active.get(&provider).cloned();
        let pinned = pins.get(&provider).cloned();
        let selected = pinned.clone().or(active.clone());
        if let Some(profile) = selected {
            let identity = provider_profiles::resolve_profile_identity(&store, &provider, &profile);
            let targets = config
                .infra
                .as_ref()
                .map(|infra| {
                    infra
                        .servers
                        .iter()
                        .filter(|s| s.provider == provider)
                        .map(|s| s.name.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            rows.push(BTreeMap::from([
                ("provider".to_string(), provider.clone()),
                (
                    "active_profile".to_string(),
                    active.unwrap_or_else(|| "none".to_string()),
                ),
                (
                    "pinned_profile".to_string(),
                    pinned.unwrap_or_else(|| "none".to_string()),
                ),
                ("selected_profile".to_string(), profile),
                (
                    "account".to_string(),
                    identity.account.unwrap_or_else(|| "unknown".to_string()),
                ),
                (
                    "organization".to_string(),
                    identity
                        .organization
                        .unwrap_or_else(|| "unknown".to_string()),
                ),
                ("targets".to_string(), targets.join(",")),
                (
                    "auth_ok".to_string(),
                    if identity.auth_ok { "true" } else { "false" }.to_string(),
                ),
            ]));
        }
    }

    if rows.is_empty() {
        return Ok(());
    }

    if output::is_json() && !assume_yes {
        anyhow::bail!(
            "Provider profile mutation guard requires confirmation for mutating commands. Re-run with -y to continue."
        );
    }

    if !output::is_json() {
        output::line("🔐 Provider Preflight (mutating command)");
        for row in &rows {
            output::line(format!(
                "- provider={} active={} pinned={} selected={} auth_ok={} account={} org={} targets=[{}]",
                row.get("provider").map(String::as_str).unwrap_or("unknown"),
                row.get("active_profile").map(String::as_str).unwrap_or("none"),
                row.get("pinned_profile").map(String::as_str).unwrap_or("none"),
                row.get("selected_profile")
                    .map(String::as_str)
                    .unwrap_or("none"),
                row.get("auth_ok").map(String::as_str).unwrap_or("false"),
                row.get("account").map(String::as_str).unwrap_or("unknown"),
                row.get("organization").map(String::as_str).unwrap_or("unknown"),
                row.get("targets").map(String::as_str).unwrap_or("")
            ));
        }
    }

    if assume_yes {
        return Ok(());
    }
    if output::is_json() {
        anyhow::bail!("Refusing mutating command without -y in JSON mode");
    }

    print!("Proceed with mutating operation using the above provider profile context? (y/N): ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    if !input.trim().to_ascii_lowercase().starts_with('y') {
        anyhow::bail!("Aborted by provider profile mutation guard");
    }
    Ok(())
}
