use anyhow::Result;

use crate::commands::up;

pub async fn run(
    config_path: &str,
    allow_local_deploy: bool,
    ensure_host_paths: bool,
) -> Result<()> {
    up::run(
        config_path,
        None,
        None,
        false,
        allow_local_deploy,
        false,
        false,
        false,
        false,
        ensure_host_paths,
    )
    .await
}
