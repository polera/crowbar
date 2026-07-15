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
    entry_count: usize,
}

pub struct InMemoryStore {
    entries: Vec<HistoryEntry>,
    index: HashMap<RequestId, usize>,
    filter_cache: FilterCache,
    max_entries: usize,
    complete_count: usize,
    error_count: usize,
}

impl InMemoryStore {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            index: HashMap::new(),
            filter_cache: FilterCache::default(),
            max_entries: max_entries.max(1),
            complete_count: 0,
            error_count: 0,
        }
    }

    pub fn insert(&mut self, request: RequestData) {
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
        self.filter_cache.entry_count = usize::MAX;
        self.recount_states();
    }

    pub fn update_response(&mut self, id: RequestId, response: ResponseData) {
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
            if entry.ws_messages.len() > MAX_STREAM_MESSAGES_PER_ENTRY {
                entry.ws_messages.drain(..100);
            }
        }
    }

    pub fn push_grpc_message(&mut self, id: RequestId, msg: GrpcMessage) {
        if let Some(&idx) = self.index.get(&id)
            && let Some(entry) = self.entries.get_mut(idx)
        {
            entry.grpc_messages.push(msg);
            if entry.grpc_messages.len() > MAX_STREAM_MESSAGES_PER_ENTRY {
                entry.grpc_messages.drain(..100);
            }
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
        if cache.filter == filter && cache.entry_count == self.entries.len() {
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
            entry_count: self.entries.len(),
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

    pub fn load_entries(&mut self, entries: Vec<HistoryEntry>) {
        self.entries.clear();
        self.index.clear();
        for entry in entries {
            let id = entry.request.id;
            let idx = self.entries.len();
            self.entries.push(entry);
            self.index.insert(id, idx);
        }
        self.recount_states();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::time::SystemTime;

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
        let mut store = InMemoryStore::new(3);
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
