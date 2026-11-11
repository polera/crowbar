use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use parking_lot::Mutex;
use tokio::sync::{mpsc, oneshot};

use crate::channel::ProxyToUi;
use crate::http::models::{RequestData, RequestId};

#[derive(Debug)]
pub enum InterceptDecision {
    Forward,
    ForwardEdited(RequestData),
    Drop,
}

struct Control {
    enabled: bool,
    pending: HashMap<RequestId, oneshot::Sender<InterceptDecision>>,
}

pub struct InterceptState {
    control: Arc<Mutex<Control>>,
}

/// Removes abandoned registrations when a client disconnects or a task is cancelled.
pub struct PendingDecision {
    id: RequestId,
    receiver: oneshot::Receiver<InterceptDecision>,
    control: Arc<Mutex<Control>>,
}

impl Future for PendingDecision {
    type Output = Result<InterceptDecision, oneshot::error::RecvError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.receiver).poll(cx)
    }
}

impl Drop for PendingDecision {
    fn drop(&mut self) {
        self.control.lock().pending.remove(&self.id);
    }
}

impl InterceptState {
    pub fn new(enabled: bool) -> Self {
        Self {
            control: Arc::new(Mutex::new(Control {
                enabled,
                pending: HashMap::new(),
            })),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.control.lock().enabled
    }

    pub fn is_pending(&self, id: RequestId) -> bool {
        self.control.lock().pending.contains_key(&id)
    }

    pub fn toggle(&self) -> bool {
        let mut control = self.control.lock();
        control.enabled = !control.enabled;
        if !control.enabled {
            Self::drain(&mut control);
        }
        control.enabled
    }

    /// Backpressure is intentional: a full UI queue must never bypass interception.
    pub async fn intercept_request(
        &self,
        request: &RequestData,
        ui_tx: &mpsc::Sender<ProxyToUi>,
    ) -> anyhow::Result<Option<PendingDecision>> {
        if !self.is_enabled() {
            return Ok(None);
        }
        let permit = ui_tx
            .reserve()
            .await
            .map_err(|_| anyhow::anyhow!("interception UI is unavailable"))?;
        let mut control = self.control.lock();
        if !control.enabled {
            return Ok(None);
        }
        let (sender, receiver) = oneshot::channel();
        control.pending.insert(request.id, sender);
        permit.send(ProxyToUi::InterceptedRequest(request.clone()));
        Ok(Some(PendingDecision {
            id: request.id,
            receiver,
            control: self.control.clone(),
        }))
    }

    pub fn resolve(&self, id: RequestId, decision: InterceptDecision) -> bool {
        self.control
            .lock()
            .pending
            .remove(&id)
            .is_some_and(|sender| sender.send(decision).is_ok())
    }

    fn drain(control: &mut Control) {
        for (_, sender) in control.pending.drain() {
            let _ = sender.send(InterceptDecision::Forward);
        }
    }

    pub fn forward_all(&self) {
        let mut control = self.control.lock();
        control.enabled = false;
        Self::drain(&mut control);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn registration_precedes_publication_and_cancelled_requests_are_removed() {
        let state = InterceptState::new(true);
        let (sender, mut receiver) = mpsc::channel(1);
        let request = crate::test_support::request();
        let decision = state
            .intercept_request(&request, &sender)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            receiver.recv().await,
            Some(ProxyToUi::InterceptedRequest(_))
        ));
        assert!(state.is_pending(request.id));
        assert!(state.resolve(request.id, InterceptDecision::Drop));
        assert!(matches!(decision.await, Ok(InterceptDecision::Drop)));
        let pending = state
            .intercept_request(&request, &sender)
            .await
            .unwrap()
            .unwrap();
        drop(pending);
        assert!(!state.is_pending(request.id));
    }

    #[tokio::test]
    async fn full_channel_waits_instead_of_bypassing_interception() {
        let state = Arc::new(InterceptState::new(true));
        let (sender, mut receiver) = mpsc::channel(1);
        sender
            .send(ProxyToUi::RequestCaptured(crate::test_support::request()))
            .await
            .unwrap();
        let request = crate::test_support::request();
        let id = request.id;
        let worker = {
            let state = state.clone();
            tokio::spawn(async move { state.intercept_request(&request, &sender).await })
        };
        tokio::task::yield_now().await;
        assert!(!worker.is_finished());
        receiver.recv().await.unwrap();
        let decision = worker.await.unwrap().unwrap().unwrap();
        receiver.recv().await.unwrap();
        assert!(state.resolve(id, InterceptDecision::Forward));
        assert!(matches!(decision.await, Ok(InterceptDecision::Forward)));
    }

    #[tokio::test]
    async fn disabling_interception_atomically_resolves_registered_requests() {
        let state = InterceptState::new(true);
        let (sender, _receiver) = mpsc::channel(1);
        let request = crate::test_support::request();
        let decision = state
            .intercept_request(&request, &sender)
            .await
            .unwrap()
            .unwrap();
        assert!(!state.toggle());
        assert!(matches!(decision.await, Ok(InterceptDecision::Forward)));
        assert!(!state.is_pending(request.id));
    }

    #[tokio::test]
    async fn unavailable_ui_is_an_error_not_permission_to_forward() {
        let state = InterceptState::new(true);
        let (sender, receiver) = mpsc::channel(1);
        drop(receiver);
        assert!(
            state
                .intercept_request(&crate::test_support::request(), &sender)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn disabling_while_registration_waits_for_capacity_leaves_no_orphan() {
        let state = Arc::new(InterceptState::new(true));
        let (sender, mut receiver) = mpsc::channel(1);
        sender
            .send(ProxyToUi::RequestCaptured(crate::test_support::request()))
            .await
            .unwrap();
        let worker = {
            let state = state.clone();
            tokio::spawn(async move {
                state
                    .intercept_request(&crate::test_support::request(), &sender)
                    .await
            })
        };
        tokio::task::yield_now().await;
        assert!(!state.toggle());
        receiver.recv().await.unwrap();
        assert!(worker.await.unwrap().unwrap().is_none());
        assert!(state.control.lock().pending.is_empty());
    }
}
