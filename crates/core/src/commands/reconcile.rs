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
}

pub async fn run(config_path: &str, args: ReconcileArgs) -> Result<()> {
    up::run(
        config_path,
        None,
        None,
        args.dry_run,
        args.allow_local_deploy,
    )
    .await?;
    status::run(config_path, args.detailed, "auto").await
}
