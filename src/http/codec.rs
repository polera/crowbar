use bytes::Bytes;

use super::models::RequestData;

pub fn request_to_lines(req: &RequestData) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!("{} {} {}", req.method, req.uri, req.version));

    for (key, value) in &req.headers {
        lines.push(format!("{}: {}", key, value));
    }

    if !req.body.is_empty() {
        lines.push(String::new());
        if let Ok(text) = std::str::from_utf8(&req.body) {
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
        timestamp: original.timestamp,
    }
}
