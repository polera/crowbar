use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table, TableState, Wrap};
use ratatui::Frame;

use crate::app::App;
use crate::http::models::EntryState;
use crate::tui::widgets::hex_view;

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    if app.store.len() == 0 {
        let msg = Paragraph::new("Waiting for requests...\n\nConfigure your browser proxy to use this address.")
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" History "),
            )
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(msg, area);
        return;
    }

    if app.history_detail_open {
        let chunks = Layout::vertical([
            Constraint::Percentage(40),
            Constraint::Percentage(60),
        ])
        .split(area);

        render_table(app, frame, chunks[0]);
        render_detail(app, frame, chunks[1]);
    } else {
        render_table(app, frame, area);
    }
}

fn render_table(app: &App, frame: &mut Frame, area: Rect) {
    let header = Row::new(vec![
        Cell::from("#"),
        Cell::from("Method"),
        Cell::from("Host"),
        Cell::from("Path"),
        Cell::from("Status"),
        Cell::from("Size"),
        Cell::from("Time"),
    ])
    .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
    .height(1);

    let rows: Vec<Row> = app
        .store
        .entries()
        .iter()
        .map(|entry| {
            let req = &entry.request;

            let path = if let Some(pos) = req.uri.find("://") {
                let after_scheme = &req.uri[pos + 3..];
                after_scheme
                    .find('/')
                    .map(|i| &after_scheme[i..])
                    .unwrap_or("/")
            } else {
                &req.uri
            };
            let path = if path.len() > 50 {
                format!("{}...", &path[..47])
            } else {
                path.to_string()
            };

            let (status, size, duration) = match &entry.response {
                Some(resp) => {
                    let status_str = resp.status.to_string();
                    let size_str = format_size(resp.body.len());
                    let dur_str = format!("{:.0?}", resp.duration);
                    (status_str, size_str, dur_str)
                }
                None => match entry.state {
                    EntryState::Pending => ("...".into(), "-".into(), "-".into()),
                    EntryState::Dropped => ("DROP".into(), "-".into(), "-".into()),
                    EntryState::Error => ("ERR".into(), "-".into(), "-".into()),
                    EntryState::Complete => ("???".into(), "-".into(), "-".into()),
                },
            };

            let method_style = match req.method.as_str() {
                "GET" => Style::default().fg(Color::Green),
                "POST" => Style::default().fg(Color::Blue),
                "PUT" => Style::default().fg(Color::Yellow),
                "DELETE" => Style::default().fg(Color::Red),
                "PATCH" => Style::default().fg(Color::Magenta),
                _ => Style::default(),
            };

            let status_style = if let Some(resp) = &entry.response {
                match resp.status {
                    200..=299 => Style::default().fg(Color::Green),
                    300..=399 => Style::default().fg(Color::Yellow),
                    400..=499 => Style::default().fg(Color::Red),
                    500..=599 => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    _ => Style::default(),
                }
            } else if entry.state == EntryState::Error {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let tls_indicator = if req.is_tls { "S" } else { " " };

            Row::new(vec![
                Cell::from(format!("{}{}", req.id, tls_indicator)),
                Cell::from(req.method.clone()).style(method_style),
                Cell::from(req.host.clone()),
                Cell::from(path),
                Cell::from(status).style(status_style),
                Cell::from(size),
                Cell::from(duration),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(6),
        Constraint::Length(7),
        Constraint::Min(15),
        Constraint::Min(20),
        Constraint::Length(6),
        Constraint::Length(8),
        Constraint::Length(10),
    ];

    let highlight_style = Style::default()
        .bg(Color::DarkGray)
        .add_modifier(Modifier::BOLD);

    let help = if app.history_detail_open {
        " History (Enter/Esc:close) "
    } else {
        " History (Enter:detail j/k:nav) "
    };

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(help))
        .row_highlight_style(highlight_style);

    let mut state = TableState::default();
    state.select(Some(app.history_selected));

    frame.render_stateful_widget(table, area, &mut state);
}

fn render_detail(app: &App, frame: &mut Frame, area: Rect) {
    let entries = app.store.entries();
    let entry = match entries.get(app.history_selected) {
        Some(e) => e,
        None => return,
    };

    let chunks = Layout::horizontal([
        Constraint::Percentage(50),
        Constraint::Percentage(50),
    ])
    .split(area);

    // Request pane
    let mut req_lines: Vec<Line> = Vec::new();
    let req = &entry.request;

    req_lines.push(Line::from(vec![
        Span::styled(&req.method, Style::default().fg(Color::Green).bold()),
        Span::raw(" "),
        Span::raw(&req.uri),
        Span::raw(" "),
        Span::styled(
            req.version.to_string(),
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    req_lines.push(Line::raw(""));

    for (key, value) in &req.headers {
        req_lines.push(Line::from(vec![
            Span::styled(key, Style::default().fg(Color::Cyan)),
            Span::raw(": "),
            Span::raw(value),
        ]));
    }

    if !req.body.is_empty() {
        req_lines.push(Line::raw(""));
        match std::str::from_utf8(&req.body) {
            Ok(text) => {
                for line in text.lines().take(50) {
                    req_lines.push(Line::raw(line.to_string()));
                }
                if text.lines().count() > 50 {
                    req_lines.push(Line::styled(
                        "... truncated",
                        Style::default().fg(Color::DarkGray),
                    ));
                }
            }
            Err(_) => {
                req_lines.push(Line::styled(
                    format!("[binary: {} bytes]", req.body.len()),
                    Style::default().fg(Color::DarkGray),
                ));
                req_lines.extend(hex_view::hex_lines(&req.body, 32));
            }
        }
    }

    let req_paragraph = Paragraph::new(Text::from(req_lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Request "),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.history_scroll, 0));

    frame.render_widget(req_paragraph, chunks[0]);

    // Response pane
    let mut resp_lines: Vec<Line> = Vec::new();

    match &entry.response {
        Some(resp) => {
            let status_style = match resp.status {
                200..=299 => Style::default().fg(Color::Green).bold(),
                300..=399 => Style::default().fg(Color::Yellow).bold(),
                400..=499 => Style::default().fg(Color::Red).bold(),
                500..=599 => Style::default().fg(Color::Red).bold(),
                _ => Style::default().bold(),
            };

            resp_lines.push(Line::from(vec![
                Span::styled(
                    resp.version.to_string(),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(" "),
                Span::styled(resp.status.to_string(), status_style),
                Span::raw(" "),
                Span::styled(&resp.reason, status_style),
                Span::raw("  "),
                Span::styled(
                    format!("{:.0?}", resp.duration),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
            resp_lines.push(Line::raw(""));

            for (key, value) in &resp.headers {
                resp_lines.push(Line::from(vec![
                    Span::styled(key, Style::default().fg(Color::Cyan)),
                    Span::raw(": "),
                    Span::raw(value),
                ]));
            }

            if !resp.body.is_empty() {
                resp_lines.push(Line::raw(""));
                match std::str::from_utf8(&resp.body) {
                    Ok(text) => {
                        for line in text.lines().take(100) {
                            resp_lines.push(Line::raw(line.to_string()));
                        }
                        if text.lines().count() > 100 {
                            resp_lines.push(Line::styled(
                                "... truncated",
                                Style::default().fg(Color::DarkGray),
                            ));
                        }
                    }
                    Err(_) => {
                        resp_lines.push(Line::styled(
                            format!("[binary: {} bytes]", resp.body.len()),
                            Style::default().fg(Color::DarkGray),
                        ));
                        resp_lines.extend(hex_view::hex_lines(&resp.body, 64));
                    }
                }
            }
        }
        None => {
            let msg = match entry.state {
                EntryState::Pending => "Awaiting response...",
                EntryState::Dropped => "Request was dropped",
                EntryState::Error => entry
                    .error_message
                    .as_deref()
                    .unwrap_or("Unknown error"),
                EntryState::Complete => "No response data",
            };
            resp_lines.push(Line::styled(
                msg,
                Style::default().fg(Color::DarkGray),
            ));
        }
    }

    let content_height = resp_lines.len() as u16;
    let visible_height = chunks[1].height.saturating_sub(2);

    let resp_paragraph = Paragraph::new(Text::from(resp_lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Response "),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.history_scroll, 0));

    frame.render_widget(resp_paragraph, chunks[1]);

    if content_height > visible_height {
        let mut scrollbar_state = ScrollbarState::new(content_height as usize)
            .position(app.history_scroll as usize);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
        frame.render_stateful_widget(
            scrollbar,
            chunks[1],
            &mut scrollbar_state,
        );
    }
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
