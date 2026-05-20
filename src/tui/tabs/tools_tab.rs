use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, ToolsMode};

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .split(area);

    render_mode_selector(app, frame, chunks[0]);

    let panes = Layout::horizontal([
        Constraint::Percentage(50),
        Constraint::Percentage(50),
    ])
    .split(chunks[1]);

    render_input(app, frame, panes[0]);
    render_output(app, frame, panes[1]);
}

fn render_mode_selector(app: &App, frame: &mut Frame, area: Rect) {
    let spans: Vec<Span> = ToolsMode::ALL
        .iter()
        .enumerate()
        .flat_map(|(i, mode)| {
            let style = if *mode == app.tools_mode {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let mut v = vec![Span::styled(format!(" {} ", mode.label()), style)];
            if i < ToolsMode::ALL.len() - 1 {
                v.push(Span::styled("│", Style::default().fg(Color::DarkGray)));
            }
            v
        })
        .collect();

    let widget = Paragraph::new(Line::from(spans)).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Tools (h/l:switch  e:edit  C-u:clear) "),
    );
    frame.render_widget(widget, area);
}

fn render_input(app: &App, frame: &mut Frame, area: Rect) {
    let border_style = if app.tools_editing {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    let title = if app.tools_editing {
        " Input (editing) "
    } else {
        " Input "
    };

    let mut lines: Vec<Line> = Vec::new();
    for (i, line) in app.tools_input.iter().enumerate() {
        if app.tools_editing && i == app.tools_cursor_line {
            let col = app.tools_cursor_col.min(line.len());
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
        } else {
            lines.push(Line::raw(line.clone()));
        }
    }

    if lines.is_empty() || (lines.len() == 1 && app.tools_input[0].is_empty() && !app.tools_editing) {
        lines = vec![Line::styled(
            "Press 'e' to edit input",
            Style::default().fg(Color::DarkGray),
        )];
    }

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

fn render_output(app: &App, frame: &mut Frame, area: Rect) {
    let output = app.tools_output();

    let lines: Vec<Line> = if output.is_empty() {
        vec![Line::styled(
            "Output will appear here",
            Style::default().fg(Color::DarkGray),
        )]
    } else {
        output.lines().map(|l| Line::raw(l.to_string())).collect()
    };

    let widget = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Output "),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.tools_scroll, 0));

    frame.render_widget(widget, area);
}
