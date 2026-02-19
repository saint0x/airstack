use anyhow::Result;

use crate::commands::up;

pub async fn run(config_path: &str, allow_local_deploy: bool) -> Result<()> {
    up::run(config_path, None, None, false, allow_local_deploy).await
}
