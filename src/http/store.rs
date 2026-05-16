use std::collections::HashMap;

use super::models::{EntryState, HistoryEntry, RequestData, RequestId, ResponseData};

pub struct InMemoryStore {
    entries: Vec<HistoryEntry>,
    index: HashMap<RequestId, usize>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            index: HashMap::new(),
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

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}
