use std::time::Duration;

use airstack_config::AirstackConfig;
use anyhow::{Context, Result};
use ftui::core::event::{Event, KeyCode, Modifiers};
use ftui::core::geometry::Rect;
use ftui::layout::{Constraint, Flex};
use ftui::render::cell::PackedRgba;
use ftui::render::frame::Frame;
use ftui::runtime::{Cmd, Model, Program, ProgramConfig};
use ftui::style::Style;
use ftui::widgets::block::Block;
use ftui::widgets::borders::BorderType;
use ftui::widgets::paragraph::Paragraph;
use ftui::widgets::Widget;

use crate::output;
use crate::state::LocalState;

const AIRSTACK_BANNER: &str = r#"
     _    _         _             _
    / \  (_)_ __ __| |_ __   ___ | | __
   / _ \ | | '__/ _` | '_ \ / _ \| |/ /
  / ___ \| | | | (_| | |_) | (_) |   <
 /_/   \_\_|_|  \__,_| .__/ \___/|_|\_\
                     |_|
"#;

const TICK_INTERVAL: Duration = Duration::from_millis(600);

const VIEWS: &[&str] = &[
    "Dashboard",
    "Servers",
    "Services",
    "Logs",
    "Scaling",
    "Network",
    "Providers",
    "Settings",
];

const PALETTE_ACTIONS: &[(&str, &str)] = &[
    ("Go Dashboard", "view:Dashboard"),
    ("Go Servers", "view:Servers"),
    ("Go Services", "view:Services"),
    ("Go Logs", "view:Logs"),
    ("Go Scaling", "view:Scaling"),
    ("Go Network", "view:Network"),
    ("Go Providers", "view:Providers"),
    ("Go Settings", "view:Settings"),
    ("Refresh Data", "refresh"),
    ("Quit Airstack", "quit"),
];

#[derive(Debug, Clone, Copy)]
enum Pane {
    Navigation,
    Workspace,
    Telemetry,
}

#[derive(Debug, Clone)]
struct TuiSummary {
    project_name: String,
    project_description: Option<String>,
    server_count: usize,
    service_count: usize,
    cache_server_count: usize,
    cache_service_count: usize,
    last_refresh_ok: bool,
}

#[derive(Debug, Clone)]
enum TuiMessage {
    Input(Event),
    Refreshed(Result<TuiSummary, String>),
}

impl From<Event> for TuiMessage {
    fn from(value: Event) -> Self {
        Self::Input(value)
    }
}

#[derive(Debug, Clone)]
struct AirstackTuiApp {
    config_path: String,
    selected_view: usize,
    active_pane: Pane,
    ticks: u64,
    summary: TuiSummary,
    palette_open: bool,
    palette_query: String,
    palette_index: usize,
}

impl AirstackTuiApp {
    fn new(config_path: String, summary: TuiSummary, preferred_view: Option<String>) -> Self {
        let selected_view = preferred_view
            .as_deref()
            .and_then(parse_view_index)
            .unwrap_or(0);

        Self {
            config_path,
            selected_view,
            active_pane: Pane::Navigation,
            ticks: 0,
            summary,
            palette_open: false,
            palette_query: String::new(),
            palette_index: 0,
        }
    }

    fn next_pane(&mut self) {
        self.active_pane = match self.active_pane {
            Pane::Navigation => Pane::Workspace,
            Pane::Workspace => Pane::Telemetry,
            Pane::Telemetry => Pane::Navigation,
        };
    }

    fn select_next_view(&mut self) {
        self.selected_view = (self.selected_view + 1) % VIEWS.len();
    }

    fn select_previous_view(&mut self) {
        self.selected_view = if self.selected_view == 0 {
            VIEWS.len() - 1
        } else {
            self.selected_view - 1
        };
    }

    fn filtered_actions(&self) -> Vec<(&'static str, &'static str)> {
        if self.palette_query.trim().is_empty() {
            return PALETTE_ACTIONS.to_vec();
        }

        let query = self.palette_query.to_ascii_lowercase();
        PALETTE_ACTIONS
            .iter()
            .copied()
            .filter(|(label, command)| {
                label.to_ascii_lowercase().contains(&query)
                    || command.to_ascii_lowercase().contains(&query)
            })
            .collect()
    }
}

impl Model for AirstackTuiApp {
    type Message = TuiMessage;

    fn init(&mut self) -> Cmd<Self::Message> {
        Cmd::batch(vec![
            Cmd::tick(TICK_INTERVAL),
            refresh_cmd(self.config_path.clone()),
        ])
    }

