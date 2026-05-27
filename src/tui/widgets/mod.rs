pub mod body_view;
pub mod diff_view;
pub mod hex_view;
pub mod logo;
pub mod status_bar;
pub mod timing_view;

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

pub fn header_lines<'a>(headers: &'a [(String, String)]) -> Vec<Line<'a>> {
    headers
        .iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(k.as_str(), Style::default().fg(Color::Cyan)),
                Span::raw(": "),
                Span::raw(v.as_str()),
            ])
        })
        .collect()
}

pub fn trailer_lines<'a>(trailers: &'a [(String, String)]) -> Vec<Line<'a>> {
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
            Span::styled(key.as_str(), Style::default().fg(Color::Cyan)),
            Span::raw(": "),
            Span::styled(value.as_str(), value_style),
        ]));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_size_bytes() {
        assert_eq!(format_size(0), "0B");
        assert_eq!(format_size(100), "100B");
        assert_eq!(format_size(1023), "1023B");
    }

    #[test]
    fn format_size_kilobytes() {
        assert_eq!(format_size(1024), "1.0KB");
        assert_eq!(format_size(1536), "1.5KB");
        assert_eq!(format_size(1024 * 1023), "1023.0KB");
    }

    #[test]
    fn format_size_megabytes() {
        assert_eq!(format_size(1024 * 1024), "1.0MB");
        assert_eq!(format_size(1024 * 1024 * 5), "5.0MB");
    }

    #[test]
    fn key_style_yellow_bold() {
        let style = key_style();
        assert_eq!(style.fg, Some(Color::Yellow));
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn dim_style_dark_gray() {
        let style = dim_style();
        assert_eq!(style.fg, Some(Color::DarkGray));
    }

    #[test]
    fn header_lines_formatting() {
        let headers = vec![
            ("content-type".into(), "text/html".into()),
            ("x-custom".into(), "value".into()),
        ];
        let lines = header_lines(&headers);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].spans.len(), 3);
        assert_eq!(lines[0].spans[0].content, "content-type");
        assert_eq!(lines[0].spans[1].content, ": ");
        assert_eq!(lines[0].spans[2].content, "text/html");
    }

    #[test]
    fn header_lines_cyan_key() {
        let headers = vec![("key".into(), "val".into())];
        let lines = header_lines(&headers);
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Cyan));
    }

    #[test]
    fn trailer_lines_has_separator() {
        let trailers = vec![("grpc-status".into(), "0".into())];
        let lines = trailer_lines(&trailers);
        assert!(lines.len() >= 3); // empty line, separator, trailer
        assert!(lines[1].spans[0].content.contains("Trailers"));
    }

    #[test]
    fn trailer_lines_grpc_status_ok_green() {
        let trailers = vec![("grpc-status".into(), "0".into())];
        let lines = trailer_lines(&trailers);
        let value_span = &lines[2].spans[2];
        assert_eq!(value_span.style.fg, Some(Color::Green));
    }

    #[test]
    fn trailer_lines_grpc_status_error_red() {
        let trailers = vec![("grpc-status".into(), "13".into())];
        let lines = trailer_lines(&trailers);
        let value_span = &lines[2].spans[2];
        assert_eq!(value_span.style.fg, Some(Color::Red));
    }

    #[test]
    fn trailer_lines_non_grpc_default_style() {
        let trailers = vec![("grpc-message".into(), "not found".into())];
        let lines = trailer_lines(&trailers);
        let value_span = &lines[2].spans[2];
        assert_eq!(value_span.style, Style::default());
    }
}
