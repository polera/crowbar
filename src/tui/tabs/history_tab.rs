use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table, TableState, Wrap};
use ratatui::Frame;

use crate::app::App;
use crate::http::models::EntryState;
use crate::tui::widgets::{body_view, format_size, logo};

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let filtered = app.store.filtered_entries_all();

    if app.store.is_empty() {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" History ");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        logo::render(frame, inner);
        return;
    }

    let has_filter = !app.history.filter.is_empty() || app.history.filtering;

    let (filter_area, content_area) = if has_filter {
        let chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(area);
        (Some(chunks[0]), chunks[1])
    } else {
        (None, area)
    };

    if let Some(filter_area) = filter_area {
        render_filter_bar(app, frame, filter_area, filtered.len());
    }

    if app.history.detail_open {
        let selected = filtered.get(app.history.selected);
        let has_ws = selected.is_some_and(|e| !e.ws_messages.is_empty());
        let has_grpc = selected.is_some_and(|e| !e.grpc_messages.is_empty());
        let has_findings = selected.is_some_and(|e| !e.findings.is_empty());

        if has_ws || has_grpc || has_findings {
            let mut constraints = vec![
                Constraint::Percentage(25),
                Constraint::Percentage(35),
            ];
            let extra_panes = has_ws as usize + has_grpc as usize + has_findings as usize;
            let remaining = 40u16 / extra_panes as u16;
            for _ in 0..extra_panes {
                constraints.push(Constraint::Percentage(remaining));
            }

            let chunks = Layout::vertical(constraints).split(content_area);

            render_table_filtered(app, &filtered, frame, chunks[0]);
            render_detail_filtered(app, &filtered, frame, chunks[1]);
            let mut pane_idx = 2;
            if has_findings {
                render_findings(app, &filtered, frame, chunks[pane_idx]);
                pane_idx += 1;
            }
            if has_ws {
                render_ws_messages(app, &filtered, frame, chunks[pane_idx]);
                pane_idx += 1;
            }
            if has_grpc {
                render_grpc_messages(app, &filtered, frame, chunks[pane_idx]);
            }
        } else {
            let chunks = Layout::vertical([
                Constraint::Percentage(40),
                Constraint::Percentage(60),
            ])
            .split(content_area);

            render_table_filtered(app, &filtered, frame, chunks[0]);
            render_detail_filtered(app, &filtered, frame, chunks[1]);
        }
    } else {
        render_table_filtered(app, &filtered, frame, content_area);
    }
}

