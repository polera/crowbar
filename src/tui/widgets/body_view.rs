use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::http::protobuf::{self, ProtoField, ProtoValue};
use crate::tui::widgets::hex_view;

pub fn body_lines<'a>(
    body: &[u8],
    content_type: Option<&str>,
    max_lines: usize,
) -> Vec<Line<'a>> {
    let ct = content_type.unwrap_or("");

    if ct.starts_with("application/grpc") {
        return render_grpc(body, max_lines);
    }

    if ct.contains("protobuf") || ct.contains("x-protobuf") {
        return render_protobuf(body, max_lines);
    }

    let text = match std::str::from_utf8(body) {
        Ok(t) => t,
        Err(_) => {
            let mut lines = vec![Line::styled(
                format!("[binary: {} bytes]", body.len()),
                Style::default().fg(Color::DarkGray),
            )];
            lines.extend(hex_view::hex_lines(body, 64));
            return lines;
        }
    };

    if ct.contains("json") {
        return render_json(text, max_lines);
    }

    if ct.contains("x-www-form-urlencoded") {
        return render_form_urlencoded(text, max_lines);
    }

    if ct.contains("multipart/form-data") {
        return render_multipart(text, ct, max_lines);
    }

    render_plain(text, max_lines)
}

fn render_plain<'a>(text: &str, max_lines: usize) -> Vec<Line<'a>> {
    let mut lines: Vec<Line<'a>> = Vec::new();
    for line in text.lines().take(max_lines) {
        lines.push(Line::raw(line.to_string()));
    }
    if text.lines().count() > max_lines {
        lines.push(Line::styled(
            "... truncated",
            Style::default().fg(Color::DarkGray),
        ));
    }
    lines
}

fn render_json<'a>(text: &str, max_lines: usize) -> Vec<Line<'a>> {
    let pretty = match serde_json::from_str::<serde_json::Value>(text) {
        Ok(val) => serde_json::to_string_pretty(&val).unwrap_or_else(|_| text.to_string()),
        Err(_) => return render_plain(text, max_lines),
    };

    let mut lines: Vec<Line<'a>> = Vec::new();
    for line in pretty.lines().take(max_lines) {
        lines.push(colorize_json_line(line));
    }
    if pretty.lines().count() > max_lines {
        lines.push(Line::styled(
            "... truncated",
            Style::default().fg(Color::DarkGray),
        ));
    }
    lines
}

fn colorize_json_line<'a>(line: &str) -> Line<'a> {
    let trimmed = line.trim_start();

    if trimmed.starts_with('"')
        && let Some(colon_pos) = trimmed.find("\": ") {
            let indent = &line[..line.len() - trimmed.len()];
            let key = &trimmed[..colon_pos + 1];
            let rest = &trimmed[colon_pos + 1..];

            return Line::from(vec![
                Span::raw(indent.to_string()),
                Span::styled(key.to_string(), Style::default().fg(Color::Cyan)),
                Span::styled(rest.to_string(), value_style(rest.trim_start().trim_start_matches(": "))),
            ]);
        }

    if trimmed.starts_with('"') {
        return Line::styled(line.to_string(), Style::default().fg(Color::Green));
    }

    if trimmed == "null" || trimmed == "null," {
        return Line::styled(line.to_string(), Style::default().fg(Color::DarkGray));
    }

    if trimmed == "true" || trimmed == "true," || trimmed == "false" || trimmed == "false," {
        return Line::styled(line.to_string(), Style::default().fg(Color::Yellow));
    }

    if trimmed.starts_with(|c: char| c.is_ascii_digit() || c == '-') {
        return Line::styled(line.to_string(), Style::default().fg(Color::Magenta));
    }

    Line::raw(line.to_string())
}

fn value_style(value: &str) -> Style {
    let v = value.trim_end_matches(',');
    if v.starts_with('"') {
        Style::default().fg(Color::Green)
    } else if v == "null" {
        Style::default().fg(Color::DarkGray)
    } else if v == "true" || v == "false" {
        Style::default().fg(Color::Yellow)
    } else if v.starts_with(|c: char| c.is_ascii_digit() || c == '-') {
        Style::default().fg(Color::Magenta)
    } else {
        Style::default()
    }
}

