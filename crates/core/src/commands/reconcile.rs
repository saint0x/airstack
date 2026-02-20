use crate::commands::deploy;
use crate::commands::{status, up};
use anyhow::Result;
use clap::Args;

#[derive(Debug, Clone, Args)]
pub struct ReconcileArgs {
    #[arg(long, help = "Show detailed post-reconcile status")]
    pub detailed: bool,
    #[arg(long, help = "Run plan only, do not change state")]
    pub dry_run: bool,
    #[arg(long, help = "Allow local deploys even when infra servers exist")]
    pub allow_local_deploy: bool,
    #[arg(long, help = "Reconcile services only (skip infra actions)")]
    pub services_only: bool,
    #[arg(long, help = "Alias for --services-only")]
    pub no_infra: bool,
}

pub async fn run(config_path: &str, args: ReconcileArgs) -> Result<()> {
    if args.services_only || args.no_infra {
        deploy::run(
            config_path,
            "all",
            None,
            args.allow_local_deploy,
            false,
            false,
            None,
            "rolling".to_string(),
            45,
        )
        .await?;
    } else {
        up::run(
            config_path,
            None,
            None,
            args.dry_run,
            args.allow_local_deploy,
            false,
            false,
            false,
            false,
        )
        .await?;
    }
    status::run(config_path, args.detailed, false, "auto").await
}
