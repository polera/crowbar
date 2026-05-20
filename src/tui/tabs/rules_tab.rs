use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table, Wrap};
use ratatui::Frame;

use crate::app::{App, RuleField};

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(3),
    ])
    .split(area);

    let rules = app.rules.read().unwrap();

    if rules.is_empty() {
        let msg = Paragraph::new(Line::styled(
            "No rules configured. Press 'a' to add a rule.",
            Style::default().fg(Color::DarkGray),
        ))
        .block(Block::default().borders(Borders::ALL).title(" Match & Replace Rules "));
        frame.render_widget(msg, chunks[0]);
    } else {
        let detail_split = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(8),
        ])
        .split(chunks[0]);

        render_table(app, &rules, frame, detail_split[0]);
        render_detail(app, &rules, frame, detail_split[1]);
    }

    render_actions(app, frame, chunks[1]);
}

fn render_table(app: &App, rules: &[crate::rules::Rule], frame: &mut Frame, area: Rect) {
    let header = Row::new(["", "Name", "Target", "Scope", "Match", "Replace", "Regex"])
        .style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

    let rows: Vec<Row> = rules
        .iter()
        .enumerate()
        .map(|(i, rule)| {
            let enabled = if rule.enabled { "ON" } else { "--" };
            let enabled_style = if rule.enabled {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let row_style = if i == app.rules_ui.selected {
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            Row::new([
                Span::styled(enabled.to_string(), enabled_style).to_string(),
                rule.name.clone(),
                rule.target.label().to_string(),
                rule.scope.label().to_string(),
                truncate(&rule.match_pattern, 25),
                truncate(&rule.replacement, 25),
                if rule.is_regex { "yes" } else { "no" }.to_string(),
            ])
            .style(row_style)
        })
        .collect();

    let widths = [
        Constraint::Length(3),
        Constraint::Min(12),
        Constraint::Length(9),
        Constraint::Length(8),
        Constraint::Min(15),
        Constraint::Min(15),
        Constraint::Length(5),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(" Match & Replace Rules "));

    frame.render_widget(table, area);
}

fn render_detail(app: &App, rules: &[crate::rules::Rule], frame: &mut Frame, area: Rect) {
    if app.rules_ui.selected >= rules.len() {
        return;
    }
    let rule = &rules[app.rules_ui.selected];

    let name_style = field_style(app, RuleField::Name);
    let pattern_style = field_style(app, RuleField::Pattern);
    let replacement_style = field_style(app, RuleField::Replacement);

    let name_val = field_value(app, RuleField::Name, &rule.name);
    let pattern_val = field_value(app, RuleField::Pattern, &rule.match_pattern);
    let replacement_val = field_value(app, RuleField::Replacement, &rule.replacement);

    let label = Style::default().fg(Color::Cyan);

    let lines = vec![
        Line::from(vec![
            Span::styled(" Name:        ", label),
            Span::styled(name_val, name_style),
        ]),
        Line::from(vec![
            Span::styled(" Pattern:     ", label),
            Span::styled(pattern_val, pattern_style),
        ]),
        Line::from(vec![
            Span::styled(" Replacement: ", label),
            Span::styled(replacement_val, replacement_style),
        ]),
        Line::from(vec![
            Span::styled(" Target: ", label),
            Span::raw(rule.target.label()),
            Span::styled("  Scope: ", label),
            Span::raw(rule.scope.label()),
            Span::styled("  Regex: ", label),
            Span::raw(if rule.is_regex { "yes" } else { "no" }),
        ]),
    ];

    let editing_title = match app.rules_ui.editing_field {
        Some(RuleField::Name) => " Detail (editing name) ",
        Some(RuleField::Pattern) => " Detail (editing pattern) ",
        Some(RuleField::Replacement) => " Detail (editing replacement) ",
        None => " Detail ",
    };

    let border_style = if app.rules_ui.editing_field.is_some() {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    let widget = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(editing_title)
                .border_style(border_style),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(widget, area);
}

fn render_actions(_app: &App, frame: &mut Frame, area: Rect) {
    let key = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);

    let line = Line::from(vec![
        Span::styled(" a", key),
        Span::raw(":add "),
        Span::styled("x", key),
        Span::raw(":delete "),
        Span::styled("Enter", key),
        Span::raw(":toggle "),
        Span::styled("n", key),
        Span::raw(":name "),
        Span::styled("p", key),
        Span::raw(":pattern "),
        Span::styled("e", key),
        Span::raw(":replace "),
        Span::styled("t", key),
        Span::raw(":target "),
        Span::styled("s", key),
        Span::raw(":scope "),
        Span::styled("R", key),
        Span::raw(":regex"),
    ]);

    let widget = Paragraph::new(line).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Actions "),
    );
    frame.render_widget(widget, area);
}

fn field_style(app: &App, field: RuleField) -> Style {
    if app.rules_ui.editing_field == Some(field) {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    }
}

fn field_value(app: &App, field: RuleField, current: &str) -> String {
    if app.rules_ui.editing_field == Some(field) {
        format!("{}|", app.rules_ui.edit_buffer)
    } else if current.is_empty() {
        "(empty)".to_string()
    } else {
        current.to_string()
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}...", &s[..max - 3])
    } else {
        s.to_string()
    }
}
