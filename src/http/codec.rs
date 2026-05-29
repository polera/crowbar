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
            let proto_type = crate::http::proto_schema::request_type(&req.uri);
            let messages = protobuf::decode_grpc_body(&req.body);
            let payloads = grpc_payloads(&req.body);
            for (i, msg) in messages.iter().enumerate() {
                if i > 0 {
                    lines.push("---".to_string());
                }
                if msg.compressed {
                    lines.push(format!("[compressed: {} bytes]", msg.size));
                    continue;
                }
                // Prefer schema decode of the raw payload when a type resolves.
                let schema_lines = proto_type.as_ref().and_then(|desc| {
                    payloads
                        .get(i)
                        .and_then(|p| crate::http::proto_schema::decode_message_text(desc, p, 0))
                });
                if let Some(named) = schema_lines {
                    lines.extend(named);
                } else if let Some(fields) = &msg.fields {
                    lines.extend(protobuf::format_proto_text(fields, 0));
                } else {
                    lines.push(format!("[binary: {} bytes]", msg.size));
                }
            }
        } else if let Ok(text) = std::str::from_utf8(&req.body) {
            lines.extend(text.lines().map(String::from));
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
        encode_grpc_body_lines(&body_lines, &uri).unwrap_or_else(|| Bytes::from(body_lines.join("\n")))
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

/// Split a gRPC body into per-frame payload slices (skipping the 5-byte header).
fn grpc_payloads(body: &[u8]) -> Vec<&[u8]> {
    let mut payloads = Vec::new();
    let mut pos = 0;
    while pos + 5 <= body.len() {
        let len = u32::from_be_bytes([body[pos + 1], body[pos + 2], body[pos + 3], body[pos + 4]])
            as usize;
        if pos + 5 + len > body.len() {
            break;
        }
        payloads.push(&body[pos + 5..pos + 5 + len]);
        pos += 5 + len;
    }
    payloads
}

fn encode_grpc_body_lines(body_lines: &[String], uri: &str) -> Option<Bytes> {
    let proto_type = crate::http::proto_schema::request_type(uri);

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
        // Prefer schema encode (named text) when a type resolves; fall back to
        // the heuristic wire encoder otherwise.
        let payload = match proto_type
            .as_ref()
            .and_then(|desc| crate::http::proto_schema::encode_message_text(desc, msg_lines))
        {
            Some(bytes) => bytes,
            None => {
                let fields = protobuf::parse_proto_text(msg_lines)?;
                protobuf::encode_raw(&fields)
            }
        };
        output.extend(protobuf::encode_grpc_frame(&payload));
    }
    Some(Bytes::from(output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::models::{HttpVersion, RequestId};
    use crate::http::proto_schema;
    use std::path::PathBuf;
    use std::time::SystemTime;

    fn grpc_request(uri: &str, body: Bytes) -> RequestData {
        RequestData {
            id: RequestId(1),
            method: "POST".into(),
            uri: uri.into(),
            host: "example.com".into(),
            version: HttpVersion::Http2,
            headers: vec![("content-type".into(), "application/grpc".into())],
            body,
            is_tls: true,
            is_grpc: true,
            timestamp: SystemTime::now(),
        }
    }

    #[test]
    fn grpc_request_schema_round_trips_via_codec() {
        let dir: PathBuf = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proto").into();
        // init() is idempotent; other tests may have set it already.
        let _ = proto_schema::init(&[dir], &[]);

        let uri = "https://example.com/sample.UserService/GetUser";
        let desc = proto_schema::request_type(uri).expect("schema resolves GetUser request");
        let payload =
            proto_schema::encode_message_text(&desc, &["1 user_id int: 7"]).expect("encode");
        let req = grpc_request(uri, Bytes::from(protobuf::encode_grpc_frame(&payload)));

        // Decode shows the named field, not just a number.
        let lines = request_to_lines(&req);
        assert!(
            lines.iter().any(|l| l.contains("user_id int: 7")),
            "expected named field, got: {lines:?}"
        );

        // Re-encoding the (unedited) text reproduces the exact wire bytes.
        let rebuilt = lines_to_request(&lines, &req);
        assert_eq!(rebuilt.body, req.body);
    }

    #[test]
    fn grpc_multi_frame_round_trips_via_codec() {
        let dir: PathBuf = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proto").into();
        let _ = proto_schema::init(&[dir], &[]);

        let uri = "https://example.com/sample.UserService/GetUser";
        let desc = proto_schema::request_type(uri).expect("schema resolves");
        let frame = |n: &str| {
            protobuf::encode_grpc_frame(
                &proto_schema::encode_message_text(&desc, &[n]).unwrap(),
            )
        };
        let mut body = frame("1 user_id int: 1");
        body.extend_from_slice(&frame("1 user_id int: 2"));
        let req = grpc_request(uri, Bytes::from(body));

        // Both frames decode, separated by the `---` marker.
        let lines = request_to_lines(&req);
        assert!(lines.iter().any(|l| l == "---"), "expected separator: {lines:?}");
        assert!(lines.iter().any(|l| l.contains("user_id int: 1")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("user_id int: 2")), "{lines:?}");

        // And the two-frame body round-trips byte-for-byte.
        let rebuilt = lines_to_request(&lines, &req);
        assert_eq!(rebuilt.body, req.body);
    }
}
