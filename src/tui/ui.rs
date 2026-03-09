use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Tabs, Wrap};

use crate::tui::app::{App, Screen};

fn field(label: &str, value: String, selected: bool) -> Line<'static> {
    if selected {
        Line::from(vec![
            Span::styled("> ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("{label}: {value}"),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ])
    } else {
        Line::from(format!("  {label}: {value}"))
    }
}

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(8),
        ])
        .split(frame.area());

    let titles: Vec<Line<'_>> = [
        Screen::Init,
        Screen::Index,
        Screen::Query,
        Screen::Status,
        Screen::Serve,
    ]
    .iter()
    .map(|s| Line::from(s.title()))
    .collect();
    let selected_tab = match app.screen {
        Screen::Init => 0,
        Screen::Index => 1,
        Screen::Query => 2,
        Screen::Status => 3,
        Screen::Serve => 4,
    };
    let tabs = Tabs::new(titles)
        .select(selected_tab)
        .block(Block::default().title("Satori").borders(Borders::ALL))
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));
    frame.render_widget(tabs, chunks[0]);

    let center_lines: Vec<Line<'_>> = match app.screen {
        Screen::Init => vec![
            field("pack", app.pack.clone(), app.field_idx == 0),
            field("provider", app.init_provider.clone(), app.field_idx == 1),
            field("model", app.init_model.clone(), app.field_idx == 2),
            field("dim", app.init_dim.clone(), app.field_idx == 3),
            field(
                "force(toggle space)",
                app.init_force.to_string(),
                app.field_idx == 4,
            ),
            Line::from("Enter runs init_pack"),
        ],
        Screen::Index => vec![
            field("pack", app.pack.clone(), app.field_idx == 0),
            field(
                "sources(csv)",
                app.index_sources.clone(),
                app.field_idx == 1,
            ),
            Line::from("Enter runs run_index"),
        ],
        Screen::Query => vec![
            field("pack", app.pack.clone(), app.field_idx == 0),
            field("query", app.query_text.clone(), app.field_idx == 1),
            field("mode", app.query_mode.clone(), app.field_idx == 2),
            field("top_k", app.query_top_k.clone(), app.field_idx == 3),
            Line::from("Enter runs run_query"),
        ],
        Screen::Status => {
            let mut lines = vec![
                field("pack", app.pack.clone(), app.field_idx == 0),
                Line::from("Enter loads status"),
            ];
            lines.extend(app.status_lines.iter().cloned().map(Line::from));
            lines
        }
        Screen::Serve => vec![
            field("pack", app.pack.clone(), app.field_idx == 0),
            field("host", app.serve_host.clone(), app.field_idx == 1),
            field("port", app.serve_port.clone(), app.field_idx == 2),
            Line::from(if app.server_running {
                "Enter stops server"
            } else {
                "Enter starts server"
            }),
        ],
    };
    let center = Paragraph::new(center_lines)
        .block(Block::default().title("Form").borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    frame.render_widget(center, chunks[1]);

    let output_lines: Vec<Line<'_>> = app.output_lines.iter().cloned().map(Line::from).collect();
    let output = Paragraph::new(output_lines)
        .block(Block::default().title("Output").borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    frame.render_widget(output, chunks[2]);
}
