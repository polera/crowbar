use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RequestId(pub u64);

impl RequestId {
    pub fn next() -> Self {
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpVersion {
    Http10,
    Http11,
    Http2,
}

impl std::fmt::Display for HttpVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpVersion::Http10 => write!(f, "HTTP/1.0"),
            HttpVersion::Http11 => write!(f, "HTTP/1.1"),
            HttpVersion::Http2 => write!(f, "HTTP/2"),
        }
    }
}

impl From<hyper::Version> for HttpVersion {
    fn from(v: hyper::Version) -> Self {
        match v {
            hyper::Version::HTTP_10 => HttpVersion::Http10,
            hyper::Version::HTTP_11 => HttpVersion::Http11,
            hyper::Version::HTTP_2 => HttpVersion::Http2,
            _ => HttpVersion::Http11,
        }
    }
}

pub fn extract_headers(headers: &hyper::HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").to_string()))
        .collect()
}

pub fn extract_trailers(trailers: Option<&hyper::HeaderMap>) -> Vec<(String, String)> {
    trailers
        .map(|t| {
            t.iter()
                .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").to_string()))
                .collect()
        })
        .unwrap_or_default()
}

pub fn status_reason(code: u16) -> &'static str {
    http::StatusCode::from_u16(code)
        .ok()
        .and_then(|s| s.canonical_reason())
        .unwrap_or("")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestData {
    pub id: RequestId,
    pub method: String,
    pub uri: String,
    pub host: String,
    pub version: HttpVersion,
    pub headers: Vec<(String, String)>,
    #[serde(with = "bytes_base64")]
    pub body: Bytes,
    pub is_tls: bool,
    #[serde(default)]
    pub is_grpc: bool,
    pub timestamp: SystemTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimingData {
    pub tcp_connect: Option<Duration>,
    pub tls_handshake: Option<Duration>,
    pub http_handshake: Option<Duration>,
    pub time_to_first_byte: Option<Duration>,
    pub content_transfer: Option<Duration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseData {
    pub status: u16,
    pub reason: String,
    pub version: HttpVersion,
    pub headers: Vec<(String, String)>,
    #[serde(with = "bytes_base64")]
    pub body: Bytes,
    #[serde(default)]
    pub trailers: Vec<(String, String)>,
    pub duration: Duration,
    #[serde(default)]
    pub timing: Option<TimingData>,
}

impl ResponseData {
    pub fn grpc_status(&self) -> Option<(u32, &'static str)> {
        let status_str = self
            .trailers
            .iter()
            .chain(self.headers.iter())
            .find(|(k, _)| k == "grpc-status")
            .map(|(_, v)| v.as_str())?;
        let code: u32 = status_str.parse().ok()?;
        Some((code, grpc_status_name(code)))
    }

    pub fn grpc_message(&self) -> Option<&str> {
        self.trailers
            .iter()
            .chain(self.headers.iter())
            .find(|(k, _)| k == "grpc-message")
            .map(|(_, v)| v.as_str())
    }
}

pub fn grpc_status_name(code: u32) -> &'static str {
    match code {
        0 => "OK",
        1 => "CANCELLED",
        2 => "UNKNOWN",
        3 => "INVALID_ARGUMENT",
        4 => "DEADLINE_EXCEEDED",
        5 => "NOT_FOUND",
        6 => "ALREADY_EXISTS",
        7 => "PERMISSION_DENIED",
        8 => "RESOURCE_EXHAUSTED",
        9 => "FAILED_PRECONDITION",
        10 => "ABORTED",
        11 => "OUT_OF_RANGE",
        12 => "UNIMPLEMENTED",
        13 => "INTERNAL",
        14 => "UNAVAILABLE",
        15 => "DATA_LOSS",
        16 => "UNAUTHENTICATED",
        _ => "UNKNOWN",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryState {
    Pending,
    Complete,
    Dropped,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageDirection {
    ClientToServer,
    ServerToClient,
}

pub type WsDirection = MessageDirection;
pub type GrpcDirection = MessageDirection;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsMessage {
    pub direction: MessageDirection,
    pub opcode: u8,
    #[serde(with = "bytes_base64")]
    pub payload: Bytes,
    pub timestamp: SystemTime,
}

impl WsMessage {
    pub fn is_text(&self) -> bool {
        self.opcode == 1
    }

    pub fn is_binary(&self) -> bool {
        self.opcode == 2
    }

    pub fn is_close(&self) -> bool {
        self.opcode == 8
    }

    pub fn text(&self) -> Option<&str> {
        if self.is_text() {
            std::str::from_utf8(&self.payload).ok()
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrpcMessage {
    pub direction: MessageDirection,
    pub compressed: bool,
    #[serde(with = "bytes_base64")]
    pub payload: Bytes,
    pub timestamp: SystemTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub request: RequestData,
    pub response: Option<ResponseData>,
    pub state: EntryState,
    pub error_message: Option<String>,
    #[serde(default)]
    pub ws_messages: Vec<WsMessage>,
    #[serde(default)]
    pub grpc_messages: Vec<GrpcMessage>,
    #[serde(default)]
    pub findings: Vec<crate::scanning::Finding>,
}

impl HistoryEntry {
    pub fn matches_filter(&self, filter: &str) -> bool {
        let req = &self.request;
        if contains_case_insensitive(&req.method, filter) {
            return true;
        }
        if contains_case_insensitive(&req.host, filter) {
            return true;
        }
        if contains_case_insensitive(&req.uri, filter) {
            return true;
        }
        if let Some(resp) = &self.response
            && resp.status.to_string().contains(filter)
        {
            return true;
        }
        if self.request.is_grpc && "grpc".starts_with(filter) {
            return true;
        }
        false
    }
}

fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    if !haystack.is_ascii() || !needle.is_ascii() {
        return haystack.to_lowercase().contains(&needle.to_lowercase());
    }
    let needle = needle.as_bytes();
    if needle.is_empty() {
        return true;
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|candidate| candidate.eq_ignore_ascii_case(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_id_monotonic() {
        let a = RequestId::next();
        let b = RequestId::next();
        assert!(b.0 > a.0);
    }

    #[test]
    fn request_id_display() {
        let id = RequestId(42);
        assert_eq!(format!("{}", id), "42");
    }

    #[test]
    fn http_version_display() {
        assert_eq!(format!("{}", HttpVersion::Http10), "HTTP/1.0");
        assert_eq!(format!("{}", HttpVersion::Http11), "HTTP/1.1");
        assert_eq!(format!("{}", HttpVersion::Http2), "HTTP/2");
    }

    #[test]
    fn http_version_from_hyper() {
        assert_eq!(
            HttpVersion::from(hyper::Version::HTTP_10),
            HttpVersion::Http10
        );
        assert_eq!(
            HttpVersion::from(hyper::Version::HTTP_11),
            HttpVersion::Http11
        );
        assert_eq!(
            HttpVersion::from(hyper::Version::HTTP_2),
            HttpVersion::Http2
        );
    }

    #[test]
    fn http_version_unknown_defaults_to_http11() {
        assert_eq!(
            HttpVersion::from(hyper::Version::HTTP_3),
            HttpVersion::Http11
        );
    }

    #[test]
    fn status_reason_known() {
        assert_eq!(status_reason(200), "OK");
        assert_eq!(status_reason(404), "Not Found");
        assert_eq!(status_reason(500), "Internal Server Error");
    }

    #[test]
    fn status_reason_unknown() {
        assert_eq!(status_reason(999), "");
    }

    #[test]
    fn grpc_status_name_known_codes() {
        assert_eq!(grpc_status_name(0), "OK");
        assert_eq!(grpc_status_name(1), "CANCELLED");
        assert_eq!(grpc_status_name(5), "NOT_FOUND");
        assert_eq!(grpc_status_name(13), "INTERNAL");
        assert_eq!(grpc_status_name(16), "UNAUTHENTICATED");
    }

    #[test]
    fn grpc_status_name_unknown_code() {
        assert_eq!(grpc_status_name(17), "UNKNOWN");
        assert_eq!(grpc_status_name(999), "UNKNOWN");
    }

    #[test]
    fn response_grpc_status_from_trailers() {
        let resp = ResponseData {
            status: 200,
            reason: "OK".into(),
            version: HttpVersion::Http2,
            headers: vec![],
            body: Bytes::new(),
            trailers: vec![("grpc-status".into(), "0".into())],
            duration: Duration::from_millis(10),
            timing: None,
        };
        assert_eq!(resp.grpc_status(), Some((0, "OK")));
    }

    #[test]
    fn response_grpc_status_from_headers() {
        let resp = ResponseData {
            status: 200,
            reason: "OK".into(),
            version: HttpVersion::Http2,
            headers: vec![("grpc-status".into(), "13".into())],
            body: Bytes::new(),
            trailers: vec![],
            duration: Duration::from_millis(10),
            timing: None,
        };
        assert_eq!(resp.grpc_status(), Some((13, "INTERNAL")));
    }

    #[test]
    fn response_grpc_status_none_when_missing() {
        let resp = ResponseData {
            status: 200,
            reason: "OK".into(),
            version: HttpVersion::Http2,
            headers: vec![],
            body: Bytes::new(),
            trailers: vec![],
            duration: Duration::from_millis(10),
            timing: None,
        };
        assert_eq!(resp.grpc_status(), None);
    }

    #[test]
    fn response_grpc_message_from_trailers() {
        let resp = ResponseData {
            status: 200,
            reason: "OK".into(),
            version: HttpVersion::Http2,
            headers: vec![],
            body: Bytes::new(),
            trailers: vec![("grpc-message".into(), "not found".into())],
            duration: Duration::from_millis(10),
            timing: None,
        };
        assert_eq!(resp.grpc_message(), Some("not found"));
    }

    #[test]
    fn ws_message_type_checks() {
        let text_msg = WsMessage {
            direction: MessageDirection::ClientToServer,
            opcode: 1,
            payload: Bytes::from("hello"),
            timestamp: SystemTime::now(),
        };
        assert!(text_msg.is_text());
        assert!(!text_msg.is_binary());
        assert!(!text_msg.is_close());
        assert_eq!(text_msg.text(), Some("hello"));

        let binary_msg = WsMessage {
            direction: MessageDirection::ServerToClient,
            opcode: 2,
            payload: Bytes::from(vec![0xFF, 0x00]),
            timestamp: SystemTime::now(),
        };
        assert!(!binary_msg.is_text());
        assert!(binary_msg.is_binary());
        assert_eq!(binary_msg.text(), None);

        let close_msg = WsMessage {
            direction: MessageDirection::ClientToServer,
            opcode: 8,
            payload: Bytes::new(),
            timestamp: SystemTime::now(),
        };
        assert!(close_msg.is_close());
    }

    #[test]
    fn ws_message_text_invalid_utf8() {
        let msg = WsMessage {
            direction: MessageDirection::ClientToServer,
            opcode: 1,
            payload: Bytes::from(vec![0xFF, 0xFE]),
            timestamp: SystemTime::now(),
        };
        assert_eq!(msg.text(), None);
    }

    fn make_entry(
        method: &str,
        host: &str,
        uri: &str,
        is_grpc: bool,
        status: Option<u16>,
    ) -> HistoryEntry {
        let request = RequestData {
            id: RequestId(1),
            method: method.into(),
            uri: uri.into(),
            host: host.into(),
            version: HttpVersion::Http11,
            headers: vec![],
            body: Bytes::new(),
            is_tls: false,
            is_grpc,
            timestamp: SystemTime::now(),
        };
        let response = status.map(|s| ResponseData {
            status: s,
            reason: "OK".into(),
            version: HttpVersion::Http11,
            headers: vec![],
            body: Bytes::new(),
            trailers: vec![],
            duration: Duration::from_millis(10),
            timing: None,
        });
        let state = if response.is_some() {
            EntryState::Complete
        } else {
            EntryState::Pending
        };
        HistoryEntry {
            request,
            response,
            state,
            error_message: None,
            ws_messages: vec![],
            grpc_messages: vec![],
            findings: vec![],
        }
    }

    #[test]
    fn matches_filter_by_method() {
        let entry = make_entry("POST", "example.com", "/api", false, Some(200));
        assert!(entry.matches_filter("post"));
        assert!(!entry.matches_filter("get"));
    }

    #[test]
    fn matches_filter_by_host() {
        let entry = make_entry("GET", "example.com", "/", false, Some(200));
        assert!(entry.matches_filter("example"));
    }

    #[test]
    fn matches_filter_unicode_case_insensitively() {
        let entry = make_entry("GET", "BÜCHER.example", "/", false, Some(200));
        assert!(entry.matches_filter("bücher"));
    }

    #[test]
    fn matches_filter_by_uri() {
        let entry = make_entry("GET", "example.com", "/api/users", false, Some(200));
        assert!(entry.matches_filter("/api"));
    }

    #[test]
    fn matches_filter_by_status() {
        let entry = make_entry("GET", "example.com", "/", false, Some(404));
        assert!(entry.matches_filter("404"));
    }

    #[test]
    fn matches_filter_grpc_keyword() {
        let entry = make_entry("POST", "example.com", "/svc", true, Some(200));
        assert!(entry.matches_filter("grpc"));
        assert!(entry.matches_filter("grp"));
    }

    #[test]
    fn matches_filter_no_match() {
        let entry = make_entry("GET", "example.com", "/", false, Some(200));
        assert!(!entry.matches_filter("zzz"));
    }

    #[test]
    fn extract_headers_basic() {
        let mut map = hyper::HeaderMap::new();
        map.insert("content-type", "text/html".parse().unwrap());
        map.insert("x-custom", "value".parse().unwrap());
        let headers = extract_headers(&map);
        assert_eq!(headers.len(), 2);
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "content-type" && v == "text/html")
        );
    }

    #[test]
    fn extract_trailers_none() {
        assert!(extract_trailers(None).is_empty());
    }

    #[test]
    fn extract_trailers_some() {
        let mut map = hyper::HeaderMap::new();
        map.insert("grpc-status", "0".parse().unwrap());
        let trailers = extract_trailers(Some(&map));
        assert_eq!(trailers.len(), 1);
        assert_eq!(trailers[0], ("grpc-status".into(), "0".into()));
    }

    #[test]
    fn bytes_base64_roundtrip() {
        use serde::{Deserialize, Serialize};

        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Wrapper {
            #[serde(with = "super::bytes_base64")]
            data: Bytes,
        }

        let original = Wrapper {
            data: Bytes::from("hello world"),
        };
        let json = serde_json::to_string(&original).unwrap();
        let restored: Wrapper = serde_json::from_str(&json).unwrap();
        assert_eq!(original, restored);
    }
}

mod bytes_base64 {
    use base64::Engine;
    use bytes::Bytes;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &Bytes, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Bytes, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(&s)
            .map(Bytes::from)
            .map_err(serde::de::Error::custom)
    }
}
