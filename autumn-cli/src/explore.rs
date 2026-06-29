//! Interactive Ratatui TUI to explore an Autumn application's `OpenAPI` schema.

use std::io;

use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};
use serde_json::Value;

/// The state of the Explore TUI.
pub struct ExploreState {
    pub routes: Vec<String>,
    pub selected_index: usize,
    pub spec: Value,
}

impl ExploreState {
    pub fn new(spec: Value) -> Self {
        let mut routes = Vec::new();
        if let Some(paths) = spec.get("paths").and_then(|p| p.as_object()) {
            for path in paths.keys() {
                routes.push(path.to_owned());
            }
        }
        routes.sort();
        Self {
            routes,
            selected_index: 0,
            spec,
        }
    }

    pub const fn next(&mut self) {
        if !self.routes.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.routes.len();
        }
    }

    pub const fn previous(&mut self) {
        if !self.routes.is_empty() {
            self.selected_index = self.selected_index.saturating_sub(1);
        }
    }
}

fn draw(frame: &mut ratatui::Frame, state: &ExploreState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)].as_ref())
        .split(frame.area());

    draw_routes_list(frame, chunks[0], state);
    draw_route_details(frame, chunks[1], state);
}

fn draw_routes_list(frame: &mut ratatui::Frame, area: Rect, state: &ExploreState) {
    let items: Vec<ListItem> = state
        .routes
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let style = if i == state.selected_index {
                Style::default().bg(Color::DarkGray).fg(Color::White)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(Span::styled(r.clone(), style)))
        })
        .collect();

    let list = List::new(items).block(Block::default().borders(Borders::ALL).title("Routes"));
    frame.render_widget(list, area);
}

use std::fmt::Write;

fn draw_route_details(frame: &mut ratatui::Frame, area: Rect, state: &ExploreState) {
    let text = state.routes.get(state.selected_index).map_or_else(
        || "No route selected.".to_owned(),
        |route| {
            state
                .spec
                .get("paths")
                .and_then(|p| p.get(route))
                .and_then(Value::as_object)
                .map_or_else(
                    || "No details available.".to_owned(),
                    |methods| {
                        let mut display = format!("Path: {route}\n\n");
                        for (method, details) in methods {
                            let _ = writeln!(display, "Method: {}", method.to_uppercase());
                            if let Some(summary) = details.get("summary").and_then(Value::as_str) {
                                let _ = writeln!(display, "Summary: {summary}");
                            }
                            display.push('\n');

                            if let Ok(pretty) = serde_json::to_string_pretty(details) {
                                display.push_str(&pretty);
                            }
                            display.push_str("\n\n---\n\n");
                        }
                        display
                    },
                )
        },
    );

    let paragraph = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title("Details"))
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

pub fn run(url: &str) {
    let client = reqwest::blocking::Client::new();
    let res = client.get(format!("{url}/api-docs/openapi.json")).send();

    let spec: Value = match res {
        Ok(res) if res.status().is_success() => res.json().unwrap_or(Value::Null),
        _ => {
            eprintln!("Failed to fetch OpenAPI spec from {url}/api-docs/openapi.json");
            return;
        }
    };

    let mut state = ExploreState::new(spec);

    enable_raw_mode().unwrap();
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).unwrap();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).unwrap();

    loop {
        terminal.draw(|f| draw(f, &state)).unwrap();

        if event::poll(std::time::Duration::from_millis(50)).unwrap() {
            let evt = event::read();
            if let Ok(Event::Key(key)) = evt {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Down | KeyCode::Char('j') => state.next(),
                    KeyCode::Up | KeyCode::Char('k') => state.previous(),
                    _ => {}
                }
            }
        }
    }

    disable_raw_mode().unwrap();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).unwrap();
    terminal.show_cursor().unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── RED PHASE TESTS ──────────────────────────────────────────────────────
    #[test]
    fn explore_state_parses_routes_from_spec() {
        let spec = json!({
            "paths": {
                "/health": {},
                "/api/users": {}
            }
        });
        let state = ExploreState::new(spec);
        assert_eq!(state.routes.len(), 2);
        assert_eq!(state.routes[0], "/api/users"); // Sorted
        assert_eq!(state.routes[1], "/health");
    }

    #[test]
    fn explore_state_handles_empty_spec() {
        let state = ExploreState::new(json!({}));
        assert_eq!(state.routes.len(), 0);
    }

    #[test]
    fn explore_state_next_previous_navigation() {
        let spec = json!({
            "paths": {
                "/a": {},
                "/b": {},
                "/c": {}
            }
        });
        let mut state = ExploreState::new(spec);
        assert_eq!(state.selected_index, 0);
        state.next();
        assert_eq!(state.selected_index, 1);
        state.next();
        assert_eq!(state.selected_index, 2);
        state.next();
        assert_eq!(state.selected_index, 0);
        state.previous();
        assert_eq!(state.selected_index, 0); // saturating_sub
        state.next();
        state.previous();
        assert_eq!(state.selected_index, 0);
    }
}
