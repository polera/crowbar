use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use crate::tui::tabs::Tab;

use super::{App, EditorTarget};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputContext {
    Help,
    QuitConfirmation,
    SaveDialog,
    CertificateInfo,
    InterceptEditor,
    RepeaterEditor,
    HistoryFilter,
    ToolsEditor,
    BindAddressEditor,
    ScopeEditor,
    RuleEditor,
    RuleImport,
    Tab,
}

impl App {
    pub fn handle_event(&mut self, event: Event) {
        if let Event::Key(key) = event {
            if key.kind != crossterm::event::KeyEventKind::Press {
                return;
            }

            match self.input_context() {
                InputContext::Help => self.show_help = false,
                InputContext::QuitConfirmation => self.handle_quit_confirm_key(key),
                InputContext::SaveDialog => self.handle_save_dialog_key(key),
                InputContext::CertificateInfo => self.handle_cert_overlay_key(key),
                InputContext::InterceptEditor => {
                    self.handle_editor_key(key, EditorTarget::Intercept);
                }
                InputContext::RepeaterEditor => {
                    self.handle_editor_key(key, EditorTarget::Repeater);
                }
                InputContext::HistoryFilter => self.handle_filter_key(key),
                InputContext::ToolsEditor => self.handle_tools_editor_key(key),
                InputContext::BindAddressEditor => self.handle_bind_addr_editor_key(key),
                InputContext::ScopeEditor => self.handle_scope_editor_key(key),
                InputContext::RuleEditor => self.handle_rules_editor_key(key),
                InputContext::RuleImport => self.handle_rules_import_editor_key(key),
                InputContext::Tab => {
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
        }
    }

    fn input_context(&self) -> InputContext {
        if self.show_help {
            InputContext::Help
        } else if self.show_quit_confirm {
            InputContext::QuitConfirmation
        } else if self.show_save_dialog {
            InputContext::SaveDialog
        } else if self.show_cert_info {
            InputContext::CertificateInfo
        } else if self.intercept_ui.editing {
            InputContext::InterceptEditor
        } else if self.repeater.editing {
            InputContext::RepeaterEditor
        } else if self.history.filtering {
            InputContext::HistoryFilter
        } else if self.tools.editing {
            InputContext::ToolsEditor
        } else if self.editing_bind_addr {
            InputContext::BindAddressEditor
        } else if self.editing_scope {
            InputContext::ScopeEditor
        } else if self.rules_ui.editing_field.is_some() {
            InputContext::RuleEditor
        } else if self.rules_ui.importing {
            InputContext::RuleImport
        } else {
            InputContext::Tab
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
