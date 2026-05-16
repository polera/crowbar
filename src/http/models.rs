use bytes::Bytes;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone)]
pub struct RequestData {
    pub id: RequestId,
    pub method: String,
    pub uri: String,
    pub host: String,
    pub version: HttpVersion,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
    pub is_tls: bool,
    pub timestamp: SystemTime,
}

#[derive(Debug, Clone)]
pub struct ResponseData {
    pub status: u16,
    pub reason: String,
    pub version: HttpVersion,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
    pub duration: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryState {
    Pending,
    Complete,
    Dropped,
    Error,
}

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub request: RequestData,
    pub response: Option<ResponseData>,
    pub state: EntryState,
    pub error_message: Option<String>,
}
