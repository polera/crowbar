use std::cell::RefCell;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use prost_reflect::MessageDescriptor;

use crate::http::protobuf::{self, ProtoField, ProtoValue};
use crate::http::proto_schema;
use crate::tui::widgets::hex_view;

thread_local! {
    static JSON_CACHE: RefCell<Option<(*const u8, usize, String)>> = const { RefCell::new(None) };
}

/// Render a body to styled lines. For gRPC/protobuf bodies, `proto_type` (a
/// message type resolved from a loaded `.proto` schema) enables named fields
/// and accurate types; pass `None` to use the schema-agnostic heuristic
/// decoder. `proto_type` is ignored for non-protobuf content.
pub fn body_lines_with_schema<'a>(
    body: &[u8],
    content_type: Option<&str>,
    max_lines: usize,
    proto_type: Option<&MessageDescriptor>,
) -> Vec<Line<'a>> {
    let ct = content_type.unwrap_or("");

    if ct.starts_with("application/grpc") {
        return render_grpc(body, max_lines, proto_type);
    }

    if ct.contains("protobuf") || ct.contains("x-protobuf") {
        return render_protobuf(body, max_lines, proto_type);
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
    let mut iter = text.lines();
    for line in iter.by_ref().take(max_lines) {
        lines.push(Line::raw(line.to_string()));
    }
    if iter.next().is_some() {
        lines.push(Line::styled(
            "... truncated",
            Style::default().fg(Color::DarkGray),
        ));
    }
    lines
}

fn render_json<'a>(text: &str, max_lines: usize) -> Vec<Line<'a>> {
    let key = (text.as_ptr(), text.len());
    let pretty = JSON_CACHE.with(|cache| {
        let cached = cache.borrow();
        if let Some((ptr, len, ref s)) = *cached
            && ptr == key.0 && len == key.1 {
                return Some(s.clone());
            }
        None
    });

    let pretty = match pretty {
        Some(s) => s,
        None => {
            let s = match serde_json::from_str::<serde_json::Value>(text) {
                Ok(val) => serde_json::to_string_pretty(&val).unwrap_or_else(|_| text.to_string()),
                Err(_) => return render_plain(text, max_lines),
            };
            JSON_CACHE.with(|cache| {
                *cache.borrow_mut() = Some((key.0, key.1, s.clone()));
            });
            s
        }
    };

    let mut lines: Vec<Line<'a>> = Vec::new();
    let mut iter = pretty.lines();
    for line in iter.by_ref().take(max_lines) {
        lines.push(colorize_json_line(line));
    }
    if iter.next().is_some() {
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

/// Split a gRPC body into frames, returning `(compressed, payload)` for each.
fn grpc_frames(body: &[u8]) -> Vec<(bool, &[u8])> {
    let mut frames = Vec::new();
    let mut pos = 0;
    while pos + 5 <= body.len() {
        let compressed = body[pos] != 0;
        let len = u32::from_be_bytes([body[pos + 1], body[pos + 2], body[pos + 3], body[pos + 4]])
            as usize;
        if pos + 5 + len > body.len() {
            break;
        }
        frames.push((compressed, &body[pos + 5..pos + 5 + len]));
        pos += 5 + len;
    }
    frames
}

fn render_grpc<'a>(
    body: &[u8],
    max_lines: usize,
    proto_type: Option<&MessageDescriptor>,
) -> Vec<Line<'a>> {
    if body.len() < 5 {
        return vec![Line::styled(
            format!("[gRPC: {} bytes, too small to decode]", body.len()),
            Style::default().fg(Color::DarkGray),
        )];
    }

    let frames = grpc_frames(body);

    if frames.is_empty() {
        let mut lines = vec![Line::styled(
            format!("[gRPC: {} bytes, invalid frame]", body.len()),
            Style::default().fg(Color::DarkGray),
        )];
        lines.extend(hex_view::hex_lines(body, 64));
        return lines;
    }

    let mut lines: Vec<Line<'a>> = Vec::new();

    for (i, (compressed, payload)) in frames.iter().enumerate() {
        if lines.len() >= max_lines {
            lines.push(Line::styled(
                "... truncated",
                Style::default().fg(Color::DarkGray),
            ));
            break;
        }

        let label = if frames.len() == 1 {
            format!(
                "── gRPC message ({} bytes{}) ──",
                payload.len(),
                if *compressed { ", compressed" } else { "" }
            )
        } else {
            format!(
                "── gRPC message {} ({} bytes{}) ──",
                i + 1,
                payload.len(),
                if *compressed { ", compressed" } else { "" }
            )
        };
        lines.push(Line::styled(label, Style::default().fg(Color::DarkGray)));

        if *compressed {
            lines.push(Line::styled(
                "  [compressed payload, cannot decode]",
                Style::default().fg(Color::Yellow),
            ));
            continue;
        }

        render_proto_payload(payload, proto_type, max_lines, &mut lines);
    }

    lines
}

