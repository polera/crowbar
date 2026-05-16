use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

pub fn hex_lines(data: &[u8], max_lines: usize) -> Vec<Line<'static>> {
    let bytes_per_line = 16;
    let mut lines = Vec::new();
    let total_lines = data.len().div_ceil(bytes_per_line);
    let display_lines = total_lines.min(max_lines);

    for i in 0..display_lines {
        let offset = i * bytes_per_line;
        let chunk = &data[offset..(offset + bytes_per_line).min(data.len())];

        let mut spans = Vec::new();

        spans.push(Span::styled(
            format!("{:08x}  ", offset),
            Style::default().fg(Color::DarkGray),
        ));

        let mut hex_part = String::with_capacity(49);
        for (j, byte) in chunk.iter().enumerate() {
            if j == 8 {
                hex_part.push(' ');
            }
            hex_part.push_str(&format!("{:02x} ", byte));
        }
        for _ in chunk.len()..bytes_per_line {
            hex_part.push_str("   ");
        }
        if chunk.len() <= 8 {
            hex_part.push(' ');
        }
        spans.push(Span::styled(hex_part, Style::default().fg(Color::Yellow)));

        spans.push(Span::raw(" |"));
        let ascii: String = chunk
            .iter()
            .map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '.' })
            .collect();
        spans.push(Span::raw(ascii));
        spans.push(Span::raw("|"));

        lines.push(Line::from(spans));
    }

    if total_lines > max_lines {
        lines.push(Line::styled(
            format!("... {} more lines", total_lines - max_lines),
            Style::default().fg(Color::DarkGray),
        ));
    }

    lines
}
