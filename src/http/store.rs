use std::collections::HashMap;

use crate::scanning::Finding;

use super::models::{EntryState, GrpcMessage, HistoryEntry, RequestData, RequestId, ResponseData, WsMessage};

#[derive(Default)]
pub struct InMemoryStore {
    entries: Vec<HistoryEntry>,
    index: HashMap<RequestId, usize>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self::default()
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
    }

    pub fn update_response(&mut self, id: RequestId, response: ResponseData) {
        if let Some(&idx) = self.index.get(&id)
            && let Some(entry) = self.entries.get_mut(idx)
        {
            entry.response = Some(response);
            entry.state = EntryState::Complete;
        }
    }

    pub fn mark_dropped(&mut self, id: RequestId) {
        if let Some(&idx) = self.index.get(&id)
            && let Some(entry) = self.entries.get_mut(idx)
        {
            entry.state = EntryState::Dropped;
        }
    }

    pub fn push_ws_message(&mut self, id: RequestId, msg: WsMessage) {
        if let Some(&idx) = self.index.get(&id)
            && let Some(entry) = self.entries.get_mut(idx)
        {
            entry.ws_messages.push(msg);
        }
    }

    pub fn push_grpc_message(&mut self, id: RequestId, msg: GrpcMessage) {
        if let Some(&idx) = self.index.get(&id)
            && let Some(entry) = self.entries.get_mut(idx)
        {
            entry.grpc_messages.push(msg);
        }
    }

    pub fn update_trailers(&mut self, id: RequestId, trailers: Vec<(String, String)>) {
        if let Some(&idx) = self.index.get(&id)
            && let Some(entry) = self.entries.get_mut(idx)
            && let Some(resp) = &mut entry.response {
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
        if let Some(&idx) = self.index.get(&id)
            && let Some(entry) = self.entries.get_mut(idx)
        {
            entry.state = EntryState::Error;
            entry.error_message = Some(error);
        }
    }

    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    pub fn filtered_entries(&self, filter: &str) -> Vec<&HistoryEntry> {
        if filter.is_empty() {
            return self.entries.iter().collect();
        }
        let filter_lower = filter.to_lowercase();
        self.entries
            .iter()
            .filter(|entry| entry.matches_filter(&filter_lower))
            .collect()
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
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