/// Render a single bare protobuf payload, preferring schema decode and falling
/// back to the heuristic decoder.
fn render_proto_payload<'a>(
    payload: &[u8],
    proto_type: Option<&MessageDescriptor>,
    max_lines: usize,
    lines: &mut Vec<Line<'a>>,
) {
    if let Some(desc) = proto_type
        && let Some(text) = proto_schema::decode_message_text(desc, payload, 0)
    {
        for line in text.into_iter().take(max_lines.saturating_sub(lines.len())) {
            lines.push(render_named_line(&line));
        }
        return;
    }

    match protobuf::decode_raw(payload) {
        Some(fields) => render_proto_fields(&fields, 1, max_lines, lines),
        None => lines.push(Line::styled(
            "  [could not decode protobuf]",
            Style::default().fg(Color::Yellow),
        )),
    }
}

fn render_protobuf<'a>(
    body: &[u8],
    max_lines: usize,
    proto_type: Option<&MessageDescriptor>,
) -> Vec<Line<'a>> {
    if let Some(desc) = proto_type
        && let Some(text) = proto_schema::decode_message_text(desc, body, 0)
    {
        let mut lines = vec![Line::styled(
            format!("[protobuf: {} bytes]", body.len()),
            Style::default().fg(Color::DarkGray),
        )];
        for line in text.into_iter().take(max_lines) {
            lines.push(render_named_line(&line));
        }
        return lines;
    }

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

/// Style a single line of schema-decoded named text, e.g.
/// `"  2 name str: alice"` or `"5 inner msg:"` or `"\"k\" => int: 1"`.
fn render_named_line<'a>(line: &str) -> Line<'a> {
    let indent_len = line.len() - line.trim_start().len();
    let indent = &line[..indent_len];
    let content = &line[indent_len..];

    let mut spans = vec![Span::raw(indent.to_string())];

    if let Some((head, value)) = content.split_once(": ") {
        spans.extend(style_head(head));
        spans.push(Span::styled(": ", Style::default().fg(Color::DarkGray)));
        let tag = head.split_whitespace().last().unwrap_or("");
        spans.push(Span::styled(value.to_string(), proto_value_style(tag)));
    } else if let Some(head) = content.strip_suffix(':') {
        // Header line for a nested message or map: no inline value.
        spans.extend(style_head(head));
        spans.push(Span::styled(":", Style::default().fg(Color::DarkGray)));
    } else {
        spans.push(Span::raw(content.to_string()));
    }

    Line::from(spans)
}

fn style_head<'a>(head: &str) -> Vec<Span<'a>> {
    let toks: Vec<&str> = head.split_whitespace().collect();
    let last = toks.len().saturating_sub(1);
    let mut spans = Vec::with_capacity(toks.len() * 2);
    for (i, tok) in toks.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        let style = if i == 0 {
            Style::default().fg(Color::Cyan) // field number or map key
        } else if *tok == "=>" || i == last {
            Style::default().fg(Color::DarkGray) // map arrow or type tag
        } else {
            Style::default().fg(Color::Yellow) // field name
        };
        spans.push(Span::styled(tok.to_string(), style));
    }
    spans
}

fn proto_value_style(tag: &str) -> Style {
    match tag {
        "str" => Style::default().fg(Color::Green),
        "hex" => Style::default().fg(Color::Yellow),
        "enum" | "bool" => Style::default().fg(Color::Blue),
        _ => Style::default().fg(Color::Magenta),
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
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let end = s.char_indices().nth(max_len).map(|(i, _)| i).unwrap_or(s.len());
        format!("{}...", &s[..end])
    }
}

fn simple_url_decode(input: &str) -> String {
    crate::http::url_decode(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Concatenate a styled line's span contents back into plain text.
    fn line_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn render_named_line_is_lossless() {
        // Styling must not drop or reorder any characters, across a scalar
        // field, a nested-message header, an indented child, and a map entry.
        for input in [
            "2 name str: alice",
            "5 inner msg:",
            "  1 verbose bool: true",
            "\"env\" => str: prod",
            "3 role enum: ROLE_ADMIN",
        ] {
            assert_eq!(line_text(&render_named_line(input)), input);
        }
    }

    #[test]
    fn render_named_line_colors_field_number() {
        // The field number (first non-empty span, after the indent span) is cyan.
        let line = render_named_line("2 name str: alice");
        let num = line.spans.iter().find(|s| !s.content.is_empty()).unwrap();
        assert_eq!(num.content.as_ref(), "2");
        assert_eq!(num.style.fg, Some(Color::Cyan));
    }
}
