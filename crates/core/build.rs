use std::process::Command;

fn cmd_out(cmd: &str) -> Option<String> {
    let out = Command::new("sh").arg("-lc").arg(cmd).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn main() {
    let git_sha = cmd_out("git rev-parse --short HEAD").unwrap_or_else(|| "unknown".to_string());
    let build_ts = cmd_out("date -u +%Y-%m-%dT%H:%M:%SZ").unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=AIRSTACK_GIT_SHA={git_sha}");
    println!("cargo:rustc-env=AIRSTACK_BUILD_TIMESTAMP={build_ts}");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
}
