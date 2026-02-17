use std::collections::BTreeSet;
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
use crate::state::{DriftReport, HealthState, LocalState};

const AIRSTACK_BANNER: &str = r#"
     _    _         _             _
    / \  (_)_ __ __| |_ __   ___ | | __
   / _ \ | | '__/ _` | '_ \ / _ \| |/ /
  / ___ \| | | | (_| | |_) | (_) |   <
 /_/   \_\_|_|  \__,_| .__/ \___/|_|\_\
                     |_|
"#;

const TICK_INTERVAL: Duration = Duration::from_millis(600);
const STONE_BG: PackedRgba = PackedRgba::rgb(31, 36, 40);
const STONE_PANEL: PackedRgba = PackedRgba::rgb(41, 47, 52);
const STONE_EDGE: PackedRgba = PackedRgba::rgb(58, 66, 74);
const STONE_MUTED: PackedRgba = PackedRgba::rgb(149, 161, 172);
const STEEL_MAIN: PackedRgba = PackedRgba::rgb(161, 194, 220);
const STEEL_BRIGHT: PackedRgba = PackedRgba::rgb(206, 226, 242);
const TEXT_MAIN: PackedRgba = PackedRgba::rgb(224, 229, 233);
const SPINNER_FRAMES: &[&str] = &["|", "/", "-", "\\"];

