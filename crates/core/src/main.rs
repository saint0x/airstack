use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

mod commands;
mod dependencies;
mod output;
mod retry;
mod state;

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
    },
    #[command(about = "Scale a service to a target replica count")]
    Scale {
        #[arg(help = "Service name")]
        service: String,
        #[arg(help = "Target number of replicas")]
        replicas: usize,
    },
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    output::configure(cli.json, cli.quiet);

    let level = if cli.verbose {
        Level::DEBUG
    } else if cli.json || cli.quiet {
        Level::ERROR
    } else {
        Level::INFO
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
            commands::up::run(&cli.config, target, provider, cli.dry_run).await
        }
        Commands::Destroy { target, force } => {
            commands::destroy::run(&cli.config, target, force || cli.yes).await
        }
        Commands::Deploy { service, target } => {
            commands::deploy::run(&cli.config, &service, target).await
        }
        Commands::Scale { service, replicas } => {
            commands::scale::run(&cli.config, &service, replicas).await
        }
        Commands::Tui { view } => commands::tui::run(&cli.config, view).await,
        Commands::Status { detailed } => commands::status::run(&cli.config, detailed).await,
        Commands::Ssh { target, command } => {
            commands::ssh::run(&cli.config, &target, command).await
        }
        Commands::Logs {
            service,
            follow,
            tail,
        } => commands::logs::run(&cli.config, &service, follow, tail).await,
    }
}
