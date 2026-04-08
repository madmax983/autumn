//! TUI load testing tool for the Autumn CLI.
//!
//! Provides a simple concurrent HTTP request generator.

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, Paragraph, Sparkline};
use reqwest::blocking::Client;
use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

pub struct Stats {
    pub min_ms: u64,
    pub max_ms: u64,
    pub avg_ms: f64,
    pub p50_ms: u64,
    pub p95_ms: u64,
    pub total_requests: u64,
    pub successful_requests: u64,
}

pub fn calculate_stats(mut latencies: Vec<u64>) -> Stats {
    if latencies.is_empty() {
        return Stats {
            min_ms: 0,
            max_ms: 0,
            avg_ms: 0.0,
            p50_ms: 0,
            p95_ms: 0,
            total_requests: 0,
            successful_requests: 0, // This is set separately usually
        };
    }

    latencies.sort_unstable();

    #[allow(clippy::cast_precision_loss)]
    let total_requests = latencies.len() as u64;
    let sum: u64 = latencies.iter().sum();
    #[allow(clippy::cast_precision_loss)]
    let avg_ms = sum as f64 / total_requests as f64;
    let min_ms = latencies[0];
    let max_ms = latencies[latencies.len() - 1];

    #[allow(clippy::cast_precision_loss, clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let p50_index = (total_requests as f64 * 0.50).round() as usize;
    let p50_ms = if p50_index > 0 && p50_index <= latencies.len() {
        latencies[p50_index - 1]
    } else {
        0
    };

    #[allow(clippy::cast_precision_loss, clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let p95_index = (total_requests as f64 * 0.95).round() as usize;
    let p95_ms = if p95_index > 0 && p95_index <= latencies.len() {
        latencies[p95_index - 1]
    } else {
        0
    };

    Stats {
        min_ms,
        max_ms,
        avg_ms,
        p50_ms,
        p95_ms,
        total_requests,
        successful_requests: 0, // Updated by caller
    }
}

pub enum RequestResult {
    Success(u64), // latency in ms
    Failure,
}

