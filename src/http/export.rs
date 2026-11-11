use std::fmt::Write;

use base64::Engine;
use serde_json::json;

use super::models::HistoryEntry;

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn to_curl(entry: &HistoryEntry) -> String {
    if entry.request_body_truncated {
        return "# Cannot replay a truncated request capture\n".into();
    }
    let req = &entry.request;
    let mut parts = Vec::new();

    let scheme = if req.is_tls { "https" } else { "http" };
    let url = if req.uri.starts_with("http") {
        req.uri.clone()
    } else {
        format!("{}://{}{}", scheme, req.host, req.uri)
    };

    parts.push(format!(
        "curl -X {} --url {}",
        shell_quote(&req.method),
        shell_quote(&url)
    ));

    for (key, value) in &req.headers {
        if key.eq_ignore_ascii_case("host") || key.eq_ignore_ascii_case("content-length") {
            continue;
        }
        parts.push(format!("  -H {}", shell_quote(&format!("{key}: {value}"))));
    }

    if !req.body.is_empty() {
        match std::str::from_utf8(&req.body) {
            Ok(text) if !text.contains('\0') && !text.starts_with('@') => {
                parts.push(format!("  --data-binary {}", shell_quote(text)));
            }
            _ => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&req.body);
                parts.push("  --data-binary @-".into());
                return format!(
                    "printf '%s' {} | base64 -d | {}",
                    shell_quote(&b64),
                    parts.join(" \\\n")
                );
            }
        }
    }

    parts.join(" \\\n")
}

pub fn to_raw(entry: &HistoryEntry) -> String {
    let mut output = String::new();
    let req = &entry.request;

    let path = extract_path(&req.uri);
    let _ = write!(output, "{} {} {}\r\n", req.method, path, req.version);

    for (key, value) in &req.headers {
        let _ = write!(output, "{}: {}\r\n", key, value);
    }
    output.push_str("\r\n");

    if !req.body.is_empty() {
        if let Ok(text) = std::str::from_utf8(&req.body) {
            output.push_str(text);
        } else {
            let _ = write!(output, "[binary: {} bytes]", req.body.len());
        }
    }

    if let Some(resp) = &entry.response {
        output.push_str("\r\n---\r\n\r\n");
        let _ = write!(
            output,
            "{} {} {}\r\n",
            resp.version, resp.status, resp.reason
        );

        for (key, value) in &resp.headers {
            let _ = write!(output, "{}: {}\r\n", key, value);
        }
        output.push_str("\r\n");

        if !resp.body.is_empty() {
            if let Ok(text) = std::str::from_utf8(&resp.body) {
                output.push_str(text);
            } else {
                let _ = write!(output, "[binary: {} bytes]", resp.body.len());
            }
        }
    }

    output
}

pub fn to_har(entries: &[HistoryEntry]) -> String {
    let har_entries: Vec<serde_json::Value> = entries
        .iter()
        .filter(|e| e.response.is_some())
        .map(har_entry)
        .collect();

    let har = json!({
        "log": {
            "version": "1.2",
            "creator": {
                "name": "crowbar",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "entries": har_entries,
        }
    });

    serde_json::to_string_pretty(&har).unwrap_or_default()
}

fn har_entry(entry: &HistoryEntry) -> serde_json::Value {
    let req = &entry.request;
    let resp = entry.response.as_ref().unwrap();

    let scheme = if req.is_tls { "https" } else { "http" };
    let url = if req.uri.starts_with("http") {
        req.uri.clone()
    } else {
        format!("{}://{}{}", scheme, req.host, req.uri)
    };

    let started = format_iso_timestamp(req.timestamp);

    let req_headers: Vec<serde_json::Value> = req
        .headers
        .iter()
        .map(|(k, v)| json!({"name": k, "value": v}))
        .collect();

    let resp_headers: Vec<serde_json::Value> = resp
        .headers
        .iter()
        .map(|(k, v)| json!({"name": k, "value": v}))
        .collect();

    let req_body_text = std::str::from_utf8(&req.body).ok().map(|s| s.to_string());
    let resp_body_text = std::str::from_utf8(&resp.body).ok().map(|s| s.to_string());

    let req_content_type = req
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.as_str())
        .unwrap_or("");

    let resp_content_type = resp
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.as_str())
        .unwrap_or("");

    let mut req_json = json!({
        "method": req.method,
        "url": url,
        "httpVersion": req.version.to_string(),
        "headers": req_headers,
        "queryString": [],
        "headersSize": -1,
        "bodySize": req.body.len() as i64,
    });

    if !req.body.is_empty() {
        req_json["postData"] = json!({
            "mimeType": req_content_type,
            "text": req_body_text.unwrap_or_default(),
        });
    }

    let mut resp_content = json!({
        "size": resp.body.len() as i64,
        "mimeType": resp_content_type,
    });
    if let Some(text) = resp_body_text {
        resp_content["text"] = json!(text);
    } else {
        resp_content["text"] = json!(base64::engine::general_purpose::STANDARD.encode(&resp.body));
        resp_content["encoding"] = json!("base64");
    }

    json!({
        "startedDateTime": started,
        "time": resp.duration.as_millis() as f64,
        "request": req_json,
        "response": {
            "status": resp.status,
            "statusText": resp.reason,
            "httpVersion": resp.version.to_string(),
            "headers": resp_headers,
            "content": resp_content,
            "headersSize": -1,
            "bodySize": resp.body.len() as i64,
        },
        "cache": {},
        "timings": har_timings(resp),
    })
}

