use anyhow::Result;
use serde::Serialize;

use crate::theme;

const ENV_JSON: &str = "AIRSTACK_OUTPUT_JSON";
const ENV_QUIET: &str = "AIRSTACK_OUTPUT_QUIET";

pub fn configure(json: bool, quiet: bool) {
    std::env::set_var(ENV_JSON, if json { "1" } else { "0" });
    std::env::set_var(ENV_QUIET, if quiet { "1" } else { "0" });
}

pub fn is_json() -> bool {
    std::env::var(ENV_JSON).unwrap_or_else(|_| "0".to_string()) == "1"
}

pub fn is_quiet() -> bool {
    std::env::var(ENV_QUIET).unwrap_or_else(|_| "0".to_string()) == "1"
}

pub fn line(message: impl AsRef<str>) {
    if !is_json() && !is_quiet() {
        println!("{}", message.as_ref());
    }
}

pub fn subtle_line(message: impl AsRef<str>) {
    if !is_json() && !is_quiet() {
        println!("{}", theme::ansi_fg(message.as_ref(), theme::GRAY_500));
    }
}

pub fn error_line(message: impl AsRef<str>) {
    if !is_json() {
        eprintln!("{}", theme::ansi_fg(message.as_ref(), theme::STEEL_200));
    }
}

pub fn emit_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
