use crate::commands;
use crate::output;
use crate::theme;
use airstack_config::AirstackConfig;
use anyhow::{Context, Result};
use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{self, ClearType};
use std::io::{self, Write};

pub async fn run(config_path: &str) -> Result<()> {
    if output::is_json() {
        anyhow::bail!("Interactive CLI mode cannot be used with --json");
    }

    loop {
        let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
        let server_names = sorted_servers(&config);
        let service_names = sorted_services(&config);

        let choices = vec![
            "Infrastructure".to_string(),
            "Services".to_string(),
            "SSH & Containers".to_string(),
            "Quick Status".to_string(),
            "Exit".to_string(),
        ];

        let title = format!(
            "Airstack CLI  ·  {}  ·  {}",
            config.project.name, config_path
        );
        let selected = select_interactive(&title, &choices, false)?;
        match selected {
            Some(0) => infrastructure_menu(config_path).await?,
            Some(1) => services_menu(config_path, &service_names).await?,
            Some(2) => remote_menu(config_path, &server_names, &service_names).await?,
            Some(3) => run_and_continue(commands::status::run(config_path, false).await),
            Some(4) | None => break,
            _ => {}
        }
    }

    Ok(())
}

async fn infrastructure_menu(config_path: &str) -> Result<()> {
    let choices = vec![
        "Status".to_string(),
        "Status (Detailed)".to_string(),
        "Up".to_string(),
        "Destroy".to_string(),
        "Back".to_string(),
    ];

    loop {
        match select_interactive("Infrastructure", &choices, true)? {
            Some(0) => run_and_continue(commands::status::run(config_path, false).await),
            Some(1) => run_and_continue(commands::status::run(config_path, true).await),
            Some(2) => {
                let provider = read_optional("Provider (blank = config default)")?;
                let target = read_optional("Target env (blank = default)")?;
                run_and_continue(commands::up::run(config_path, target, provider, false).await);
            }
            Some(3) => {
                if confirm("Destroy infrastructure? This is destructive", false)? {
                    let target = read_optional("Target env (blank = default)")?;
                    run_and_continue(commands::destroy::run(config_path, target, true).await);
                }
            }
            Some(4) | None => break,
            _ => {}
        }
    }

    Ok(())
}

async fn services_menu(config_path: &str, service_names: &[String]) -> Result<()> {
    let choices = vec![
        "Deploy".to_string(),
        "Scale".to_string(),
        "Logs".to_string(),
        "Back".to_string(),
    ];

    loop {
        match select_interactive("Services", &choices, true)? {
            Some(0) => {
                let deploy_target = select_with_extra(
                    "Select service to deploy",
                    service_names,
                    Some("all".to_string()),
                    "all",
                )?;
                if let Some(service) = deploy_target {
                    run_and_continue(commands::deploy::run(config_path, &service, None).await);
                }
            }
            Some(1) => {
                if let Some(service) = select_from_list("Select service to scale", service_names)? {
                    let replicas = prompt_usize("Replica count", Some(1))?;
                    run_and_continue(commands::scale::run(config_path, &service, replicas).await);
                }
            }
            Some(2) => {
                if let Some(service) = select_from_list("Select service logs", service_names)? {
                    let follow = confirm("Follow logs", false)?;
                    let tail = prompt_optional_usize("Tail lines (blank = full)")?;
                    run_and_continue(
                        commands::logs::run(config_path, &service, follow, tail).await,
                    );
                }
            }
            Some(3) | None => break,
            _ => {}
        }
    }

    Ok(())
}

async fn remote_menu(
    config_path: &str,
    server_names: &[String],
    service_names: &[String],
) -> Result<()> {
    let choices = vec![
        "SSH".to_string(),
        "Container Exec".to_string(),
        "Back".to_string(),
    ];

    loop {
        match select_interactive("SSH & Containers", &choices, true)? {
            Some(0) => {
                if let Some(server) = select_from_list("Select server", server_names)? {
                    let cmd = read_optional("SSH command (blank = interactive shell)")?;
                    let command = split_command(cmd);
                    run_and_continue(commands::ssh::run(config_path, &server, command).await);
                }
            }
            Some(1) => {
                if let Some(server) = select_from_list("Select server", server_names)? {
                    let container = select_with_extra(
                        "Container",
                        service_names,
                        Some("manual".to_string()),
                        "manual entry",
                    )?;
                    let container = match container {
                        Some(c) if c == "manual" => read_required("Container name")?,
                        Some(c) => c,
                        None => continue,
                    };
                    let cmd = read_optional(
                        "Container command (blank = interactive shell in container)",
                    )?;
                    run_and_continue(
                        commands::cexec::run(config_path, &server, &container, split_command(cmd))
                            .await,
                    );
                }
            }
            Some(2) | None => break,
            _ => {}
        }
    }

    Ok(())
}

fn sorted_servers(config: &AirstackConfig) -> Vec<String> {
    let mut servers: Vec<String> = config
        .infra
        .as_ref()
        .map(|infra| infra.servers.iter().map(|s| s.name.clone()).collect())
        .unwrap_or_default();
    servers.sort();
    servers
}

fn sorted_services(config: &AirstackConfig) -> Vec<String> {
    let mut services: Vec<String> = config
        .services
        .as_ref()
        .map(|s| s.keys().cloned().collect())
        .unwrap_or_default();
    services.sort();
    services
}

