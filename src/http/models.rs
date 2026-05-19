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
    pub duration: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryState {
    Pending,
    Complete,
    Dropped,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WsDirection {
    ClientToServer,
    ServerToClient,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsMessage {
    pub direction: WsDirection,
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
pub struct HistoryEntry {
    pub request: RequestData,
    pub response: Option<ResponseData>,
    pub state: EntryState,
    pub error_message: Option<String>,
    #[serde(default)]
    pub ws_messages: Vec<WsMessage>,
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
        if let Some(resp) = &self.response {
            if resp.status.to_string().contains(filter) {
                return true;
            }
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
            .map(|v| Bytes::from(v))
            .map_err(serde::de::Error::custom)
    }
}
