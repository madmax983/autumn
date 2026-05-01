use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

// A simplistic parsed .eml message for the TUI
#[allow(dead_code)]
struct MailMessage {
    path: PathBuf,
    filename: String,
    subject: String,
    to: String,
    content: String,
}

impl MailMessage {
    fn load(path: &Path) -> Option<Self> {
        let filename = path.file_name()?.to_string_lossy().to_string();
        let content = fs::read_to_string(path).ok()?;

        let mut subject = String::new();
        let mut to = String::new();

        // Very basic parsing to extract Subject and To headers
        for line in content.lines() {
            if line.starts_with("Subject: ") {
                subject = line.trim_start_matches("Subject: ").to_string();
            } else if line.starts_with("To: ") {
                to = line.trim_start_matches("To: ").to_string();
            }
            if line.is_empty() {
                // End of headers
                break;
            }
        }

        Some(Self {
            path: path.to_path_buf(),
            filename,
            subject,
            to,
            content,
        })
    }
}

struct MailState {
    dir: PathBuf,
    messages: Vec<MailMessage>,
    list_state: ListState,
}

impl MailState {
    fn new(dir: impl AsRef<Path>) -> Self {
        Self {
            dir: dir.as_ref().to_path_buf(),
            messages: Vec::new(),
            list_state: ListState::default(),
        }
    }

    fn refresh(&mut self) {
        self.messages.clear();

        if let Ok(entries) = fs::read_dir(&self.dir) {
            let mut paths: Vec<_> = entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().is_some_and(|ext| ext == "eml"))
                .collect();

            // Sort by filename descending (newest first, based on default timestamp format)
            paths.sort_by(|a, b| b.cmp(a));

            for path in paths {
                if let Some(msg) = MailMessage::load(&path) {
                    self.messages.push(msg);
                }
            }
        }

        // Adjust selection if out of bounds after refresh
        if self.messages.is_empty() {
            self.list_state.select(None);
        } else if let Some(selected) = self.list_state.selected() {
            if selected >= self.messages.len() {
                self.list_state.select(Some(self.messages.len() - 1));
            }
        } else {
            self.list_state.select(Some(0));
        }
    }

    fn next(&mut self) {
        if self.messages.is_empty() {
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) => {
                if i >= self.messages.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    fn previous(&mut self) {
        if self.messages.is_empty() {
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.messages.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }
}

pub fn run(dir: &str) {
    let mut state = MailState::new(dir);
    state.refresh();

    // Setup terminal
    terminal::enable_raw_mode().expect("failed to enable raw mode");
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, cursor::Hide).expect("failed to setup terminal");
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).expect("failed to create terminal");

    let result = run_loop(&mut terminal, &mut state);

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
    state: &mut MailState,
) -> io::Result<()> {
    loop {
        terminal.draw(|frame| draw(frame, state))?;

        if event::poll(Duration::from_millis(500))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            return Ok(());
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            state.next();
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            state.previous();
                        }
                        KeyCode::Char('r') => {
                            state.refresh();
                        }
                        _ => {}
                    }
                }
            }
        }
        // Removed auto-refresh here to improve performance
    }
}

fn draw(frame: &mut ratatui::Frame, state: &mut MailState) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(area);

    draw_list(frame, chunks[0], state);
    draw_detail(frame, chunks[1], state);
}

fn draw_list(frame: &mut ratatui::Frame, area: Rect, state: &mut MailState) {
    let block = Block::default()
        .title(" Local Mails ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let items: Vec<ListItem> = state
        .messages
        .iter()
        .map(|msg| {
            let to = if msg.to.is_empty() {
                "Unknown"
            } else {
                &msg.to
            };
            let subject = if msg.subject.is_empty() {
                "(No Subject)"
            } else {
                &msg.subject
            };

            let line1 = Line::from(vec![
                Span::styled("To: ", Style::default().fg(Color::DarkGray)),
                Span::styled(to, Style::default().fg(Color::Cyan)),
            ]);
            let line2 = Line::from(Span::styled(subject, Style::default().fg(Color::White)));

            ListItem::new(vec![line1, line2, Line::raw("")])
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(Color::Rgb(40, 40, 40))
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    frame.render_stateful_widget(list, area, &mut state.list_state);
}

fn draw_detail(frame: &mut ratatui::Frame, area: Rect, state: &MailState) {
    let block = Block::default()
        .title(" Message Detail ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    if let Some(selected_idx) = state.list_state.selected() {
        if let Some(msg) = state.messages.get(selected_idx) {
            let text = Text::raw(&msg.content);
            let paragraph = Paragraph::new(text).block(block).wrap(Wrap { trim: false });
            frame.render_widget(paragraph, area);
            return;
        }
    }

    let empty = Paragraph::new(Text::styled(
        "No message selected or directory is empty.",
        Style::default().fg(Color::DarkGray),
    ))
    .block(block);
    frame.render_widget(empty, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::fs::File;
    use std::io::Write;

    #[test]
    fn test_mail_message_load() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.eml");
        let mut file = File::create(&file_path).unwrap();
        file.write_all(b"To: admin@example.com\nSubject: Hello TUI\n\nBody of message").unwrap();

        let msg = MailMessage::load(&file_path).unwrap();
        assert_eq!(msg.to, "admin@example.com");
        assert_eq!(msg.subject, "Hello TUI");
    }

    #[test]
    fn test_mail_state_refresh() {
        let dir = tempdir().unwrap();

        let file1_path = dir.path().join("1.eml");
        let mut file1 = File::create(&file1_path).unwrap();
        file1.write_all(b"To: u1\nSubject: s1\n\n1").unwrap();

        let file2_path = dir.path().join("2.eml");
        let mut file2 = File::create(&file2_path).unwrap();
        file2.write_all(b"To: u2\nSubject: s2\n\n2").unwrap();

        let mut state = MailState::new(dir.path());
        state.refresh();
        assert_eq!(state.messages.len(), 2);
    }
}
