use anyhow::{bail, Result};

pub async fn run(_config_path: &str, _view: Option<String>) -> Result<()> {
    bail!(
        "TUI support is disabled in this build. Rebuild with: cargo build -p airstack-core --features tui"
    )
}
