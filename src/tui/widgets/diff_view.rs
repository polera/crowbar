use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use similar::{ChangeTag, TextDiff};

pub fn diff_lines<'a>(original: &[String], modified: &[String]) -> Vec<Line<'a>> {
    // Diff the existing line slices directly. Joining both inputs copied every
    // request byte on each frame while the diff view was visible.
    let original: Vec<&str> = original.iter().map(String::as_str).collect();
    let modified: Vec<&str> = modified.iter().map(String::as_str).collect();
    let diff = TextDiff::from_slices(&original, &modified);
    let mut lines = Vec::new();

    for change in diff.iter_all_changes() {
        let (prefix, style) = match change.tag() {
            ChangeTag::Delete => ("-", Style::default().fg(Color::Red)),
            ChangeTag::Insert => ("+", Style::default().fg(Color::Green)),
            ChangeTag::Equal => (" ", Style::default().fg(Color::DarkGray)),
        };

        let text = change.as_str().unwrap_or("");
        lines.push(Line::from(vec![
            Span::styled(prefix, style.add_modifier(Modifier::BOLD)),
            Span::styled(format!(" {}", text), style),
        ]));
    }

    if lines.is_empty() {
        lines.push(Line::styled(
            "  No differences",
            Style::default().fg(Color::DarkGray),
        ));
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_input() {
        let lines = vec!["GET /path HTTP/1.1".into(), "Host: example.com".into()];
        let result = diff_lines(&lines, &lines);
        assert!(result.iter().all(|l| {
            let text = l.spans.first().map(|s| s.content.as_ref()).unwrap_or("");
            text == " "
        }));
    }

    #[test]
    fn added_line() {
        let old = vec!["line1".into()];
        let new = vec!["line1".into(), "line2".into()];
        let result = diff_lines(&old, &new);
        let has_addition = result
            .iter()
            .any(|l| l.spans.first().map(|s| s.content.as_ref()) == Some("+"));
        assert!(has_addition);
    }

    #[test]
    fn removed_line() {
        let old = vec!["line1".into(), "line2".into()];
        let new = vec!["line1".into()];
        let result = diff_lines(&old, &new);
        let has_deletion = result
            .iter()
            .any(|l| l.spans.first().map(|s| s.content.as_ref()) == Some("-"));
        assert!(has_deletion);
    }
}
