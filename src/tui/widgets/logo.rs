use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use std::sync::LazyLock;

const LOGO: &str = include_str!("../../../assets/logo.txt");
const VERSION_TEXT: &str = concat!("v", env!("CARGO_PKG_VERSION"));

struct LogoGeometry {
    lines: Vec<(&'static str, u16)>,
    max_width: u16,
    total_height: u16,
}

static LOGO_GEOMETRY: LazyLock<LogoGeometry> = LazyLock::new(|| {
    let lines: Vec<_> = LOGO
        .lines()
        .map(|line| (line, line.chars().count() as u16))
        .collect();
    let max_width = lines.iter().map(|(_, width)| *width).max().unwrap_or(0);
    let total_height = lines.len() as u16 + 2;
    LogoGeometry {
        lines,
        max_width,
        total_height,
    }
});

pub fn render(frame: &mut Frame, area: Rect) {
    let geometry = &*LOGO_GEOMETRY;

    if area.height < geometry.total_height + 2 || area.width < geometry.max_width {
        return;
    }

    let y_offset = (area.height.saturating_sub(geometry.total_height)) / 2;

    for (i, (line, line_width)) in geometry.lines.iter().enumerate() {
        let x_offset = (area.width.saturating_sub(*line_width)) / 2;
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
        buf.set_line(
            area.x + x_offset,
            y,
            &Line::from(span),
            area.width.saturating_sub(x_offset),
        );
    }

    let version_y = area.y + y_offset + geometry.lines.len() as u16 + 1;
    if version_y < area.y + area.height {
        let version_width = VERSION_TEXT.len() as u16;
        let version_x = (area.width.saturating_sub(version_width)) / 2;
        let version_span = Span::styled(VERSION_TEXT, Style::default().fg(Color::DarkGray));
        let buf = frame.buffer_mut();
        buf.set_line(
            area.x + version_x,
            version_y,
            &Line::from(version_span),
            area.width.saturating_sub(version_x),
        );
    }
}
