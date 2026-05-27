use crate::channel::ProxyToUi;
use crate::editor::TextEditor;
use crate::http::codec;
use crate::http::models::{RequestData, RequestId};
use crate::http::sequence::StepState;
use crate::proxy::intercept::InterceptDecision;
use crate::proxy::repeater;
use crate::tui::tabs::Tab;

use super::App;

impl App {
    pub(super) fn set_status(&mut self, msg: impl Into<String>) {
        self.status_message = Some((msg.into(), std::time::Instant::now()));
    }

    pub(super) fn forward_edited_intercept(&mut self) {
        if let Some(original) = self.intercept_ui.queue.pop_front() {
            let edited = codec::lines_to_request(&self.intercept_ui.editor.lines, &original);
            self.intercept_state
                .resolve(original.id, InterceptDecision::ForwardEdited(edited));
        }
        self.intercept_ui.editing = false;
        self.intercept_ui.editor = TextEditor::new(vec![], self.editor_mode);
        self.intercept_ui.scroll = 0;
    }

    pub(super) fn send_to_repeater(&mut self) {
        self.store.refresh_filter_cache(&self.history.filter);
        if let Some(entry) = self.store.filtered_entry(self.history.selected) {
            let req = &entry.request;
            self.repeater.editor = TextEditor::new(codec::request_to_lines(req), self.editor_mode);
            self.repeater.original = Some(req.clone());
            self.repeater.response = None;
            self.repeater.error = None;
            self.repeater.pending = false;
            self.repeater.editing = false;
            self.repeater.req_scroll = 0;
            self.repeater.resp_scroll = 0;
            self.active_tab = Tab::Repeater;
        }
    }

    pub(super) fn load_macro_step(&mut self, send: bool) {
        if let Some(step) = self.macros.steps.get(self.macros.selected) {
            let req = &step.request;
            self.repeater.editor = TextEditor::new(codec::request_to_lines(req), self.editor_mode);
            self.repeater.original = Some(req.clone());
            self.repeater.response = None;
            self.repeater.error = None;
            self.repeater.pending = false;
            self.repeater.editing = false;
            self.repeater.req_scroll = 0;
            self.repeater.resp_scroll = 0;
            self.macros.show = false;
            if send {
                self.repeater_send();
            }
        }
    }

    pub(super) fn repeater_send(&mut self) {
        if self.repeater.editor.lines.is_empty() || self.repeater.pending {
            return;
        }

        let original = self.repeater.original.clone().unwrap_or(RequestData {
            id: RequestId::next(),
            method: "GET".into(),
            uri: "/".into(),
            host: "localhost".into(),
            version: crate::http::models::HttpVersion::Http11,
            headers: Vec::new(),
            body: bytes::Bytes::new(),
            is_tls: false,
            is_grpc: false,
            timestamp: std::time::SystemTime::now(),
        });

        let request = codec::lines_to_request(&self.repeater.editor.lines, &original);
        self.repeater.original = Some(request.clone());
        self.repeater.pending = true;
        self.repeater.response = None;
        self.repeater.error = None;
        self.repeater.resp_scroll = 0;

        let ui_tx = self.ui_tx.clone();
        tokio::spawn(async move {
            repeater::send_request(request, ui_tx).await;
        });
    }

    pub(super) fn macro_run(&mut self) {
        if self.macros.steps.is_empty() || self.macros.running {
            return;
        }

        for step in &mut self.macros.steps {
            step.state = StepState::Pending;
            step.response = None;
            step.error = None;
        }

        self.macros.running = true;
        self.macros.current_step = 0;
        self.macro_send_next();
    }

    fn macro_send_next(&mut self) {
        if self.macros.current_step >= self.macros.steps.len() {
            self.macros.running = false;
            self.set_status(format!("Macro complete ({} steps)", self.macros.steps.len()));
            return;
        }

        self.macros.steps[self.macros.current_step].state = StepState::Running;
        let request = self.macros.steps[self.macros.current_step].request.clone();
        let step_idx = self.macros.current_step;
        let ui_tx = self.ui_tx.clone();

        tokio::spawn(async move {
            let ui_tx_inner = ui_tx.clone();
            match repeater::send_raw_request(request).await {
                Ok(resp) => {
                    let _ = ui_tx_inner.send(ProxyToUi::MacroResponse(step_idx, resp));
                }
                Err(e) => {
                    let _ = ui_tx_inner.send(ProxyToUi::MacroError(step_idx, e));
                }
            }
        });
    }

    pub(super) fn export_to_file(&mut self, prefix: &str, ext: &str, content: &str) {
        let dir = dirs::home_dir()
            .unwrap_or_default()
            .join(".crowbar")
            .join("exports");
        if std::fs::create_dir_all(&dir).is_err() {
            self.set_status("Failed to create exports directory");
            return;
        }
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let path = dir.join(format!("{}-{}.{}", prefix, ts, ext));
        match std::fs::write(&path, content) {
            Ok(_) => {
                self.set_status(format!("Exported to {}", path.display()));
            }
            Err(e) => {
                self.set_status(format!("Export failed: {}", e));
            }
        }
    }

    pub fn handle_proxy_message(&mut self, msg: ProxyToUi) {
        match msg {
            ProxyToUi::RequestCaptured(req) => {
                self.store.insert(req);
                if !self.history.detail_open && self.store.len() > 1 {
                    self.history.selected = self.store.len() - 1;
                }
            }
            ProxyToUi::ResponseReceived(id, resp) => {
                if let Some(entry) = self.store.get(id) {
                    let findings = crate::scanning::scan_response(&entry.request, &resp);
                    self.store.update_response(id, resp);
                    if !findings.is_empty() {
                        self.store.set_findings(id, findings);
                    }
                } else {
                    self.store.update_response(id, resp);
                }
            }
            ProxyToUi::RequestError(id, err) => {
                self.store.mark_error(id, err);
            }
            ProxyToUi::InterceptedRequest(req) => {
                self.intercept_ui.queue.push_back(req);
            }
            ProxyToUi::RepeaterResponse(resp) => {
                self.repeater.pending = false;
                self.repeater.response = Some(resp);
                self.repeater.error = None;
            }
            ProxyToUi::RepeaterError(err) => {
                self.repeater.pending = false;
                self.repeater.error = Some(err);
            }
            ProxyToUi::WebSocketFrame(id, msg) => {
                self.store.push_ws_message(id, msg);
            }
            ProxyToUi::GrpcFrame(id, msg) => {
                self.store.push_grpc_message(id, msg);
            }
            ProxyToUi::GrpcTrailers(id, trailers) => {
                self.store.update_trailers(id, trailers);
            }
            ProxyToUi::MacroResponse(step_idx, resp) => {
                if step_idx < self.macros.steps.len() {
                    self.macros.steps[step_idx].response = Some(resp);
                    self.macros.steps[step_idx].state = StepState::Complete;
                    self.macros.current_step = step_idx + 1;
                    self.macro_send_next();
                }
            }
            ProxyToUi::MacroError(step_idx, err) => {
                if step_idx < self.macros.steps.len() {
                    self.macros.steps[step_idx].error = Some(err);
                    self.macros.steps[step_idx].state = StepState::Error;
                    self.macros.running = false;
                    self.set_status(format!("Macro stopped at step {} (error)", step_idx + 1));
                }
            }
        }
    }
}