    fn update(&mut self, msg: Self::Message) -> Cmd<Self::Message> {
        match msg {
            TuiMessage::Input(Event::Tick) => {
                self.ticks = self.ticks.wrapping_add(1);
                Cmd::batch(vec![
                    Cmd::tick(TICK_INTERVAL),
                    refresh_cmd(self.config_path.clone()),
                ])
            }
            TuiMessage::Refreshed(result) => {
                match result {
                    Ok(summary) => {
                        self.summary = summary;
                    }
                    Err(_) => {
                        self.summary.last_refresh_ok = false;
                    }
                }
                Cmd::none()
            }
            TuiMessage::Input(Event::Key(key)) => {
                if key.modifiers.contains(Modifiers::CTRL) && key.is_char('c') {
                    return Cmd::quit();
                }

                if self.palette_open {
                    return handle_palette_input(self, key);
                }

                match key.code {
                    KeyCode::Escape => Cmd::quit(),
                    KeyCode::Char('q') => Cmd::quit(),
                    KeyCode::Char(':') => {
                        self.palette_open = true;
                        self.palette_query.clear();
                        self.palette_index = 0;
                        Cmd::none()
                    }
                    KeyCode::Tab => {
                        self.next_pane();
                        Cmd::none()
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.select_next_view();
                        Cmd::none()
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.select_previous_view();
                        Cmd::none()
                    }
                    KeyCode::Char(c) if c.is_ascii_digit() => {
                        let idx = (c as u8 - b'0') as usize;
                        if idx >= 1 && idx <= VIEWS.len() {
                            self.selected_view = idx - 1;
                        }
                        Cmd::none()
                    }
                    _ => Cmd::none(),
                }
            }
            _ => Cmd::none(),
        }
    }

    fn view(&self, frame: &mut Frame) {
        let root = Rect::new(0, 0, frame.width(), frame.height());
        if root.width < 40 || root.height < 12 {
            Paragraph::new("Terminal too small. Resize to at least 40x12.")
                .block(
                    Block::bordered()
                        .title("Airstack TUI")
                        .border_type(BorderType::Rounded),
                )
                .render(root, frame);
            return;
        }

        let rows = Flex::vertical()
            .constraints([Constraint::Fixed(4), Constraint::Fill, Constraint::Fixed(3)])
            .split(root);
        let header = rows[0];
        let body = rows[1];
        let footer = rows[2];

        let cols = Flex::horizontal()
            .constraints([
                Constraint::Percentage(22.0),
                Constraint::Percentage(56.0),
                Constraint::Percentage(22.0),
            ])
            .gap(1)
            .split(body);

        render_header(
            header,
            self.selected_view,
            self.ticks,
            self.active_pane,
            self.summary.last_refresh_ok,
            frame,
        );
        render_navigation(cols[0], self.selected_view, self.active_pane, frame);
        render_workspace(
            cols[1],
            self.selected_view,
            &self.summary,
            self.active_pane,
            frame,
        );
        render_telemetry(cols[2], &self.summary, self.active_pane, frame);
        render_footer(footer, self.palette_open, frame);

        if self.palette_open {
            render_palette(root, self, frame);
        }
    }
}

pub async fn run(config_path: &str, view: Option<String>) -> Result<()> {
    if output::is_json() {
        anyhow::bail!("`airstack tui` is interactive and does not support --json.");
    }

    let summary = load_summary(config_path).context("Failed to load initial TUI summary")?;

    if !output::is_quiet() {
        output::line(AIRSTACK_BANNER);
        output::line("Launching embedded Airstack TUI...");
    }

    let model = AirstackTuiApp::new(config_path.to_string(), summary, view);
    let config = ProgramConfig::fullscreen().with_mouse();
    let mut program = Program::with_config(model, config)
        .context("Failed to initialize embedded FrankenTUI runtime")?;
    program.run().context("Airstack TUI runtime failed")?;
    Ok(())
}

fn refresh_cmd(config_path: String) -> Cmd<TuiMessage> {
    Cmd::task(move || TuiMessage::Refreshed(load_summary(&config_path).map_err(|e| e.to_string())))
}

fn load_summary(config_path: &str) -> Result<TuiSummary> {
    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let state = LocalState::load(&config.project.name)?;

    Ok(TuiSummary {
        project_name: config.project.name,
        project_description: config.project.description,
        server_count: config.infra.as_ref().map(|i| i.servers.len()).unwrap_or(0),
        service_count: config.services.as_ref().map(|s| s.len()).unwrap_or(0),
        cache_server_count: state.servers.len(),
        cache_service_count: state.services.len(),
        last_refresh_ok: true,
    })
}

fn parse_view_index(view: &str) -> Option<usize> {
    let normalized = view.trim().to_ascii_lowercase();
    VIEWS
        .iter()
        .position(|candidate| candidate.to_ascii_lowercase() == normalized)
}

