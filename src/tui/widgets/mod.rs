pub mod body_view;
pub mod diff_view;
pub mod hex_view;
pub mod logo;
pub mod status_bar;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

pub fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

pub fn key_style() -> Style {
    Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD)
}

pub fn dim_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

pub fn header_lines(headers: &[(String, String)]) -> Vec<Line<'static>> {
    headers
        .iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(k.clone(), Style::default().fg(Color::Cyan)),
                Span::raw(": "),
                Span::raw(v.clone()),
            ])
        })
        .collect()
}

pub fn trailer_lines(trailers: &[(String, String)]) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::raw(""),
        Line::styled(
            "──── Trailers ────",
            Style::default().fg(Color::DarkGray),
        ),
    ];
    for (key, value) in trailers {
        let value_style = if key == "grpc-status" {
            if value == "0" {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Red)
            }
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            Span::styled(key.clone(), Style::default().fg(Color::Cyan)),
            Span::raw(": "),
            Span::styled(value.clone(), value_style),
        ]));
    }
    lines
}
