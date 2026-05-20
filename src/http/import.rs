use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde::Deserialize;

use super::models::{EntryState, HistoryEntry, HttpVersion, RequestData, RequestId, ResponseData};

pub fn load_file(path: &Path) -> anyhow::Result<Vec<HistoryEntry>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "har" => load_har(path),
        "json" => {
            match super::session::load(path) {
                Ok(entries) => Ok(entries),
                Err(_) => load_har(path),
            }
        }
        _ => super::session::load(path),
    }
}

fn load_har(path: &Path) -> anyhow::Result<Vec<HistoryEntry>> {
    let content = std::fs::read_to_string(path)?;
    let har: HarFile = serde_json::from_str(&content)?;

    let entries = har
        .log
        .entries
        .into_iter()
        .map(convert_har_entry)
        .collect();

    Ok(entries)
}

fn convert_har_entry(entry: HarEntry) -> HistoryEntry {
    let req = &entry.request;

    let host = extract_host(&req.url);
    let is_tls = req.url.starts_with("https");

    let version = parse_http_version(&req.http_version);

    let headers: Vec<(String, String)> = req
        .headers
        .iter()
        .map(|h| (h.name.clone(), h.value.clone()))
        .collect();

    let body = req
        .post_data
        .as_ref()
        .map(|pd| Bytes::from(pd.text.clone().unwrap_or_default()))
        .unwrap_or_default();

    let timestamp = parse_iso_timestamp(&entry.started_date_time)
        .unwrap_or(SystemTime::now());

    let request_data = RequestData {
        id: RequestId::next(),
        method: req.method.clone(),
        uri: req.url.clone(),
        host,
        version,
        headers,
        body,
        is_tls,
        is_grpc: false,
        timestamp,
    };

    let resp = &entry.response;
    let resp_version = parse_http_version(&resp.http_version);
    let resp_headers: Vec<(String, String)> = resp
        .headers
        .iter()
        .map(|h| (h.name.clone(), h.value.clone()))
        .collect();

    let resp_body = resp
        .content
        .text
        .as_ref()
        .map(|t| Bytes::from(t.clone()))
        .unwrap_or_default();

    let duration = Duration::from_millis(entry.time.max(0.0) as u64);

    let response_data = ResponseData {
        status: resp.status,
        reason: resp.status_text.clone(),
        version: resp_version,
        headers: resp_headers,
        body: resp_body,
        trailers: Vec::new(),
        duration,
    };

    HistoryEntry {
        request: request_data,
        response: Some(response_data),
        state: EntryState::Complete,
        error_message: None,
        ws_messages: Vec::new(),
        grpc_messages: Vec::new(),
        findings: Vec::new(),
    }
}

fn extract_host(url: &str) -> String {
    if let Some(pos) = url.find("://") {
        let after_scheme = &url[pos + 3..];
        let end = after_scheme
            .find('/')
            .unwrap_or(after_scheme.len());
        let host_port = &after_scheme[..end];
        host_port.split(':').next().unwrap_or(host_port).to_string()
    } else {
        url.split('/').next().unwrap_or("unknown").to_string()
    }
}

fn parse_http_version(v: &str) -> HttpVersion {
    match v {
        "HTTP/1.0" | "http/1.0" => HttpVersion::Http10,
        "HTTP/2" | "HTTP/2.0" | "h2" => HttpVersion::Http2,
        _ => HttpVersion::Http11,
    }
}

fn parse_iso_timestamp(s: &str) -> Option<SystemTime> {
    let s = s.trim();
    let date_part = s.get(..10)?;
    let time_part = s.get(11..19)?;

    let mut date_iter = date_part.split('-');
    let year: u64 = date_iter.next()?.parse().ok()?;
    let month: u64 = date_iter.next()?.parse().ok()?;
    let day: u64 = date_iter.next()?.parse().ok()?;

    let mut time_iter = time_part.split(':');
    let hour: u64 = time_iter.next()?.parse().ok()?;
    let min: u64 = time_iter.next()?.parse().ok()?;
    let sec: u64 = time_iter.next()?.parse().ok()?;

    let mut days: u64 = 0;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }

    let months = if is_leap(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    for m in months.iter().take((month as usize).saturating_sub(1)) {
        days += m;
    }
    days += day.saturating_sub(1);

    let total_secs = days * 86400 + hour * 3600 + min * 60 + sec;
    Some(UNIX_EPOCH + Duration::from_secs(total_secs))
}

fn is_leap(y: u64) -> bool {
    super::is_leap(y)
}

#[derive(Deserialize)]
struct HarFile {
    log: HarLog,
}

#[derive(Deserialize)]
struct HarLog {
    entries: Vec<HarEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HarEntry {
    started_date_time: String,
    time: f64,
    request: HarRequest,
    response: HarResponse,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HarRequest {
    method: String,
    url: String,
    http_version: String,
    headers: Vec<HarHeader>,
    post_data: Option<HarPostData>,
}

#[derive(Deserialize)]
struct HarPostData {
    text: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HarResponse {
    status: u16,
    status_text: String,
    http_version: String,
    headers: Vec<HarHeader>,
    content: HarContent,
}

#[derive(Deserialize)]
struct HarHeader {
    name: String,
    value: String,
}

#[derive(Deserialize)]
struct HarContent {
    text: Option<String>,
}
