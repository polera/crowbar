use bytes::Bytes;

use super::models::RequestData;
use super::protobuf;

pub fn request_to_lines(req: &RequestData) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!("{} {} {}", req.method, req.uri, req.version));

    for (key, value) in &req.headers {
        lines.push(format!("{}: {}", key, value));
    }

    if !req.body.is_empty() {
        lines.push(String::new());

        if req.is_grpc {
            let messages = protobuf::decode_grpc_body(&req.body);
            if messages.len() == 1 {
                if let Some(fields) = &messages[0].fields {
                    for line in protobuf::format_proto_text(fields, 0) {
                        lines.push(line);
                    }
                } else {
                    lines.push(format!("[binary: {} bytes]", req.body.len()));
                }
            } else {
                for (i, msg) in messages.iter().enumerate() {
                    if i > 0 {
                        lines.push("---".to_string());
                    }
                    if let Some(fields) = &msg.fields {
                        for line in protobuf::format_proto_text(fields, 0) {
                            lines.push(line);
                        }
                    } else {
                        let payload_len = msg.size;
                        lines.push(format!("[binary: {} bytes]", payload_len));
                    }
                }
            }
        } else if let Ok(text) = std::str::from_utf8(&req.body) {
            for line in text.lines() {
                lines.push(line.to_string());
            }
        } else {
            lines.push(format!("[binary: {} bytes]", req.body.len()));
        }
    }

    lines
}

pub fn lines_to_request(lines: &[String], original: &RequestData) -> RequestData {
    let mut headers = Vec::new();
    let mut body_lines = Vec::new();
    let mut in_body = false;
    let mut method = original.method.clone();
    let mut uri = original.uri.clone();

    for (i, line) in lines.iter().enumerate() {
        if i == 0 {
            let parts: Vec<&str> = line.splitn(3, ' ').collect();
            if parts.len() >= 2 {
                method = parts[0].to_string();
                uri = parts[1].to_string();
            }
            continue;
        }

        if in_body {
            body_lines.push(line.clone());
            continue;
        }

        if line.is_empty() {
            in_body = true;
            continue;
        }

        if let Some((key, value)) = line.split_once(':') {
            headers.push((key.trim().to_string(), value.trim().to_string()));
        }
    }

    let body = if body_lines.is_empty() {
        Bytes::new()
    } else if original.is_grpc {
        encode_grpc_body_lines(&body_lines).unwrap_or_else(|| Bytes::from(body_lines.join("\n")))
    } else {
        Bytes::from(body_lines.join("\n"))
    };

    let host = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("host"))
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| original.host.clone());

    RequestData {
        id: original.id,
        method,
        uri,
        host,
        version: original.version,
        headers,
        body,
        is_tls: original.is_tls,
        is_grpc: original.is_grpc,
        timestamp: original.timestamp,
    }
}

fn encode_grpc_body_lines(body_lines: &[String]) -> Option<Bytes> {
    let mut messages: Vec<Vec<&str>> = Vec::new();
    let mut current: Vec<&str> = Vec::new();

    for line in body_lines {
        if line.trim() == "---" {
            if !current.is_empty() {
                messages.push(current);
                current = Vec::new();
            }
        } else {
            current.push(line.as_str());
        }
    }
    if !current.is_empty() {
        messages.push(current);
    }

    let mut output = Vec::new();
    for msg_lines in &messages {
        let fields = protobuf::parse_proto_text(msg_lines)?;
        let payload = protobuf::encode_raw(&fields);
        output.extend(protobuf::encode_grpc_frame(&payload));
    }
    Some(Bytes::from(output))
}