fn format_iso_timestamp(timestamp: std::time::SystemTime) -> String {
    let nanos = match timestamp.duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => i128::try_from(duration.as_nanos()).ok(),
        Err(error) => i128::try_from(error.duration().as_nanos())
            .ok()
            .map(|nanos| -nanos),
    };
    nanos
        .and_then(|nanos| time::OffsetDateTime::from_unix_timestamp_nanos(nanos).ok())
        .and_then(|date| {
            date.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".into())
}

fn har_timings(resp: &super::models::ResponseData) -> serde_json::Value {
    match &resp.timing {
        Some(t) => {
            let dur_ms = |d: &Option<std::time::Duration>| -> f64 {
                d.map(|d| d.as_secs_f64() * 1000.0).unwrap_or(-1.0)
            };
            json!({
                "blocked": -1,
                "dns": -1,
                "connect": dur_ms(&t.tcp_connect),
                "send": 0,
                "wait": dur_ms(&t.time_to_first_byte),
                "receive": dur_ms(&t.content_transfer),
                "ssl": dur_ms(&t.tls_handshake),
            })
        }
        None => {
            json!({
                "send": 0,
                "wait": resp.duration.as_millis() as f64,
                "receive": 0,
            })
        }
    }
}

fn extract_path(uri: &str) -> &str {
    super::extract_path(uri)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::models::*;
    use bytes::Bytes;
    use std::time::{Duration, UNIX_EPOCH};

    #[cfg(unix)]
    #[test]
    fn exported_shell_arguments_cannot_execute_syntax() {
        let mut request = make_request(
            "GET';printf injected;#",
            "http://example.com/?q=';printf injected;#",
            "example.com",
            false,
        );
        request.headers = vec![(
            "X-';printf injected;#".into(),
            "$(printf injected)\n'".into(),
        )];
        request.body = Bytes::from_static(b"body'\n$(printf injected)");
        let entry = make_entry(request.clone(), None);
        // Replace curl with an argument dumper, never invoking a network client.
        let script = format!("curl() {{ printf '%s\\0' \"$@\"; }}\n{}", to_curl(&entry));
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .output()
            .unwrap();
        assert!(output.status.success());
        let args: Vec<_> = output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|arg| !arg.is_empty())
            .collect();
        assert_eq!(args[1], request.method.as_bytes());
        assert_eq!(args[3], request.uri.as_bytes());
        assert_eq!(
            args[5],
            format!("{}: {}", request.headers[0].0, request.headers[0].1).as_bytes()
        );
        assert_eq!(args[7], request.body.as_ref());
    }

    fn make_request(method: &str, uri: &str, host: &str, is_tls: bool) -> RequestData {
        RequestData {
            id: RequestId(1),
            method: method.into(),
            uri: uri.into(),
            host: host.into(),
            version: HttpVersion::Http11,
            headers: vec![],
            body: Bytes::new(),
            is_tls,
            is_grpc: false,
            timestamp: UNIX_EPOCH + Duration::from_secs(1704067200), // 2024-01-01T00:00:00Z
        }
    }

    fn make_response(status: u16) -> ResponseData {
        ResponseData {
            status,
            reason: status_reason(status).into(),
            version: HttpVersion::Http11,
            headers: vec![],
            body: Bytes::new(),
            trailers: vec![],
            duration: Duration::from_millis(150),
            timing: None,
        }
    }

    fn make_entry(req: RequestData, resp: Option<ResponseData>) -> HistoryEntry {
        HistoryEntry {
            request: req,
            response: resp,
            state: EntryState::Complete,
            error_message: None,
            ws_messages: vec![],
            grpc_messages: vec![],
            findings: vec![],
            request_body_truncated: false,
        }
    }

    #[test]
    fn to_curl_basic_get() {
        let req = make_request("GET", "/path", "example.com", true);
        let entry = make_entry(req, None);
        let curl = to_curl(&entry);
        assert!(curl.starts_with("curl -X 'GET' --url 'https://example.com/path'"));
    }

    #[test]
    fn to_curl_preserves_full_url() {
        let req = make_request("GET", "http://example.com/full", "example.com", false);
        let entry = make_entry(req, None);
        let curl = to_curl(&entry);
        assert!(curl.contains("'http://example.com/full'"));
    }

    #[test]
    fn to_curl_skips_host_and_content_length() {
        let mut req = make_request("GET", "/", "example.com", false);
        req.headers = vec![
            ("Host".into(), "example.com".into()),
            ("Content-Length".into(), "0".into()),
            ("Accept".into(), "text/html".into()),
        ];
        let entry = make_entry(req, None);
        let curl = to_curl(&entry);
        assert!(!curl.contains("-H 'Host:"));
        assert!(!curl.contains("-H 'Content-Length:"));
        assert!(curl.contains("-H 'Accept: text/html'"));
    }

    #[test]
    fn to_curl_text_body() {
        let mut req = make_request("POST", "/api", "example.com", false);
        req.body = Bytes::from("{\"key\":\"value\"}");
        let entry = make_entry(req, None);
        let curl = to_curl(&entry);
        assert!(curl.contains("--data-binary '{\"key\":\"value\"}'"));
    }

    #[test]
    fn to_curl_binary_body_base64() {
        let mut req = make_request("POST", "/upload", "example.com", false);
        req.body = Bytes::from(vec![0xFF, 0x00, 0xAB]);
        let entry = make_entry(req, None);
        let curl = to_curl(&entry);
        assert!(curl.contains("--data-binary"));
        assert!(curl.contains("base64"));
    }

    #[test]
    fn to_curl_escapes_single_quotes() {
        let mut req = make_request("POST", "/api", "example.com", false);
        req.headers = vec![("X-Test".into(), "it's a test".into())];
        let entry = make_entry(req, None);
        let curl = to_curl(&entry);
        assert!(curl.contains("it'\\''s a test"));
    }

    #[test]
    fn to_raw_request_only() {
        let req = make_request("GET", "/path", "example.com", false);
        let entry = make_entry(req, None);
        let raw = to_raw(&entry);
        assert!(raw.starts_with("GET /path HTTP/1.1\r\n"));
        assert!(!raw.contains("---"));
    }

    #[test]
    fn to_raw_with_response() {
        let req = make_request("GET", "/", "example.com", false);
        let resp = make_response(200);
        let entry = make_entry(req, Some(resp));
        let raw = to_raw(&entry);
        assert!(raw.contains("---"));
        assert!(raw.contains("HTTP/1.1 200 OK"));
    }

    #[test]
    fn to_raw_with_headers_and_body() {
        let mut req = make_request("POST", "/api", "example.com", false);
        req.headers = vec![("content-type".into(), "application/json".into())];
        req.body = Bytes::from("{\"a\":1}");
        let entry = make_entry(req, None);
        let raw = to_raw(&entry);
        assert!(raw.contains("content-type: application/json\r\n"));
        assert!(raw.contains("{\"a\":1}"));
    }

    #[test]
    fn to_raw_binary_body() {
        let mut req = make_request("POST", "/", "example.com", false);
        req.body = Bytes::from(vec![0xFF, 0x00]);
        let entry = make_entry(req, None);
        let raw = to_raw(&entry);
        assert!(raw.contains("[binary: 2 bytes]"));
    }

    #[test]
    fn to_har_filters_no_response() {
        let req = make_request("GET", "/", "example.com", false);
        let entry = make_entry(req, None);
        let har = to_har(&[entry]);
        let parsed: serde_json::Value = serde_json::from_str(&har).unwrap();
        assert_eq!(parsed["log"]["entries"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn to_har_structure() {
        let req = make_request("GET", "/api", "example.com", true);
        let resp = make_response(200);
        let entry = make_entry(req, Some(resp));
        let har = to_har(&[entry]);
        let parsed: serde_json::Value = serde_json::from_str(&har).unwrap();
        assert_eq!(parsed["log"]["version"], "1.2");
        assert_eq!(parsed["log"]["creator"]["name"], "crowbar");

        let har_entry = &parsed["log"]["entries"][0];
        assert_eq!(har_entry["request"]["method"], "GET");
        assert_eq!(har_entry["request"]["url"], "https://example.com/api");
        assert_eq!(har_entry["response"]["status"], 200);
    }

    #[test]
    fn to_har_timestamp_format() {
        let req = make_request("GET", "/", "example.com", false);
        let resp = make_response(200);
        let entry = make_entry(req, Some(resp));
        let har = to_har(&[entry]);
        let parsed: serde_json::Value = serde_json::from_str(&har).unwrap();
        let started = parsed["log"]["entries"][0]["startedDateTime"]
            .as_str()
            .unwrap();
        assert!(started.ends_with('Z'));
        assert!(started.contains('T'));
        assert_eq!(started.len(), 20);
    }

    #[test]
    fn to_har_timings_without_timing_data() {
        let req = make_request("GET", "/", "example.com", false);
        let resp = make_response(200);
        let entry = make_entry(req, Some(resp));
        let har = to_har(&[entry]);
        let parsed: serde_json::Value = serde_json::from_str(&har).unwrap();
        let timings = &parsed["log"]["entries"][0]["timings"];
        assert_eq!(timings["send"], 0);
        assert_eq!(timings["receive"], 0);
    }

    #[test]
    fn to_har_timings_with_timing_data() {
        let req = make_request("GET", "/", "example.com", false);
        let mut resp = make_response(200);
        resp.timing = Some(TimingData {
            tcp_connect: Some(Duration::from_millis(10)),
            tls_handshake: Some(Duration::from_millis(20)),
            http_handshake: None,
            time_to_first_byte: Some(Duration::from_millis(50)),
            content_transfer: Some(Duration::from_millis(30)),
        });
        let entry = make_entry(req, Some(resp));
        let har = to_har(&[entry]);
        let parsed: serde_json::Value = serde_json::from_str(&har).unwrap();
        let timings = &parsed["log"]["entries"][0]["timings"];
        assert!(timings["connect"].as_f64().unwrap() > 0.0);
        assert!(timings["ssl"].as_f64().unwrap() > 0.0);
        assert!(timings["wait"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn iso_timestamp_epoch() {
        assert_eq!(format_iso_timestamp(UNIX_EPOCH), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn iso_timestamp_known_date() {
        // 2024-01-01T00:00:00Z = 1704067200
        assert_eq!(
            format_iso_timestamp(UNIX_EPOCH + Duration::from_secs(1704067200)),
            "2024-01-01T00:00:00Z"
        );
    }

    #[test]
    fn iso_timestamp_with_time() {
        // 2024-01-01T12:30:45Z = 1704067200 + 12*3600 + 30*60 + 45 = 1704112245
        assert_eq!(
            format_iso_timestamp(UNIX_EPOCH + Duration::from_secs(1704112245)),
            "2024-01-01T12:30:45Z"
        );
    }

    #[test]
    fn iso_timestamp_preserves_subseconds_and_dates_before_epoch() {
        assert_eq!(
            format_iso_timestamp(UNIX_EPOCH + Duration::from_millis(125)),
            "1970-01-01T00:00:00.125Z"
        );
        assert_eq!(
            format_iso_timestamp(UNIX_EPOCH - Duration::from_secs(1)),
            "1969-12-31T23:59:59Z"
        );
    }
}
