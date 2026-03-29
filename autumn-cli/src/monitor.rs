//! Live monitoring TUI dashboard for Autumn applications.
//!
//! Connects to a running Autumn app's actuator endpoints and renders
//! real-time metrics, health status, and task information in a rich
//! terminal UI.

use std::collections::HashMap;
use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Bar, BarChart, BarGroup, Block, Borders, Cell, Padding, Paragraph, Row, Sparkline, Table, Tabs,
    Wrap,
};
use serde::Deserialize;

// ── Actuator response types ───────────────────────────────────

#[derive(Debug, Deserialize, Default, Clone)]
struct HealthResponse {
    status: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    profile: String,
    #[serde(default)]
    uptime: String,
    #[serde(default)]
    checks: Option<HealthChecks>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct HealthChecks {
    database: Option<DatabaseCheck>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct DatabaseCheck {
    status: String,
    pool_size: u64,
    active_connections: u64,
    idle_connections: u64,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct MetricsResponse {
    #[serde(default)]
    http: HttpMetrics,
    #[serde(default)]
    database: Option<DbPoolMetrics>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct HttpMetrics {
    #[serde(default)]
    requests_total: u64,
    #[serde(default)]
    requests_active: u64,
    #[serde(default)]
    latency_ms: LatencySnapshot,
    #[serde(default)]
    by_route: HashMap<String, RouteSnapshot>,
    #[serde(default)]
    by_status: StatusSnapshot,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct LatencySnapshot {
    #[serde(default)]
    p50: u64,
    #[serde(default)]
    p95: u64,
    #[serde(default)]
    p99: u64,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct RouteSnapshot {
    #[serde(default)]
    count: u64,
    #[serde(default)]
    p50_ms: u64,
    #[serde(default)]
    p95_ms: u64,
    #[serde(default)]
    p99_ms: u64,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct StatusSnapshot {
    #[serde(rename = "2xx", default)]
    s2xx: u64,
    #[serde(rename = "3xx", default)]
    s3xx: u64,
    #[serde(rename = "4xx", default)]
    s4xx: u64,
    #[serde(rename = "5xx", default)]
    s5xx: u64,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct DbPoolMetrics {
    #[serde(default)]
    pool_size: u64,
    #[serde(default)]
    active_connections: u64,
    #[serde(default)]
    idle_connections: u64,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct TasksResponse {
    #[serde(default)]
    scheduled_tasks: HashMap<String, TaskStatus>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct TaskStatus {
    #[serde(default)]
    schedule: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    #[allow(dead_code)]
    last_run: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    last_duration_ms: Option<u64>,
    #[serde(default)]
    #[allow(dead_code)]
    last_result: Option<String>,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    total_runs: u64,
    #[serde(default)]
    total_failures: u64,
}

// ── Dashboard state ───────────────────────────────────────────

/// Maximum sparkline history depth.
const SPARKLINE_DEPTH: usize = 120;

struct DashboardState {
    base_url: String,
    health: HealthResponse,
    metrics: MetricsResponse,
    tasks: TasksResponse,
    /// Rolling throughput samples (requests in last interval).
    throughput_history: Vec<u64>,
    /// Rolling p50 latency samples.
    latency_p50_history: Vec<u64>,
    /// Rolling p99 latency samples.
    latency_p99_history: Vec<u64>,
    /// Previous total requests for computing delta.
    prev_requests_total: u64,
    /// Whether the app is reachable.
    connected: bool,
    /// Last error message.
    last_error: Option<String>,
    /// When we last polled.
    last_poll: Instant,
    /// Route table scroll offset.
    route_scroll: usize,
    /// Currently selected tab.
    active_tab: usize,
    /// Tick counter for animations.
    tick: u64,
}

impl DashboardState {
    fn new(base_url: String) -> Self {
        Self {
            base_url,
            health: HealthResponse::default(),
            metrics: MetricsResponse::default(),
            tasks: TasksResponse::default(),
            throughput_history: Vec::with_capacity(SPARKLINE_DEPTH),
            latency_p50_history: Vec::with_capacity(SPARKLINE_DEPTH),
            latency_p99_history: Vec::with_capacity(SPARKLINE_DEPTH),
            prev_requests_total: 0,
            connected: false,
            last_error: None,
            last_poll: Instant::now(),
            route_scroll: 0,
            active_tab: 0,
            tick: 0,
        }
    }

    fn poll(&mut self) {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(2))
            .build();

        let client = match client {
            Ok(c) => c,
            Err(e) => {
                self.connected = false;
                self.last_error = Some(format!("HTTP client error: {e}"));
                return;
            }
        };

        // Fetch health
        match client
            .get(format!("{}/actuator/health", self.base_url))
            .send()
        {
            Ok(resp) if resp.status().is_success() || resp.status().as_u16() == 503 => {
                self.connected = true;
                self.last_error = None;
                if let Ok(h) = resp.json::<HealthResponse>() {
                    self.health = h;
                }
            }
            Ok(resp) => {
                self.connected = true;
                self.last_error = Some(format!("Health returned {}", resp.status()));
            }
            Err(e) => {
                self.connected = false;
                self.last_error = Some(format!("Connection failed: {e}"));
                return;
            }
        }

        // Fetch metrics
        if let Ok(resp) = client
            .get(format!("{}/actuator/metrics", self.base_url))
            .send()
        {
            if let Ok(m) = resp.json::<MetricsResponse>() {
                // Compute throughput delta
                let delta = m
                    .http
                    .requests_total
                    .saturating_sub(self.prev_requests_total);
                if self.prev_requests_total > 0 || !self.throughput_history.is_empty() {
                    self.throughput_history.push(delta);
                    if self.throughput_history.len() > SPARKLINE_DEPTH {
                        self.throughput_history.remove(0);
                    }
                }
                self.prev_requests_total = m.http.requests_total;

                // Track latency history
                self.latency_p50_history.push(m.http.latency_ms.p50);
                if self.latency_p50_history.len() > SPARKLINE_DEPTH {
                    self.latency_p50_history.remove(0);
                }
                self.latency_p99_history.push(m.http.latency_ms.p99);
                if self.latency_p99_history.len() > SPARKLINE_DEPTH {
                    self.latency_p99_history.remove(0);
                }

                self.metrics = m;
            }
        }

        // Fetch tasks (best effort, may 404 in prod mode)
        if let Ok(resp) = client
            .get(format!("{}/actuator/tasks", self.base_url))
            .send()
        {
            if let Ok(t) = resp.json::<TasksResponse>() {
                self.tasks = t;
            }
        }

        self.last_poll = Instant::now();
    }
}

// ── TUI rendering ─────────────────────────────────────────────

pub fn run(url: &str, poll_secs: u64) {
    // Normalize URL
    let base_url = url.trim_end_matches('/').to_string();
    let poll_interval = Duration::from_secs(poll_secs);

    let mut state = DashboardState::new(base_url);

    // Initial poll
    state.poll();

    // Setup terminal
    terminal::enable_raw_mode().expect("failed to enable raw mode");
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, cursor::Hide).expect("failed to setup terminal");
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).expect("failed to create terminal");

    let result = run_loop(&mut terminal, &mut state, poll_interval);

    // Restore terminal
    terminal::disable_raw_mode().expect("failed to disable raw mode");
    execute!(terminal.backend_mut(), LeaveAlternateScreen, cursor::Show)
        .expect("failed to restore terminal");

    if let Err(e) = result {
        eprintln!("Error: {e}");
    }
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut DashboardState,
    poll_interval: Duration,
) -> io::Result<()> {
    loop {
        terminal.draw(|frame| draw(frame, state))?;

        // Poll for input with short timeout for smooth animations
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Tab => {
                            state.active_tab = (state.active_tab + 1) % 2;
                        }
                        KeyCode::BackTab => {
                            state.active_tab = usize::from(state.active_tab == 0);
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            state.route_scroll = state.route_scroll.saturating_add(1);
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            state.route_scroll = state.route_scroll.saturating_sub(1);
                        }
                        KeyCode::Home | KeyCode::Char('g') => {
                            state.route_scroll = 0;
                        }
                        _ => {}
                    }
                }
            }
        }

        state.tick += 1;

        // Poll actuator endpoints
        if state.last_poll.elapsed() >= poll_interval {
            state.poll();
        }
    }
}

fn draw(frame: &mut ratatui::Frame, state: &DashboardState) {
    let area = frame.area();

    // Main layout: header, body, footer
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(10),   // body
            Constraint::Length(1), // footer
        ])
        .split(area);

    draw_header(frame, main_chunks[0], state);

    match state.active_tab {
        0 => draw_overview_tab(frame, main_chunks[1], state),
        1 => draw_routes_tab(frame, main_chunks[1], state),
        _ => {}
    }

    draw_footer(frame, main_chunks[2], state);
}

fn draw_header(frame: &mut ratatui::Frame, area: Rect, state: &DashboardState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(30), // logo
            Constraint::Min(20),    // tabs
            Constraint::Length(28), // connection status
        ])
        .split(area);

    // Logo / title
    let status_color = if !state.connected {
        Color::Red
    } else if state.health.status == "ok" {
        Color::Green
    } else {
        Color::Yellow
    };

    let title = Paragraph::new(Line::from(vec![
        Span::styled("  🍂 ", Style::default().fg(Color::Rgb(204, 120, 50))),
        Span::styled(
            "autumn",
            Style::default()
                .fg(Color::Rgb(204, 120, 50))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" monitor", Style::default().fg(Color::Gray)),
    ]))
    .block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(title, chunks[0]);

    // Tabs
    let tab_titles = vec!["Overview", "Routes"];
    let tabs = Tabs::new(tab_titles)
        .select(state.active_tab)
        .style(Style::default().fg(Color::DarkGray))
        .highlight_style(
            Style::default()
                .fg(Color::Rgb(204, 120, 50))
                .add_modifier(Modifier::BOLD),
        )
        .divider(Span::raw(" | "))
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(Color::DarkGray)),
        );
    frame.render_widget(tabs, chunks[1]);

    // Connection status
    let (indicator, label) = if !state.connected {
        ("●", "disconnected")
    } else if state.health.status == "ok" {
        ("●", "healthy")
    } else {
        ("●", "degraded")
    };

    let conn = Paragraph::new(Line::from(vec![
        Span::styled(indicator, Style::default().fg(status_color)),
        Span::raw(" "),
        Span::styled(label, Style::default().fg(status_color)),
        Span::raw("  "),
        Span::styled(
            if state.health.profile.is_empty() {
                String::new()
            } else {
                format!("[{}]", state.health.profile)
            },
            Style::default().fg(Color::DarkGray),
        ),
    ]))
    .alignment(Alignment::Right)
    .block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(conn, chunks[2]);
}

