use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

mod commands;
mod dependencies;
mod deploy_runtime;
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
        help = "Configuration file path",
        default_value = "airstack.toml"
    )]
    config: String,

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
    let _ = dotenvy::dotenv();

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

    match cli.command {
        Commands::Init { name } => commands::init::run(name, &cli.config).await,
        Commands::Up { target, provider } => {
            commands::up::run(
                &cli.config,
                target,
                provider,
                cli.dry_run,
                cli.allow_local_deploy,
            )
            .await
        }
        Commands::Destroy { target, force } => {
            commands::destroy::run(&cli.config, target, force || cli.yes).await
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
                &cli.config,
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
        } => commands::cexec::run(&cli.config, &server, &container, command).await,
        Commands::Scale { service, replicas } => {
            commands::scale::run(&cli.config, &service, replicas).await
        }
        Commands::Cli => commands::cli::run(&cli.config).await,
        Commands::Tui { view } => commands::tui::run(&cli.config, view).await,
        Commands::Status {
            detailed,
            probe,
            source,
        } => commands::status::run(&cli.config, detailed, probe, &source).await,
        Commands::Ssh { target, command } => {
            commands::ssh::run(&cli.config, &target, command).await
        }
        Commands::Logs {
            service,
            follow,
            tail,
        } => commands::logs::run(&cli.config, &service, follow, tail).await,
        Commands::Plan { include_destroy } => {
            commands::plan::run(&cli.config, include_destroy).await
        }
        Commands::Apply => commands::apply::run(&cli.config, cli.allow_local_deploy).await,
        Commands::Edge { command } => commands::edge::run(&cli.config, command).await,
        Commands::Doctor => commands::doctor::run(&cli.config).await,
        Commands::GoLive(args) => commands::golive::run(&cli.config, args).await,
        Commands::Drift => commands::drift::run(&cli.config).await,
        Commands::Registry { command } => commands::registry::run(&cli.config, command).await,
        Commands::Reconcile(mut args) => {
            args.allow_local_deploy = cli.allow_local_deploy;
            commands::reconcile::run(&cli.config, args).await
        }
        Commands::Runbook => commands::runbook::run(&cli.config).await,
        Commands::Secrets { command } => commands::secrets::run(&cli.config, command).await,
        Commands::Backup { command } => commands::backup::run(&cli.config, command).await,
        Commands::Release(args) => commands::release::run(&cli.config, args).await,
        Commands::Ship(mut args) => {
            args.allow_local_deploy = cli.allow_local_deploy;
            commands::ship::run(&cli.config, args).await
        }
        Commands::SupportBundle(args) => commands::support_bundle::run(&cli.config, args).await,
    }
}
