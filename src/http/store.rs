use std::collections::HashMap;

use crate::scanning::Finding;

use super::models::{
    EntryState, GrpcMessage, HistoryEntry, RequestData, RequestId, ResponseData, WsMessage,
};

const MAX_STREAM_MESSAGES_PER_ENTRY: usize = 1_000;

#[derive(Default)]
struct FilterCache {
    filter: String,
    indices: Vec<usize>,
    revision: u64,
}

pub struct InMemoryStore {
    entries: Vec<HistoryEntry>,
    index: HashMap<RequestId, usize>,
    filter_cache: FilterCache,
    max_entries: usize,
    complete_count: usize,
    error_count: usize,
    revision: u64,
    max_body_bytes: usize,
}

impl InMemoryStore {
    pub fn new(max_entries: usize, max_body_bytes: usize) -> Self {
        Self {
            entries: Vec::new(),
            index: HashMap::new(),
            filter_cache: FilterCache::default(),
            max_entries: max_entries.max(1),
            complete_count: 0,
            error_count: 0,
            revision: 1,
            max_body_bytes,
        }
    }

    pub fn insert(&mut self, request: RequestData) {
        self.revision = self.revision.wrapping_add(1);
        let id = request.id;
        let idx = self.entries.len();
        self.entries.push(HistoryEntry {
            request,
            response: None,
            state: EntryState::Pending,
            error_message: None,
            ws_messages: Vec::new(),
            grpc_messages: Vec::new(),
            findings: Vec::new(),
            request_body_truncated: false,
        });
        self.index.insert(id, idx);
        self.evict_oldest_if_needed();
    }

    fn evict_oldest_if_needed(&mut self) {
        if self.entries.len() <= self.max_entries {
            return;
        }
        let drop_count = (self.max_entries / 10).max(1);
        self.entries.drain(..drop_count);
        self.index.clear();
        for (idx, entry) in self.entries.iter().enumerate() {
            self.index.insert(entry.request.id, idx);
        }
        self.recount_states();
    }

    pub fn update_response(&mut self, id: RequestId, response: ResponseData) {
        self.revision = self.revision.wrapping_add(1);
        if let Some(&idx) = self.index.get(&id) {
            self.set_state(idx, EntryState::Complete);
            if let Some(entry) = self.entries.get_mut(idx) {
                entry.response = Some(response);
            }
        }
    }

    pub fn mark_dropped(&mut self, id: RequestId) {
        if let Some(&idx) = self.index.get(&id) {
            self.set_state(idx, EntryState::Dropped);
        }
    }

    pub fn push_ws_message(&mut self, id: RequestId, msg: WsMessage) {
        if let Some(&idx) = self.index.get(&id)
            && let Some(entry) = self.entries.get_mut(idx)
        {
            entry.ws_messages.push(msg);
            trim_messages(&mut entry.ws_messages, self.max_body_bytes, |message| {
                message.payload.len()
            });
        }
    }

    pub fn push_grpc_message(&mut self, id: RequestId, msg: GrpcMessage) {
        if let Some(&idx) = self.index.get(&id)
            && let Some(entry) = self.entries.get_mut(idx)
        {
            // Retain a replayable prefix of complete client frames, bounded by the body limit.
            if msg.direction == super::models::GrpcDirection::ClientToServer
                && !entry.request_body_truncated
            {
                if entry
                    .request
                    .body
                    .len()
                    .saturating_add(msg.payload.len())
                    .saturating_add(5)
                    > self.max_body_bytes
                {
                    entry.request_body_truncated = true;
                } else {
                    let mut body = bytes::BytesMut::from(entry.request.body.as_ref());
                    body.extend_from_slice(&[u8::from(msg.compressed)]);
                    body.extend_from_slice(&(msg.payload.len() as u32).to_be_bytes());
                    body.extend_from_slice(&msg.payload);
                    entry.request.body = body.freeze();
                }
            }
            entry.grpc_messages.push(msg);
            trim_messages(&mut entry.grpc_messages, self.max_body_bytes, |message| {
                message.payload.len()
            });
        }
    }

    pub fn update_trailers(&mut self, id: RequestId, trailers: Vec<(String, String)>) {
        if let Some(&idx) = self.index.get(&id)
            && let Some(entry) = self.entries.get_mut(idx)
            && let Some(resp) = &mut entry.response
        {
            resp.trailers = trailers;
        }
    }