pub fn run(url: &str, concurrency: u64, requests: u64) {
    if let Err(e) = run_inner(url, concurrency, requests) {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

pub fn run_inner(url: &str, concurrency: u64, total_requests: u64) -> Result<(), String> {
    if concurrency == 0 {
        return Err("Concurrency must be greater than 0".to_string());
    }

    // Safety check to prevent OOM on very large requests, limit to 10M
    if total_requests > 10_000_000 {
        return Err("Requests limit exceeded. Maximum allowed is 10,000,000".to_string());
    }

    let client = Arc::new(Client::new());
    let requests_per_worker = total_requests / concurrency;
    let extra_requests = total_requests % concurrency;

    let (tx, rx): (Sender<RequestResult>, Receiver<RequestResult>) = mpsc::channel();

    let mut handles = vec![];

    let url_arc = Arc::new(url.to_string());

    for i in 0..concurrency {
        let client_clone = Arc::clone(&client);
        let url_clone = Arc::clone(&url_arc);
        let tx_clone = tx.clone();

        let worker_requests = if i == 0 {
            requests_per_worker + extra_requests
        } else {
            requests_per_worker
        };

        let handle = thread::spawn(move || {
            for _ in 0..worker_requests {
                let start = Instant::now();
                match client_clone.get(&*url_clone).send() {
                    Ok(res) => {
                        let elapsed = u64::try_from(start.elapsed().as_millis()).unwrap_or(0);
                        if res.status().is_success() {
                            let _ = tx_clone.send(RequestResult::Success(elapsed));
                        } else {
                            let _ = tx_clone.send(RequestResult::Failure);
                        }
                    }
                    Err(_) => {
                        let _ = tx_clone.send(RequestResult::Failure);
                    }
                }
            }
        });

        handles.push(handle);
    }

    // We drop our transmitter so the loop below terminates
    drop(tx);

    let result = run_tui(url, total_requests, &rx);

    for handle in handles {
        handle.join().map_err(|_| "Thread panicked")?;
    }

    result
}

struct TuiState {
    url: String,
    total_requested: u64,
    completed: u64,
    failures: u64,
    latencies: Vec<u64>,
    rps_history: VecDeque<u64>,
    last_tick_completed: u64,
    last_tick_time: Instant,
}

impl TuiState {
    fn new(url: String, total_requested: u64) -> Self {
        Self {
            url,
            total_requested,
            completed: 0,
            failures: 0,
            latencies: Vec::with_capacity(usize::try_from(total_requested).unwrap_or(0)),
            rps_history: VecDeque::from(vec![0; 120]), // Max 120 width
            last_tick_completed: 0,
            last_tick_time: Instant::now(),
        }
    }

    fn update(&mut self, result: &RequestResult) {
        self.completed += 1;
        match result {
            RequestResult::Success(latency) => self.latencies.push(*latency),
            RequestResult::Failure => self.failures += 1,
        }
    }

    fn tick_rps(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_tick_time).as_secs_f64();

        // Prevent division by zero and excessive spam
        if elapsed >= 0.5 {
            let newly_completed = self.completed.saturating_sub(self.last_tick_completed);
            #[allow(clippy::cast_precision_loss, clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            let rps = (newly_completed as f64 / elapsed).round() as u64;

            self.rps_history.pop_front();
            self.rps_history.push_back(rps);

            self.last_tick_completed = self.completed;
            self.last_tick_time = now;
        }
    }
}

fn run_tui(url: &str, total_requests: u64, rx: &Receiver<RequestResult>) -> Result<(), String> {
    terminal::enable_raw_mode().map_err(|e| format!("Failed to enable raw mode: {e}"))?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, cursor::Hide)
        .map_err(|e| format!("Failed to initialize terminal: {e}"))?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal =
        Terminal::new(backend).map_err(|e| format!("Failed to create terminal: {e}"))?;

    let mut state = TuiState::new(url.to_string(), total_requests);

    let ui_res = ui_loop(&mut terminal, &mut state, rx);

    // Cleanup terminal
    terminal::disable_raw_mode().map_err(|e| format!("Failed to disable raw mode: {e}"))?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, cursor::Show)
        .map_err(|e| format!("Failed to cleanup terminal: {e}"))?;

    ui_res?;

    // Print final summary
    let mut final_stats = calculate_stats(state.latencies);
    final_stats.total_requests = state.completed;
    final_stats.successful_requests = state.completed.saturating_sub(state.failures);

    println!("\n🚀 Load Test Complete: {url}");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!(
        "Requests: {} total, {} successful, {} failed",
        final_stats.total_requests, final_stats.successful_requests, state.failures
    );
    println!(
        "Latency:  min {}ms, max {}ms, avg {:.2}ms",
        final_stats.min_ms, final_stats.max_ms, final_stats.avg_ms
    );
    println!(
        "          p50 {}ms, p95 {}ms",
        final_stats.p50_ms, final_stats.p95_ms
    );
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    Ok(())
}

fn ui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut TuiState,
    rx: &Receiver<RequestResult>,
) -> Result<(), String> {
    let tick_rate = Duration::from_millis(50);
    let mut last_tick = Instant::now();

    loop {
        // Drain incoming messages
        loop {
            match rx.try_recv() {
                Ok(res) => state.update(&res),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    // Channels disconnected means all workers have finished or panicked.
                    // If we haven't hit the target, it means a worker panicked. We should exit.
                    if state.completed < state.total_requested {
                        return Err("Worker threads died unexpectedly before completing all requests".to_string());
                    }
                    break;
                }
            }
        }

        // Periodic state updates (e.g. RPS calculation)
        if last_tick.elapsed() >= tick_rate {
            state.tick_rps();
            last_tick = Instant::now();
        }

        terminal
            .draw(|f| draw_ui(f, state))
            .map_err(|e| format!("Failed to draw UI: {e}"))?;

        // Process user input if available
        if event::poll(Duration::from_millis(10)).unwrap_or(false) {
            if let Event::Key(key) = event::read().unwrap() {
                if key.kind == KeyEventKind::Press && key.code == KeyCode::Char('q') {
                    return Err("Load test cancelled by user".to_string());
                }
            }
        }

        if state.completed >= state.total_requested {
            // Do one final draw to show 100%
            terminal
                .draw(|f| draw_ui(f, state))
                .map_err(|e| format!("Failed to draw UI: {e}"))?;

            // Brief pause so user sees completion
            thread::sleep(Duration::from_millis(500));
            break;
        }
    }

    Ok(())
}

