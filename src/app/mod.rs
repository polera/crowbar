use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::sync::mpsc;

mod actions;
mod dialogs;
mod encode;
mod event;
mod handlers;
mod render;

use crate::channel::ProxyToUi;
use crate::editor::{EditorMode, TextEditor};
use crate::http::models::{RequestData, ResponseData};
use crate::http::sequence::SequenceStep;
use crate::http::store::InMemoryStore;
use crate::proxy::intercept::InterceptState;
use crate::proxy::scope::Scope;
use crate::rules::SharedRules;
use crate::tui::tabs::Tab;

pub struct HistoryState {
    pub selected: usize,
    pub detail_open: bool,
    pub scroll: u16,
    pub filter: String,
    pub filtering: bool,
}

pub struct InterceptUiState {
    pub queue: VecDeque<RequestData>,
    pub scroll: u16,
    pub editing: bool,
    pub editor: TextEditor,
}

pub struct RepeaterState {
    pub original: Option<RequestData>,
    pub response: Option<ResponseData>,
    pub error: Option<String>,
    pub pending: bool,
    pub editing: bool,
    pub editor: TextEditor,
    pub req_scroll: u16,
    pub resp_scroll: u16,
    pub show_diff: bool,
}

pub struct MacroState {
    pub steps: Vec<SequenceStep>,
    pub selected: usize,
    pub running: bool,
    pub show: bool,
    pub current_step: usize,
}

pub struct RulesUiState {
    pub selected: usize,
    pub editing_field: Option<RuleField>,
    pub edit_buffer: String,
    pub importing: bool,
    pub import_buffer: String,
}

pub struct ToolsState {
    pub mode: ToolsMode,
    pub editor: TextEditor,
    pub editing: bool,
    pub scroll: u16,
}

pub struct App {
    pub active_tab: Tab,
    pub should_quit: bool,
    pub store: InMemoryStore,
    pub bind_addr: SocketAddr,
    pub intercept_state: Arc<InterceptState>,
    pub scope: Arc<Scope>,
    pub ui_tx: mpsc::UnboundedSender<ProxyToUi>,

    pub history: HistoryState,
    pub intercept_ui: InterceptUiState,
    pub repeater: RepeaterState,
    pub macros: MacroState,
    pub rules_ui: RulesUiState,
    pub tools: ToolsState,

    pub show_help: bool,
    pub status_message: Option<(String, std::time::Instant)>,
    pub rules: SharedRules,
    pub editor_mode: EditorMode,
    pub proxy_running: bool,

    // Bind address editing
    pub editing_bind_addr: bool,
    pub bind_addr_buffer: String,
    pub pending_rebind: Option<SocketAddr>,

    // Scope editing
    pub editing_scope: bool,
    pub scope_buffer: String,

    // Certificate info overlay
    pub show_cert_info: bool,
    pub cert_export_editing: bool,
    pub cert_export_buffer: String,

    // Session save dialog
    pub show_save_dialog: bool,
    pub save_buffer: String,
    pub save_on_confirm_quit: bool,

    // Quit confirmation
    pub show_quit_confirm: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolsMode {
    UrlEncode,
    UrlDecode,
    Base64Encode,
    Base64Decode,
    HexEncode,
    HexDecode,
}

impl ToolsMode {
    pub const ALL: [ToolsMode; 6] = [
        ToolsMode::UrlEncode,
        ToolsMode::UrlDecode,
        ToolsMode::Base64Encode,
        ToolsMode::Base64Decode,
        ToolsMode::HexEncode,
        ToolsMode::HexDecode,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ToolsMode::UrlEncode => "URL Encode",
            ToolsMode::UrlDecode => "URL Decode",
            ToolsMode::Base64Encode => "Base64 Encode",
            ToolsMode::Base64Decode => "Base64 Decode",
            ToolsMode::HexEncode => "Hex Encode",
            ToolsMode::HexDecode => "Hex Decode",
        }
    }

    pub fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|m| *m == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        let idx = Self::ALL.iter().position(|m| *m == self).unwrap_or(0);
        Self::ALL[(idx + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

impl App {
    pub fn new(
        bind_addr: SocketAddr,
        intercept_state: Arc<InterceptState>,
        scope: Arc<Scope>,
        rules: SharedRules,
        ui_tx: mpsc::UnboundedSender<ProxyToUi>,
        editor_mode: EditorMode,
    ) -> Self {
        Self {
            active_tab: Tab::History,
            should_quit: false,
            store: InMemoryStore::new(),
            bind_addr,
            intercept_state,
            scope,
            ui_tx,
            history: HistoryState {
                selected: 0,
                detail_open: false,
                scroll: 0,
                filter: String::new(),
                filtering: false,
            },
            intercept_ui: InterceptUiState {
                queue: VecDeque::new(),
                scroll: 0,
                editing: false,
                editor: TextEditor::new(vec![], editor_mode),
            },
            repeater: RepeaterState {
                original: None,
                response: None,
                error: None,
                pending: false,
                editing: false,
                editor: TextEditor::new(vec![], editor_mode),
                req_scroll: 0,
                resp_scroll: 0,
                show_diff: false,
            },
            macros: MacroState {
                steps: Vec::new(),
                selected: 0,
                running: false,
                show: false,
                current_step: 0,
            },
            show_help: false,
            status_message: None,
            rules,
            rules_ui: RulesUiState {
                selected: 0,
                editing_field: None,
                edit_buffer: String::new(),
                importing: false,
                import_buffer: String::new(),
            },
            tools: ToolsState {
                mode: ToolsMode::UrlEncode,
                editor: TextEditor::new(vec![String::new()], editor_mode),
                editing: false,
                scroll: 0,
            },
            editor_mode,
            proxy_running: true,
            editing_bind_addr: false,
            bind_addr_buffer: String::new(),
            pending_rebind: None,
            editing_scope: false,
            scope_buffer: String::new(),
            show_cert_info: false,
            cert_export_editing: false,
            cert_export_buffer: String::new(),
            show_save_dialog: false,
            save_buffer: String::new(),
            save_on_confirm_quit: false,
            show_quit_confirm: false,
        }
    }

    pub fn intercept_enabled(&self) -> bool {
        self.intercept_state.is_enabled()
    }

}

pub(super) enum EditorTarget {
    Intercept,
    Repeater,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleField {
    Name,
    Pattern,
    Replacement,
}