fn render_form_urlencoded<'a>(text: &str, max_lines: usize) -> Vec<Line<'a>> {
    let mut lines: Vec<Line<'a>> = Vec::new();

    lines.push(Line::styled(
        "[form-urlencoded]",
        Style::default().fg(Color::DarkGray),
    ));

    for (i, pair) in text.split('&').take(max_lines).enumerate() {
        if i >= max_lines - 1 {
            lines.push(Line::styled(
                "... truncated",
                Style::default().fg(Color::DarkGray),
            ));
            break;
        }

        if let Some((key, value)) = pair.split_once('=') {
            let decoded_key = simple_url_decode(key);
            let decoded_value = simple_url_decode(value);
            lines.push(Line::from(vec![
                Span::styled(decoded_key, Style::default().fg(Color::Cyan)),
                Span::styled(" = ", Style::default().fg(Color::DarkGray)),
                Span::styled(decoded_value, Style::default().fg(Color::Green)),
            ]));
        } else {
            lines.push(Line::raw(pair.to_string()));
        }
    }

    lines
}

fn render_multipart<'a>(text: &str, content_type: &str, max_lines: usize) -> Vec<Line<'a>> {
    let boundary = content_type
        .split("boundary=")
        .nth(1)
        .map(|b| b.trim_matches('"').trim())
        .unwrap_or("");

    let mut lines: Vec<Line<'a>> = Vec::new();

    lines.push(Line::styled(
        format!("[multipart/form-data, boundary={}]", boundary),
        Style::default().fg(Color::DarkGray),
    ));

    if boundary.is_empty() {
        lines.extend(render_plain(text, max_lines));
        return lines;
    }

    let separator = format!("--{}", boundary);

    for part in text.split(&separator) {
        let part = part.trim();
        if part.is_empty() || part == "--" {
            continue;
        }

        if lines.len() >= max_lines {
            lines.push(Line::styled(
                "... truncated",
                Style::default().fg(Color::DarkGray),
            ));
            break;
        }

        lines.push(Line::styled(
            "──── part ────".to_string(),
            Style::default().fg(Color::DarkGray),
        ));

        let mut in_headers = true;
        for line in part.lines() {
            if lines.len() >= max_lines {
                break;
            }

            if in_headers {
                if line.is_empty() {
                    in_headers = false;
                    lines.push(Line::raw(""));
                    continue;
                }
                if let Some((key, value)) = line.split_once(':') {
                    lines.push(Line::from(vec![
                        Span::styled(key.to_string(), Style::default().fg(Color::Cyan)),
                        Span::raw(":"),
                        Span::raw(value.to_string()),
                    ]));
                } else {
                    lines.push(Line::raw(line.to_string()));
                }
            } else {
                lines.push(Line::raw(line.to_string()));
            }
        }
    }

    lines
}

fn render_grpc<'a>(body: &[u8], max_lines: usize) -> Vec<Line<'a>> {
    if body.len() < 5 {
        return vec![Line::styled(
            format!("[gRPC: {} bytes, too small to decode]", body.len()),
            Style::default().fg(Color::DarkGray),
        )];
    }

    let messages = protobuf::decode_grpc_body(body);

    if messages.is_empty() {
        let mut lines = vec![Line::styled(
            format!("[gRPC: {} bytes, invalid frame]", body.len()),
            Style::default().fg(Color::DarkGray),
        )];
        lines.extend(hex_view::hex_lines(body, 64));
        return lines;
    }

    let mut lines: Vec<Line<'a>> = Vec::new();

    for (i, msg) in messages.iter().enumerate() {
        if lines.len() >= max_lines {
            lines.push(Line::styled(
                "... truncated",
                Style::default().fg(Color::DarkGray),
            ));
            break;
        }

        let label = if messages.len() == 1 {
            format!(
                "── gRPC message ({} bytes{}) ──",
                msg.size,
                if msg.compressed { ", compressed" } else { "" }
            )
        } else {
            format!(
                "── gRPC message {} ({} bytes{}) ──",
                i + 1,
                msg.size,
                if msg.compressed { ", compressed" } else { "" }
            )
        };
        lines.push(Line::styled(label, Style::default().fg(Color::DarkGray)));

        if msg.compressed {
            lines.push(Line::styled(
                "  [compressed payload, cannot decode]",
                Style::default().fg(Color::Yellow),
            ));
            continue;
        }

        match &msg.fields {
            Some(fields) => render_proto_fields(fields, 1, max_lines, &mut lines),
            None => lines.push(Line::styled(
                "  [could not decode protobuf]",
                Style::default().fg(Color::Yellow),
            )),
        }
    }

    lines
}

