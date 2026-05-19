use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

const LOGO: &str = include_str!("../../../assets/logo.txt");

pub fn render(frame: &mut Frame, area: Rect) {
    let logo_lines: Vec<&str> = LOGO.lines().collect();
    let logo_height = logo_lines.len() as u16;
    let max_width = logo_lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u16;

    if area.height < logo_height + 2 || area.width < max_width {
        return;
    }

    let y_offset = (area.height.saturating_sub(logo_height)) / 2;

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
}
