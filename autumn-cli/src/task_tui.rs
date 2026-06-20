use std::io::{self, BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::task::{TaskListing, TaskOptions};

// TUI application state
struct TuiState {
    tasks: Vec<TaskListing>,
    list_state: ListState,
    running: bool,
    output: Vec<String>,
    status: Option<bool>,
}

enum AppEvent {
    Input(Event),
    Tick,
    LogLine(String),
    TaskFinished(bool),
}

pub fn run(opts: &TaskOptions<'_>, binary: &std::path::Path) {
    // 1. Fetch task listing using AUTUMN_LIST_TASKS
    let output = Command::new(binary)
        .env("AUTUMN_LIST_TASKS", "1")
        .env("AUTUMN_ENV", opts.profile)
        .env("AUTUMN_PROFILE", opts.profile)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .unwrap_or_else(|error| {
            eprintln!("Failed to run {}: {error}", binary.display());
            std::process::exit(1);
        });

    if !output.status.success() {
        eprintln!(
            "Binary exited with status {} while listing tasks",
            output.status
        );
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let tasks: Vec<TaskListing> = serde_json::from_str(&stdout).unwrap_or_else(|error| {
        eprintln!("Failed to parse task listing JSON: {error}");
        eprintln!("Raw output: {stdout}");
        std::process::exit(1);
    });

    if tasks.is_empty() {
        println!("No tasks registered.");
        return;
    }

    // 2. Start TUI
    terminal::enable_raw_mode().expect("failed to enable raw mode");
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, cursor::Hide).expect("failed to setup terminal");
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).expect("failed to create terminal");

    let mut state = TuiState {
        tasks,
        list_state: ListState::default(),
        running: false,
        output: Vec::new(),
        status: None,
    };
    state.list_state.select(Some(0));

    let res = run_app(&mut terminal, &mut state, opts, binary);

    // 3. Restore terminal
    terminal::disable_raw_mode().expect("failed to disable raw mode");
    execute!(terminal.backend_mut(), LeaveAlternateScreen, cursor::Show)
        .expect("failed to restore terminal");

    if let Err(e) = res {
        eprintln!("Error: {e}");
    }
}

#[allow(
    clippy::too_many_lines,
    clippy::collapsible_if,
    clippy::match_same_arms
)]
fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut TuiState,
    opts: &TaskOptions<'_>,
    binary: &std::path::Path,
) -> io::Result<()> {
    let (tx, rx) = mpsc::channel();

    let tick_rate = Duration::from_millis(100);
    let tx_clone = tx.clone();
    thread::spawn(move || {
        #[allow(clippy::collapsible_if)]
        loop {
            if event::poll(tick_rate).unwrap() {
                if let Ok(evt) = event::read() {
                    let _ = tx_clone.send(AppEvent::Input(evt));
                }
            }
            let _ = tx_clone.send(AppEvent::Tick);
        }
    });

    loop {
        terminal.draw(|f| ui(f, state))?;

        match rx.recv().unwrap() {
            AppEvent::Input(Event::Key(key)) => {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            if !state.running {
                                return Ok(());
                            }
                        }
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            return Ok(());
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            if !state.running {
                                let i = match state.list_state.selected() {
                                    Some(i) => {
                                        if i >= state.tasks.len() - 1 {
                                            0
                                        } else {
                                            i + 1
                                        }
                                    }
                                    None => 0,
                                };
                                state.list_state.select(Some(i));
                            }
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            if !state.running {
                                let i = match state.list_state.selected() {
                                    Some(i) => {
                                        if i == 0 {
                                            state.tasks.len() - 1
                                        } else {
                                            i - 1
                                        }
                                    }
                                    None => 0,
                                };
                                state.list_state.select(Some(i));
                            }
                        }
                        KeyCode::Enter => {
                            if !state.running {
                                if let Some(i) = state.list_state.selected() {
                                    let task_name = state.tasks[i].name.clone();
                                    state.running = true;
                                    state.output.clear();
                                    state.status = None;

                                    let tx_bg = tx.clone();
                                    let binary_path = binary.to_path_buf();
                                    let profile = opts.profile.to_string();
                                    let args = opts.args.to_vec();

                                    thread::spawn(move || {
                                        let args_json = serde_json::to_string(&args).unwrap();
                                        let mut cmd = Command::new(binary_path)
                                            .env("AUTUMN_RUN_TASK", &task_name)
                                            .env("AUTUMN_TASK_ARGS_JSON", args_json)
                                            .env("AUTUMN_ENV", &profile)
                                            .env("AUTUMN_PROFILE", &profile)
                                            .stdout(Stdio::piped())
                                            .stderr(Stdio::piped())
                                            .spawn()
                                            .expect("failed to spawn task");

                                        let tx_bg_stdout = tx_bg.clone();
                                        if let Some(stdout) = cmd.stdout.take() {
                                            thread::spawn(move || {
                                                let reader = BufReader::new(stdout);
                                                for l in reader.lines().map_while(Result::ok) {
                                                    let _ = tx_bg_stdout.send(AppEvent::LogLine(l));
                                                }
                                            });
                                        }

                                        let tx_bg_stderr = tx_bg.clone();
                                        if let Some(stderr) = cmd.stderr.take() {
                                            thread::spawn(move || {
                                                let reader = BufReader::new(stderr);
                                                for l in reader.lines().map_while(Result::ok) {
                                                    let _ = tx_bg_stderr.send(AppEvent::LogLine(l));
                                                }
                                            });
                                        }

                                        let status = cmd.wait().expect("failed to wait");
                                        let _ =
                                            tx_bg.send(AppEvent::TaskFinished(status.success()));
                                    });
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            AppEvent::Input(_) | AppEvent::Tick => {}
            AppEvent::LogLine(line) => {
                state.output.push(line);
            }
            AppEvent::TaskFinished(success) => {
                state.running = false;
                state.status = Some(success);
            }
        }
    }
}

fn ui(f: &mut ratatui::Frame, state: &mut TuiState) {
    let size = f.area();

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)].as_ref())
        .split(size);

    let items: Vec<ListItem> = state
        .tasks
        .iter()
        .map(|t| {
            ListItem::new(Line::from(vec![Span::styled(
                t.name.clone(),
                Style::default(),
            )]))
        })
        .collect();

    let tasks_list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Tasks "))
        .highlight_style(
            Style::default()
                .bg(if state.running {
                    Color::DarkGray
                } else {
                    Color::Green
                })
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    f.render_stateful_widget(tasks_list, chunks[0], &mut state.list_state);

    let selected_desc = state
        .list_state
        .selected()
        .map_or("Select a task.", |i| state.tasks[i].description.as_str());

    let title = if state.running {
        " Running... "
    } else if let Some(success) = state.status {
        if success {
            " Finished (Success) "
        } else {
            " Finished (Failed) "
        }
    } else {
        " Description "
    };

    let border_color = if state.running {
        Color::Yellow
    } else if let Some(success) = state.status {
        if success { Color::Green } else { Color::Red }
    } else {
        Color::Reset
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let text = if state.running || state.status.is_some() {
        state.output.join("\n")
    } else {
        selected_desc.to_string()
    };

    let desc_par = Paragraph::new(text).block(block).wrap(Wrap { trim: false });

    f.render_widget(desc_par, chunks[1]);
}