fn handle_palette_input(
    app: &mut AirstackTuiApp,
    key: ftui::core::event::KeyEvent,
) -> Cmd<TuiMessage> {
    match key.code {
        KeyCode::Escape => {
            app.palette_open = false;
            app.palette_query.clear();
            app.palette_index = 0;
            Cmd::none()
        }
        KeyCode::Backspace => {
            app.palette_query.pop();
            app.palette_index = 0;
            Cmd::none()
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let actions = app.filtered_actions();
            if !actions.is_empty() {
                app.palette_index = (app.palette_index + 1) % actions.len();
            }
            Cmd::none()
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let actions = app.filtered_actions();
            if !actions.is_empty() {
                app.palette_index = if app.palette_index == 0 {
                    actions.len() - 1
                } else {
                    app.palette_index - 1
                };
            }
            Cmd::none()
        }
        KeyCode::Enter => {
            let actions = app.filtered_actions();
            if actions.is_empty() {
                return Cmd::none();
            }
            let (_, command) = actions[app.palette_index.min(actions.len() - 1)];
            app.palette_open = false;
            app.palette_query.clear();
            app.palette_index = 0;

            if command == "quit" {
                return Cmd::quit();
            }
            if command == "refresh" {
                return refresh_cmd(app.config_path.clone());
            }
            if let Some(view_name) = command.strip_prefix("view:") {
                if let Some(idx) = parse_view_index(view_name) {
                    app.selected_view = idx;
                }
            }
            Cmd::none()
        }
        KeyCode::Char(c) if !c.is_control() => {
            app.palette_query.push(c);
            app.palette_index = 0;
            Cmd::none()
        }
        _ => Cmd::none(),
    }
}

fn render_header(
    area: Rect,
    selected_view: usize,
    ticks: u64,
    active_pane: Pane,
    refresh_ok: bool,
    frame: &mut Frame,
) {
    let pane = match active_pane {
        Pane::Navigation => "Navigation",
        Pane::Workspace => "Workspace",
        Pane::Telemetry => "Telemetry",
    };
    let health = if refresh_ok { "SYNCED" } else { "STALE" };
    let title = format!("Airstack // {} // Focus: {}", VIEWS[selected_view], pane);
    Paragraph::new(format!(
        "{title}\nTick: {ticks} | Sync: {health} | Press q/Esc to quit"
    ))
    .style(
        Style::new()
            .fg(PackedRgba::rgb(225, 245, 235))
            .bg(PackedRgba::rgb(20, 26, 28))
            .bold(),
    )
    .block(
        Block::bordered()
            .title("Airstack Runtime")
            .border_type(BorderType::Double),
    )
    .render(area, frame);
}

fn render_navigation(area: Rect, selected_view: usize, active_pane: Pane, frame: &mut Frame) {
    let mut lines = String::new();
    for (idx, view) in VIEWS.iter().enumerate() {
        if idx == selected_view {
            lines.push_str(&format!("> {:>2}. {}\n", idx + 1, view));
        } else {
            lines.push_str(&format!("  {:>2}. {}\n", idx + 1, view));
        }
    }
    lines.push_str("\nKeys: j/k, arrows, tab\nPalette: :");

    let border_style = if matches!(active_pane, Pane::Navigation) {
        Style::new().fg(PackedRgba::rgb(102, 226, 156)).bold()
    } else {
        Style::new().fg(PackedRgba::rgb(90, 120, 110))
    };

    Paragraph::new(lines)
        .style(Style::new().fg(PackedRgba::rgb(222, 236, 229)))
        .block(
            Block::bordered()
                .title("Navigation")
                .border_type(BorderType::Rounded)
                .border_style(border_style),
        )
        .render(area, frame);
}

fn render_workspace(
    area: Rect,
    selected_view: usize,
    summary: &TuiSummary,
    active_pane: Pane,
    frame: &mut Frame,
) {
    let description = summary
        .project_description
        .clone()
        .unwrap_or_else(|| "No description configured.".to_string());

    let content = format!(
        "{}\n\nProject: {}\nDescription: {}\n\nInfra servers: {}\nServices: {}\n\nState cache servers: {}\nState cache services: {}\n\nProduction notes:\n- Dependency-aware lifecycle is active\n- JSON output mode is available in CLI\n- Drift signals are surfaced in status\n- Live summary refresh runs on ticks",
        workspace_heading(selected_view),
        summary.project_name,
        description,
        summary.server_count,
        summary.service_count,
        summary.cache_server_count,
        summary.cache_service_count
    );

    let border_style = if matches!(active_pane, Pane::Workspace) {
        Style::new().fg(PackedRgba::rgb(255, 210, 120)).bold()
    } else {
        Style::new().fg(PackedRgba::rgb(130, 120, 90))
    };

    Paragraph::new(content)
        .style(Style::new().fg(PackedRgba::rgb(246, 240, 225)))
        .block(
            Block::bordered()
                .title("Workspace")
                .border_type(BorderType::Rounded)
                .border_style(border_style),
        )
        .render(area, frame);
}

