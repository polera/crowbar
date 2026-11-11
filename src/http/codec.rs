use base64::Engine;
use bytes::Bytes;

use super::models::RequestData;
use super::protobuf;

const BASE64_MARKER: &str = "[crowbar:base64]";

#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("Expected METHOD URI HTTP/VERSION on the first line")]
    StartLine,
    #[error("Invalid header line: {0}")]
    Header(String),
    #[error("Invalid request: {0}")]
    Request(#[from] http::Error),
    #[error("Invalid edited body: {0}")]
    Body(&'static str),
}

pub fn request_to_lines(req: &RequestData) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!("{} {} {}", req.method, req.uri, req.version));

    for (key, value) in &req.headers {
        lines.push(format!("{}: {}", key, value));
    }

    if !req.body.is_empty() {
        lines.push(String::new());

        if let Some(messages) = req
            .is_grpc
            .then(|| protobuf::decode_grpc_body(&req.body))
            .filter(|messages| {
                !messages.is_empty()
                    && messages
                        .iter()
                        .all(|message| !message.compressed && message.fields.is_some())
            })
        {
            let proto_type = crate::http::proto_schema::request_type(&req.uri);
            let payloads = grpc_payloads(&req.body);
            for (i, msg) in messages.iter().enumerate() {
                if i > 0 {
                    lines.push("---".to_string());
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
        } else if !req.is_grpc
            && let Ok(text) = std::str::from_utf8(&req.body)
        {
            lines.extend(
                text.split('\n')
                    .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string()),
            );
        } else {
            lines.push(BASE64_MARKER.into());
            lines.push(base64::engine::general_purpose::STANDARD.encode(&req.body));
        }
    }

    lines
}