fn draw_overview_tab(frame: &mut ratatui::Frame, area: Rect, state: &DashboardState) {
    // Split into top row and bottom row
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(9),  // stats cards row
            Constraint::Length(10), // sparklines
            Constraint::Min(8),     // status codes + tasks
        ])
        .split(area);

    draw_stats_cards(frame, rows[0], state);
    draw_sparklines(frame, rows[1], state);
    draw_bottom_panels(frame, rows[2], state);
}

fn draw_stats_cards(frame: &mut ratatui::Frame, area: Rect, state: &DashboardState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
        ])
        .split(area);

    let m = &state.metrics.http;

    // Card 1: Total Requests
    let total_block = make_card_block("Total Requests");
    let total = Paragraph::new(Text::from(vec![
        Line::raw(""),
        Line::from(Span::styled(
            format_number(m.requests_total),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            format!("{} active", m.requests_active),
            Style::default().fg(Color::DarkGray),
        )),
    ]))
    .alignment(Alignment::Center)
    .block(total_block);
    frame.render_widget(total, chunks[0]);

    // Card 2: Throughput (req/s)
    let rps = state.throughput_history.last().copied().unwrap_or(0);
    let rps_block = make_card_block("Throughput");
    let rps_widget = Paragraph::new(Text::from(vec![
        Line::raw(""),
        Line::from(Span::styled(
            format!("{rps}"),
            Style::default()
                .fg(if rps > 0 {
                    Color::Green
                } else {
                    Color::DarkGray
                })
                .add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(Span::styled("req/s", Style::default().fg(Color::DarkGray))),
    ]))
    .alignment(Alignment::Center)
    .block(rps_block);
    frame.render_widget(rps_widget, chunks[1]);

    // Card 3: p50 Latency
    let p50_block = make_card_block("p50 Latency");
    let p50_widget = Paragraph::new(Text::from(vec![
        Line::raw(""),
        Line::from(Span::styled(
            format!("{}ms", m.latency_ms.p50),
            Style::default()
                .fg(latency_color(m.latency_ms.p50))
                .add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(Span::styled("median", Style::default().fg(Color::DarkGray))),
    ]))
    .alignment(Alignment::Center)
    .block(p50_block);
    frame.render_widget(p50_widget, chunks[2]);

    // Card 4: p95 Latency
    let p95_block = make_card_block("p95 Latency");
    let p95_widget = Paragraph::new(Text::from(vec![
        Line::raw(""),
        Line::from(Span::styled(
            format!("{}ms", m.latency_ms.p95),
            Style::default()
                .fg(latency_color(m.latency_ms.p95))
                .add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            "95th pct",
            Style::default().fg(Color::DarkGray),
        )),
    ]))
    .alignment(Alignment::Center)
    .block(p95_block);
    frame.render_widget(p95_widget, chunks[3]);

    // Card 5: p99 Latency
    let p99_block = make_card_block("p99 Latency");
    let p99_widget = Paragraph::new(Text::from(vec![
        Line::raw(""),
        Line::from(Span::styled(
            format!("{}ms", m.latency_ms.p99),
            Style::default()
                .fg(latency_color(m.latency_ms.p99))
                .add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            "99th pct",
            Style::default().fg(Color::DarkGray),
        )),
    ]))
    .alignment(Alignment::Center)
    .block(p99_block);
    frame.render_widget(p99_widget, chunks[4]);
}

fn draw_sparklines(frame: &mut ratatui::Frame, area: Rect, state: &DashboardState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    // Throughput sparkline
    let throughput_block = Block::default()
        .title(Span::styled(
            " Throughput (req/s) ",
            Style::default()
                .fg(Color::Rgb(204, 120, 50))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let throughput_sparkline = Sparkline::default()
        .block(throughput_block)
        .data(&state.throughput_history)
        .style(Style::default().fg(Color::Green));
    frame.render_widget(throughput_sparkline, chunks[0]);

    // Latency sparkline (p99)
    let latency_block = Block::default()
        .title(Span::styled(
            " Latency p99 (ms) ",
            Style::default()
                .fg(Color::Rgb(204, 120, 50))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let latency_sparkline = Sparkline::default()
        .block(latency_block)
        .data(&state.latency_p99_history)
        .style(Style::default().fg(Color::Rgb(255, 150, 50)));
    frame.render_widget(latency_sparkline, chunks[1]);
}

fn draw_bottom_panels(frame: &mut ratatui::Frame, area: Rect, state: &DashboardState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30), // status codes
            Constraint::Percentage(35), // health & db
            Constraint::Percentage(35), // tasks
        ])
        .split(area);

    draw_status_codes(frame, chunks[0], state);
    draw_health_panel(frame, chunks[1], state);
    draw_tasks_panel(frame, chunks[2], state);
}

fn draw_status_codes(frame: &mut ratatui::Frame, area: Rect, state: &DashboardState) {
    let s = &state.metrics.http.by_status;

    let bar_group = BarGroup::default().bars(&[
        Bar::default()
            .value(s.s2xx)
            .label("2xx".into())
            .style(Style::default().fg(Color::Green)),
        Bar::default()
            .value(s.s3xx)
            .label("3xx".into())
            .style(Style::default().fg(Color::Cyan)),
        Bar::default()
            .value(s.s4xx)
            .label("4xx".into())
            .style(Style::default().fg(Color::Yellow)),
        Bar::default()
            .value(s.s5xx)
            .label("5xx".into())
            .style(Style::default().fg(Color::Red)),
    ]);

    let chart = BarChart::default()
        .block(
            Block::default()
                .title(Span::styled(
                    " Status Codes ",
                    Style::default()
                        .fg(Color::Rgb(204, 120, 50))
                        .add_modifier(Modifier::BOLD),
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .data(bar_group)
        .bar_width(5)
        .bar_gap(2)
        .value_style(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );

    frame.render_widget(chart, area);
}

fn draw_health_panel(frame: &mut ratatui::Frame, area: Rect, state: &DashboardState) {
    let block = Block::default()
        .title(Span::styled(
            " Health & Info ",
            Style::default()
                .fg(Color::Rgb(204, 120, 50))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let mut lines = vec![
        info_line(
            "Status",
            &state.health.status,
            status_color(&state.health.status),
        ),
        info_line("Version", &state.health.version, Color::White),
        info_line("Profile", &state.health.profile, Color::Cyan),
        info_line("Uptime", &state.health.uptime, Color::White),
    ];

    // DB pool info
    if let Some(db) = &state.metrics.database {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "Database Pool",
            Style::default()
                .fg(Color::Rgb(204, 120, 50))
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
        lines.push(info_line(
            "Pool Size",
            &db.pool_size.to_string(),
            Color::White,
        ));
        lines.push(info_line(
            "Active",
            &db.active_connections.to_string(),
            Color::Yellow,
        ));
        lines.push(info_line(
            "Idle",
            &db.idle_connections.to_string(),
            Color::Green,
        ));
    } else if let Some(checks) = &state.health.checks {
        if let Some(db) = &checks.database {
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(
                "Database Pool",
                Style::default()
                    .fg(Color::Rgb(204, 120, 50))
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )));
            lines.push(info_line("DB Status", &db.status, status_color(&db.status)));
            lines.push(info_line(
                "Pool Size",
                &db.pool_size.to_string(),
                Color::White,
            ));
            lines.push(info_line(
                "Active",
                &db.active_connections.to_string(),
                Color::Yellow,
            ));
            lines.push(info_line(
                "Idle",
                &db.idle_connections.to_string(),
                Color::Green,
            ));
        }
    }

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

fn draw_tasks_panel(frame: &mut ratatui::Frame, area: Rect, state: &DashboardState) {
    let block = Block::default()
        .title(Span::styled(
            " Scheduled Tasks ",
            Style::default()
                .fg(Color::Rgb(204, 120, 50))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    if state.tasks.scheduled_tasks.is_empty() {
        let no_tasks = Paragraph::new(Text::from(vec![
            Line::raw(""),
            Line::from(Span::styled(
                "No scheduled tasks",
                Style::default().fg(Color::DarkGray),
            )),
        ]))
        .alignment(Alignment::Center)
        .block(block);
        frame.render_widget(no_tasks, area);
        return;
    }

    let mut lines = Vec::new();
    for (name, task) in &state.tasks.scheduled_tasks {
        let status_icon = match task.status.as_str() {
            "running" => Span::styled("▶ ", Style::default().fg(Color::Green)),
            "idle" => Span::styled("◆ ", Style::default().fg(Color::DarkGray)),
            _ => Span::styled("? ", Style::default().fg(Color::Yellow)),
        };

        lines.push(Line::from(vec![
            status_icon,
            Span::styled(
                name,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(&task.schedule, Style::default().fg(Color::DarkGray)),
            Span::raw("  "),
            Span::styled(
                format!("runs: {}", task.total_runs),
                Style::default().fg(Color::Cyan),
            ),
            if task.total_failures > 0 {
                Span::styled(
                    format!("  fails: {}", task.total_failures),
                    Style::default().fg(Color::Red),
                )
            } else {
                Span::raw("")
            },
        ]));

        if let Some(err) = &task.last_error {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    truncate(err, 40),
                    Style::default()
                        .fg(Color::Red)
                        .add_modifier(Modifier::ITALIC),
                ),
            ]));
        }
    }

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

fn draw_routes_tab(frame: &mut ratatui::Frame, area: Rect, state: &DashboardState) {
    let block = Block::default()
        .title(Span::styled(
            " Routes ",
            Style::default()
                .fg(Color::Rgb(204, 120, 50))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .padding(Padding::new(1, 1, 0, 0));

    let header = Row::new(vec![
        Cell::from("Route").style(
            Style::default()
                .fg(Color::Rgb(204, 120, 50))
                .add_modifier(Modifier::BOLD),
        ),
        Cell::from("Count").style(
            Style::default()
                .fg(Color::Rgb(204, 120, 50))
                .add_modifier(Modifier::BOLD),
        ),
        Cell::from("p50").style(
            Style::default()
                .fg(Color::Rgb(204, 120, 50))
                .add_modifier(Modifier::BOLD),
        ),
        Cell::from("p95").style(
            Style::default()
                .fg(Color::Rgb(204, 120, 50))
                .add_modifier(Modifier::BOLD),
        ),
        Cell::from("p99").style(
            Style::default()
                .fg(Color::Rgb(204, 120, 50))
                .add_modifier(Modifier::BOLD),
        ),
        Cell::from("Bar").style(
            Style::default()
                .fg(Color::Rgb(204, 120, 50))
                .add_modifier(Modifier::BOLD),
        ),
    ])
    .height(1)
    .bottom_margin(1);

    let mut routes: Vec<_> = state.metrics.http.by_route.iter().collect();
    routes.sort_by(|a, b| b.1.count.cmp(&a.1.count));

    let max_count = routes.first().map_or(1, |r| r.1.count.max(1));

    let rows: Vec<Row> = routes
        .iter()
        .enumerate()
        .map(|(i, (name, snap))| {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            let bar_width = ((snap.count as f64 / max_count as f64) * 20.0) as usize;
            let bar = "█".repeat(bar_width);

            let bg = if i % 2 == 0 {
                Color::Reset
            } else {
                Color::Rgb(30, 30, 30)
            };

            Row::new(vec![
                Cell::from((*name).clone()).style(Style::default().fg(Color::White)),
                Cell::from(format_number(snap.count)).style(Style::default().fg(Color::Cyan)),
                Cell::from(format!("{}ms", snap.p50_ms))
                    .style(Style::default().fg(latency_color(snap.p50_ms))),
                Cell::from(format!("{}ms", snap.p95_ms))
                    .style(Style::default().fg(latency_color(snap.p95_ms))),
                Cell::from(format!("{}ms", snap.p99_ms))
                    .style(Style::default().fg(latency_color(snap.p99_ms))),
                Cell::from(bar).style(Style::default().fg(Color::Green)),
            ])
            .style(Style::default().bg(bg))
        })
        .collect();

    let widths = [
        Constraint::Min(30),
        Constraint::Length(10),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Min(20),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    frame.render_widget(table, area);
}

fn draw_footer(frame: &mut ratatui::Frame, area: Rect, state: &DashboardState) {
    let elapsed = state.last_poll.elapsed().as_secs();

    let mut spans = vec![
        Span::styled(
            " q",
            Style::default()
                .fg(Color::Rgb(204, 120, 50))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" quit  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "Tab",
            Style::default()
                .fg(Color::Rgb(204, 120, 50))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" switch view  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "j/k",
            Style::default()
                .fg(Color::Rgb(204, 120, 50))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" scroll  ", Style::default().fg(Color::DarkGray)),
    ];

    if let Some(err) = &state.last_error {
        spans.push(Span::styled(
            format!("  ⚠ {}", truncate(err, 50)),
            Style::default().fg(Color::Red),
        ));
    } else {
        spans.push(Span::styled(
            format!("  polled {elapsed}s ago"),
            Style::default().fg(Color::DarkGray),
        ));
    }

    let footer = Paragraph::new(Line::from(spans));
    frame.render_widget(footer, area);
}

// ── Helpers ───────────────────────────────────────────────────

fn make_card_block(title: &str) -> Block<'_> {
    Block::default()
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(Color::Rgb(204, 120, 50))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
}

fn info_line(label: &str, value: &str, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {label}: "), Style::default().fg(Color::DarkGray)),
        Span::styled(value.to_string(), Style::default().fg(color)),
    ])
}

const fn latency_color(ms: u64) -> Color {
    match ms {
        0..=10 => Color::Green,
        11..=50 => Color::Cyan,
        51..=200 => Color::Yellow,
        201..=1000 => Color::Rgb(255, 150, 50),
        _ => Color::Red,
    }
}

fn status_color(status: &str) -> Color {
    match status {
        "ok" | "up" => Color::Green,
        "degraded" => Color::Yellow,
        "down" => Color::Red,
        _ => Color::DarkGray,
    }
}

#[allow(clippy::cast_precision_loss)]
fn format_number(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}...", &s[..max.saturating_sub(3)])
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_number_plain() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
    }

    #[test]
    fn format_number_thousands() {
        assert_eq!(format_number(1_500), "1.5K");
        assert_eq!(format_number(42_000), "42.0K");
    }

    #[test]
    fn format_number_millions() {
        assert_eq!(format_number(2_500_000), "2.5M");
    }

    #[test]
    fn latency_color_green_for_fast() {
        assert_eq!(latency_color(5), Color::Green);
    }

    #[test]
    fn latency_color_red_for_slow() {
        assert_eq!(latency_color(5000), Color::Red);
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        assert_eq!(truncate("hello world this is long", 10), "hello w...");
    }

    #[test]
    fn status_color_mapping() {
        assert_eq!(status_color("ok"), Color::Green);
        assert_eq!(status_color("degraded"), Color::Yellow);
        assert_eq!(status_color("down"), Color::Red);
        assert_eq!(status_color("unknown"), Color::DarkGray);
    }

    #[test]
    fn dashboard_state_initial() {
        let state = DashboardState::new("http://localhost:3000".to_string());
        assert!(!state.connected);
        assert_eq!(state.prev_requests_total, 0);
        assert!(state.throughput_history.is_empty());
    }

    #[test]
    fn deserialize_health_response() {
        let json = r#"{"status":"ok","version":"0.1.0","profile":"dev","uptime":"1h 23m"}"#;
        let health: HealthResponse = serde_json::from_str(json).unwrap();
        assert_eq!(health.status, "ok");
        assert_eq!(health.profile, "dev");
    }

    #[test]
    fn deserialize_metrics_response() {
        let json = r#"{
            "http": {
                "requests_total": 150,
                "requests_active": 3,
                "latency_ms": {"p50": 5, "p95": 25, "p99": 100},
                "by_route": {
                    "GET /": {"count": 100, "p50_ms": 3, "p95_ms": 10, "p99_ms": 50}
                },
                "by_status": {"2xx": 140, "3xx": 5, "4xx": 3, "5xx": 2}
            }
        }"#;
        let metrics: MetricsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(metrics.http.requests_total, 150);
        assert_eq!(metrics.http.by_status.s2xx, 140);
        assert_eq!(metrics.http.by_route["GET /"].count, 100);
    }

    #[test]
    fn deserialize_tasks_response() {
        let json = r#"{"scheduled_tasks":{"cleanup":{"schedule":"every 5m","status":"idle","total_runs":10,"total_failures":1}}}"#;
        let tasks: TasksResponse = serde_json::from_str(json).unwrap();
        assert_eq!(tasks.scheduled_tasks["cleanup"].total_runs, 10);
    }
}
