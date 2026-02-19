use crate::commands;
use crate::output;
use airstack_config::AirstackConfig;
use anyhow::{Context, Result};
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};

pub async fn run(config_path: &str) -> Result<()> {
    if output::is_json() {
        anyhow::bail!("Interactive CLI mode cannot be used with --json");
    }

    let theme = ColorfulTheme::default();

    loop {
        let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
        let server_names = sorted_servers(&config);
        let service_names = sorted_services(&config);

        let title = format!("Airstack CLI ({})", config.project.name);
        let choice = select_index(
            &theme,
            &title,
            &[
                "Infrastructure",
                "Services",
                "Planning & Safety",
                "Edge",
                "SSH & Containers",
                "Quick Status",
                "Exit",
            ],
        )?;

        match choice {
            0 => infrastructure_menu(&theme, config_path).await?,
            1 => services_menu(&theme, config_path, &service_names).await?,
            2 => planning_menu(&theme, config_path).await?,
            3 => edge_menu(&theme, config_path).await?,
            4 => remote_menu(&theme, config_path, &server_names, &service_names).await?,
            5 => run_and_continue(commands::status::run(config_path, false, "auto").await),
            6 => break,
            _ => {}
        }
    }

    Ok(())
}

async fn infrastructure_menu(theme: &ColorfulTheme, config_path: &str) -> Result<()> {
    loop {
        let choice = select_index(
            theme,
            "Infrastructure",
            &["Status", "Status (Detailed)", "Up", "Destroy", "Back"],
        )?;
        match choice {
            0 => run_and_continue(commands::status::run(config_path, false, "auto").await),
            1 => run_and_continue(commands::status::run(config_path, true, "auto").await),
            2 => {
                let provider = read_optional(theme, "Provider (blank = config default)")?;
                let target = read_optional(theme, "Target env (blank = default)")?;
                run_and_continue(
                    commands::up::run(config_path, target, provider, false, false).await,
                );
            }
            3 => {
                let confirmed = Confirm::with_theme(theme)
                    .with_prompt("Destroy infrastructure? This is destructive")
                    .default(false)
                    .interact()
                    .context("Failed to read confirmation")?;
                if confirmed {
                    let target = read_optional(theme, "Target env (blank = default)")?;
                    run_and_continue(commands::destroy::run(config_path, target, true).await);
                }
            }
            4 => break,
            _ => {}
        }
    }
    Ok(())
}

async fn services_menu(
    theme: &ColorfulTheme,
    config_path: &str,
    service_names: &[String],
) -> Result<()> {
    loop {
        let choice = select_index(
            theme,
            "Services",
            &["Deploy", "Scale", "Logs", "Release", "Back"],
        )?;
        match choice {
            0 => {
                let mut options = service_names.to_vec();
                options.push("all".to_string());
                options.push("Back".to_string());
                if let Some(selected) =
                    select_from_list(theme, "Select service to deploy", &options)?
                {
                    if selected != "Back" {
                        run_and_continue(
                            commands::deploy::run(
                                config_path,
                                &selected,
                                None,
                                false,
                                false,
                                true,
                                None,
                                "rolling".to_string(),
                                45,
                            )
                            .await,
                        );
                    }
                }
            }
            1 => {
                if let Some(service) =
                    select_from_list_with_back(theme, "Select service to scale", service_names)?
                {
                    let replicas: usize = Input::with_theme(theme)
                        .with_prompt("Replica count")
                        .default(1)
                        .interact_text()
                        .context("Failed to read replica count")?;
                    run_and_continue(commands::scale::run(config_path, &service, replicas).await);
                }
            }
            2 => {
                if let Some(service) =
                    select_from_list_with_back(theme, "Select service logs", service_names)?
                {
                    let follow = Confirm::with_theme(theme)
                        .with_prompt("Follow logs")
                        .default(false)
                        .interact()
                        .context("Failed to read follow option")?;
                    let tail = read_optional_usize(theme, "Tail lines (blank = full)")?;
                    run_and_continue(
                        commands::logs::run(config_path, &service, follow, tail).await,
                    );
                }
            }
            3 => {
                if let Some(service) =
                    select_from_list_with_back(theme, "Select service to release", service_names)?
                {
                    let push = Confirm::with_theme(theme)
                        .with_prompt("Push image after build")
                        .default(false)
                        .interact()
                        .context("Failed to read push option")?;
                    let update_config = Confirm::with_theme(theme)
                        .with_prompt("Update config image reference")
                        .default(true)
                        .interact()
                        .context("Failed to read update-config option")?;
                    let tag = read_optional(theme, "Tag (blank = git sha)")?;
                    run_and_continue(
                        commands::release::run(
                            config_path,
                            commands::release::ReleaseArgs {
                                service,
                                tag,
                                push,
                                update_config,
                            },
                        )
                        .await,
                    );
                }
            }
            4 => break,
            _ => {}
        }
    }
    Ok(())
}