fn render_protobuf<'a>(body: &[u8], max_lines: usize) -> Vec<Line<'a>> {
    match protobuf::decode_raw(body) {
        Some(fields) => {
            let mut lines = vec![Line::styled(
                format!("[protobuf: {} bytes]", body.len()),
                Style::default().fg(Color::DarkGray),
            )];
            render_proto_fields(&fields, 1, max_lines, &mut lines);
            lines
        }
        None => {
            let mut lines = vec![Line::styled(
                format!("[protobuf: {} bytes, could not decode]", body.len()),
                Style::default().fg(Color::DarkGray),
            )];
            lines.extend(hex_view::hex_lines(body, 64));
            lines
        }
    }
}

fn render_proto_fields<'a>(
    fields: &[ProtoField],
    depth: usize,
    max_lines: usize,
    lines: &mut Vec<Line<'a>>,
) {
    let indent = "  ".repeat(depth);

    for field in fields {
        if lines.len() >= max_lines {
            lines.push(Line::styled(
                "... truncated",
                Style::default().fg(Color::DarkGray),
            ));
            return;
        }

        match &field.value {
            ProtoValue::Varint(v) => {
                lines.push(Line::from(vec![
                    Span::raw(indent.clone()),
                    Span::styled(
                        field.number.to_string(),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(": ", Style::default().fg(Color::DarkGray)),
                    Span::styled(v.to_string(), Style::default().fg(Color::Magenta)),
                ]));
            }
            ProtoValue::Fixed64(v) => {
                let display = protobuf::format_fixed64(*v);
                let is_double = display.contains('.');
                let mut spans = vec![
                    Span::raw(indent.clone()),
                    Span::styled(
                        field.number.to_string(),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(": ", Style::default().fg(Color::DarkGray)),
                    Span::styled(display, Style::default().fg(Color::Magenta)),
                ];
                if is_double {
                    spans.push(Span::styled(
                        " (double)",
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                lines.push(Line::from(spans));
            }
            ProtoValue::Fixed32(v) => {
                let display = protobuf::format_fixed32(*v);
                let is_float = display.contains('.');
                let mut spans = vec![
                    Span::raw(indent.clone()),
                    Span::styled(
                        field.number.to_string(),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(": ", Style::default().fg(Color::DarkGray)),
                    Span::styled(display, Style::default().fg(Color::Magenta)),
                ];
                if is_float {
                    spans.push(Span::styled(
                        " (float)",
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                lines.push(Line::from(spans));
            }
            ProtoValue::String(s) => {
                lines.push(Line::from(vec![
                    Span::raw(indent.clone()),
                    Span::styled(
                        field.number.to_string(),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(": ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("\"{}\"", truncate_string(s, 200)),
                        Style::default().fg(Color::Green),
                    ),
                ]));
            }
            ProtoValue::Message(sub_fields) => {
                lines.push(Line::from(vec![
                    Span::raw(indent.clone()),
                    Span::styled(
                        field.number.to_string(),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(" {", Style::default().fg(Color::DarkGray)),
                ]));
                render_proto_fields(sub_fields, depth + 1, max_lines, lines);
                if lines.len() < max_lines {
                    lines.push(Line::from(vec![
                        Span::raw(indent.clone()),
                        Span::styled("}", Style::default().fg(Color::DarkGray)),
                    ]));
                }
            }
            ProtoValue::Bytes(data) => {
                lines.push(Line::from(vec![
                    Span::raw(indent.clone()),
                    Span::styled(
                        field.number.to_string(),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(": ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("<{} bytes>", data.len()),
                        Style::default().fg(Color::Yellow),
                    ),
                ]));
            }
        }
    }
}

fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

fn simple_url_decode(input: &str) -> String {
    crate::http::url_decode(input)
}
