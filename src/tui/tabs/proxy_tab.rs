use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;
use crate::tui::widgets::logo;

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(3),
    ])
    .split(area);

    render_status_line(app, frame, chunks[0]);

    if app.intercept_ui.editing {
        render_editor(app, frame, chunks[1]);
    } else {
        render_current_request(app, frame, chunks[1]);
    }

    render_actions(app, frame, chunks[2]);
}

fn render_status_line(app: &App, frame: &mut Frame, area: Rect) {
    if app.editing_bind_addr {
        let line = Line::from(vec![
            Span::raw(" Bind address: "),
            Span::styled(&app.bind_addr_buffer, Style::default().fg(Color::White)),
            Span::styled("\u{2588}", Style::default().fg(Color::Yellow)),
        ]);
        let widget = Paragraph::new(line).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Proxy ")
                .border_style(Style::default().fg(Color::Yellow)),
        );
        frame.render_widget(widget, area);
        return;
    }

    if app.editing_scope {
        let line = Line::from(vec![
            Span::raw(" Scope: "),
            Span::styled(&app.scope_buffer, Style::default().fg(Color::White)),
            Span::styled("\u{2588}", Style::default().fg(Color::Yellow)),
            Span::styled("  (comma-separated, e.g. *.example.com, api.test.com)", Style::default().fg(Color::DarkGray)),
        ]);
        let widget = Paragraph::new(line).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Proxy ")
                .border_style(Style::default().fg(Color::Yellow)),
        );
        frame.render_widget(widget, area);
        return;
    }

    let intercept_status = if app.intercept_enabled() {
        Span::styled(
            " INTERCEPT ON ",
            Style::default()
                .bg(Color::Red)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            " INTERCEPT OFF ",
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White),
        )
    };

    let queue_count = app.intercept_ui.queue.len();
    let queue_text = if queue_count > 0 {
        format!("  {} request{} queued", queue_count, if queue_count == 1 { "" } else { "s" })
    } else {
        String::new()
    };

    let line = Line::from(vec![
        Span::raw(" "),
        intercept_status,
        Span::styled(queue_text, Style::default().fg(Color::Yellow)),
    ]);

    let widget = Paragraph::new(line).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Proxy "),
    );
    frame.render_widget(widget, area);
}

fn render_current_request(app: &App, frame: &mut Frame, area: Rect) {
    let content = if let Some(req) = app.intercept_ui.queue.front() {
        let mut lines: Vec<Line> = Vec::new();

        lines.push(Line::from(vec![
            Span::styled(&req.method, Style::default().fg(Color::Green).bold()),
            Span::raw(" "),
            Span::raw(&req.uri),
            Span::raw(" "),
            Span::styled(req.version.to_string(), Style::default().fg(Color::DarkGray)),
        ]));
        lines.push(Line::raw(""));

        for (key, value) in &req.headers {
            lines.push(Line::from(vec![
                Span::styled(key, Style::default().fg(Color::Cyan)),
                Span::raw(": "),
                Span::raw(value),
            ]));
        }

        if !req.body.is_empty() {
            lines.push(Line::raw(""));
            match std::str::from_utf8(&req.body) {
                Ok(text) => {
                    for line in text.lines().take(50) {
                        lines.push(Line::raw(line.to_string()));
                    }
                }
                Err(_) => {
                    lines.push(Line::styled(
                        format!("[binary: {} bytes]", req.body.len()),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
            }
        }

        Text::from(lines)
    } else if app.intercept_enabled() {
        Text::styled(
            "Waiting for requests...\n\nIntercept is ON. Incoming requests will be held here.",
            Style::default().fg(Color::DarkGray),
        )
    } else {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Current Request ");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        logo::render(frame, inner);
        return;
    };

    let widget = Paragraph::new(content)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Current Request "),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.intercept_ui.scroll, 0));

    frame.render_widget(widget, area);
}

fn render_editor(app: &App, frame: &mut Frame, area: Rect) {
    let lines = app.intercept_ui.editor.render_lines(true);

    let title = {
        let mode_label = app.intercept_ui.editor.mode_label();
        if mode_label.is_empty() {
            " Edit Request (Enter:confirm Esc:cancel) ".to_string()
        } else {
            format!(" Edit Request ({}) (Enter:confirm q:cancel) ", mode_label)
        }
    };

    let widget = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.intercept_ui.scroll, 0));

    frame.render_widget(widget, area);
}

fn render_actions(app: &App, frame: &mut Frame, area: Rect) {
    let has_request = !app.intercept_ui.queue.is_empty();

    let actions = if app.intercept_ui.editing || app.editing_bind_addr || app.editing_scope {
        Line::from(vec![
            Span::styled(" Enter ", Style::default().fg(Color::Green).bold()),
            Span::raw("confirm  "),
            Span::styled(" Esc ", Style::default().fg(Color::Red).bold()),
            Span::raw("cancel"),
        ])
    } else {
        Line::from(vec![
            Span::styled(" i ", key_style()),
            Span::raw("toggle  "),
            if has_request {
                Span::styled(" f ", key_style())
            } else {
                Span::styled(" f ", dim_key_style())
            },
            Span::raw("forward  "),
            if has_request {
                Span::styled(" d ", key_style())
            } else {
                Span::styled(" d ", dim_key_style())
            },
            Span::raw("drop  "),
            if has_request {
                Span::styled(" e ", key_style())
            } else {
                Span::styled(" e ", dim_key_style())
            },
            Span::raw("edit  "),
            Span::styled(" b ", key_style()),
            Span::raw("bind  "),
            Span::styled(" s ", key_style()),
            Span::raw("scope  "),
            Span::styled(" C ", key_style()),
            Span::raw("cert  "),
            Span::styled(" j/k ", key_style()),
            Span::raw("scroll"),
        ])
    };

    let widget = Paragraph::new(actions).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Actions "),
    );
    frame.render_widget(widget, area);
}

use crate::tui::widgets::{key_style, dim_style as dim_key_style};
