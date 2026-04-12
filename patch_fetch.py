import re

with open("autumn-cli/src/monitor.rs", "r") as f:
    content = f.read()

# 1. Add ConfigPropsResponse type
content = re.sub(
    r'struct LoggersResponse \{',
    r'pub type ConfigPropsResponse = std::collections::HashMap<String, ConfigProperty>;\n\n#[derive(Debug, Deserialize, Default, Clone)]\nstruct ConfigProperty {\n    value: serde_json::Value,\n    source: String,\n}\n\n#[derive(Debug, Deserialize, Default, Clone)]\nstruct LoggersResponse {',
    content
)

# 2. Add to DashboardState
content = re.sub(
    r'loggers: LoggersResponse,',
    r'loggers: LoggersResponse,\n    config_props: ConfigPropsResponse,',
    content
)

# 3. Add to DashboardState::new
content = re.sub(
    r'loggers: LoggersResponse::default\(\),',
    r'loggers: LoggersResponse::default(),\n            config_props: ConfigPropsResponse::default(),',
    content
)

# 4. Add to poll
content = re.sub(
    r'self\.fetch_loggers\(&client\);',
    r'self.fetch_loggers(&client);\n        self.fetch_config_props(&client);',
    content
)

# 5. Add fetch_config_props
fetch_fn = """
    fn fetch_config_props(&mut self, client: &reqwest::blocking::Client) {
        if let Ok(resp) = client
            .get(format!("{}/actuator/configprops", self.base_url))
            .send()
        {
            if let Ok(c) = resp.json::<ConfigPropsResponse>() {
                self.config_props = c;
            }
        }
    }
}
"""

content = re.sub(
    r'\}\n\n// ── TUI rendering ─────────────────────────────────────────────',
    fetch_fn + '\n// ── TUI rendering ─────────────────────────────────────────────',
    content
)

# 6. Add Config tab logic
content = re.sub(
    r'let tab_titles = vec!\["Overview", "Routes", "Loggers"\];',
    r'let tab_titles = vec!["Overview", "Routes", "Loggers", "Config"];',
    content
)

content = re.sub(
    r'state\.active_tab = \(state\.active_tab \+ 1\) % 3;',
    r'state.active_tab = (state.active_tab + 1) % 4;',
    content
)

content = re.sub(
    r'if state\.active_tab == 0 \{\n\s+state\.active_tab = 2;',
    r'if state.active_tab == 0 {\n                                state.active_tab = 3;',
    content
)

content = re.sub(
    r'2 => draw_loggers_tab\(frame, main_chunks\[1\], state\),\n\s+_ => \{\}',
    r'2 => draw_loggers_tab(frame, main_chunks[1], state),\n        3 => draw_config_tab(frame, main_chunks[1], state),\n        _ => {}',
    content
)

# 7. Add Config tab drawer
draw_fn = """
fn draw_config_tab(frame: &mut ratatui::Frame, area: Rect, state: &DashboardState) {
    let block = Block::default()
        .title(Span::styled(
            " Configuration Properties ",
            Style::default()
                .fg(Color::Rgb(204, 120, 50))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .padding(Padding::new(1, 1, 0, 0));

    let header = Row::new(vec![
        Cell::from("Property").style(
            Style::default()
                .fg(Color::Rgb(204, 120, 50))
                .add_modifier(Modifier::BOLD),
        ),
        Cell::from("Value").style(
            Style::default()
                .fg(Color::Rgb(204, 120, 50))
                .add_modifier(Modifier::BOLD),
        ),
        Cell::from("Source").style(
            Style::default()
                .fg(Color::Rgb(204, 120, 50))
                .add_modifier(Modifier::BOLD),
        ),
    ])
    .height(1)
    .bottom_margin(1);

    let mut props: Vec<_> = state.config_props.iter().collect();
    props.sort_by(|a, b| a.0.cmp(b.0));

    let rows: Vec<Row> = props
        .into_iter()
        .map(|(k, v)| {
            let val_str = match &v.value {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };

            Row::new(vec![
                Cell::from(k.clone()).style(Style::default().fg(Color::White)),
                Cell::from(val_str).style(Style::default().fg(Color::Cyan)),
                Cell::from(v.source.clone()).style(Style::default().fg(Color::DarkGray)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Percentage(35),
        Constraint::Percentage(45),
        Constraint::Percentage(20),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .column_spacing(2);

    frame.render_widget(table, area);
}
"""

content = content.replace("fn draw_footer(", draw_fn + "\nfn draw_footer(")

with open("autumn-cli/src/monitor.rs", "w") as f:
    f.write(content)
