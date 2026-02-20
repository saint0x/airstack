use airstack_config::AirstackConfig;
use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

mod commands;
mod dependencies;
mod deploy_runtime;
mod env_loader;
mod output;
mod retry;
mod secrets_store;
mod ssh_utils;
mod state;
mod theme;

#[derive(Parser)]
#[command(name = "airstack")]
#[command(about = "Modular, type-safe infrastructure SDK and CLI")]
#[command(version = env!("CARGO_PKG_VERSION"))]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, global = true, help = "Enable verbose output")]
    verbose: bool,

    #[arg(
        long,
        global = true,
        help = "Configuration file path (default: ./airstack.toml in current directory)"
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
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Initialize a new Airstack project")]
    Init {
        #[arg(help = "Project name")]
        name: Option<String>,
    },
    #[command(about = "Provision infrastructure and deploy services")]
    Up {
        #[arg(long, help = "Target environment")]
        target: Option<String>,
        #[arg(long, help = "Infrastructure provider")]
        provider: Option<String>,
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
    },
    #[command(about = "Execute a command inside a container on a remote server")]
    Cexec {
        #[arg(help = "Server name")]
        server: String,
        #[arg(help = "Container name")]
        container: String,
        #[arg(help = "Command to execute in container", last = true)]
        command: Vec<String>,
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
    #[command(about = "Show status of infrastructure and services")]
    Status {
        #[arg(long, help = "Show detailed status")]
        detailed: bool,
        #[arg(long, help = "Run active health probes for services")]
        probe: bool,
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
    },
    #[command(about = "Show logs for a service")]
    Logs {
        #[arg(help = "Service name")]
        service: String,
        #[arg(long, short = 'f', help = "Follow log output")]
        follow: bool,
        #[arg(long, help = "Number of lines to show")]
        tail: Option<usize>,
    },
    #[command(about = "Preview planned infra/service actions")]
    Plan {
        #[arg(long, help = "Include destroy actions for unmanaged resources")]
        include_destroy: bool,
    },
    #[command(about = "Apply desired infrastructure and services")]
    Apply,
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
    #[command(about = "Build/publish release image for a service")]
    Release(commands::release::ReleaseArgs),
    #[command(about = "Atomic latest-code ship (build/push/deploy with rollback)")]
    Ship(commands::ship::ShipArgs),
    #[command(about = "Collect status/log/diagnostic artifacts for bug reports")]
    SupportBundle(commands::support_bundle::SupportBundleArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    env_loader::load_airstack_env();

    let cli = Cli::parse();
    if let Some(env_name) = &cli.env {
        std::env::set_var("AIRSTACK_ENV", env_name);
    }
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

    let config_path = match (&cli.command, &cli.config) {
        (Commands::Init { .. }, Some(path)) => path.clone(),
        (Commands::Init { .. }, None) => "airstack.toml".to_string(),
        (_, Some(path)) => path.clone(),
        (_, None) => AirstackConfig::get_config_path()?
            .to_string_lossy()
            .to_string(),
    };

    match cli.command {
        Commands::Init { name } => commands::init::run(name, &config_path).await,
        Commands::Up { target, provider } => {
            commands::up::run(
                &config_path,
                target,
                provider,
                cli.dry_run,
                cli.allow_local_deploy,
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
            )
            .await
        }
        Commands::Cexec {
            server,
            container,
            command,
        } => commands::cexec::run(&config_path, &server, &container, command).await,
        Commands::Scale { service, replicas } => {
            commands::scale::run(&config_path, &service, replicas).await
        }
        Commands::Cli => commands::cli::run(&config_path).await,
        Commands::Tui { view } => commands::tui::run(&config_path, view).await,
        Commands::Status {
            detailed,
            probe,
            source,
        } => commands::status::run(&config_path, detailed, probe, &source).await,
        Commands::Ssh { target, command } => {
            commands::ssh::run(&config_path, &target, command).await
        }
        Commands::Logs {
            service,
            follow,
            tail,
        } => commands::logs::run(&config_path, &service, follow, tail).await,
        Commands::Plan { include_destroy } => {
            commands::plan::run(&config_path, include_destroy).await
        }
        Commands::Apply => commands::apply::run(&config_path, cli.allow_local_deploy).await,
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
        Commands::Release(args) => commands::release::run(&config_path, args).await,
        Commands::Ship(mut args) => {
            args.allow_local_deploy = cli.allow_local_deploy;
            commands::ship::run(&config_path, args).await
        }
        Commands::SupportBundle(args) => commands::support_bundle::run(&config_path, args).await,
    }
}
