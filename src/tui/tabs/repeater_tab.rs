use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;
use crate::http::codec;
use crate::http::sequence::StepState;
use crate::tui::widgets::{body_view, diff_view, logo};

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(3),
    ])
    .split(area);

    if app.macro_show {
        let panes = Layout::horizontal([
            Constraint::Percentage(50),
            Constraint::Percentage(50),
        ])
        .split(chunks[0]);

        render_macro_list(app, frame, panes[0]);
        render_macro_detail(app, frame, panes[1]);
        render_macro_actions(app, frame, chunks[1]);
    } else {
        let panes = Layout::horizontal([
            Constraint::Percentage(50),
            Constraint::Percentage(50),
        ])
        .split(chunks[0]);

        render_request_editor(app, frame, panes[0]);
        render_response(app, frame, panes[1]);
        render_actions(app, frame, chunks[1]);
    }
}

fn render_request_editor(app: &App, frame: &mut Frame, area: Rect) {
    if app.repeater_lines.is_empty() {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Request ");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        logo::render(frame, inner);
        return;
    }

    if app.repeater_show_diff && !app.repeater_editing {
        render_diff(app, frame, area);
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

fn render_diff(app: &App, frame: &mut Frame, area: Rect) {
    let original_lines = match &app.repeater_original {
        Some(orig) => codec::request_to_lines(orig),
        None => vec![],
    };

    let lines = diff_view::diff_lines(&original_lines, &app.repeater_lines);

    let widget = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Diff (original vs current) ")
                .border_style(Style::default().fg(Color::Magenta)),
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
            let content_type = resp.headers.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                .map(|(_, v)| v.as_str());
            lines.extend(body_view::body_lines(&resp.body, content_type, 500));
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
        let diff_label = if app.repeater_show_diff { "d:raw" } else { "d:diff" };
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
            if has_request {
                Span::styled(format!(" {} ", diff_label), key_style())
            } else {
                Span::styled(format!(" {} ", diff_label), dim_style())
            },
            Span::raw("  "),
            Span::styled(" M ", key_style()),
            Span::raw("macro  "),
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

fn render_macro_list(app: &App, frame: &mut Frame, area: Rect) {
    if app.macro_steps.is_empty() {
        let msg = Paragraph::new(Line::styled(
            "No macro steps. Press 'm' in History to add requests.",
            Style::default().fg(Color::DarkGray),
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Macro ({} steps) ", app.macro_steps.len())),
        );
        frame.render_widget(msg, area);
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    for (i, step) in app.macro_steps.iter().enumerate() {
        let selected = i == app.macro_selected;
        let (state_icon, state_style) = match step.state {
            StepState::Pending => ("  ", Style::default().fg(Color::DarkGray)),
            StepState::Running => (">>", Style::default().fg(Color::Yellow)),
            StepState::Complete => ("OK", Style::default().fg(Color::Green)),
            StepState::Error => ("!!", Style::default().fg(Color::Red)),
        };

        let method_style = match step.request.method.as_str() {
            "GET" => Style::default().fg(Color::Green),
            "POST" => Style::default().fg(Color::Blue),
            "PUT" => Style::default().fg(Color::Yellow),
            "DELETE" | "PATCH" => Style::default().fg(Color::Red),
            _ => Style::default(),
        };

        let status_str = step
            .response
            .as_ref()
            .map(|r| r.status.to_string())
            .unwrap_or_else(|| "-".into());

        let row_style = if selected {
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        let path = if step.request.uri.len() > 35 {
            format!("{}...", &step.request.uri[..32])
        } else {
            step.request.uri.clone()
        };

        lines.push(Line::from(vec![
            Span::styled(format!(" {:>2}. ", i + 1), Style::default().fg(Color::DarkGray)),
            Span::styled(format!("[{}] ", state_icon), state_style),
            Span::styled(format!("{:<7}", step.request.method), method_style),
            Span::raw(format!("{:<36} ", path)),
            Span::raw(status_str),
        ]).style(row_style));
    }

    let title = if app.macro_running {
        format!(" Macro ({} steps) [RUNNING] ", app.macro_steps.len())
    } else {
        format!(" Macro ({} steps) ", app.macro_steps.len())
    };

    let border_style = if app.macro_running {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    let widget = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(border_style),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(widget, area);
}

fn render_macro_detail(app: &App, frame: &mut Frame, area: Rect) {
    let step = match app.macro_steps.get(app.macro_selected) {
        Some(s) => s,
        None => {
            let widget = Paragraph::new(Line::styled(
                "Select a step to view details",
                Style::default().fg(Color::DarkGray),
            ))
            .block(Block::default().borders(Borders::ALL).title(" Step Detail "));
            frame.render_widget(widget, area);
            return;
        }
    };

    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(vec![
        Span::styled(&step.request.method, Style::default().fg(Color::Green).bold()),
        Span::raw(" "),
        Span::raw(&step.request.uri),
    ]));

    for (key, value) in &step.request.headers {
        lines.push(Line::from(vec![
            Span::styled(key, Style::default().fg(Color::Cyan)),
            Span::raw(": "),
            Span::raw(value),
        ]));
    }

    if let Some(ref resp) = step.response {
        lines.push(Line::raw(""));
        let status_style = match resp.status {
            200..=299 => Style::default().fg(Color::Green).bold(),
            300..=399 => Style::default().fg(Color::Yellow).bold(),
            _ => Style::default().fg(Color::Red).bold(),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{} {}", resp.status, resp.reason), status_style),
            Span::raw("  "),
            Span::styled(format!("{:.0?}", resp.duration), Style::default().fg(Color::DarkGray)),
            Span::raw("  "),
            Span::styled(format_size(resp.body.len()), Style::default().fg(Color::DarkGray)),
        ]));

        for (key, value) in &resp.headers {
            lines.push(Line::from(vec![
                Span::styled(key, Style::default().fg(Color::Cyan)),
                Span::raw(": "),
                Span::raw(value),
            ]));
        }

        if !resp.body.is_empty() {
            lines.push(Line::raw(""));
            let ct = resp.headers.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                .map(|(_, v)| v.as_str());
            lines.extend(body_view::body_lines(&resp.body, ct, 100));
        }
    } else if let Some(ref err) = step.error {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            format!("Error: {}", err),
            Style::default().fg(Color::Red),
        ));
    }

    let widget = Paragraph::new(Text::from(lines))
        .block(Block::default().borders(Borders::ALL).title(" Step Detail "))
        .wrap(Wrap { trim: false })
        .scroll((app.repeater_resp_scroll, 0));

    frame.render_widget(widget, area);
}

fn render_macro_actions(app: &App, frame: &mut Frame, area: Rect) {
    let has_steps = !app.macro_steps.is_empty();

    let line = Line::from(vec![
        if has_steps && !app.macro_running {
            Span::styled(" Ctrl+Enter ", key_style())
        } else {
            Span::styled(" Ctrl+Enter ", dim_style())
        },
        Span::raw("run  "),
        Span::styled(" M ", key_style()),
        Span::raw("repeater  "),
        if has_steps && !app.macro_running {
            Span::styled(" x ", key_style())
        } else {
            Span::styled(" x ", dim_style())
        },
        Span::raw("remove  "),
        if has_steps && !app.macro_running {
            Span::styled(" X ", key_style())
        } else {
            Span::styled(" X ", dim_style())
        },
        Span::raw("clear all  "),
        Span::styled(" j/k ", key_style()),
        Span::raw("navigate  "),
        Span::styled(" J/K ", key_style()),
        Span::raw("scroll detail"),
    ]);

    let widget = Paragraph::new(line).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Macro Actions "),
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
