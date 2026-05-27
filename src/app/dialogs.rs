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
        match key.code {
            KeyCode::Esc => {
                self.show_save_dialog = false;
                self.save_buffer.clear();
                self.save_on_confirm_quit = false;
            }
            KeyCode::Enter => {
                let path = std::path::PathBuf::from(self.save_buffer.trim());
                self.show_save_dialog = false;
                self.save_buffer.clear();
                let quit_after = self.save_on_confirm_quit;
                self.save_on_confirm_quit = false;
                self.save_session_to(&path);
                if quit_after {
                    self.should_quit = true;
                }
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

    fn save_session_to(&mut self, path: &std::path::Path) {
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            self.set_status(format!("Save failed: {}", e));
            return;
        }
        let macro_requests: Vec<_> = self.macros.steps.iter().map(|s| s.request.clone()).collect();
        let session = crate::http::session::Session::new(self.store.entries().to_vec(), macro_requests);
        match std::fs::File::create(path) {
            Ok(file) => {
                let writer = std::io::BufWriter::new(file);
                match serde_json::to_writer_pretty(writer, &session) {
                    Ok(()) => {
                        self.set_status(format!("Saved to {}", path.display()));
                    }
                    Err(e) => {
                        self.set_status(format!("Save failed: {}", e));
                    }
                }
            }
            Err(e) => {
                self.set_status(format!("Save failed: {}", e));
            }
        }
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
                self.set_status(format!("Exported {} rules to {}", rules.len(), path.display()));
            }
            Err(e) => {
                self.set_status(format!("Export failed: {}", e));
            }
        }
    }

    pub fn load_session(&mut self, path: &std::path::Path) {
        match crate::http::import::load_file(path) {
            Ok(session) => {
                self.store.load_entries(session.entries);
                if let Some(saved) = session.macros {
                    self.macros.steps = saved
                        .steps
                        .into_iter()
                        .map(SequenceStep::new)
                        .collect();
                    self.macros.selected = 0;
                    self.macros.running = false;
                    self.macros.current_step = 0;
                }
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
