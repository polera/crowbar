use std::time::Duration;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::http::models::TimingData;

pub fn timing_lines(timing: &TimingData, total: Duration) -> Vec<Line<'static>> {
    let total_ms = total.as_secs_f64() * 1000.0;
    if total_ms == 0.0 {
        return vec![];
    }

    let phases: Vec<(&str, Option<Duration>, Color)> = vec![
        ("TCP Connect", timing.tcp_connect, Color::Cyan),
        ("TLS Handshake", timing.tls_handshake, Color::Magenta),
        ("HTTP Handshake", timing.http_handshake, Color::Blue),
        ("Server Wait", timing.time_to_first_byte, Color::Yellow),
        ("Content Xfer", timing.content_transfer, Color::Green),
    ];

    let bar_width: usize = 24;
    let mut lines = Vec::new();

    lines.push(Line::styled(
        "\u{2500}\u{2500}\u{2500}\u{2500} Timing \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
        Style::default().fg(Color::DarkGray),
    ));

    for (label, duration, color) in &phases {
        if let Some(dur) = duration {
            let ms = dur.as_secs_f64() * 1000.0;
            let fraction = ms / total_ms;
            let filled = (fraction * bar_width as f64)
                .round()
                .max(1.0)
                .min(bar_width as f64) as usize;

            let bar: String = "\u{2588}".repeat(filled);
            let pad: String = "\u{2591}".repeat(bar_width.saturating_sub(filled));

            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:>14}  ", label),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(bar, Style::default().fg(*color)),
                Span::styled(pad, Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("  {}", format_duration_ms(ms)),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
    }

    lines.push(Line::from(vec![
        Span::styled(
            format!("{:>14}  ", "Total"),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            format_duration_ms(total_ms),
            Style::default().fg(Color::White),
        ),
    ]));

    lines
}

fn format_duration_ms(ms: f64) -> String {
    if ms < 1.0 {
        format!("{:.2}ms", ms)
    } else if ms < 10.0 {
        format!("{:.1}ms", ms)
    } else if ms < 1000.0 {
        format!("{:.0}ms", ms)
    } else {
        format!("{:.2}s", ms / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_phase(line: &Line<'_>, label: &str, duration: &str, filled: usize, color: Color) {
        assert_eq!(line.spans.len(), 4);
        assert_eq!(line.spans[0].content.trim(), label);
        assert_eq!(line.spans[1].content, "█".repeat(filled));
        assert_eq!(line.spans[1].style.fg, Some(color));
        assert_eq!(line.spans[2].content, "░".repeat(24 - filled));
        assert_eq!(line.spans[3].content.trim(), duration);
    }

    fn assert_total(line: &Line<'_>, duration: &str) {
        assert_eq!(line.spans.len(), 2);
        assert_eq!(line.spans[0].content.trim(), "Total");
        assert_eq!(line.spans[1].content, duration);
    }

    #[test]
    fn format_duration_sub_ms() {
        assert_eq!(format_duration_ms(0.42), "0.42ms");
        assert_eq!(format_duration_ms(0.01), "0.01ms");
    }

    #[test]
    fn format_duration_small_ms() {
        assert_eq!(format_duration_ms(5.3), "5.3ms");
        assert_eq!(format_duration_ms(9.9), "9.9ms");
    }

    #[test]
    fn format_duration_large_ms() {
        assert_eq!(format_duration_ms(100.0), "100ms");
        assert_eq!(format_duration_ms(999.0), "999ms");
    }

    #[test]
    fn format_duration_seconds() {
        assert_eq!(format_duration_ms(1000.0), "1.00s");
        assert_eq!(format_duration_ms(2500.0), "2.50s");
    }

    #[test]
    fn timing_lines_zero_total_returns_empty() {
        let timing = TimingData {
            tcp_connect: Some(Duration::from_millis(10)),
            tls_handshake: None,
            http_handshake: None,
            time_to_first_byte: None,
            content_transfer: None,
        };
        assert!(timing_lines(&timing, Duration::ZERO).is_empty());
    }

    #[test]
    fn timing_lines_all_phases() {
        let timing = TimingData {
            tcp_connect: Some(Duration::from_millis(10)),
            tls_handshake: Some(Duration::from_millis(20)),
            http_handshake: Some(Duration::from_millis(5)),
            time_to_first_byte: Some(Duration::from_millis(50)),
            content_transfer: Some(Duration::from_millis(30)),
        };
        let total = Duration::from_millis(115);
        let lines = timing_lines(&timing, total);
        assert_eq!(lines.len(), 7);
        assert!(lines[0].to_string().contains("Timing"));
        assert_phase(&lines[1], "TCP Connect", "10ms", 2, Color::Cyan);
        assert_phase(&lines[2], "TLS Handshake", "20ms", 4, Color::Magenta);
        assert_phase(&lines[3], "HTTP Handshake", "5.0ms", 1, Color::Blue);
        assert_phase(&lines[4], "Server Wait", "50ms", 10, Color::Yellow);
        assert_phase(&lines[5], "Content Xfer", "30ms", 6, Color::Green);
        assert_total(&lines[6], "115ms");
    }

    #[test]
    fn timing_lines_partial_phases() {
        let timing = TimingData {
            tcp_connect: Some(Duration::from_millis(10)),
            tls_handshake: None,
            http_handshake: None,
            time_to_first_byte: Some(Duration::from_millis(50)),
            content_transfer: None,
        };
        let total = Duration::from_millis(60);
        let lines = timing_lines(&timing, total);
        assert_eq!(lines.len(), 4);
        assert_phase(&lines[1], "TCP Connect", "10ms", 4, Color::Cyan);
        assert_phase(&lines[2], "Server Wait", "50ms", 20, Color::Yellow);
        assert_total(&lines[3], "60ms");
    }

    #[test]
    fn timing_bars_remain_within_width_for_tiny_and_over_total_phases() {
        let timing = TimingData {
            tcp_connect: Some(Duration::from_micros(1)),
            tls_handshake: Some(Duration::from_millis(200)),
            http_handshake: None,
            time_to_first_byte: None,
            content_transfer: None,
        };
        let lines = timing_lines(&timing, Duration::from_millis(100));
        assert_eq!(lines.len(), 4);
        assert_phase(&lines[1], "TCP Connect", "0.00ms", 1, Color::Cyan);
        assert_phase(&lines[2], "TLS Handshake", "200ms", 24, Color::Magenta);
        assert_total(&lines[3], "100ms");
    }
}