const VIEWS: &[&str] = &[
    "Dashboard",
    "Servers",
    "Services",
    "Logs",
    "Scaling",
    "Network",
    "Providers",
    "SSH",
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
    ("Go SSH", "view:SSH"),
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
struct TuiServer {
    name: String,
    provider: String,
    region: String,
    server_type: String,
    cached_id: Option<String>,
    cached_public_ip: Option<String>,
    cached_health: HealthState,
    cached_last_status: Option<String>,
    cached_last_checked_unix: u64,
}

#[derive(Debug, Clone)]
struct TuiService {
    name: String,
    image: String,
    ports: Vec<u16>,
    depends_on: Vec<String>,
    cached_replicas: Option<usize>,
    cached_containers: Vec<String>,
    cached_health: HealthState,
    cached_last_status: Option<String>,
    cached_last_checked_unix: u64,
}

#[derive(Debug, Clone)]
struct TuiSummary {
    project_name: String,
    project_description: Option<String>,
    state_updated_at_unix: u64,
    server_count: usize,
    service_count: usize,
    cache_server_count: usize,
    cache_service_count: usize,
    last_refresh_ok: bool,
    drift: DriftReport,
    servers: Vec<TuiServer>,
    services: Vec<TuiService>,
    providers: Vec<String>,
    healthy_count: usize,
    degraded_count: usize,
    unhealthy_count: usize,
    unknown_count: usize,
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
        render_background(root, frame);
        if root.width < 40 || root.height < 12 {
            Paragraph::new("WARN  terminal too small\n  resize to at least 40x12")
                .style(Style::new().fg(STEEL_BRIGHT).bg(STONE_BG).bold())
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
        render_navigation(
            cols[0],
            self.selected_view,
            self.active_pane,
            self.ticks,
            frame,
        );
        render_workspace(
            cols[1],
            self.selected_view,
            &self.summary,
            self.active_pane,
            frame,
        );
        render_telemetry(cols[2], &self.summary, self.active_pane, self.ticks, frame);
        render_footer(footer, self.palette_open, self.ticks, frame);

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
    let drift = state.detect_drift(&config);

    let servers = config
        .infra
        .as_ref()
        .map(|infra| {
            infra
                .servers
                .iter()
                .map(|server| {
                    let cached = state.servers.get(&server.name);
                    TuiServer {
                        name: server.name.clone(),
                        provider: server.provider.clone(),
                        region: server.region.clone(),
                        server_type: server.server_type.clone(),
                        cached_id: cached.and_then(|s| s.id.clone()),
                        cached_public_ip: cached.and_then(|s| s.public_ip.clone()),
                        cached_health: cached.map(|s| s.health).unwrap_or(HealthState::Unknown),
                        cached_last_status: cached.and_then(|s| s.last_status.clone()),
                        cached_last_checked_unix: cached.map(|s| s.last_checked_unix).unwrap_or(0),
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut services = config
        .services
        .as_ref()
        .map(|svc| {
            svc.iter()
                .map(|(name, cfg)| {
                    let cached = state.services.get(name);
                    TuiService {
                        name: name.clone(),
                        image: cfg.image.clone(),
                        ports: cfg.ports.clone(),
                        depends_on: cfg.depends_on.clone().unwrap_or_default(),
                        cached_replicas: cached.map(|s| s.replicas),
                        cached_containers: cached.map(|s| s.containers.clone()).unwrap_or_default(),
                        cached_health: cached.map(|s| s.health).unwrap_or(HealthState::Unknown),
                        cached_last_status: cached.and_then(|s| s.last_status.clone()),
                        cached_last_checked_unix: cached.map(|s| s.last_checked_unix).unwrap_or(0),
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    services.sort_by(|a, b| a.name.cmp(&b.name));

    let mut providers = BTreeSet::new();
    for server in &servers {
        providers.insert(server.provider.clone());
    }
    providers.insert("docker".to_string());

    let mut healthy_count = 0usize;
    let mut degraded_count = 0usize;
    let mut unhealthy_count = 0usize;
    let mut unknown_count = 0usize;
    for srv in state.servers.values() {
        match srv.health {
            HealthState::Healthy => healthy_count += 1,
            HealthState::Degraded => degraded_count += 1,
            HealthState::Unhealthy => unhealthy_count += 1,
            HealthState::Unknown => unknown_count += 1,
        }
    }
    for svc in state.services.values() {
        match svc.health {
            HealthState::Healthy => healthy_count += 1,
            HealthState::Degraded => degraded_count += 1,
            HealthState::Unhealthy => unhealthy_count += 1,
            HealthState::Unknown => unknown_count += 1,
        }
    }

    Ok(TuiSummary {
        project_name: config.project.name,
        project_description: config.project.description,
        state_updated_at_unix: state.updated_at_unix,
        server_count: servers.len(),
        service_count: services.len(),
        cache_server_count: state.servers.len(),
        cache_service_count: state.services.len(),
        last_refresh_ok: true,
        drift,
        servers,
        services,
        providers: providers.into_iter().collect(),
        healthy_count,
        degraded_count,
        unhealthy_count,
        unknown_count,
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
    let header = render_panel(area, "Airstack Runtime", true, frame);
    let pane = match active_pane {
        Pane::Navigation => "Navigation",
        Pane::Workspace => "Workspace",
        Pane::Telemetry => "Telemetry",
    };
    let health = if refresh_ok { "SYNCED" } else { "STALE" };
    let spin = spinner_frame(ticks);
    let shimmer = shimmer_line(header.width as usize, ticks);
    let meter = pulse_meter(16, ticks);
    Paragraph::new(format!(
        "view: {:<10}  focus: {:<10}  tick: {:<8}  sync:{} {}\n{}\nmotion: [{}]\n\nkeys: q/esc quit | : palette | tab pane switch",
        VIEWS[selected_view], pane, ticks, health, spin, shimmer, meter
    ))
    .style(Style::new().fg(STEEL_BRIGHT).bg(STONE_PANEL).bold())
    .render(header, frame);
}

fn render_navigation(
    area: Rect,
    selected_view: usize,
    active_pane: Pane,
    ticks: u64,
    frame: &mut Frame,
) {
    let nav = render_panel(
        area,
        "Views",
        matches!(active_pane, Pane::Navigation),
        frame,
    );
    let mut lines = String::new();
    lines.push_str("index  name\n");
    lines.push_str("-----  ----------------\n");
    let nav_cursor = if ticks % 2 == 0 { ">" } else { ">>" };
    for (idx, view) in VIEWS.iter().enumerate() {
        if idx == selected_view {
            lines.push_str(&format!("{}{:>3}   {}\n", nav_cursor, idx + 1, view));
        } else {
            lines.push_str(&format!(" {:>3}   {}\n", idx + 1, view));
        }
    }
    lines.push_str(&format!(
        "\nj/k or arrows: move\n1..9: jump\nspin: {}",
        spinner_frame(ticks)
    ));

    Paragraph::new(lines)
        .style(Style::new().fg(TEXT_MAIN).bg(STONE_PANEL))
        .render(nav, frame);
}

fn render_workspace(
    area: Rect,
    selected_view: usize,
    summary: &TuiSummary,
    active_pane: Pane,
    frame: &mut Frame,
) {
    let workspace = render_panel(
        area,
        VIEWS[selected_view],
        matches!(active_pane, Pane::Workspace),
        frame,
    );
    let description = summary
        .project_description
        .clone()
        .unwrap_or_else(|| "No description configured.".to_string());

    let content = match selected_view {
        0 => render_dashboard_view(summary, &description),
        1 => render_servers_view(summary),
        2 => render_services_view(summary),
        3 => render_logs_view(summary),
        4 => render_scaling_view(summary),
        5 => render_network_view(summary),
        6 => render_providers_view(summary),
        7 => render_ssh_view(summary),
        8 => render_settings_view(summary),
        _ => "Workspace".to_string(),
    };

    Paragraph::new(content)
        .style(Style::new().fg(TEXT_MAIN).bg(STONE_PANEL))
        .render(workspace, frame);
}

fn render_dashboard_view(summary: &TuiSummary, description: &str) -> String {
    format!(
        "project\n  name: {}\n  description: {}\n\ninventory\n  servers  desired:{}  cached:{}\n  services desired:{}  cached:{}\n\nhealth snapshot\n  healthy:{}  degraded:{}  unhealthy:{}  unknown:{}\n\ndrift\n  missing servers:{}  extra servers:{}\n  missing services:{}  extra services:{}\n\ncache timestamp\n  updated_at_unix:{}",
        summary.project_name,
        description,
        summary.server_count,
        summary.cache_server_count,
        summary.service_count,
        summary.cache_service_count,
        summary.healthy_count,
        summary.degraded_count,
        summary.unhealthy_count,
        summary.unknown_count,
        summary.drift.missing_servers_in_cache.len(),
        summary.drift.extra_servers_in_cache.len(),
        summary.drift.missing_services_in_cache.len(),
        summary.drift.extra_services_in_cache.len(),
        summary.state_updated_at_unix,
    )
}

fn render_servers_view(summary: &TuiSummary) -> String {
    let mut lines = vec!["server inventory".to_string(), String::new()];

    if summary.servers.is_empty() {
        lines.push("no servers defined in config".to_string());
    } else {
        for server in &summary.servers {
            let cached = if server.cached_id.is_some() || server.cached_public_ip.is_some() {
                "cached"
            } else {
                "not-cached"
            };
            lines.push(format!(
                "{} [{}] {} {}\n  health:{}  cache:{}",
                server.name,
                server.provider,
                server.region,
                server.server_type,
                server.cached_health.as_str(),
                cached
            ));
            if let Some(id) = &server.cached_id {
                lines.push(format!("  id: {}", id));
            }
            if let Some(ip) = &server.cached_public_ip {
                lines.push(format!("  public_ip: {}", ip));
            }
            if let Some(status) = &server.cached_last_status {
                lines.push(format!(
                    "  last_status: {} @ {}",
                    status, server.cached_last_checked_unix
                ));
            }
            lines.push(String::new());
        }
    }

    lines.join("\n")
}

fn render_services_view(summary: &TuiSummary) -> String {
    let mut lines = vec!["service topology".to_string(), String::new()];

    if summary.services.is_empty() {
        lines.push("no services defined in config".to_string());
    } else {
        for service in &summary.services {
            let deps = if service.depends_on.is_empty() {
                "none".to_string()
            } else {
                service.depends_on.join(",")
            };
            let cached_replicas = service
                .cached_replicas
                .map(|n| n.to_string())
                .unwrap_or_else(|| "n/a".to_string());
            lines.push(format!(
                "{}\n  image:{}\n  ports:{:?}\n  deps:{}\n  replicas:{}  health:{}",
                service.name,
                service.image,
                service.ports,
                deps,
                cached_replicas,
                service.cached_health.as_str()
            ));
            if !service.cached_containers.is_empty() {
                lines.push(format!(
                    "  containers: {}",
                    service.cached_containers.join(", ")
                ));
            }
            if let Some(status) = &service.cached_last_status {
                lines.push(format!(
                    "  last_status: {} @ {}",
                    status, service.cached_last_checked_unix
                ));
            }
            lines.push(String::new());
        }
    }

    lines.join("\n")
}

fn render_logs_view(summary: &TuiSummary) -> String {
    let hot_service = summary
        .services
        .first()
        .map(|s| s.name.as_str())
        .unwrap_or("<service>");

    format!(
        "quick commands\n  airstack logs {hot_service} --follow\n  airstack logs {hot_service} --tail 200\n  airstack --json logs {hot_service} --tail 100\n\nstream state\n  refresh:{}  cached_services:{}",
        if summary.last_refresh_ok { "healthy" } else { "stale" },
        summary.cache_service_count,
    )
}

fn render_scaling_view(summary: &TuiSummary) -> String {
    let mut lines = vec!["scaling plan".to_string(), String::new()];

    if summary.services.is_empty() {
        lines.push("no services available for scaling".to_string());
        return lines.join("\n");
    }

    for service in &summary.services {
        let cached_replicas = service
            .cached_replicas
            .map(|n| n.to_string())
            .unwrap_or_else(|| "n/a".to_string());
        let signal = match service.cached_replicas {
            Some(1) => "in-sync",
            Some(_) => "scaled",
            None => "not-deployed",
        };
        lines.push(format!(
            "{}\n  desired:1  cached:{}  state:{}",
            service.name, cached_replicas, signal
        ));
        lines.push(String::new());
    }
    lines.push("command: airstack scale <service> <replicas>".to_string());

    lines.join("\n")
}

fn render_network_view(summary: &TuiSummary) -> String {
    let mut lines = vec!["network exposure".to_string(), String::new()];

    if summary.services.is_empty() {
        lines.push("no service ports configured".to_string());
    } else {
        for service in &summary.services {
            if service.ports.is_empty() {
                lines.push(format!("{}: no exposed ports", service.name));
                continue;
            }
            let ports = service
                .ports
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join(",");
            lines.push(format!("{}: ports {}", service.name, ports));
        }
    }

    lines.push(String::new());
    lines.push("proxy/load-balancer integration planned".to_string());
    lines.join("\n")
}

fn render_providers_view(summary: &TuiSummary) -> String {
    let mut lines = vec!["providers".to_string(), String::new()];

    for provider in &summary.providers {
        let capability = if provider == "docker" {
            "container-runtime"
        } else {
            "infrastructure"
        };
        lines.push(format!("{} ({})", provider, capability));
    }

    if summary.providers.is_empty() {
        lines.push("no providers discovered from config/state".to_string());
    }

    lines.push(String::new());
    lines.push("provider discovery and capability flags remain in roadmap".to_string());
    lines.join("\n")
}

fn render_ssh_view(summary: &TuiSummary) -> String {
    let mut lines = vec![
        "workspace.ssh".to_string(),
        String::new(),
        "embedded terminal panel is bootstrapped".to_string(),
        "full session multiplexing remains planned".to_string(),
        String::new(),
        "server targets".to_string(),
    ];

    if summary.servers.is_empty() {
        lines.push("  - none".to_string());
    } else {
        for server in &summary.servers {
            lines.push(format!(
                "  - {} ({}/{})",
                server.name, server.provider, server.region
            ));
        }
    }

    lines.push(String::new());
    lines.push("command: airstack ssh <server> [command ...]".to_string());
    lines.join("\n")
}

fn render_settings_view(summary: &TuiSummary) -> String {
    format!(
        "runtime settings\n  project:{}\n  refresh_interval:{}ms\n  json_mode:unsupported in tui\n  quiet_banner:{}\n\nnotes\n  - live refresh on periodic tick\n  - cached state drift surfaced in telemetry\n  - command palette supports view jumps and refresh",
        summary.project_name,
        TICK_INTERVAL.as_millis(),
        if output::is_quiet() { "enabled" } else { "disabled" }
    )
}

fn render_telemetry(
    area: Rect,
    summary: &TuiSummary,
    active_pane: Pane,
    ticks: u64,
    frame: &mut Frame,
) {
    let telemetry = render_panel(
        area,
        "Telemetry",
        matches!(active_pane, Pane::Telemetry),
        frame,
    );
    let drift_lines = [
        ("Missing srv", &summary.drift.missing_servers_in_cache),
        ("Extra srv", &summary.drift.extra_servers_in_cache),
        ("Missing svc", &summary.drift.missing_services_in_cache),
        ("Extra svc", &summary.drift.extra_services_in_cache),
    ];

    let focus = if matches!(active_pane, Pane::Telemetry) {
        "FOCUS"
    } else {
        "idle"
    };
    let mut content = format!(
        "pane:{}\n\nsync:{} {}\nservers expected:{} cached:{}\nservices expected:{} cached:{}\n\nhealth totals\n  healthy:{}  degraded:{}\n  unhealthy:{}  unknown:{}\n\nactivity:{}\n\ndrift",
        focus,
        if summary.last_refresh_ok {
            "SYNCED"
        } else {
            "STALE"
        },
        spinner_frame(ticks),
        summary.server_count,
        summary.cache_server_count,
        summary.service_count,
        summary.cache_service_count,
        summary.healthy_count,
        summary.degraded_count,
        summary.unhealthy_count,
        summary.unknown_count,
        pulse_meter(12, ticks),
    );

    for (label, items) in drift_lines {
        if items.is_empty() {
            content.push_str(&format!("\n  {}: none", label));
        } else {
            content.push_str(&format!("\n  {}: {}", label, items.join(", ")));
        }
    }

    Paragraph::new(content)
        .style(Style::new().fg(TEXT_MAIN).bg(STONE_PANEL))
        .render(telemetry, frame);
}

fn render_footer(area: Rect, palette_open: bool, ticks: u64, frame: &mut Frame) {
    let footer = render_panel(area, "Controls", false, frame);
    let message = if palette_open {
        format!(
            "PALETTE mode {} | type filter | Enter run | Esc close",
            spinner_frame(ticks)
        )
    } else {
        format!(
            "Tab focus | j/k view | 1..9 jump | : palette | q quit | {}",
            shimmer_line(14, ticks)
        )
    };
    Paragraph::new(message)
        .style(Style::new().fg(STONE_MUTED).bg(STONE_PANEL))
        .render(footer, frame);
}

fn render_palette(root: Rect, app: &AirstackTuiApp, frame: &mut Frame) {
    let popup = centered_rect(root, 65, 42);
    let inner = render_panel(popup, "Command Palette", true, frame);
    let actions = app.filtered_actions();

    let mut lines = String::new();
    lines.push_str(&format!("palette.query = {}\n\n", app.palette_query));
    if actions.is_empty() {
        lines.push_str("  WARN  no matching actions");
    } else {
        for (idx, (label, command)) in actions.iter().enumerate() {
            if idx == app.palette_index {
                lines.push_str(&format!("  > {} ({})\n", label, command));
            } else {
                lines.push_str(&format!("    {} ({})\n", label, command));
            }
        }
    }

    Paragraph::new(lines)
        .style(Style::new().fg(TEXT_MAIN).bg(STONE_PANEL).bold())
        .render(inner, frame);
}

fn spinner_frame(ticks: u64) -> &'static str {
    SPINNER_FRAMES[(ticks as usize) % SPINNER_FRAMES.len()]
}

fn pulse_meter(width: usize, ticks: u64) -> String {
    if width == 0 {
        return String::new();
    }
    let head = (ticks as usize) % width;
    let mut out = String::with_capacity(width);
    for idx in 0..width {
        if idx == head {
            out.push('>');
        } else if idx < head {
            out.push('=');
        } else {
            out.push('.');
        }
    }
    out
}

fn shimmer_line(width: usize, ticks: u64) -> String {
    if width < 4 {
        return "-".repeat(width);
    }
    let mut chars = vec!['-'; width];
    let pos = (ticks as usize) % width;
    chars[pos] = '*';
    chars.into_iter().collect()
}

fn render_panel(area: Rect, title: &str, focused: bool, frame: &mut Frame) -> Rect {
    Paragraph::new("")
        .style(Style::new().bg(STONE_PANEL))
        .render(area, frame);

    Block::bordered()
        .title(title)
        .border_type(BorderType::Square)
        .border_style(
            Style::new()
                .fg(if focused { STEEL_MAIN } else { STONE_EDGE })
                .bg(STONE_PANEL),
        )
        .render(area, frame);

    inset(area, 1)
}

fn inset(area: Rect, pad: u16) -> Rect {
    let width = area.width.saturating_sub(pad.saturating_mul(2)).max(1);
    let height = area.height.saturating_sub(pad.saturating_mul(2)).max(1);
    Rect::new(
        area.x.saturating_add(pad),
        area.y.saturating_add(pad),
        width,
        height,
    )
}

fn render_background(area: Rect, frame: &mut Frame) {
    Paragraph::new("")
        .style(Style::new().bg(STONE_BG))
        .render(area, frame);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_summary() -> TuiSummary {
        TuiSummary {
            project_name: "demo".to_string(),
            project_description: Some("demo project".to_string()),
            state_updated_at_unix: 1_700_000_000,
            server_count: 1,
            service_count: 2,
            cache_server_count: 1,
            cache_service_count: 2,
            last_refresh_ok: true,
            drift: DriftReport {
                missing_servers_in_cache: vec!["srv-missing".to_string()],
                extra_servers_in_cache: vec![],
                missing_services_in_cache: vec!["svc-missing".to_string()],
                extra_services_in_cache: vec!["svc-extra".to_string()],
            },
            servers: vec![TuiServer {
                name: "srv-1".to_string(),
                provider: "hetzner".to_string(),
                region: "nbg1".to_string(),
                server_type: "cx21".to_string(),
                cached_id: Some("123".to_string()),
                cached_public_ip: Some("1.2.3.4".to_string()),
                cached_health: HealthState::Healthy,
                cached_last_status: Some("Running".to_string()),
                cached_last_checked_unix: 1_700_000_100,
            }],
            services: vec![
                TuiService {
                    name: "api".to_string(),
                    image: "api:v1".to_string(),
                    ports: vec![3000],
                    depends_on: vec!["db".to_string()],
                    cached_replicas: Some(2),
                    cached_containers: vec!["api".to_string(), "api-2".to_string()],
                    cached_health: HealthState::Healthy,
                    cached_last_status: Some("Running".to_string()),
                    cached_last_checked_unix: 1_700_000_120,
                },
                TuiService {
                    name: "db".to_string(),
                    image: "postgres:15".to_string(),
                    ports: vec![5432],
                    depends_on: vec![],
                    cached_replicas: Some(1),
                    cached_containers: vec!["db".to_string()],
                    cached_health: HealthState::Degraded,
                    cached_last_status: Some("Restarting".to_string()),
                    cached_last_checked_unix: 1_700_000_110,
                },
            ],
            providers: vec!["docker".to_string(), "hetzner".to_string()],
            healthy_count: 2,
            degraded_count: 1,
            unhealthy_count: 0,
            unknown_count: 0,
        }
    }

    #[test]
    fn parse_view_index_handles_case_insensitive_names() {
        assert_eq!(parse_view_index("dashboard"), Some(0));
        assert_eq!(parse_view_index("SSH"), Some(7));
        assert_eq!(parse_view_index("settings"), Some(8));
        assert_eq!(parse_view_index("unknown"), None);
    }

    #[test]
    fn filtered_actions_matches_label_and_command() {
        let mut app = AirstackTuiApp::new("airstack.toml".to_string(), sample_summary(), None);
        app.palette_query = "ssh".to_string();
        let ssh_actions = app.filtered_actions();
        assert!(ssh_actions.iter().any(|(label, _)| *label == "Go SSH"));

        app.palette_query = "refresh".to_string();
        let refresh_actions = app.filtered_actions();
        assert!(refresh_actions
            .iter()
            .any(|(_, command)| *command == "refresh"));
    }

    #[test]
    fn dashboard_view_includes_drift_counts() {
        let summary = sample_summary();
        let rendered = render_dashboard_view(&summary, "demo project");
        assert!(rendered.contains("missing servers:1"));
        assert!(rendered.contains("missing services:1"));
        assert!(rendered.contains("extra services:1"));
    }

    #[test]
    fn services_view_includes_dependency_and_replicas() {
        let summary = sample_summary();
        let rendered = render_services_view(&summary);
        assert!(rendered.contains("api\n  image:api:v1"));
        assert!(rendered.contains("deps:db"));
        assert!(rendered.contains("replicas:2"));
    }

    #[test]
    fn ssh_view_includes_servers_and_command_hint() {
        let summary = sample_summary();
        let rendered = render_ssh_view(&summary);
        assert!(rendered.contains("srv-1 (hetzner/nbg1)"));
        assert!(rendered.contains("airstack ssh <server> [command ...]"));
    }
}
