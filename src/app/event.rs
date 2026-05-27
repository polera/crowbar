use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use crate::tui::tabs::Tab;

use super::{App, EditorTarget};

impl App {
    pub fn handle_event(&mut self, event: Event) {
        if let Event::Key(key) = event {
            if key.kind != crossterm::event::KeyEventKind::Press {
                return;
            }

            if self.show_help {
                self.show_help = false;
                return;
            }

            if self.show_quit_confirm {
                self.handle_quit_confirm_key(key);
                return;
            }

            if self.show_save_dialog {
                self.handle_save_dialog_key(key);
                return;
            }

            if self.show_cert_info {
                self.handle_cert_overlay_key(key);
                return;
            }

            if self.intercept_ui.editing {
                self.handle_editor_key(key, EditorTarget::Intercept);
                return;
            }

            if self.repeater.editing {
                self.handle_editor_key(key, EditorTarget::Repeater);
                return;
            }

            if self.history.filtering {
                self.handle_filter_key(key);
                return;
            }

            if self.tools.editing {
                self.handle_tools_editor_key(key);
                return;
            }

            if self.editing_bind_addr {
                self.handle_bind_addr_editor_key(key);
                return;
            }

            if self.editing_scope {
                self.handle_scope_editor_key(key);
                return;
            }

            if self.rules_ui.editing_field.is_some() {
                self.handle_rules_editor_key(key);
                return;
            }

            if self.rules_ui.importing {
                self.handle_rules_import_editor_key(key);
                return;
            }

            if self.handle_global_key(key) {
                return;
            }
            match self.active_tab {
                Tab::History => self.handle_history_key(key),
                Tab::Proxy => self.handle_proxy_key(key),
                Tab::Repeater => self.handle_repeater_key(key),
                Tab::Rules => self.handle_rules_key(key),
                Tab::Tools => self.handle_tools_key(key),
            }
        }
    }

    fn handle_global_key(&mut self, key: KeyEvent) -> bool {
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                self.show_quit_confirm = true;
                true
            }
            (KeyModifiers::NONE, KeyCode::Char('q')) => {
                if !self.history.detail_open {
                    self.show_quit_confirm = true;
                    return true;
                }
                false
            }
            (KeyModifiers::NONE, KeyCode::Tab) => {
                self.active_tab = self.active_tab.next();
                true
            }
            (KeyModifiers::SHIFT, KeyCode::BackTab) => {
                self.active_tab = self.active_tab.prev();
                true
            }
            (_, KeyCode::Char('1')) => {
                self.active_tab = Tab::Proxy;
                true
            }
            (_, KeyCode::Char('2')) => {
                self.active_tab = Tab::History;
                true
            }
            (_, KeyCode::Char('3')) => {
                self.active_tab = Tab::Repeater;
                true
            }
            (_, KeyCode::Char('4')) => {
                self.active_tab = Tab::Rules;
                true
            }
            (_, KeyCode::Char('5')) => {
                self.active_tab = Tab::Tools;
                true
            }
            (KeyModifiers::NONE, KeyCode::Char('?')) => {
                self.show_help = true;
                true
            }
            (KeyModifiers::CONTROL, KeyCode::Char('s')) => {
                self.open_save_dialog(false);
                true
            }
            (_, KeyCode::F(2)) => {
                self.editor_mode = self.editor_mode.toggle();
                self.tools.editor.set_mode(self.editor_mode);
                self.intercept_ui.editor.set_mode(self.editor_mode);
                self.repeater.editor.set_mode(self.editor_mode);
                self.set_status(format!("Editor mode: {}", self.editor_mode.label()));
                true
            }
            _ => false,
        }
    }
}