fn render_filter_bar(app: &App, frame: &mut Frame, area: Rect, match_count: usize) {
    let cursor_indicator = if app.history.filtering { "█" } else { "" };
    let count_text = if app.history.filter.is_empty() {
        String::new()
    } else {
        format!(" ({} matches)", match_count)
    };

    let line = Line::from(vec![
        Span::styled(" /", Style::default().fg(Color::Yellow)),
        Span::raw(&app.history.filter),
        Span::styled(cursor_indicator, Style::default().fg(Color::Yellow)),
        Span::styled(count_text, Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_table_filtered(app: &App, filtered: &[&crate::http::models::HistoryEntry], frame: &mut Frame, area: Rect) {
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

    let rows: Vec<Row> = filtered
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
            let path = if path.chars().count() > 50 {
                let end = path.char_indices().nth(47).map(|(i, _)| i).unwrap_or(path.len());
                format!("{}...", &path[..end])
            } else {
                path.to_string()
            };

            let (status, size, duration) = match &entry.response {
                Some(resp) => {
                    let status_str = if req.is_grpc {
                        match resp.grpc_status() {
                            Some((_, name)) => format!("g:{}", name),
                            None => resp.status.to_string(),
                        }
                    } else {
                        resp.status.to_string()
                    };
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

            let status_style = if req.is_grpc {
                if let Some(resp) = &entry.response {
                    match resp.grpc_status() {
                        Some((0, _)) => Style::default().fg(Color::Green),
                        Some(_) => Style::default().fg(Color::Red),
                        None => Style::default().fg(Color::DarkGray),
                    }
                } else {
                    Style::default().fg(Color::DarkGray)
                }
            } else if let Some(resp) = &entry.response {
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
        Constraint::Length(12),
        Constraint::Length(8),
        Constraint::Length(10),
    ];

    let highlight_style = Style::default()
        .bg(Color::DarkGray)
        .add_modifier(Modifier::BOLD);

    let help = if app.history.detail_open {
        " History (Enter/Esc:close) "
    } else {
        " History (Enter:detail j/k:nav /:filter) "
    };

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(help))
        .row_highlight_style(highlight_style);

    let mut state = TableState::default();
    state.select(Some(app.history.selected));

    frame.render_stateful_widget(table, area, &mut state);
}

fn render_detail_filtered(app: &App, filtered: &[&crate::http::models::HistoryEntry], frame: &mut Frame, area: Rect) {
    let entry = match filtered.get(app.history.selected) {
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

    req_lines.extend(crate::tui::widgets::header_lines(&req.headers));

    if !req.body.is_empty() {
        req_lines.push(Line::raw(""));
        let content_type = req.headers.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.as_str());
        req_lines.extend(body_view::body_lines(&req.body, content_type, 100));
    }

    let req_paragraph = Paragraph::new(Text::from(req_lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Request "),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.history.scroll, 0));

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

            if entry.request.is_grpc
                && let Some((code, name)) = resp.grpc_status() {
                    let grpc_style = if code == 0 {
                        Style::default().fg(Color::Green).bold()
                    } else {
                        Style::default().fg(Color::Red).bold()
                    };
                    let mut grpc_spans = vec![
                        Span::styled("gRPC ", Style::default().fg(Color::DarkGray)),
                        Span::styled(format!("{} {}", code, name), grpc_style),
                    ];
                    if let Some(msg) = resp.grpc_message()
                        && !msg.is_empty() {
                            grpc_spans.push(Span::styled(
                                format!("  {}", msg),
                                Style::default().fg(Color::Yellow),
                            ));
                        }
                    resp_lines.push(Line::from(grpc_spans));
                }

            resp_lines.push(Line::raw(""));

            resp_lines.extend(crate::tui::widgets::header_lines(&resp.headers));

            if !resp.body.is_empty() {
                resp_lines.push(Line::raw(""));
                let content_type = resp.headers.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                    .map(|(_, v)| v.as_str());
                resp_lines.extend(body_view::body_lines(&resp.body, content_type, 200));
            }

            if !resp.trailers.is_empty() {
                resp_lines.extend(crate::tui::widgets::trailer_lines(&resp.trailers));
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
        .scroll((app.history.scroll, 0));

    frame.render_widget(resp_paragraph, chunks[1]);

    if content_height > visible_height {
        let mut scrollbar_state = ScrollbarState::new(content_height as usize)
            .position(app.history.scroll as usize);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
        frame.render_stateful_widget(
            scrollbar,
            chunks[1],
            &mut scrollbar_state,
        );
    }
}

fn render_findings(app: &App, filtered: &[&crate::http::models::HistoryEntry], frame: &mut Frame, area: Rect) {
    let entry = match filtered.get(app.history.selected) {
        Some(e) => e,
        None => return,
    };

    use crate::scanning::Severity;

    let mut lines: Vec<Line> = Vec::new();
    for finding in &entry.findings {
        let severity_style = match finding.severity {
            Severity::High => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            Severity::Medium => Style::default().fg(Color::Yellow),
            Severity::Low => Style::default().fg(Color::Cyan),
            Severity::Info => Style::default().fg(Color::DarkGray),
        };

        lines.push(Line::from(vec![
            Span::styled(
                format!(" [{:>4}] ", finding.severity.label()),
                severity_style,
            ),
            Span::styled(&finding.title, Style::default().add_modifier(Modifier::BOLD)),
        ]));
        lines.push(Line::from(vec![
            Span::raw("         "),
            Span::styled(&finding.detail, Style::default().fg(Color::DarkGray)),
        ]));
    }

    let title = format!(" Findings ({}) ", entry.findings.len());
    let widget = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.history.scroll, 0));

    frame.render_widget(widget, area);
}

fn render_ws_messages(app: &App, filtered: &[&crate::http::models::HistoryEntry], frame: &mut Frame, area: Rect) {
    let entry = match filtered.get(app.history.selected) {
        Some(e) => e,
        None => return,
    };

    use crate::http::models::WsDirection;

    let mut lines: Vec<Line> = Vec::new();
    for (i, msg) in entry.ws_messages.iter().enumerate() {
        let dir_span = match msg.direction {
            WsDirection::ClientToServer => Span::styled(
                ">>> ",
                Style::default().fg(Color::Green),
            ),
            WsDirection::ServerToClient => Span::styled(
                "<<< ",
                Style::default().fg(Color::Cyan),
            ),
        };

        let type_label = if msg.is_text() {
            "text"
        } else if msg.is_binary() {
            "bin"
        } else if msg.is_close() {
            "close"
        } else {
            "ctrl"
        };

        let preview = if let Some(text) = msg.text() {
            let truncated: String = text.chars().take(120).collect();
            if truncated.len() < text.len() {
                format!("{truncated}...")
            } else {
                truncated
            }
        } else {
            format!("[{} bytes]", msg.payload.len())
        };

        let idx_span = Span::styled(
            format!("{:>4} ", i + 1),
            Style::default().fg(Color::DarkGray),
        );

        let type_span = Span::styled(
            format!("[{}] ", type_label),
            Style::default().fg(Color::DarkGray),
        );

        lines.push(Line::from(vec![
            idx_span,
            dir_span,
            type_span,
            Span::raw(preview),
        ]));
    }

    let title = format!(" WebSocket ({} messages) ", entry.ws_messages.len());
    let widget = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.history.scroll, 0));

    frame.render_widget(widget, area);
}

fn render_grpc_messages(app: &App, filtered: &[&crate::http::models::HistoryEntry], frame: &mut Frame, area: Rect) {
    let entry = match filtered.get(app.history.selected) {
        Some(e) => e,
        None => return,
    };

    use crate::http::models::GrpcDirection;

    let mut lines: Vec<Line> = Vec::new();
    for (i, msg) in entry.grpc_messages.iter().enumerate() {
        let dir_span = match msg.direction {
            GrpcDirection::ClientToServer => Span::styled(
                ">>> ",
                Style::default().fg(Color::Green),
            ),
            GrpcDirection::ServerToClient => Span::styled(
                "<<< ",
                Style::default().fg(Color::Cyan),
            ),
        };

        let size = format_size(msg.payload.len());
        let compressed_label = if msg.compressed { " [compressed]" } else { "" };

        let preview = if msg.payload.is_empty() {
            "[empty]".to_string()
        } else {
            use crate::http::protobuf::decode_raw;
            if let Some(fields) = decode_raw(&msg.payload) {
                let parts: Vec<String> = fields
                    .iter()
                    .take(3)
                    .map(|f| format!("{}={}", f.number, f.value))
                    .collect();
                let mut s = parts.join(", ");
                if fields.len() > 3 {
                    s.push_str(&format!(" (+{} more)", fields.len() - 3));
                }
                s
            } else {
                format!("[{} bytes]", msg.payload.len())
            }
        };

        let idx_span = Span::styled(
            format!("{:>4} ", i + 1),
            Style::default().fg(Color::DarkGray),
        );

        let size_span = Span::styled(
            format!("[{}{}] ", size, compressed_label),
            Style::default().fg(Color::DarkGray),
        );

        lines.push(Line::from(vec![
            idx_span,
            dir_span,
            size_span,
            Span::raw(preview),
        ]));
    }

    let title = format!(" gRPC Messages ({}) ", entry.grpc_messages.len());
    let widget = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.history.scroll, 0));

    frame.render_widget(widget, area);
}