async fn planning_menu(theme: &ColorfulTheme, config_path: &str) -> Result<()> {
    loop {
        let choice = select_index(
            theme,
            "Planning & Safety",
            &["Plan", "Apply", "Doctor", "Runbook", "Secrets List", "Back"],
        )?;
        match choice {
            0 => run_and_continue(commands::plan::run(config_path, false).await),
            1 => run_and_continue(commands::apply::run(config_path, false).await),
            2 => run_and_continue(commands::doctor::run(config_path).await),
            3 => run_and_continue(commands::runbook::run(config_path).await),
            4 => run_and_continue(
                commands::secrets::run(config_path, commands::secrets::SecretsCommands::List).await,
            ),
            5 => break,
            _ => {}
        }
    }
    Ok(())
}

async fn edge_menu(theme: &ColorfulTheme, config_path: &str) -> Result<()> {
    loop {
        let choice = select_index(
            theme,
            "Edge",
            &["Plan", "Validate", "Status", "Diagnose", "Apply", "Back"],
        )?;
        match choice {
            0 => run_and_continue(
                commands::edge::run(config_path, commands::edge::EdgeCommands::Plan).await,
            ),
            1 => run_and_continue(
                commands::edge::run(config_path, commands::edge::EdgeCommands::Validate).await,
            ),
            2 => run_and_continue(
                commands::edge::run(config_path, commands::edge::EdgeCommands::Status).await,
            ),
            3 => run_and_continue(
                commands::edge::run(config_path, commands::edge::EdgeCommands::Diagnose).await,
            ),
            4 => run_and_continue(
                commands::edge::run(config_path, commands::edge::EdgeCommands::Apply).await,
            ),
            5 => break,
            _ => {}
        }
    }
    Ok(())
}

async fn remote_menu(
    theme: &ColorfulTheme,
    config_path: &str,
    server_names: &[String],
    service_names: &[String],
) -> Result<()> {
    loop {
        let choice = select_index(
            theme,
            "SSH & Containers",
            &["SSH", "Container Exec", "Back"],
        )?;
        match choice {
            0 => {
                if let Some(server) =
                    select_from_list_with_back(theme, "Select server", server_names)?
                {
                    let cmd = read_optional(theme, "SSH command (blank = interactive shell)")?;
                    run_and_continue(
                        commands::ssh::run(config_path, &server, split_command(cmd)).await,
                    );
                }
            }
            1 => {
                if let Some(server) =
                    select_from_list_with_back(theme, "Select server", server_names)?
                {
                    let mut containers = service_names.to_vec();
                    containers.push("manual entry".to_string());
                    containers.push("Back".to_string());
                    let selected = match select_from_list(theme, "Container", &containers)? {
                        Some(v) if v == "Back" => continue,
                        Some(v) if v == "manual entry" => read_required(theme, "Container name")?,
                        Some(v) => v,
                        None => continue,
                    };
                    let cmd =
                        read_optional(theme, "Container command (blank = interactive shell)")?;
                    run_and_continue(
                        commands::cexec::run(config_path, &server, &selected, split_command(cmd))
                            .await,
                    );
                }
            }
            2 => break,
            _ => {}
        }
    }
    Ok(())
}

fn select_index(theme: &ColorfulTheme, prompt: &str, options: &[&str]) -> Result<usize> {
    Select::with_theme(theme)
        .with_prompt(prompt)
        .items(options)
        .default(0)
        .interact()
        .context("Failed to select menu option")
}

fn select_from_list(
    theme: &ColorfulTheme,
    prompt: &str,
    options: &[String],
) -> Result<Option<String>> {
    if options.is_empty() {
        output::subtle_line(format!("{prompt}: no entries available"));
        return Ok(None);
    }
    let index = Select::with_theme(theme)
        .with_prompt(prompt)
        .items(options)
        .default(0)
        .interact()
        .context("Failed to select option")?;
    Ok(options.get(index).cloned())
}

fn select_from_list_with_back(
    theme: &ColorfulTheme,
    prompt: &str,
    options: &[String],
) -> Result<Option<String>> {
    let mut items = options.to_vec();
    items.push("Back".to_string());
    match select_from_list(theme, prompt, &items)? {
        Some(v) if v == "Back" => Ok(None),
        other => Ok(other),
    }
}

fn read_required(theme: &ColorfulTheme, prompt: &str) -> Result<String> {
    let value: String = Input::with_theme(theme)
        .with_prompt(prompt)
        .allow_empty(false)
        .interact_text()
        .context("Failed to read input")?;
    Ok(value.trim().to_string())
}

fn read_optional(theme: &ColorfulTheme, prompt: &str) -> Result<Option<String>> {
    let value: String = Input::with_theme(theme)
        .with_prompt(prompt)
        .allow_empty(true)
        .interact_text()
        .context("Failed to read input")?;
    let value = value.trim();
    if value.is_empty() {
        Ok(None)
    } else {
        Ok(Some(value.to_string()))
    }
}

fn read_optional_usize(theme: &ColorfulTheme, prompt: &str) -> Result<Option<usize>> {
    let value = read_optional(theme, prompt)?;
    match value {
        Some(v) => {
            let parsed = v
                .parse::<usize>()
                .context("Please enter a valid positive integer")?;
            Ok(Some(parsed))
        }
        None => Ok(None),
    }
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

fn split_command(command: Option<String>) -> Vec<String> {
    command
        .unwrap_or_default()
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
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
