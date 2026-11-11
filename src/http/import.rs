use std::path::Path;
use std::time::{Duration, SystemTime};

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
    let content = super::session::read_input(path)?;
    let har: HarFile = serde_json::from_slice(&content)?;

    har.log.entries.into_iter().map(convert_har_entry).collect()
}

fn convert_har_entry(entry: HarEntry) -> anyhow::Result<HistoryEntry> {
    let req = &entry.request;

    let host = extract_host(&req.url);
    let is_tls = req
        .url
        .parse::<http::Uri>()
        .ok()
        .is_some_and(|uri| uri.scheme_str() == Some("https"));

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
        .ok_or_else(|| anyhow::anyhow!("Invalid HAR timestamp {:?}", entry.started_date_time))?;

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
        .map(|t| {
            if resp.content.encoding.as_deref() == Some("base64") {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD
                    .decode(t)
                    .map(Bytes::from)
                    .map_err(anyhow::Error::from)
            } else {
                Ok(Bytes::from(t.clone()))
            }
        })
        .transpose()?
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

    Ok(HistoryEntry {
        request: request_data,
        response: Some(response_data),
        state: EntryState::Complete,
        error_message: None,
        ws_messages: Vec::new(),
        grpc_messages: Vec::new(),
        findings: Vec::new(),
        request_body_truncated: false,
    })
}

fn extract_host(url: &str) -> String {
    let absolute = if url.contains("://") {
        url.to_string()
    } else {
        format!("http://{url}")
    };
    absolute
        .parse::<http::Uri>()
        .ok()
        .and_then(|uri| {
            uri.host()
                .map(|host| host.trim_matches(['[', ']']).to_string())
        })
        .unwrap_or_default()
}

fn parse_http_version(v: &str) -> HttpVersion {
    match v {
        "HTTP/1.0" | "http/1.0" => HttpVersion::Http10,
        "HTTP/2" | "HTTP/2.0" | "h2" => HttpVersion::Http2,
        _ => HttpVersion::Http11,
    }
}

fn parse_iso_timestamp(s: &str) -> Option<SystemTime> {
    let timestamp =
        time::OffsetDateTime::parse(s.trim(), &time::format_description::well_known::Rfc3339)
            .ok()?;
    Some(timestamp.into())
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
    encoding: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    #[test]
    fn timestamps_honor_offsets_fractional_seconds_and_invalid_dates() {
        let utc = parse_iso_timestamp("2024-01-01T00:00:00.125Z").unwrap();
        assert_eq!(
            parse_iso_timestamp("2024-01-01T01:00:00.125+01:00"),
            Some(utc)
        );
        assert_eq!(utc.duration_since(UNIX_EPOCH).unwrap().subsec_millis(), 125);
        assert!(parse_iso_timestamp("2024-02-31T00:00:00Z").is_none());
        assert!(parse_iso_timestamp("99999999999999999999-01-01T00:00:00Z").is_none());
        assert_eq!(extract_host("https://[::1]:8443/path?q=1"), "::1");
    }

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
        let expected = 1_718_461_800; // 2024-06-15T14:30:00Z
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
                    encoding: None,
                },
            },
        };

        let result = convert_har_entry(entry).unwrap();
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
                content: HarContent {
                    text: None,
                    encoding: None,
                },
            },
        };

        let result = convert_har_entry(entry).unwrap();
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
                content: HarContent {
                    text: None,
                    encoding: None,
                },
            },
        };

        let result = convert_har_entry(entry).unwrap();
        assert_eq!(result.response.unwrap().duration, Duration::from_millis(0));
    }
}
