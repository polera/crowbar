use crossterm::event::{KeyCode, KeyEvent};

use crate::http::sequence::SequenceStep;

use super::App;

impl App {
    pub(super) fn handle_quit_confirm_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                if !self.store.is_empty() {
                    self.show_quit_confirm = false;
                    self.open_save_dialog(true);
                } else {
                    self.should_quit = true;
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                self.should_quit = true;
            }
            KeyCode::Esc => {
                self.show_quit_confirm = false;
            }
            _ => {}
        }
    }

    pub(super) fn open_save_dialog(&mut self, quit_after: bool) {
        let name = crate::http::session::auto_save_name();
        if let Ok(dir) = crate::http::session::sessions_dir() {
            self.save_buffer = dir.join(format!("{}.json", name)).display().to_string();
        } else {
            self.save_buffer = format!("{}.json", name);
        }
        self.save_on_confirm_quit = quit_after;
        self.show_save_dialog = true;
    }

    pub(super) fn handle_save_dialog_key(&mut self, key: KeyEvent) {
        if self.saving_session {
            return;
        }
        match key.code {
            KeyCode::Esc => {
                self.show_save_dialog = false;
                self.save_buffer.clear();
                self.save_on_confirm_quit = false;
            }
            KeyCode::Enter => {
                let path = std::path::PathBuf::from(self.save_buffer.trim());
                let quit_after = self.save_on_confirm_quit;
                self.save_session_to(path, quit_after);
            }
            KeyCode::Char(c) => {
                self.save_buffer.push(c);
            }
            KeyCode::Backspace => {
                self.save_buffer.pop();
            }
            _ => {}
        }
    }

    fn save_session_to(&mut self, path: std::path::PathBuf, quit_after: bool) {
        let macro_requests: Vec<_> = self
            .macros
            .steps
            .iter()
            .map(|s| s.request.clone())
            .collect();
        let session =
            crate::http::session::Session::new(self.store.entries().to_vec(), macro_requests);
        self.saving_session = true;
        let ui_tx = self.ui_tx.clone();
        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                crate::http::session::save_to(&session, &path)?;
                Ok::<_, anyhow::Error>(path)
            })
            .await;
            let result = match result {
                Ok(result) => result.map_err(|error| format!("{error:#}")),
                Err(error) => Err(error.to_string()),
            };
            let _ = ui_tx
                .send(crate::channel::ProxyToUi::SessionSaved(result, quit_after))
                .await;
        });
    }

    pub(super) fn export_rules(&mut self) {
        let rules = self.rules.read().clone();
        if rules.is_empty() {
            self.set_status("No rules to export");
            return;
        }
        let name = crate::rules::persist::auto_save_name();
        match crate::rules::persist::save(&rules, &name) {
            Ok(path) => {
                self.set_status(format!(
                    "Exported {} rules to {}",
                    rules.len(),
                    path.display()
                ));
            }
            Err(e) => {
                self.set_status(format!("Export failed: {}", e));
            }
        }
    }

    pub fn load_session(&mut self, path: &std::path::Path) {
        match crate::http::import::load_file(path) {
            Ok(session) => {
                if session.macros.as_ref().is_some_and(|saved| {
                    saved
                        .steps
                        .iter()
                        .any(|request| request.body.len() > self.proxy_limits.max_body_bytes)
                }) {
                    self.set_status("Load failed: macro request body exceeds configured limit");
                    return;
                }
                if let Err(error) = self.store.load_entries(session.entries) {
                    self.set_status(format!("Load failed: {error}"));
                    return;
                }
                self.macros.steps = session
                    .macros
                    .map(|saved| {
                        saved
                            .steps
                            .into_iter()
                            .map(|mut request| {
                                request.id = crate::http::models::RequestId::next();
                                SequenceStep::new(request)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.macros.selected = 0;
                self.macros.running = false;
                self.macros.run_id = None;
                self.macros.current_step = 0;
                let macro_count = self.macros.steps.len();
                let msg = if macro_count > 0 {
                    format!(
                        "Loaded {} entries, {} macro steps",
                        self.store.len(),
                        macro_count
                    )
                } else {
                    format!("Loaded {} entries", self.store.len())
                };
                self.set_status(msg);
            }
            Err(e) => {
                self.set_status(format!("Load failed: {}", e));
            }
        }
    }

    pub(super) fn handle_cert_overlay_key(&mut self, key: KeyEvent) {
        if self.cert_export_editing {
            match key.code {
                KeyCode::Esc => {
                    self.cert_export_editing = false;
                    self.cert_export_buffer.clear();
                }
                KeyCode::Enter => {
                    let path = self.cert_export_buffer.trim().to_string();
                    self.cert_export_editing = false;
                    self.cert_export_buffer.clear();
                    self.show_cert_info = false;
                    self.export_ca_cert(Some(&path));
                }
                KeyCode::Char(c) => {
                    self.cert_export_buffer.push(c);
                }
                KeyCode::Backspace => {
                    self.cert_export_buffer.pop();
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.show_cert_info = false;
            }
            KeyCode::Char('s') => {
                self.show_cert_info = false;
                self.export_ca_cert(None);
            }
            KeyCode::Char('p') => {
                self.cert_export_editing = true;
                self.cert_export_buffer = String::from("crowbar-ca.pem");
            }
            _ => {}
        }
    }

    fn export_ca_cert(&mut self, path: Option<&str>) {
        let ca_dir = match dirs::home_dir() {
            Some(h) => h.join(".crowbar"),
            None => {
                self.set_status("Cannot find home directory");
                return;
            }
        };
        let cert_path = ca_dir.join("ca.pem");

        if !cert_path.exists() {
            self.set_status("No CA certificate found — restart crowbar to generate one");
            return;
        }

        let pem = match std::fs::read_to_string(&cert_path) {
            Ok(p) => p,
            Err(e) => {
                self.set_status(format!("Failed to read CA cert: {}", e));
                return;
            }
        };

        let dest = path.unwrap_or("crowbar-ca.pem");
        match std::fs::write(dest, &pem) {
            Ok(_) => {
                let abs = std::path::Path::new(dest)
                    .canonicalize()
                    .unwrap_or_else(|_| std::path::PathBuf::from(dest));
                self.set_status(format!("CA certificate exported to {}", abs.display()));
            }
            Err(e) => {
                self.set_status(format!("Export failed: {}", e));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn failed_save_keeps_dialog_and_does_not_quit() {
        let (mut app, mut receiver) = crate::test_support::app();
        let directory = tempfile::tempdir().unwrap();
        app.show_save_dialog = true;
        app.save_on_confirm_quit = true;
        app.save_buffer = directory.path().display().to_string();
        let original_path = app.save_buffer.clone();
        app.handle_save_dialog_key(KeyEvent::new(
            KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        app.handle_proxy_message(receiver.recv().await.unwrap());
        assert!(!app.should_quit);
        assert!(app.show_save_dialog);
        assert_eq!(app.save_buffer, original_path);
        assert!(!app.saving_session);
    }

    #[tokio::test]
    async fn successful_save_can_quit_and_is_loadable() {
        let (mut app, mut receiver) = crate::test_support::app();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        app.store.insert(crate::test_support::request());
        app.show_save_dialog = true;
        app.save_on_confirm_quit = true;
        app.save_buffer = path.display().to_string();
        app.handle_save_dialog_key(KeyEvent::new(
            KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        app.handle_proxy_message(receiver.recv().await.unwrap());
        assert!(app.should_quit);
        assert!(!app.show_save_dialog);
        assert_eq!(crate::http::session::load(&path).unwrap().entries.len(), 1);
    }
}
