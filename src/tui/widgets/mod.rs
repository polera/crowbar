pub mod body_view;
pub mod diff_view;
pub mod hex_view;
pub mod logo;
pub mod status_bar;

use ratatui::style::{Color, Modifier, Style};

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
