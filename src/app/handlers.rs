use std::net::SocketAddr;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::editor::{EditorAction, EditorMode, TextEditor};
use crate::http::codec;
use crate::http::sequence::SequenceStep;
use crate::proxy::intercept::InterceptDecision;

use super::{App, EditorTarget, RuleField, ToolsMode};

impl App {
    pub(super) fn handle_proxy_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('i') => {
                let now_enabled = self.intercept_state.toggle();
                if !now_enabled {
                    self.intercept_state.forward_all();
                    self.intercept_ui.queue.clear();
                }
            }
            KeyCode::Char('f') => {
                if let Some(req) = self.intercept_ui.queue.pop_front() {
                    self.intercept_state
                        .resolve(req.id, InterceptDecision::Forward);
                    self.intercept_ui.scroll = 0;
                }
            }
            KeyCode::Char('d') => {
                if let Some(req) = self.intercept_ui.queue.pop_front() {
                    self.store.mark_dropped(req.id);
                    self.intercept_state
                        .resolve(req.id, InterceptDecision::Drop);
                    self.intercept_ui.scroll = 0;
                }
            }
            KeyCode::Char('e') => {
                if let Some(req) = self.intercept_ui.queue.front() {
                    self.intercept_ui.editor =
                        TextEditor::new(codec::request_to_lines(req), self.editor_mode);
                    self.intercept_ui.editing = true;
                    self.intercept_ui.scroll = 0;
                }
            }
            KeyCode::Char('b') => {
                self.bind_addr_buffer = self.bind_addr.to_string();
                self.editing_bind_addr = true;
            }
            KeyCode::Char('s') => {
                self.scope_buffer = self.scope.patterns().join(", ");
                self.editing_scope = true;
            }
            KeyCode::Char('C') => {
                self.show_cert_info = true;
                self.cert_export_editing = false;
                self.cert_export_buffer.clear();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.intercept_ui.scroll = self.intercept_ui.scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.intercept_ui.scroll += 1;
            }
            _ => {}
        }
    }

    pub(super) fn handle_bind_addr_editor_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.editing_bind_addr = false;
                self.bind_addr_buffer.clear();
            }
            KeyCode::Enter => {
                let input = self.bind_addr_buffer.trim().to_string();
                let parsed = input
                    .parse::<SocketAddr>()
                    .or_else(|_| format!("127.0.0.1:{}", input).parse::<SocketAddr>());
                match parsed {
                    Ok(addr) if addr.ip().is_loopback() || self.allow_remote => {
                        self.pending_rebind = Some(addr);
                        self.editing_bind_addr = false;
                        self.bind_addr_buffer.clear();
                    }
                    Ok(addr) => {
                        self.set_status(format!(
                            "Refusing remote bind {} without --allow-remote",
                            addr
                        ));
                        self.editing_bind_addr = false;
                        self.bind_addr_buffer.clear();
                    }
                    Err(_) => {
                        self.set_status(format!("Invalid address: {}", input));
                        self.editing_bind_addr = false;
                        self.bind_addr_buffer.clear();
                    }
                }
            }
            KeyCode::Char(c) => {
                self.bind_addr_buffer.push(c);
            }
            KeyCode::Backspace => {
                self.bind_addr_buffer.pop();
            }
            _ => {}
        }
    }

    pub(super) fn handle_scope_editor_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.editing_scope = false;
                self.scope_buffer.clear();
            }
            KeyCode::Enter => {
                let patterns: Vec<String> = self
                    .scope_buffer
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                let count = patterns.len();
                self.scope.set_patterns(patterns);
                self.editing_scope = false;
                self.scope_buffer.clear();
                self.set_status(if count == 0 {
                    "Scope cleared — capturing all traffic".to_string()
                } else {
                    format!(
                        "Scope updated ({} pattern{})",
                        count,
                        if count == 1 { "" } else { "s" }
                    )
                });
            }
            KeyCode::Char(c) => {
                self.scope_buffer.push(c);
            }
            KeyCode::Backspace => {
                self.scope_buffer.pop();
            }
            _ => {}
        }
    }

    pub(super) fn handle_history_key(&mut self, key: KeyEvent) {
        self.store.refresh_filter_cache(&self.history.filter);
        let entry_count = self.store.filtered_count();

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.history.detail_open {
                    self.history.scroll = self.history.scroll.saturating_sub(1);
                } else if self.history.selected > 0 {
                    self.history.selected -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.history.detail_open {
                    self.history.scroll += 1;
                } else if entry_count > 0 && self.history.selected < entry_count - 1 {
                    self.history.selected += 1;
                }
            }
            KeyCode::Home | KeyCode::Char('g') if !self.history.detail_open => {
                self.history.selected = 0;
            }
            KeyCode::End | KeyCode::Char('G') if !self.history.detail_open && entry_count > 0 => {
                self.history.selected = entry_count - 1;
            }
            KeyCode::Enter => {
                if self.history.detail_open {
                    self.history.detail_open = false;
                    self.history.scroll = 0;
                } else if entry_count > 0 {
                    self.history.detail_open = true;
                    self.history.scroll = 0;
                }
            }
            KeyCode::Esc if self.history.detail_open => {
                self.history.detail_open = false;
                self.history.scroll = 0;
            }
            KeyCode::Char('r') if entry_count > 0 => {
                self.send_to_repeater();
            }
            KeyCode::Char('/') if !self.history.detail_open => {
                self.history.filtering = true;
            }
            KeyCode::Char('c') => {
                if entry_count > 0
                    && let Some(entry) = self.store.filtered_entry(self.history.selected)
                {
                    let curl = crate::http::export::to_curl(entry);
                    self.export_to_file("curl", "sh", &curl);
                }
            }
            KeyCode::Char('w') => {
                if entry_count > 0
                    && let Some(entry) = self.store.filtered_entry(self.history.selected)
                {
                    let raw = crate::http::export::to_raw(entry);
                    self.export_to_file("raw", "txt", &raw);
                }
            }
            KeyCode::Char('h') if !self.history.detail_open => {
                let entries: Vec<_> = self.store.filtered_entries_iter().cloned().collect();
                let har = crate::http::export::to_har(&entries);
                self.export_to_file("har", "har", &har);
            }
            KeyCode::Char('m') => {
                if entry_count > 0
                    && let Some(entry) = self.store.filtered_entry(self.history.selected)
                {
                    self.macros
                        .steps
                        .push(SequenceStep::new(entry.request.clone()));
                    self.set_status(format!(
                        "Added to macro ({} steps)",
                        self.macros.steps.len()
                    ));
                }
            }
            _ => {}
        }
    }

    pub(super) fn handle_rules_key(&mut self, key: KeyEvent) {
        let count = self.rules.read().len();

        match key.code {
            KeyCode::Char('a') => {
                let mut rules = self.rules.write();
                let name = format!("Rule {}", rules.len() + 1);
                rules.push(crate::rules::Rule::new(name));
                self.rules_ui.selected = rules.len() - 1;
            }
            KeyCode::Char('x') if count > 0 => {
                let mut rules = self.rules.write();
                rules.remove(self.rules_ui.selected);
                if self.rules_ui.selected >= rules.len() && !rules.is_empty() {
                    self.rules_ui.selected = rules.len() - 1;
                }
            }
            KeyCode::Enter if count > 0 => {
                let mut rules = self.rules.write();
                rules[self.rules_ui.selected].enabled = !rules[self.rules_ui.selected].enabled;
            }
            KeyCode::Char('t') if count > 0 => {
                let mut rules = self.rules.write();
                rules[self.rules_ui.selected].target = rules[self.rules_ui.selected].target.next();
            }
            KeyCode::Char('s') if count > 0 => {
                let mut rules = self.rules.write();
                rules[self.rules_ui.selected].scope = rules[self.rules_ui.selected].scope.next();
            }
            KeyCode::Char('R') if count > 0 => {
                let mut rules = self.rules.write();
                rules[self.rules_ui.selected].is_regex = !rules[self.rules_ui.selected].is_regex;
                rules[self.rules_ui.selected].invalidate_regex();
            }
            KeyCode::Char('n') if count > 0 => {
                self.rules_ui.edit_buffer = {
                    let rules = self.rules.read();
                    rules[self.rules_ui.selected].name.clone()
                };
                self.rules_ui.editing_field = Some(RuleField::Name);
            }
            KeyCode::Char('p') if count > 0 => {
                self.rules_ui.edit_buffer = {
                    let rules = self.rules.read();
                    rules[self.rules_ui.selected].match_pattern.clone()
                };
                self.rules_ui.editing_field = Some(RuleField::Pattern);
            }
            KeyCode::Char('e') if count > 0 => {
                self.rules_ui.edit_buffer = {
                    let rules = self.rules.read();
                    rules[self.rules_ui.selected].replacement.clone()
                };
                self.rules_ui.editing_field = Some(RuleField::Replacement);
            }
            KeyCode::Up | KeyCode::Char('k') if self.rules_ui.selected > 0 => {
                self.rules_ui.selected -= 1;
            }
            KeyCode::Down | KeyCode::Char('j')
                if count > 0 && self.rules_ui.selected < count - 1 =>
            {
                self.rules_ui.selected += 1;
            }
            KeyCode::Char('E') => {
                self.export_rules();
            }
            KeyCode::Char('I') => {
                self.rules_ui.importing = true;
                self.rules_ui.import_buffer.clear();
            }
            _ => {}
        }
    }

    pub(super) fn handle_rules_import_editor_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.rules_ui.importing = false;
                self.rules_ui.import_buffer.clear();
            }
            KeyCode::Enter => {
                let raw = &self.rules_ui.import_buffer;
                let expanded = match raw.strip_prefix("~/") {
                    Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
                    None => std::path::PathBuf::from(raw),
                };
                match crate::rules::persist::load(&expanded) {
                    Ok(imported) => {
                        let count = imported.len();
                        self.rules.write().extend(imported);
                        self.set_status(format!(
                            "Imported {} rules from {}",
                            count,
                            expanded.display()
                        ));
                    }
                    Err(e) => {
                        self.set_status(format!("Import failed: {}", e));
                    }
                }
                self.rules_ui.importing = false;
                self.rules_ui.import_buffer.clear();
            }
            KeyCode::Char(c) => {
                self.rules_ui.import_buffer.push(c);
            }
            KeyCode::Backspace => {
                self.rules_ui.import_buffer.pop();
            }
            _ => {}
        }
    }

    pub(super) fn handle_rules_editor_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.rules_ui.editing_field = None;
                self.rules_ui.edit_buffer.clear();
            }
            KeyCode::Enter => {
                if let Some(field) = self.rules_ui.editing_field {
                    let mut rules = self.rules.write();
                    if self.rules_ui.selected < rules.len() {
                        match field {
                            RuleField::Name => {
                                rules[self.rules_ui.selected].name =
                                    self.rules_ui.edit_buffer.clone();
                            }
                            RuleField::Pattern => {
                                rules[self.rules_ui.selected].match_pattern =
                                    self.rules_ui.edit_buffer.clone();
                                rules[self.rules_ui.selected].invalidate_regex();
                            }
                            RuleField::Replacement => {
                                rules[self.rules_ui.selected].replacement =
                                    self.rules_ui.edit_buffer.clone();
                            }
                        }
                    }
                }
                self.rules_ui.editing_field = None;
                self.rules_ui.edit_buffer.clear();
            }
            KeyCode::Char(c) => {
                self.rules_ui.edit_buffer.push(c);
            }
            KeyCode::Backspace => {
                self.rules_ui.edit_buffer.pop();
            }
            _ => {}
        }
    }

    pub(super) fn handle_tools_key(&mut self, key: KeyEvent) {
        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Char('e')) => {
                self.tools.editing = true;
                self.tools.editor.cursor_line = 0;
                self.tools.editor.cursor_col = 0;
                if self.editor_mode == EditorMode::Vim {
                    self.tools.editor.vim_mode = crate::editor::VimMode::Insert;
                }
            }
            (KeyModifiers::NONE, KeyCode::Right | KeyCode::Char('l')) => {
                self.tools.mode = self.tools.mode.next();
            }
            (KeyModifiers::NONE, KeyCode::Left | KeyCode::Char('h')) => {
                self.tools.mode = self.tools.mode.prev();
            }
            (KeyModifiers::NONE, KeyCode::Char('j') | KeyCode::Down) => {
                self.tools.scroll += 1;
            }
            (KeyModifiers::NONE, KeyCode::Char('k') | KeyCode::Up) => {
                self.tools.scroll = self.tools.scroll.saturating_sub(1);
            }
            (KeyModifiers::CONTROL, KeyCode::Char('y')) => {
                let output = self.tools_output();
                match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(output)) {
                    Ok(()) => {
                        self.set_status("Copied to clipboard");
                    }
                    Err(e) => {
                        self.set_status(format!("Clipboard error: {}", e));
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn handle_tools_editor_key(&mut self, key: KeyEvent) {
        match self.tools.editor.handle_key(key) {
            EditorAction::Consumed => {}
            EditorAction::ExitEditor => {
                self.tools.editing = false;
            }
            EditorAction::Enter => {
                self.tools.editor.insert_newline();
            }
            EditorAction::CtrlEnter => {}
            EditorAction::Custom(k) => {
                if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('u') {
                    self.tools.editor.clear();
                }
            }
        }
    }

    pub fn tools_input_text(&self) -> String {
        self.tools.editor.lines.join("\n")
    }

    pub fn tools_output(&self) -> String {
        use base64::Engine;
        let input = self.tools_input_text();
        match self.tools.mode {
            ToolsMode::UrlEncode => super::encode::url_encode(&input),
            ToolsMode::UrlDecode => crate::http::url_decode(&input),
            ToolsMode::Base64Encode => {
                base64::engine::general_purpose::STANDARD.encode(input.as_bytes())
            }
            ToolsMode::Base64Decode => {
                match base64::engine::general_purpose::STANDARD.decode(input.trim()) {
                    Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                    Err(e) => format!("Error: {}", e),
                }
            }
            ToolsMode::HexEncode => super::encode::hex_encode(input.as_bytes()),
            ToolsMode::HexDecode => match super::encode::hex_decode(input.trim()) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(e) => format!("Error: {}", e),
            },
        }
    }

    pub(super) fn handle_filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.history.filtering = false;
                self.history.filter.clear();
                self.history.selected = 0;
            }
            KeyCode::Enter => {
                self.history.filtering = false;
                self.history.selected = 0;
            }
            KeyCode::Backspace => {
                self.history.filter.pop();
                self.history.selected = 0;
            }
            KeyCode::Char(c) => {
                self.history.filter.push(c);
                self.history.selected = 0;
            }
            _ => {}
        }
    }

    pub(super) fn handle_repeater_key(&mut self, key: KeyEvent) {
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Enter) => {
                if self.macros.show {
                    self.macro_run();
                } else {
                    self.repeater_send();
                }
            }
            (KeyModifiers::NONE, KeyCode::Char('e')) if self.repeater.editor.has_content() => {
                self.repeater.editing = true;
                self.repeater.editor.cursor_line = 0;
                self.repeater.editor.cursor_col = 0;
                if self.editor_mode == EditorMode::Vim {
                    self.repeater.editor.vim_mode = crate::editor::VimMode::Insert;
                }
            }
            (KeyModifiers::NONE, KeyCode::Char('d'))
                if !self.macros.show && self.repeater.original.is_some() =>
            {
                self.repeater.show_diff = !self.repeater.show_diff;
            }
            (KeyModifiers::NONE, KeyCode::Enter)
                if self.macros.show && !self.macros.steps.is_empty() && !self.macros.running =>
            {
                self.load_macro_step(true);
            }
            (KeyModifiers::NONE, KeyCode::Char('e'))
                if self.macros.show && !self.macros.steps.is_empty() && !self.macros.running =>
            {
                self.load_macro_step(false);
            }
            (KeyModifiers::SHIFT, KeyCode::Char('M')) => {
                self.macros.show = !self.macros.show;
            }
            (KeyModifiers::NONE, KeyCode::Char('j') | KeyCode::Down) => {
                if self.macros.show {
                    if !self.macros.steps.is_empty()
                        && self.macros.selected < self.macros.steps.len() - 1
                    {
                        self.macros.selected += 1;
                    }
                } else {
                    self.repeater.req_scroll += 1;
                }
            }
            (KeyModifiers::NONE, KeyCode::Char('k') | KeyCode::Up) => {
                if self.macros.show {
                    if self.macros.selected > 0 {
                        self.macros.selected -= 1;
                    }
                } else {
                    self.repeater.req_scroll = self.repeater.req_scroll.saturating_sub(1);
                }
            }
            (KeyModifiers::SHIFT, KeyCode::Char('J')) => {
                self.repeater.resp_scroll += 1;
            }
            (KeyModifiers::SHIFT, KeyCode::Char('K')) => {
                self.repeater.resp_scroll = self.repeater.resp_scroll.saturating_sub(1);
            }
            (KeyModifiers::NONE, KeyCode::Char('x'))
                if self.macros.show && !self.macros.steps.is_empty() && !self.macros.running =>
            {
                self.macros.steps.remove(self.macros.selected);
                if self.macros.selected >= self.macros.steps.len() && !self.macros.steps.is_empty()
                {
                    self.macros.selected = self.macros.steps.len() - 1;
                }
            }
            (KeyModifiers::NONE, KeyCode::Char('X'))
            | (KeyModifiers::SHIFT, KeyCode::Char('X'))
                if self.macros.show && !self.macros.running =>
            {
                self.macros.steps.clear();
                self.macros.selected = 0;
            }
            _ => {}
        }
    }

    pub(super) fn handle_editor_key(&mut self, key: KeyEvent, target: EditorTarget) {
        let editor = match target {
            EditorTarget::Intercept => &mut self.intercept_ui.editor,
            EditorTarget::Repeater => &mut self.repeater.editor,
        };

        let action = editor.handle_key(key);

        match action {
            EditorAction::Consumed => {}
            EditorAction::ExitEditor => match target {
                EditorTarget::Intercept => {
                    self.intercept_ui.editing = false;
                    self.intercept_ui.editor = TextEditor::new(vec![], self.editor_mode);
                }
                EditorTarget::Repeater => {
                    self.repeater.editing = false;
                }
            },
            EditorAction::Enter => match target {
                EditorTarget::Intercept => self.forward_edited_intercept(),
                EditorTarget::Repeater => {
                    self.repeater.editor.insert_newline();
                }
            },
            EditorAction::CtrlEnter => match target {
                EditorTarget::Intercept => self.forward_edited_intercept(),
                EditorTarget::Repeater => {
                    self.repeater.editing = false;
                    self.repeater_send();
                }
            },
            EditorAction::Custom(_) => {}
        }
    }
}
