Wait! I need to implement ONE new interesting feature.
Let's build `autumn explore`.

The `autumn explore` command connects `reqwest` and `ratatui` (combining OpenAPI + TUI).
I'll create `autumn-cli/src/explore.rs`.

```rust
use std::io;

use clap::Args;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Terminal,
};
use serde_json::Value;

pub fn run(url: &str) {
    // 1. Fetch OpenAPI spec
    let client = reqwest::blocking::Client::new();
    let res = client.get(format!("{}/api-docs/openapi.json", url)).send();

    let spec: Value = match res {
        Ok(res) if res.status().is_success() => res.json().unwrap_or_else(|_| Value::Null),
        _ => {
            eprintln!("Failed to fetch OpenAPI spec from {}/api-docs/openapi.json", url);
            return;
        }
    };

    // Parse routes
    // Launch TUI
}
```

Wait, parsing OpenAPI JSON into a usable structure.
```json
{
  "paths": {
    "/health": {
      "get": {
        "summary": "Health check",
        ...
      }
    }
  }
}
```

Is there anything else that would be cooler?
"The Interface: Add a TUI (Text UI) to a CLI tool using ratatui"
"The Exporter: Add Export to JSON/CSV/PNG functionality"
"The Simulator: Add a 'Debug Mode' that auto-plays the game or queries the DB"

What about a **Database Seeder that auto-plays DB queries (Faker)?**
Or a **Live Markdown preview endpoint** using HTMX?

Let's stick to `autumn explore`. It matches the "Add a TUI to a CLI tool using ratatui" prompt pattern exactly!

Let's quickly check how `autumn monitor` is implemented, so we can structure `explore.rs` similarly.
