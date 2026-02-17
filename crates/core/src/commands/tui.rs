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

const TICK_INTERVAL: Duration = Duration::from_millis(500);

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
}

#[derive(Debug, Clone)]
struct AirstackTuiApp {
    selected_view: usize,
    active_pane: Pane,
    ticks: u64,
    summary: TuiSummary,
}

impl AirstackTuiApp {
    fn new(summary: TuiSummary, preferred_view: Option<String>) -> Self {
        let selected_view = preferred_view
            .as_deref()
            .and_then(parse_view_index)
            .unwrap_or(0);

        Self {
            selected_view,
            active_pane: Pane::Navigation,
            ticks: 0,
            summary,
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
}

impl Model for AirstackTuiApp {
    type Message = Event;

    fn init(&mut self) -> Cmd<Self::Message> {
        Cmd::tick(TICK_INTERVAL)
    }

    fn update(&mut self, msg: Self::Message) -> Cmd<Self::Message> {
        match msg {
            Event::Tick => {
                self.ticks = self.ticks.wrapping_add(1);
                Cmd::tick(TICK_INTERVAL)
            }
            Event::Key(key) => {
                if key.modifiers.contains(Modifiers::CTRL) && key.is_char('c') {
                    return Cmd::quit();
                }

                match key.code {
                    KeyCode::Escape => Cmd::quit(),
                    KeyCode::Char('q') => Cmd::quit(),
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
            frame,
        );
        render_navigation(cols[0], self.selected_view, self.active_pane, frame);
        render_workspace(
            cols[1],
            self.selected_view,
            self.summary.clone(),
            self.active_pane,
            frame,
        );
        render_telemetry(cols[2], self.summary.clone(), self.active_pane, frame);
        render_footer(footer, frame);
    }
}

pub async fn run(config_path: &str, view: Option<String>) -> Result<()> {
    if output::is_json() {
        anyhow::bail!("`airstack tui` is interactive and does not support --json.");
    }

    let config = AirstackConfig::load(config_path).context("Failed to load configuration")?;
    let state = LocalState::load(&config.project.name)?;

    let summary = TuiSummary {
        project_name: config.project.name.clone(),
        project_description: config.project.description.clone(),
        server_count: config.infra.as_ref().map(|i| i.servers.len()).unwrap_or(0),
        service_count: config.services.as_ref().map(|s| s.len()).unwrap_or(0),
        cache_server_count: state.servers.len(),
        cache_service_count: state.services.len(),
    };

    if !output::is_quiet() {
        output::line(AIRSTACK_BANNER);
        output::line("Launching embedded Airstack TUI...");
    }

    let model = AirstackTuiApp::new(summary, view);
    let config = ProgramConfig::fullscreen().with_mouse();
    let mut program = Program::with_config(model, config)
        .context("Failed to initialize embedded FrankenTUI runtime")?;
    program.run().context("Airstack TUI runtime failed")?;
    Ok(())
}

fn parse_view_index(view: &str) -> Option<usize> {
    let normalized = view.trim().to_ascii_lowercase();
    VIEWS
        .iter()
        .position(|candidate| candidate.to_ascii_lowercase() == normalized)
}

fn render_header(
    area: Rect,
    selected_view: usize,
    ticks: u64,
    active_pane: Pane,
    frame: &mut Frame,
) {
    let pane = match active_pane {
        Pane::Navigation => "Navigation",
        Pane::Workspace => "Workspace",
        Pane::Telemetry => "Telemetry",
    };
    let title = format!("Airstack // {} // Focus: {}", VIEWS[selected_view], pane);
    Paragraph::new(format!("{title}\nTick: {ticks} | Press q/Esc to quit"))
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
    lines.push_str("\nKeys: j/k, arrows, tab");

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
    summary: TuiSummary,
    active_pane: Pane,
    frame: &mut Frame,
) {
    let description = summary
        .project_description
        .unwrap_or_else(|| "No description configured.".to_string());

    let content = format!(
        "{}\n\nProject: {}\nDescription: {}\n\nInfra servers: {}\nServices: {}\n\nState cache servers: {}\nState cache services: {}\n\nProduction notes:\n- Dependency-aware lifecycle is active\n- JSON output mode is available in CLI\n- Drift signals are surfaced in status",
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

fn render_telemetry(area: Rect, summary: TuiSummary, active_pane: Pane, frame: &mut Frame) {
    let content = format!(
        "Health: DEGRADED\n\nExpected servers: {}\nCached servers: {}\n\nExpected services: {}\nCached services: {}\n\nQuick actions:\n- up\n- deploy all\n- scale <svc> <n>\n- status --detailed",
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

fn render_footer(area: Rect, frame: &mut Frame) {
    Paragraph::new("Tab: switch pane | j/k or arrows: switch view | 1..8: jump view | q/Esc: quit")
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