    pub fn mark_request_capture_truncated(&mut self, id: RequestId) {
        if let Some(&index) = self.index.get(&id) {
            self.entries[index].request_body_truncated = true;
        }
    }

    pub fn set_findings(&mut self, id: RequestId, findings: Vec<Finding>) {
        if let Some(&idx) = self.index.get(&id)
            && let Some(entry) = self.entries.get_mut(idx)
        {
            entry.findings = findings;
        }
    }

    pub fn mark_error(&mut self, id: RequestId, error: String) {
        if let Some(&idx) = self.index.get(&id) {
            self.set_state(idx, EntryState::Error);
            if let Some(entry) = self.entries.get_mut(idx) {
                entry.error_message = Some(error);
            }
        }
    }

    pub fn get(&self, id: RequestId) -> Option<&HistoryEntry> {
        self.index.get(&id).and_then(|&idx| self.entries.get(idx))
    }

    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    pub fn state_counts(&self) -> (usize, usize) {
        (self.complete_count, self.error_count)
    }

    fn set_state(&mut self, idx: usize, state: EntryState) {
        let Some(entry) = self.entries.get_mut(idx) else {
            return;
        };
        match entry.state {
            EntryState::Complete => self.complete_count = self.complete_count.saturating_sub(1),
            EntryState::Error => self.error_count = self.error_count.saturating_sub(1),
            _ => {}
        }
        entry.state = state;
        match state {
            EntryState::Complete => self.complete_count += 1,
            EntryState::Error => self.error_count += 1,
            _ => {}
        }
    }

    fn recount_states(&mut self) {
        self.complete_count = self
            .entries
            .iter()
            .filter(|entry| entry.state == EntryState::Complete)
            .count();
        self.error_count = self
            .entries
            .iter()
            .filter(|entry| entry.state == EntryState::Error)
            .count();
    }

    pub fn refresh_filter_cache(&mut self, filter: &str) {
        let cache = &self.filter_cache;
        if cache.filter == filter && cache.revision == self.revision {
            return;
        }

        let indices = if filter.is_empty() {
            (0..self.entries.len()).collect()
        } else {
            let filter_lower = filter.to_lowercase();
            self.entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| entry.matches_filter(&filter_lower))
                .map(|(i, _)| i)
                .collect()
        };

