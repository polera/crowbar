use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

const LOGO: &str = include_str!("../../../assets/logo.txt");

pub fn render(frame: &mut Frame, area: Rect) {
    let logo_lines: Vec<&str> = LOGO.lines().collect();
    let version_text = format!("v{}", env!("CARGO_PKG_VERSION"));
    let total_height = logo_lines.len() as u16 + 2;
    let max_width = logo_lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u16;

    if area.height < total_height + 2 || area.width < max_width {
        return;
    }

    let y_offset = (area.height.saturating_sub(total_height)) / 2;

    for (i, line) in logo_lines.iter().enumerate() {
        let line_width = line.chars().count() as u16;
        let x_offset = (area.width.saturating_sub(line_width)) / 2;
        let y = area.y + y_offset + i as u16;

        if y >= area.y + area.height {
            break;
        }

        let style = match i {
            1..=5 => Style::default().fg(Color::Cyan),
            6..=9 => Style::default().fg(Color::DarkGray),
            11 | 13 => Style::default().fg(Color::Yellow),
            12 => Style::default().fg(Color::White),
            14 | 15 => Style::default().fg(Color::DarkGray),
            _ => Style::default(),
        };

        let span = Span::styled(*line, style);
        let buf = frame.buffer_mut();
        buf.set_line(area.x + x_offset, y, &Line::from(span), area.width.saturating_sub(x_offset));
    }

    let version_y = area.y + y_offset + logo_lines.len() as u16 + 1;
    if version_y < area.y + area.height {
        let version_width = version_text.chars().count() as u16;
        let version_x = (area.width.saturating_sub(version_width)) / 2;
        let version_span = Span::styled(version_text, Style::default().fg(Color::DarkGray));
        let buf = frame.buffer_mut();
        buf.set_line(area.x + version_x, version_y, &Line::from(version_span), area.width.saturating_sub(version_x));
    }
}
