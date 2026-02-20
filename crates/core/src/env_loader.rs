use std::path::{Path, PathBuf};

pub fn load_airstack_env() {
    for path in env_candidates() {
        if path.exists() {
            let _ = dotenvy::from_path(&path);
            return;
        }
    }

    // Backward-compatible fallback: allow project-local .env when no global env file exists.
    let _ = dotenvy::dotenv();
}

fn env_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Ok(explicit) = std::env::var("AIRSTACK_ENV_FILE") {
        if !explicit.trim().is_empty() {
            paths.push(PathBuf::from(explicit));
        }
    }

    if let Ok(home) = std::env::var("AIRSTACK_HOME") {
        if !home.trim().is_empty() {
            paths.push(Path::new(&home).join(".env"));
        }
    }

    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".airstack").join(".env"));
        paths.push(home.join(".config").join("airstack").join(".env"));
    }

    paths
}

#[cfg(test)]
mod tests {
    use super::env_candidates;

    #[test]
    fn env_candidates_include_standard_global_locations() {
        let candidates = env_candidates();
        let rendered = candidates
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains(".airstack/.env") || rendered.contains(".config/airstack/.env"));
    }
}