fn draw_ui(f: &mut ratatui::Frame, state: &mut TuiState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(2)
        .constraints([
            Constraint::Length(3), // Header/Progress
            Constraint::Length(5), // Sparkline
            Constraint::Min(4),    // Stats
        ])
        .split(f.area());

    // 1. Progress Bar
    #[allow(clippy::cast_precision_loss)]
    let ratio = if state.total_requested == 0 {
        1.0
    } else {
        (state.completed as f64 / state.total_requested as f64).clamp(0.0, 1.0)
    };

    let gauge_label = format!("{} / {} Requests", state.completed, state.total_requested);
    let gauge = Gauge::default()
        .block(
            Block::default()
                .title(format!(" 🚀 Load Testing {} ", state.url))
                .borders(Borders::ALL),
        )
        .gauge_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .ratio(ratio)
        .label(gauge_label);
    f.render_widget(gauge, chunks[0]);

    // 2. Sparkline
    let rps_slice = state.rps_history.make_contiguous();
    // We must pass an array or slice of non-mutable values, so we just map it.
    let rps_data: Vec<u64> = rps_slice.to_vec();
    let current_rps = rps_data.last().copied().unwrap_or(0);
    let sparkline = Sparkline::default()
        .block(
            Block::default()
                .title(format!(" RPS (Current: {current_rps}) "))
                .borders(Borders::ALL),
        )
        .data(rps_data.as_slice())
        .style(Style::default().fg(Color::Green));
    f.render_widget(sparkline, chunks[1]);

    // 3. Stats Output
    let current_stats = calculate_stats(state.latencies.clone());

    let stats_text = vec![
        Line::from(vec![
            Span::styled("Success: ", Style::default().fg(Color::Green)),
            Span::raw(format!(
                "{}    ",
                state.completed.saturating_sub(state.failures)
            )),
            Span::styled("Failures: ", Style::default().fg(Color::Red)),
            Span::raw(format!("{}    ", state.failures)),
            Span::styled("Total: ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{}", state.completed)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Min: ", Style::default().fg(Color::Gray)),
            Span::raw(format!("{}ms    ", current_stats.min_ms)),
            Span::styled("Max: ", Style::default().fg(Color::Gray)),
            Span::raw(format!("{}ms    ", current_stats.max_ms)),
            Span::styled("Avg: ", Style::default().fg(Color::Gray)),
            Span::raw(format!("{:.2}ms", current_stats.avg_ms)),
        ]),
        Line::from(vec![
            Span::styled("p50: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{}ms    ", current_stats.p50_ms)),
            Span::styled("p95: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{}ms", current_stats.p95_ms)),
        ]),
    ];

    let info = Paragraph::new(stats_text).block(
        Block::default()
            .title(" Live Stats (Press 'q' to quit) ")
            .borders(Borders::ALL),
    );
    f.render_widget(info, chunks[2]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::float_cmp)]
    fn test_calculate_stats_empty() {
        let stats = calculate_stats(vec![]);
        assert_eq!(stats.min_ms, 0);
        assert_eq!(stats.max_ms, 0);
        assert_eq!(stats.avg_ms, 0.0);
        assert_eq!(stats.p50_ms, 0);
        assert_eq!(stats.p95_ms, 0);
        assert_eq!(stats.total_requests, 0);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn test_calculate_stats_single() {
        let stats = calculate_stats(vec![42]);
        assert_eq!(stats.min_ms, 42);
        assert_eq!(stats.max_ms, 42);
        assert_eq!(stats.avg_ms, 42.0);
        assert_eq!(stats.p50_ms, 42);
        assert_eq!(stats.p95_ms, 42);
        assert_eq!(stats.total_requests, 1);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn test_calculate_stats_multiple() {
        // [10, 20, 30, 40, 50, 60, 70, 80, 90, 100]
        let latencies = vec![50, 10, 100, 20, 90, 30, 80, 40, 70, 60];
        let stats = calculate_stats(latencies);

        assert_eq!(stats.min_ms, 10);
        assert_eq!(stats.max_ms, 100);
        assert_eq!(stats.avg_ms, 55.0);
        assert_eq!(stats.total_requests, 10);
        // p50 index = 10 * 0.50 = 5. So latencies[4] in 1-based or index 4. latencies sorted: [10,20,30,40,50,60,70,80,90,100]. p50 -> 50.
        assert_eq!(stats.p50_ms, 50);
        // p95 index = 10 * 0.95 = 10 (rounded 9.5). So latencies[9] in 1-based or index 9. -> 100.
        assert_eq!(stats.p95_ms, 100);
    }
}
