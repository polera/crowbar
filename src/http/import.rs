use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde::Deserialize;

use super::models::{EntryState, HistoryEntry, HttpVersion, RequestData, RequestId, ResponseData};

pub fn load_file(path: &Path) -> anyhow::Result<super::session::Session> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "har" => load_har(path).map(|entries| super::session::Session::new(entries, Vec::new())),
        "json" => match super::session::load(path) {
            Ok(session) => Ok(session),
            Err(session_err) => load_har(path)
                .map(|entries| super::session::Session::new(entries, Vec::new()))
                .map_err(|har_err| {
                    anyhow::anyhow!("Failed as session ({session_err}) and as HAR ({har_err})")
                }),
        },
        _ => super::session::load(path),
    }
}

fn load_har(path: &Path) -> anyhow::Result<Vec<HistoryEntry>> {
    let content = std::fs::read_to_string(path)?;
    let har: HarFile = serde_json::from_str(&content)?;

    let entries = har.log.entries.into_iter().map(convert_har_entry).collect();

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

    let timestamp = parse_iso_timestamp(&entry.started_date_time).unwrap_or(SystemTime::now());

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
        timing: None,
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
        let end = after_scheme.find('/').unwrap_or(after_scheme.len());
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

    let days = super::date_to_days(year, month, day);
    let total_secs = days * 86400 + hour * 3600 + min * 60 + sec;
    Some(UNIX_EPOCH + Duration::from_secs(total_secs))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_http_version_http10() {
        assert_eq!(parse_http_version("HTTP/1.0"), HttpVersion::Http10);
        assert_eq!(parse_http_version("http/1.0"), HttpVersion::Http10);
    }

    #[test]
    fn parse_http_version_http11() {
        assert_eq!(parse_http_version("HTTP/1.1"), HttpVersion::Http11);
        assert_eq!(parse_http_version("unknown"), HttpVersion::Http11);
    }

    #[test]
    fn parse_http_version_http2() {
        assert_eq!(parse_http_version("HTTP/2"), HttpVersion::Http2);
        assert_eq!(parse_http_version("HTTP/2.0"), HttpVersion::Http2);
        assert_eq!(parse_http_version("h2"), HttpVersion::Http2);
    }

    #[test]
    fn extract_host_https() {
        assert_eq!(extract_host("https://example.com/path"), "example.com");
    }

    #[test]
    fn extract_host_with_port() {
        assert_eq!(extract_host("https://example.com:8443/path"), "example.com");
    }

    #[test]
    fn extract_host_http() {
        assert_eq!(extract_host("http://api.example.com/v1"), "api.example.com");
    }

    #[test]
    fn extract_host_no_path() {
        assert_eq!(extract_host("https://example.com"), "example.com");
    }

    #[test]
    fn extract_host_no_scheme() {
        assert_eq!(extract_host("example.com/path"), "example.com");
    }

    #[test]
    fn parse_iso_timestamp_valid() {
        let ts = parse_iso_timestamp("2024-01-01T00:00:00Z").unwrap();
        let secs = ts.duration_since(UNIX_EPOCH).unwrap().as_secs();
        assert_eq!(secs, 1704067200);
    }

    #[test]
    fn parse_iso_timestamp_with_time() {
        let ts = parse_iso_timestamp("2024-06-15T14:30:00.000Z").unwrap();
        let secs = ts.duration_since(UNIX_EPOCH).unwrap().as_secs();
        let expected = super::super::date_to_days(2024, 6, 15) * 86400 + 14 * 3600 + 30 * 60;
        assert_eq!(secs, expected);
    }

    #[test]
    fn parse_iso_timestamp_too_short() {
        assert!(parse_iso_timestamp("2024").is_none());
    }

    #[test]
    fn parse_iso_timestamp_invalid() {
        assert!(parse_iso_timestamp("not-a-date-time").is_none());
    }

    #[test]
    fn convert_har_entry_basic() {
        let entry = HarEntry {
            started_date_time: "2024-01-01T12:00:00Z".into(),
            time: 150.0,
            request: HarRequest {
                method: "POST".into(),
                url: "https://api.example.com/v1/users".into(),
                http_version: "HTTP/2".into(),
                headers: vec![HarHeader {
                    name: "content-type".into(),
                    value: "application/json".into(),
                }],
                post_data: Some(HarPostData {
                    text: Some("{\"name\":\"test\"}".into()),
                }),
            },
            response: HarResponse {
                status: 201,
                status_text: "Created".into(),
                http_version: "HTTP/2".into(),
                headers: vec![],
                content: HarContent {
                    text: Some("{\"id\":1}".into()),
                },
            },
        };

        let result = convert_har_entry(entry);
        assert_eq!(result.request.method, "POST");
        assert_eq!(result.request.host, "api.example.com");
        assert!(result.request.is_tls);
        assert_eq!(result.request.version, HttpVersion::Http2);
        assert_eq!(result.request.body, Bytes::from("{\"name\":\"test\"}"));
        assert_eq!(result.request.headers.len(), 1);

        let resp = result.response.unwrap();
        assert_eq!(resp.status, 201);
        assert_eq!(resp.reason, "Created");
        assert_eq!(resp.body, Bytes::from("{\"id\":1}"));
        assert_eq!(resp.duration, Duration::from_millis(150));
    }

    #[test]
    fn convert_har_entry_no_body() {
        let entry = HarEntry {
            started_date_time: "2024-01-01T00:00:00Z".into(),
            time: 50.0,
            request: HarRequest {
                method: "GET".into(),
                url: "http://example.com/".into(),
                http_version: "HTTP/1.1".into(),
                headers: vec![],
                post_data: None,
            },
            response: HarResponse {
                status: 200,
                status_text: "OK".into(),
                http_version: "HTTP/1.1".into(),
                headers: vec![],
                content: HarContent { text: None },
            },
        };

        let result = convert_har_entry(entry);
        assert!(!result.request.is_tls);
        assert!(result.request.body.is_empty());
        assert!(result.response.unwrap().body.is_empty());
    }

    #[test]
    fn convert_har_entry_negative_time() {
        let entry = HarEntry {
            started_date_time: "2024-01-01T00:00:00Z".into(),
            time: -1.0,
            request: HarRequest {
                method: "GET".into(),
                url: "http://example.com/".into(),
                http_version: "HTTP/1.1".into(),
                headers: vec![],
                post_data: None,
            },
            response: HarResponse {
                status: 200,
                status_text: "OK".into(),
                http_version: "HTTP/1.1".into(),
                headers: vec![],
                content: HarContent { text: None },
            },
        };

        let result = convert_har_entry(entry);
        assert_eq!(result.response.unwrap().duration, Duration::from_millis(0));
    }
}
