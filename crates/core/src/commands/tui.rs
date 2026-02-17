use anyhow::{Context, Result};
use std::path::Path;
use tokio::process::Command;

use crate::output;

const AIRSTACK_BANNER: &str = r#"
     _    _         _             _
    / \  (_)_ __ __| |_ __   ___ | | __
   / _ \ | | '__/ _` | '_ \ / _ \| |/ /
  / ___ \| | | | (_| | |_) | (_) |   <
 /_/   \_\_|_|  \__,_| .__/ \___/|_|\_\
                     |_|
"#;

pub async fn run(view: Option<String>) -> Result<()> {
    let project_root = std::env::current_dir().context("Failed to resolve current directory")?;
    let frankentui_manifest = project_root.join("frankentui").join("Cargo.toml");

    if !frankentui_manifest.exists() {
        anyhow::bail!(
            "FrankenTUI is not available at {}. Run 'git submodule update --init --recursive'.",
            frankentui_manifest.display()
        );
    }

    if !output::is_json() {
        output::line(AIRSTACK_BANNER);
        output::line("Launching Airstack TUI on FrankenTUI runtime...");
    }

    let mut cmd = Command::new("cargo");
    cmd.arg("run")
        .arg("-p")
        .arg("ftui-demo-showcase")
        .arg("--manifest-path")
        .arg(path_as_str(&frankentui_manifest)?);

    if let Some(view_name) = view {
        cmd.env("FTUI_HARNESS_VIEW", view_name);
    }

    let status = cmd
        .status()
        .await
        .context("Failed to launch FrankenTUI showcase runner")?;

    if !status.success() {
        anyhow::bail!("TUI process exited with code {:?}", status.code());
    }

    Ok(())
}

fn path_as_str(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("Invalid UTF-8 path: {}", path.display()))
}
