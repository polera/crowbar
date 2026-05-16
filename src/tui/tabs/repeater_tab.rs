use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;
use crate::tui::widgets::hex_view;

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(3),
    ])
    .split(area);

    let panes = Layout::horizontal([
        Constraint::Percentage(50),
        Constraint::Percentage(50),
    ])
    .split(chunks[0]);

    render_request_editor(app, frame, panes[0]);
    render_response(app, frame, panes[1]);
    render_actions(app, frame, chunks[1]);
}

fn render_request_editor(app: &App, frame: &mut Frame, area: Rect) {
    if app.repeater_lines.is_empty() {
        let msg = Paragraph::new(
            "No request loaded.\n\nSelect a request in History and press 'r' to send it here.",
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Request "),
        )
        .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(msg, area);
        return;
    }

    let border_style = if app.repeater_editing {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    let mut lines: Vec<Line> = Vec::new();

    for (i, line) in app.repeater_lines.iter().enumerate() {
        if app.repeater_editing && i == app.repeater_cursor_line {
            let col = app.repeater_cursor_col.min(line.len());
            let before = &line[..col];
            let cursor_char = line.get(col..col + 1).unwrap_or(" ");
            let after = if col + 1 < line.len() {
                &line[col + 1..]
            } else {
                ""
            };

            lines.push(Line::from(vec![
                Span::raw(before.to_string()),
                Span::styled(
                    cursor_char.to_string(),
                    Style::default().bg(Color::White).fg(Color::Black),
                ),
                Span::raw(after.to_string()),
            ]));
        } else if i == 0 {
                let parts: Vec<&str> = line.splitn(3, ' ').collect();
                if parts.len() >= 2 {
                    lines.push(Line::from(vec![
                        Span::styled(parts[0], Style::default().fg(Color::Green).bold()),
                        Span::raw(" "),
                        Span::raw(parts[1..].join(" ")),
                    ]));
                } else {
                    lines.push(Line::raw(line.clone()));
                }
            } else if let Some((key, value)) = line.split_once(':') {
                lines.push(Line::from(vec![
                    Span::styled(key, Style::default().fg(Color::Cyan)),
                    Span::raw(":"),
                    Span::raw(value),
                ]));
            } else {
                lines.push(Line::raw(line.clone()));
            }
    }

    let title = if app.repeater_editing {
        " Request (editing) "
    } else {
        " Request "
    };

    let widget = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(border_style),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.repeater_req_scroll, 0));

    frame.render_widget(widget, area);
}

fn render_response(app: &App, frame: &mut Frame, area: Rect) {
    if app.repeater_pending {
        let msg = Paragraph::new("Sending request...")
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Response "),
            )
            .style(Style::default().fg(Color::Yellow));
        frame.render_widget(msg, area);
        return;
    }

    if let Some(ref error) = app.repeater_error {
        let msg = Paragraph::new(format!("Error: {}", error))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Response "),
            )
            .style(Style::default().fg(Color::Red))
            .wrap(Wrap { trim: false });
        frame.render_widget(msg, area);
        return;
    }

    let content = if let Some(ref resp) = app.repeater_response {
        let mut lines: Vec<Line> = Vec::new();

        let status_style = match resp.status {
            200..=299 => Style::default().fg(Color::Green).bold(),
            300..=399 => Style::default().fg(Color::Yellow).bold(),
            400..=499 => Style::default().fg(Color::Red).bold(),
            500..=599 => Style::default().fg(Color::Red).bold(),
            _ => Style::default().bold(),
        };

        lines.push(Line::from(vec![
            Span::styled(resp.version.to_string(), Style::default().fg(Color::DarkGray)),
            Span::raw(" "),
            Span::styled(resp.status.to_string(), status_style),
            Span::raw(" "),
            Span::styled(&resp.reason, status_style),
            Span::raw("  "),
            Span::styled(
                format!("{:.0?}", resp.duration),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw("  "),
            Span::styled(
                format_size(resp.body.len()),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
        lines.push(Line::raw(""));

        for (key, value) in &resp.headers {
            lines.push(Line::from(vec![
                Span::styled(key, Style::default().fg(Color::Cyan)),
                Span::raw(": "),
                Span::raw(value),
            ]));
        }

        if !resp.body.is_empty() {
            lines.push(Line::raw(""));
            match std::str::from_utf8(&resp.body) {
                Ok(text) => {
                    for line in text.lines().take(200) {
                        lines.push(Line::raw(line.to_string()));
                    }
                    if text.lines().count() > 200 {
                        lines.push(Line::styled(
                            "... truncated",
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                }
                Err(_) => {
                    lines.push(Line::styled(
                        format!("[binary: {} bytes]", resp.body.len()),
                        Style::default().fg(Color::DarkGray),
                    ));
                    lines.extend(hex_view::hex_lines(&resp.body, 64));
                }
            }
        }

        Text::from(lines)
    } else {
        Text::styled(
            "No response yet.\n\nPress Ctrl+Enter to send the request.",
            Style::default().fg(Color::DarkGray),
        )
    };

    let widget = Paragraph::new(content)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Response "),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.repeater_resp_scroll, 0));

    frame.render_widget(widget, area);
}

fn render_actions(app: &App, frame: &mut Frame, area: Rect) {
    let has_request = !app.repeater_lines.is_empty();

    let actions = if app.repeater_editing {
        Line::from(vec![
            Span::styled(" Ctrl+Enter ", key_style()),
            Span::raw("send  "),
            Span::styled(" Esc ", key_style()),
            Span::raw("stop editing  "),
            Span::styled(" arrows ", dim_style()),
            Span::raw("navigate"),
        ])
    } else {
        Line::from(vec![
            if has_request {
                Span::styled(" Ctrl+Enter ", key_style())
            } else {
                Span::styled(" Ctrl+Enter ", dim_style())
            },
            Span::raw("send  "),
            if has_request {
                Span::styled(" e ", key_style())
            } else {
                Span::styled(" e ", dim_style())
            },
            Span::raw("edit  "),
            Span::styled(" j/k ", key_style()),
            Span::raw("scroll req  "),
            Span::styled(" J/K ", key_style()),
            Span::raw("scroll resp"),
        ])
    };

    let widget = Paragraph::new(actions).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Actions "),
    );
    frame.render_widget(widget, area);
}

fn key_style() -> Style {
    Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD)
}

fn dim_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