fn render_telemetry(area: Rect, summary: &TuiSummary, active_pane: Pane, frame: &mut Frame) {
    let content = format!(
        "Health: {}\n\nExpected servers: {}\nCached servers: {}\n\nExpected services: {}\nCached services: {}\n\nQuick actions:\n- up\n- deploy all\n- scale <svc> <n>\n- status --detailed\n- : -> command palette",
        if summary.last_refresh_ok {
            "SYNCED"
        } else {
            "STALE"
        },
        summary.server_count,
        summary.cache_server_count,
        summary.service_count,
        summary.cache_service_count
    );

    let border_style = if matches!(active_pane, Pane::Telemetry) {
        Style::new().fg(PackedRgba::rgb(120, 205, 255)).bold()
    } else {
        Style::new().fg(PackedRgba::rgb(95, 120, 145))
    };

    Paragraph::new(content)
        .style(Style::new().fg(PackedRgba::rgb(220, 232, 244)))
        .block(
            Block::bordered()
                .title("Telemetry")
                .border_type(BorderType::Rounded)
                .border_style(border_style),
        )
        .render(area, frame);
}

fn render_footer(area: Rect, palette_open: bool, frame: &mut Frame) {
    let message = if palette_open {
        "Palette open: type to filter, Enter to run, Esc to close"
    } else {
        "Tab: switch pane | j/k: switch view | 1..8: jump view | : command palette | q: quit"
    };
    Paragraph::new(message)
        .style(
            Style::new()
                .fg(PackedRgba::rgb(197, 210, 206))
                .bg(PackedRgba::rgb(23, 28, 30)),
        )
        .block(
            Block::bordered()
                .title("Controls")
                .border_type(BorderType::Rounded),
        )
        .render(area, frame);
}

fn render_palette(root: Rect, app: &AirstackTuiApp, frame: &mut Frame) {
    let popup = centered_rect(root, 65, 42);
    let actions = app.filtered_actions();

    let mut lines = String::new();
    lines.push_str(&format!("Query: {}\n\n", app.palette_query));
    if actions.is_empty() {
        lines.push_str("No matching actions.");
    } else {
        for (idx, (label, command)) in actions.iter().enumerate() {
            if idx == app.palette_index {
                lines.push_str(&format!("> {} ({})\n", label, command));
            } else {
                lines.push_str(&format!("  {} ({})\n", label, command));
            }
        }
    }

    let overlay_title = "Command Palette";
    Paragraph::new(lines)
        .style(
            Style::new()
                .fg(PackedRgba::rgb(240, 240, 230))
                .bg(PackedRgba::rgb(30, 32, 36)),
        )
        .block(
            Block::bordered()
                .title(overlay_title)
                .border_type(BorderType::Double)
                .border_style(Style::new().fg(PackedRgba::rgb(180, 220, 255)).bold()),
        )
        .render(popup, frame);
}

fn centered_rect(area: Rect, width_percent: u16, height_percent: u16) -> Rect {
    let vertical = Flex::vertical()
        .constraints([
            Constraint::Percentage(((100 - height_percent) / 2) as f32),
            Constraint::Percentage(height_percent as f32),
            Constraint::Percentage(((100 - height_percent) / 2) as f32),
        ])
        .split(area);
    let middle = vertical[1];
    let horizontal = Flex::horizontal()
        .constraints([
            Constraint::Percentage(((100 - width_percent) / 2) as f32),
            Constraint::Percentage(width_percent as f32),
            Constraint::Percentage(((100 - width_percent) / 2) as f32),
        ])
        .split(middle);
    horizontal[1]
}

fn workspace_heading(selected_view: usize) -> &'static str {
    match selected_view {
        0 => "Dashboard: high-level operational summary.",
        1 => "Servers: provider inventory and host-level posture.",
        2 => "Services: deployment topology and dependencies.",
        3 => "Logs: streaming tail and structured filters.",
        4 => "Scaling: replica controls and convergence feedback.",
        5 => "Network: ports, routes, and north-south flows.",
        6 => "Providers: capability matrix and auth state.",
        7 => "Settings: config, output mode, and runtime knobs.",
        _ => "Workspace",
    }
}
