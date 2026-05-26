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
        if req.method.to_lowercase().contains(filter) {
            return true;
        }
        if req.host.to_lowercase().contains(filter) {
            return true;
        }
        if req.uri.to_lowercase().contains(filter) {
            return true;
        }
        if let Some(resp) = &self.response
            && resp.status.to_string().contains(filter) {
                return true;
            }
        if self.request.is_grpc && "grpc".starts_with(filter) {
            return true;
        }
        false
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
