use crate::channel::ProxyToUi;
use crate::editor::TextEditor;
use crate::http::codec;
use crate::http::models::{RequestData, RequestId};
use crate::http::sequence::StepState;
use crate::proxy::intercept::InterceptDecision;
use crate::proxy::repeater;
use crate::tui::tabs::Tab;

use super::{App, RequestState};

impl App {
    pub(super) fn set_status(&mut self, msg: impl Into<String>) {
        self.status_message = Some((msg.into(), std::time::Instant::now()));
    }

    pub(super) fn forward_edited_intercept(&mut self) {
        if let Some(original) = self.intercept_ui.queue.front() {
            let edited = match codec::lines_to_request(&self.intercept_ui.editor.lines, original) {
                Ok(edited) => edited,
                Err(error) => {
                    self.set_status(error.to_string());
                    return;
                }
            };
            self.intercept_state
                .resolve(original.id, InterceptDecision::ForwardEdited(edited));
            self.intercept_ui.queue.pop_front();
        }
        self.intercept_ui.editing = false;
        self.intercept_ui.editor = TextEditor::new(vec![], self.editor_mode);
        self.intercept_ui.scroll = 0;
    }

    pub(super) fn send_to_repeater(&mut self) {
        self.store.refresh_filter_cache(&self.history.filter);
        if let Some(entry) = self.store.filtered_entry(self.history.selected) {
            if entry.request_body_truncated {
                self.set_status("Cannot replay a truncated request capture");
                return;
            }
            let req = &entry.request;
            self.repeater.editor = TextEditor::new(codec::request_to_lines(req), self.editor_mode);
            self.repeater.original = Some(req.clone());
            self.repeater.operation = RequestState::Idle;
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
            self.repeater.operation = RequestState::Idle;
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
        if self.repeater.editor.lines.is_empty() || self.repeater.operation.is_pending() {
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

        let request = match codec::lines_to_request(&self.repeater.editor.lines, &original) {
            Ok(request) => request,
            Err(error) => {
                self.set_status(error.to_string());
                return;
            }
        };
        let operation_id = RequestId::next();
        self.repeater.operation = RequestState::Sending(operation_id);
        self.repeater.resp_scroll = 0;

        let ui_tx = self.ui_tx.clone();
        let limits = self.proxy_limits;
        tokio::spawn(async move {
            let result = repeater::send_raw_request(request, limits)
                .await
                .map_err(|error| format!("{error:#}"));
            let _ = ui_tx
                .send(ProxyToUi::RepeaterFinished(operation_id, result))
                .await;
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
        self.macros.run_id = Some(RequestId::next());
        self.macros.current_step = 0;
        self.macro_send_next();
    }

    fn macro_send_next(&mut self) {
        if self.macros.current_step >= self.macros.steps.len() {
            self.macros.running = false;
            self.macros.run_id = None;
            self.set_status(format!(
                "Macro complete ({} steps)",
                self.macros.steps.len()
            ));
            return;
        }

        self.macros.steps[self.macros.current_step].state = StepState::Running;
        let request = self.macros.steps[self.macros.current_step].request.clone();
        let step_idx = self.macros.current_step;
        let ui_tx = self.ui_tx.clone();
        let Some(run_id) = self.macros.run_id else {
            return;
        };
        let limits = self.proxy_limits;

        tokio::spawn(async move {
            let result = repeater::send_raw_request(request, limits)
                .await
                .map_err(|error| format!("{error:#}"));
            let _ = ui_tx
                .send(ProxyToUi::MacroFinished(run_id, step_idx, result))
                .await;
        });
    }

    pub(super) fn export_to_file(&mut self, prefix: &str, ext: &str, content: &str) {
        let dir = dirs::home_dir()
            .unwrap_or_default()
            .join(".crowbar")
            .join("exports");
        if crate::fs_security::ensure_private_dir(&dir).is_err() {
            self.set_status("Failed to create exports directory");
            return;
        }
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let path = dir.join(format!("{}-{}.{}", prefix, ts, ext));
        match crate::fs_security::write_private(&path, content) {
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
            ProxyToUi::SessionSaved(result, quit_after) => {
                self.saving_session = false;
                match result {
                    Ok(path) => {
                        self.show_save_dialog = false;
                        self.save_buffer.clear();
                        self.save_on_confirm_quit = false;
                        self.should_quit = quit_after;
                        self.set_status(format!("Saved to {}", path.display()));
                    }
                    Err(error) => self.set_status(format!("Save failed: {error}")),
                }
            }
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
                if self.intercept_state.is_pending(req.id) {
                    self.intercept_ui.queue.push_back(req);
                }
            }
            ProxyToUi::RepeaterFinished(id, result) => {
                if matches!(self.repeater.operation, RequestState::Sending(current) if current == id)
                {
                    self.repeater.operation = match result {
                        Ok(response) => RequestState::Complete(response),
                        Err(error) => RequestState::Failed(error),
                    };
                }
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
            ProxyToUi::RequestCaptureTruncated(id) => self.store.mark_request_capture_truncated(id),
            ProxyToUi::MacroFinished(run_id, step_idx, result) => {
                if self.macros.run_id != Some(run_id)
                    || !self.macros.running
                    || step_idx != self.macros.current_step
                {
                    return;
                }
                if let Some(step) = self.macros.steps.get_mut(step_idx) {
                    match result {
                        Ok(response) => {
                            step.response = Some(response);
                            step.state = StepState::Complete;
                            self.macros.current_step += 1;
                            self.macro_send_next();
                        }
                        Err(error) => {
                            step.error = Some(error);
                            step.state = StepState::Error;
                            self.macros.running = false;
                            self.macros.run_id = None;
                            self.set_status(format!(
                                "Macro stopped at step {} (error)",
                                step_idx + 1
                            ));
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_repeater_completion_cannot_replace_current_operation() {
        let (mut app, _receiver) = crate::test_support::app();
        let old = RequestId::next();
        let current = RequestId::next();
        app.repeater.operation = RequestState::Sending(current);
        app.handle_proxy_message(ProxyToUi::RepeaterFinished(
            old,
            Ok(crate::test_support::response()),
        ));
        assert!(matches!(app.repeater.operation, RequestState::Sending(id) if id == current));
        app.handle_proxy_message(ProxyToUi::RepeaterFinished(current, Err("failed".into())));
        assert_eq!(app.repeater.operation.error(), Some("failed"));
    }
}