pub fn lines_to_request(
    lines: &[String],
    original: &RequestData,
) -> Result<RequestData, EditError> {
    let mut headers = Vec::new();
    let mut body_lines = Vec::new();
    let mut in_body = false;
    let mut method = original.method.clone();
    let mut uri = original.uri.clone();
    let mut version = original.version;

    for (i, line) in lines.iter().enumerate() {
        if i == 0 {
            let mut parts = line.split_whitespace();
            method = parts.next().ok_or(EditError::StartLine)?.to_string();
            uri = parts.next().ok_or(EditError::StartLine)?.to_string();
            version = match parts.next() {
                Some("HTTP/1.0") => super::models::HttpVersion::Http10,
                Some("HTTP/1.1") => super::models::HttpVersion::Http11,
                Some("HTTP/2" | "HTTP/2.0") => super::models::HttpVersion::Http2,
                _ => return Err(EditError::StartLine),
            };
            if parts.next().is_some() {
                return Err(EditError::StartLine);
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
            headers.push((
                key.trim().to_string(),
                value.strip_prefix(' ').unwrap_or(value).to_string(),
            ));
        } else {
            return Err(EditError::Header(line.clone()));
        }
    }

    if lines.is_empty() {
        return Err(EditError::StartLine);
    }
    let original_lines = request_to_lines(original);
    let original_body = original_lines
        .iter()
        .position(String::is_empty)
        .map(|index| &original_lines[index + 1..])
        .unwrap_or(&[]);
    let body = if body_lines == original_body {
        original.body.clone()
    } else if body_lines.is_empty() {
        Bytes::new()
    } else if body_lines[0] == BASE64_MARKER {
        Bytes::from(
            base64::engine::general_purpose::STANDARD
                .decode(body_lines[1..].join("").trim())
                .map_err(|_| EditError::Body("invalid base64"))?,
        )
    } else if original.is_grpc {
        encode_grpc_body_lines(&body_lines, &uri).ok_or(EditError::Body("invalid protobuf text"))?
    } else {
        let separator = if original.body.windows(2).any(|pair| pair == b"\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        Bytes::from(body_lines.join(separator))
    };

    let host = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("host"))
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| original.host.clone());

    // Validate at the editor boundary, rather than silently sending malformed edits.
    crate::proxy::build_forwarding_request(&method, &uri, &headers, body.clone())?;
    let is_tls = uri
        .parse::<http::Uri>()
        .ok()
        .and_then(|uri| {
            uri.scheme_str()
                .map(|scheme| scheme == "https" || scheme == "wss")
        })
        .unwrap_or(original.is_tls);
    Ok(RequestData {
        id: original.id,
        method,
        uri,
        host,
        version,
        headers,
        body,
        is_tls,
        is_grpc: original.is_grpc,
        timestamp: original.timestamp,
    })
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

    #[test]
    fn unedited_and_header_only_edits_preserve_body_bytes() {
        for body in [
            b"abc\r\n".as_slice(),
            b"one\r\ntwo\r\n",
            b"\xff\x00",
            b"",
            b"a\n\n",
            b"@file",
        ] {
            let mut request = crate::test_support::request();
            request.body = Bytes::copy_from_slice(body);
            let mut lines = request_to_lines(&request);
            assert_eq!(
                lines_to_request(&lines, &request).unwrap().body,
                request.body
            );
            lines[0] = lines[0].replace("POST", "PUT");
            assert_eq!(
                lines_to_request(&lines, &request).unwrap().body,
                request.body
            );
        }
    }

    #[test]
    fn binary_and_compressed_grpc_use_explicit_base64_edits() {
        let mut request = crate::test_support::request();
        request.is_grpc = true;
        request.body = Bytes::from_static(b"\x01\x00\x00\x00\x02hi");
        let mut lines = request_to_lines(&request);
        assert!(lines.iter().any(|line| line == BASE64_MARKER));
        assert_eq!(
            lines_to_request(&lines, &request).unwrap().body,
            request.body
        );
        *lines.last_mut().unwrap() = "not base64!".into();
        assert!(lines_to_request(&lines, &request).is_err());
    }

    #[test]
    fn malformed_edits_are_errors() {
        let request = crate::test_support::request();
        assert!(lines_to_request(&[], &request).is_err());
        assert!(lines_to_request(&["GET / HTTP/9".into()], &request).is_err());
        assert!(
            lines_to_request(&["GET / HTTP/1.1".into(), "broken header".into()], &request).is_err()
        );
    }

    proptest::proptest! {
        #[test]
        fn arbitrary_bodies_survive_an_unedited_round_trip(body in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..4096)) {
            let mut request = crate::test_support::request();
            request.body = Bytes::from(body);
            let rebuilt = lines_to_request(&request_to_lines(&request), &request).unwrap();
            proptest::prop_assert_eq!(rebuilt.body, request.body);
        }
    }

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
        let rebuilt = lines_to_request(&lines, &req).unwrap();
        assert_eq!(rebuilt.body, req.body);
    }

    #[test]
    fn grpc_multi_frame_round_trips_via_codec() {
        let dir: PathBuf = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proto").into();
        let _ = proto_schema::init(&[dir], &[]);

        let uri = "https://example.com/sample.UserService/GetUser";
        let desc = proto_schema::request_type(uri).expect("schema resolves");
        let frame = |n: &str| {
            protobuf::encode_grpc_frame(&proto_schema::encode_message_text(&desc, &[n]).unwrap())
        };
        let mut body = frame("1 user_id int: 1");
        body.extend_from_slice(&frame("1 user_id int: 2"));
        let req = grpc_request(uri, Bytes::from(body));

        // Both frames decode, separated by the `---` marker.
        let lines = request_to_lines(&req);
        assert!(
            lines.iter().any(|l| l == "---"),
            "expected separator: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("user_id int: 1")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("user_id int: 2")),
            "{lines:?}"
        );

        // And the two-frame body round-trips byte-for-byte.
        let rebuilt = lines_to_request(&lines, &req).unwrap();
        assert_eq!(rebuilt.body, req.body);
    }
}
