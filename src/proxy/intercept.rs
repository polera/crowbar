use std::sync::atomic::{AtomicBool, Ordering};

use dashmap::DashMap;
use tokio::sync::{mpsc, oneshot};
use tracing::debug;

use crate::channel::ProxyToUi;
use crate::http::models::{RequestData, RequestId};

#[derive(Debug)]
pub enum InterceptDecision {
    Forward,
    ForwardEdited(RequestData),
    Drop,
}

pub struct InterceptState {
    enabled: AtomicBool,
    pending: DashMap<RequestId, oneshot::Sender<InterceptDecision>>,
}

impl InterceptState {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled: AtomicBool::new(enabled),
            pending: DashMap::new(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn toggle(&self) -> bool {
        let prev = self.enabled.fetch_xor(true, Ordering::Relaxed);
        !prev
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Called by the proxy when a request should be intercepted.
    /// Sends the request to the TUI and returns a receiver for the decision.
    /// Returns None if the channel to the TUI is closed.
    pub fn intercept_request(
        &self,
        request: &RequestData,
        ui_tx: &mpsc::UnboundedSender<ProxyToUi>,
    ) -> Option<oneshot::Receiver<InterceptDecision>> {
        if !self.is_enabled() {
            return None;
        }

        let (tx, rx) = oneshot::channel();
        let id = request.id;

        if ui_tx
            .send(ProxyToUi::InterceptedRequest(request.clone()))
            .is_err()
        {
            return None;
        }

        self.pending.insert(id, tx);
        debug!("Request {} queued for intercept", id);
        Some(rx)
    }

    /// Called by the TUI when the user makes a decision about an intercepted request.
    pub fn resolve(&self, id: RequestId, decision: InterceptDecision) -> bool {
        if let Some((_, tx)) = self.pending.remove(&id) {
            let _ = tx.send(decision);
            true
        } else {
            false
        }
    }

    /// Forward all pending requests (used when intercept is toggled off).
    pub fn forward_all(&self) {
        let keys: Vec<RequestId> = self.pending.iter().map(|r| *r.key()).collect();
        for id in keys {
            if let Some((_, tx)) = self.pending.remove(&id) {
                let _ = tx.send(InterceptDecision::Forward);
            }
        }
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}