        self.filter_cache = FilterCache {
            filter: filter.to_string(),
            indices,
            revision: self.revision,
        };
    }

    pub fn filtered_count(&self) -> usize {
        self.filter_cache.indices.len()
    }

    pub fn filtered_entry(&self, filtered_idx: usize) -> Option<&HistoryEntry> {
        self.filter_cache
            .indices
            .get(filtered_idx)
            .and_then(|&i| self.entries.get(i))
    }

    pub fn filtered_entries_iter(&self) -> impl Iterator<Item = &HistoryEntry> {
        self.filter_cache
            .indices
            .iter()
            .filter_map(|&i| self.entries.get(i))
    }

    pub fn filtered_entries_all(&self) -> Vec<&HistoryEntry> {
        self.filtered_entries_iter().collect()
    }

    pub fn load_entries(&mut self, entries: Vec<HistoryEntry>) -> anyhow::Result<()> {
        let skip = entries.len().saturating_sub(self.max_entries);
        let entries: Vec<_> = entries.into_iter().skip(skip).collect();
        for entry in &entries {
            anyhow::ensure!(
                entry.request.body.len() <= self.max_body_bytes
                    && entry
                        .response
                        .as_ref()
                        .is_none_or(|response| response.body.len() <= self.max_body_bytes),
                "Session body exceeds configured limit"
            );
        }
        self.entries.clear();
        self.index.clear();
        for mut entry in entries {
            // Saved IDs belong to another process. Never reuse them in the live index.
            entry.request.id = RequestId::next();
            trim_messages(&mut entry.ws_messages, self.max_body_bytes, |message| {
                message.payload.len()
            });
            trim_messages(&mut entry.grpc_messages, self.max_body_bytes, |message| {
                message.payload.len()
            });
            let id = entry.request.id;
            let idx = self.entries.len();
            self.entries.push(entry);
            self.index.insert(id, idx);
        }
        self.recount_states();
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn trim_messages<T>(messages: &mut Vec<T>, max_bytes: usize, size: impl Fn(&T) -> usize) {
    let mut bytes = 0usize;
    let mut keep = 0;
    for message in messages.iter().rev().take(MAX_STREAM_MESSAGES_PER_ENTRY) {
        let Some(total) = bytes.checked_add(size(message)) else {
            break;
        };
        if total > max_bytes {
            break;
        }
        bytes = total;
        keep += 1;
    }
    messages.drain(..messages.len() - keep);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::time::SystemTime;

    #[test]
    fn status_filters_refresh_without_new_entries() {
        let mut store = InMemoryStore::new(10, 1024);
        let request = crate::test_support::request();
        store.insert(request.clone());
        store.refresh_filter_cache("200");
        assert_eq!(store.filtered_count(), 0);
        store.update_response(request.id, crate::test_support::response());
        store.refresh_filter_cache("200");
        assert_eq!(store.filtered_count(), 1);
    }

    #[test]
    fn loading_enforces_retention_rekeys_ids_and_invalidates_same_size_cache() {
        let mut store = InMemoryStore::new(1, 1024);
        let mut first = crate::test_support::entry();
        first.request.host = "first.example".into();
        store.load_entries(vec![first]).unwrap();
        store.refresh_filter_cache("first.example");
        assert_eq!(store.filtered_count(), 1);
        let old_id = store.entries()[0].request.id;
        let second = crate::test_support::entry();
        let saved_id = second.request.id;
        store
            .load_entries(vec![crate::test_support::entry(), second])
            .unwrap();
        assert_eq!(store.len(), 1);
        let new_id = store.entries()[0].request.id;
        assert_ne!(old_id, new_id);
        assert_ne!(saved_id, new_id);
        assert_ne!(RequestId::next(), new_id);
        store.refresh_filter_cache("first.example");
        assert_eq!(store.filtered_count(), 0);
    }

    #[test]
    fn oversized_load_fails_without_replacing_current_history() {
        let mut store = InMemoryStore::new(10, 2);
        store.insert(crate::test_support::request());
        let id = store.entries()[0].request.id;
        let mut entry = crate::test_support::entry();
        entry.request.body = Bytes::from_static(b"big");
        assert!(store.load_entries(vec![entry]).is_err());
        assert_eq!(store.entries()[0].request.id, id);
    }

    #[test]
    fn streamed_client_frames_form_a_bounded_prefix_and_are_marked_truncated() {
        let mut store = InMemoryStore::new(10, 8);
        let mut request = crate::test_support::request();
        request.is_grpc = true;
        let id = request.id;
        store.insert(request);
        for payload in [b"abc".as_slice(), b"def", b"z"] {
            store.push_grpc_message(
                id,
                GrpcMessage {
                    direction: super::super::models::GrpcDirection::ClientToServer,
                    compressed: false,
                    payload: Bytes::copy_from_slice(payload),
                    timestamp: SystemTime::now(),
                },
            );
        }
        let entry = store.get(id).unwrap();
        assert_eq!(entry.request.body.as_ref(), b"\x00\x00\x00\x00\x03abc");
        assert!(entry.request_body_truncated);
    }

    fn request(id: u64) -> RequestData {
        RequestData {
            id: RequestId(id),
            method: "GET".into(),
            uri: "/".into(),
            host: "example.com".into(),
            version: super::super::models::HttpVersion::Http11,
            headers: Vec::new(),
            body: Bytes::new(),
            is_tls: false,
            is_grpc: false,
            timestamp: SystemTime::now(),
        }
    }

    #[test]
    fn history_is_bounded_and_state_counts_track_transitions() {
        let mut store = InMemoryStore::new(3, 1024);
        for id in 1..=4 {
            store.insert(request(id));
        }
        assert_eq!(store.len(), 3);
        assert!(store.get(RequestId(1)).is_none());

        store.mark_error(RequestId(2), "failure".into());
        assert_eq!(store.state_counts(), (0, 1));
        store.mark_dropped(RequestId(2));
        assert_eq!(store.state_counts(), (0, 0));
    }
}
