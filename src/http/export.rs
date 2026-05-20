use std::fmt::Write;

use base64::Engine;
use serde_json::json;

use super::models::HistoryEntry;

pub fn to_curl(entry: &HistoryEntry) -> String {
    let req = &entry.request;
    let mut parts = Vec::new();

    let scheme = if req.is_tls { "https" } else { "http" };
    let url = if req.uri.starts_with("http") {
        req.uri.clone()
    } else {
        format!("{}://{}{}", scheme, req.host, req.uri)
    };

    parts.push(format!("curl -X {} '{}'", req.method, url));

    for (key, value) in &req.headers {
        if key.eq_ignore_ascii_case("host") || key.eq_ignore_ascii_case("content-length") {
            continue;
        }
        parts.push(format!("  -H '{}: {}'", key, value.replace('\'', "'\\''")));
    }

    if !req.body.is_empty() {
        match std::str::from_utf8(&req.body) {
            Ok(text) => {
                parts.push(format!("  -d '{}'", text.replace('\'', "'\\''")));
            }
            Err(_) => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&req.body);
                parts.push(format!("  --data-binary @<(echo '{}' | base64 -d)", b64));
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
        let _ = write!(output, "{} {} {}\r\n", resp.version, resp.status, resp.reason);

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

    let timestamp = req
        .timestamp
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let started = chrono_lite_iso(timestamp.as_secs()).to_string();

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
        "timings": {
            "send": 0,
            "wait": resp.duration.as_millis() as f64,
            "receive": 0,
        },
    })
}

fn chrono_lite_iso(epoch_secs: u64) -> String {
    let secs_per_day: u64 = 86400;
    let days = epoch_secs / secs_per_day;
    let time_of_day = epoch_secs % secs_per_day;

    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    let (year, month, day) = days_to_date(days);

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds
    )
}

fn days_to_date(days: u64) -> (u64, u64, u64) {
    let mut y = 1970;
    let mut remaining = days;

    loop {
        let days_in_year = if is_leap(y) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        y += 1;
    }

    let months = if is_leap(y) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut m = 0;
    for days_in_month in months {
        if remaining < days_in_month {
            break;
        }
        remaining -= days_in_month;
        m += 1;
    }

    (y, m + 1, remaining + 1)
}

fn is_leap(y: u64) -> bool {
    super::is_leap(y)
}

fn extract_path(uri: &str) -> &str {
    super::extract_path(uri)
}