fn run_and_continue(result: Result<()>) {
    if let Err(e) = result {
        output::error_line(format!("ERROR: {e:#}"));
    }
}

fn select_from_list(title: &str, values: &[String]) -> Result<Option<String>> {
    select_with_extra(title, values, None, "")
}

fn select_with_extra(
    title: &str,
    values: &[String],
    extra_value: Option<String>,
    extra_label: &str,
) -> Result<Option<String>> {
    if values.is_empty() && extra_value.is_none() {
        output::subtle_line(format!("{title}: no entries available"));
        return Ok(None);
    }

    let mut options = values.to_vec();
    if extra_value.is_some() {
        options.push(extra_label.to_string());
    }
    options.push("Back".to_string());

    match select_interactive(title, &options, true)? {
        Some(index) if index < values.len() => Ok(Some(values[index].clone())),
        Some(index) if extra_value.is_some() && index == values.len() => Ok(extra_value),
        _ => Ok(None),
    }
}

fn select_interactive(title: &str, options: &[String], allow_back: bool) -> Result<Option<usize>> {
    let _raw = RawModeGuard::new()?;
    let mut index = 0usize;

    loop {
        render_menu(title, options, index, allow_back)?;

        if let Event::Key(key) = event::read().context("Failed to read keyboard input")? {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    index = if index == 0 {
                        options.len().saturating_sub(1)
                    } else {
                        index - 1
                    };
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    index = (index + 1) % options.len().max(1);
                }
                KeyCode::Enter | KeyCode::Right => return Ok(Some(index)),
                KeyCode::Esc | KeyCode::Left | KeyCode::Char('q') if allow_back => return Ok(None),
                _ => {}
            }
        }
    }
}

fn render_menu(title: &str, options: &[String], selected: usize, allow_back: bool) -> Result<()> {
    let mut stdout = io::stdout();
    execute!(
        stdout,
        terminal::Clear(ClearType::All),
        cursor::MoveTo(0, 0)
    )
    .context("Failed to render menu")?;

    println!(
        "{}",
        theme::ansi_fg(theme::ansi_bold(title), theme::STEEL_200)
    );
    let controls = if allow_back {
        "↑/↓ move  •  →/Enter select  •  ←/Esc back"
    } else {
        "↑/↓ move  •  →/Enter select"
    };
    println!("{}", theme::ansi_fg(controls, theme::GRAY_500));
    println!();

    for (idx, option) in options.iter().enumerate() {
        if idx == selected {
            println!(
                "{}",
                theme::ansi_fg(format!("› {}", theme::ansi_bold(option)), theme::OCEAN_400)
            );
        } else {
            println!(
                "{}",
                theme::ansi_fg(format!("  {}", option), theme::GRAY_500)
            );
        }
    }

    stdout.flush().context("Failed to flush menu output")
}

fn confirm(prompt: &str, default: bool) -> Result<bool> {
    let suffix = if default { "[Y/n]" } else { "[y/N]" };
    loop {
        let input = read_line(&format!("{prompt} {suffix}: "))?;
        let trimmed = input.trim().to_ascii_lowercase();
        if trimmed.is_empty() {
            return Ok(default);
        }
        match trimmed.as_str() {
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => output::error_line("Please answer y or n."),
        }
    }
}

fn prompt_usize(prompt: &str, default: Option<usize>) -> Result<usize> {
    loop {
        let suffix = default.map(|d| format!(" [{d}]")).unwrap_or_default();
        let input = read_line(&format!("{prompt}{suffix}: "))?;
        let trimmed = input.trim();
        if trimmed.is_empty() {
            if let Some(d) = default {
                return Ok(d);
            }
        } else if let Ok(v) = trimmed.parse::<usize>() {
            if v > 0 {
                return Ok(v);
            }
        }
        output::error_line("Please enter a positive integer.");
    }
}

fn prompt_optional_usize(prompt: &str) -> Result<Option<usize>> {
    loop {
        let input = read_line(&format!("{prompt}: "))?;
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        if let Ok(v) = trimmed.parse::<usize>() {
            return Ok(Some(v));
        }
        output::error_line("Please enter a valid integer or leave blank.");
    }
}

fn read_required(prompt: &str) -> Result<String> {
    loop {
        let value = read_line(&format!("{prompt}: "))?;
        if !value.trim().is_empty() {
            return Ok(value.trim().to_string());
        }
        output::error_line("Input cannot be empty.");
    }
}

fn read_optional(prompt: &str) -> Result<Option<String>> {
    let value = read_line(&format!("{prompt}: "))?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}

fn read_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush().context("Failed to flush stdout")?;
    let mut buf = String::new();
    io::stdin()
        .read_line(&mut buf)
        .context("Failed to read stdin")?;
    Ok(buf)
}

fn split_command(command: Option<String>) -> Vec<String> {
    command
        .unwrap_or_default()
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

struct RawModeGuard;

impl RawModeGuard {
    fn new() -> Result<Self> {
        terminal::enable_raw_mode().context("Failed to enable raw mode")?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::split_command;

    #[test]
    fn split_command_empty() {
        assert!(split_command(None).is_empty());
        assert!(split_command(Some("   ".to_string())).is_empty());
    }

    #[test]
    fn split_command_whitespace_split() {
        assert_eq!(
            split_command(Some("echo hello world".to_string())),
            vec!["echo", "hello", "world"]
        );
    }
}
