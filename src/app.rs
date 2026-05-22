use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Tabs, Wrap};
use ratatui::Frame;
use tokio::sync::mpsc;

use crate::channel::ProxyToUi;
use crate::editor::{EditorAction, EditorMode, TextEditor};
use crate::http::codec;
use crate::http::models::{EntryState, RequestData, RequestId, ResponseData};
use crate::http::sequence::{SequenceStep, StepState};
use crate::http::store::InMemoryStore;
use crate::proxy::intercept::{InterceptDecision, InterceptState};
use crate::proxy::repeater;
use crate::proxy::scope::Scope;
use crate::rules::SharedRules;
use crate::tui::tabs::history_tab;
use crate::tui::tabs::proxy_tab;
use crate::tui::tabs::repeater_tab;
use crate::tui::tabs::rules_tab;
use crate::tui::tabs::tools_tab;
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

    fn handle_quit_confirm_key(&mut self, key: KeyEvent) {
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
                self.status_message = Some((
                    format!("Editor mode: {}", self.editor_mode.label()),
                    std::time::Instant::now(),
                ));
                true
            }
            _ => false,
        }
    }

    fn open_save_dialog(&mut self, quit_after: bool) {
        let name = crate::http::session::auto_save_name();
        if let Ok(dir) = crate::http::session::sessions_dir() {
            self.save_buffer = dir.join(format!("{}.json", name)).display().to_string();
        } else {
            self.save_buffer = format!("{}.json", name);
        }
        self.save_on_confirm_quit = quit_after;
        self.show_save_dialog = true;
    }

    fn handle_save_dialog_key(&mut self, key: KeyEvent) {
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
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                self.status_message = Some((
                    format!("Save failed: {}", e),
                    std::time::Instant::now(),
                ));
                return;
            }
        }
        let session = crate::http::session::Session::from_entries(self.store.entries());
        match serde_json::to_string_pretty(&session) {
            Ok(json) => match std::fs::write(path, json) {
                Ok(()) => {
                    self.status_message = Some((
                        format!("Saved to {}", path.display()),
                        std::time::Instant::now(),
                    ));
                }
                Err(e) => {
                    self.status_message = Some((
                        format!("Save failed: {}", e),
                        std::time::Instant::now(),
                    ));
                }
            },
            Err(e) => {
                self.status_message = Some((
                    format!("Save failed: {}", e),
                    std::time::Instant::now(),
                ));
            }
        }
    }

    fn export_rules(&mut self) {
        let rules = self.rules.read().unwrap().clone();
        if rules.is_empty() {
            self.status_message = Some((
                "No rules to export".into(),
                std::time::Instant::now(),
            ));
            return;
        }
        let name = crate::rules::persist::auto_save_name();
        match crate::rules::persist::save(&rules, &name) {
            Ok(path) => {
                self.status_message = Some((
                    format!("Exported {} rules to {}", rules.len(), path.display()),
                    std::time::Instant::now(),
                ));
            }
            Err(e) => {
                self.status_message = Some((
                    format!("Export failed: {}", e),
                    std::time::Instant::now(),
                ));
            }
        }
    }

    pub fn load_session(&mut self, path: &std::path::Path) {
        match crate::http::import::load_file(path) {
            Ok(entries) => {
                self.store.load_entries(entries);
                self.status_message = Some((
                    format!("Loaded {} entries", self.store.len()),
                    std::time::Instant::now(),
                ));
            }
            Err(e) => {
                self.status_message = Some((
                    format!("Load failed: {}", e),
                    std::time::Instant::now(),
                ));
            }
        }
    }

    fn handle_proxy_key(&mut self, key: KeyEvent) {
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
                    self.intercept_ui.editor = TextEditor::new(codec::request_to_lines(req), self.editor_mode);
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

    fn handle_bind_addr_editor_key(&mut self, key: KeyEvent) {
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
                    Ok(addr) => {
                        self.pending_rebind = Some(addr);
                        self.editing_bind_addr = false;
                        self.bind_addr_buffer.clear();
                    }
                    Err(_) => {
                        self.status_message = Some((
                            format!("Invalid address: {}", input),
                            std::time::Instant::now(),
                        ));
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

    fn handle_scope_editor_key(&mut self, key: KeyEvent) {
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
                self.status_message = Some((
                    if count == 0 {
                        "Scope cleared — capturing all traffic".to_string()
                    } else {
                        format!("Scope updated ({} pattern{})", count, if count == 1 { "" } else { "s" })
                    },
                    std::time::Instant::now(),
                ));
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

    fn handle_cert_overlay_key(&mut self, key: KeyEvent) {
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
                self.status_message = Some((
                    "Cannot find home directory".to_string(),
                    std::time::Instant::now(),
                ));
                return;
            }
        };
        let cert_path = ca_dir.join("ca.pem");

        if !cert_path.exists() {
            self.status_message = Some((
                "No CA certificate found — restart crowbar to generate one".to_string(),
                std::time::Instant::now(),
            ));
            return;
        }

        let pem = match std::fs::read_to_string(&cert_path) {
            Ok(p) => p,
            Err(e) => {
                self.status_message = Some((
                    format!("Failed to read CA cert: {}", e),
                    std::time::Instant::now(),
                ));
                return;
            }
        };

        let dest = path.unwrap_or("crowbar-ca.pem");
        match std::fs::write(dest, &pem) {
            Ok(_) => {
                let abs = std::path::Path::new(dest)
                    .canonicalize()
                    .unwrap_or_else(|_| std::path::PathBuf::from(dest));
                self.status_message = Some((
                    format!("CA certificate exported to {}", abs.display()),
                    std::time::Instant::now(),
                ));
            }
            Err(e) => {
                self.status_message = Some((
                    format!("Export failed: {}", e),
                    std::time::Instant::now(),
                ));
            }
        }
    }

    fn handle_history_key(&mut self, key: KeyEvent) {
        let filtered = self.store.filtered_entries(&self.history.filter);
        let entry_count = filtered.len();

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
            KeyCode::Home | KeyCode::Char('g')
                if !self.history.detail_open => {
                    self.history.selected = 0;
                }
            KeyCode::End | KeyCode::Char('G')
                if !self.history.detail_open && entry_count > 0 => {
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
            KeyCode::Esc
                if self.history.detail_open => {
                    self.history.detail_open = false;
                    self.history.scroll = 0;
                }
            KeyCode::Char('r')
                if entry_count > 0 => {
                    self.send_to_repeater();
                }
            KeyCode::Char('/')
                if !self.history.detail_open => {
                    self.history.filtering = true;
                }
            KeyCode::Char('c') => {
                if entry_count > 0
                    && let Some(entry) = filtered.get(self.history.selected) {
                        let curl = crate::http::export::to_curl(entry);
                        self.export_to_file("curl", "sh", &curl);
                    }
            }
            KeyCode::Char('w') => {
                if entry_count > 0
                    && let Some(entry) = filtered.get(self.history.selected) {
                        let raw = crate::http::export::to_raw(entry);
                        self.export_to_file("raw", "txt", &raw);
                    }
            }
            KeyCode::Char('h')
                if !self.history.detail_open => {
                    let entries: Vec<_> = filtered.iter().map(|e| (*e).clone()).collect();
                    let har = crate::http::export::to_har(&entries);
                    self.export_to_file("har", "har", &har);
                }
            KeyCode::Char('m') => {
                if entry_count > 0
                    && let Some(entry) = filtered.get(self.history.selected) {
                        self.macros.steps.push(SequenceStep::new(entry.request.clone()));
                        self.status_message = Some((
                            format!("Added to macro ({} steps)", self.macros.steps.len()),
                            std::time::Instant::now(),
                        ));
                    }
            }
            _ => {}
        }
    }

    fn export_to_file(&mut self, prefix: &str, ext: &str, content: &str) {
        let dir = dirs::home_dir()
            .unwrap_or_default()
            .join(".crowbar")
            .join("exports");
        if std::fs::create_dir_all(&dir).is_err() {
            self.status_message = Some((
                "Failed to create exports directory".into(),
                std::time::Instant::now(),
            ));
            return;
        }
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let path = dir.join(format!("{}-{}.{}", prefix, ts, ext));
        match std::fs::write(&path, content) {
            Ok(_) => {
                self.status_message = Some((
                    format!("Exported to {}", path.display()),
                    std::time::Instant::now(),
                ));
            }
            Err(e) => {
                self.status_message = Some((
                    format!("Export failed: {}", e),
                    std::time::Instant::now(),
                ));
            }
        }
    }

    fn handle_rules_key(&mut self, key: KeyEvent) {
        let rules = self.rules.read().unwrap();
        let count = rules.len();
        drop(rules);

        match key.code {
            KeyCode::Char('a') => {
                let mut rules = self.rules.write().unwrap();
                let name = format!("Rule {}", rules.len() + 1);
                rules.push(crate::rules::Rule::new(name));
                self.rules_ui.selected = rules.len() - 1;
            }
            KeyCode::Char('x')
                if count > 0 => {
                    let mut rules = self.rules.write().unwrap();
                    rules.remove(self.rules_ui.selected);
                    if self.rules_ui.selected >= rules.len() && !rules.is_empty() {
                        self.rules_ui.selected = rules.len() - 1;
                    }
                }
            KeyCode::Enter
                if count > 0 => {
                    let mut rules = self.rules.write().unwrap();
                    rules[self.rules_ui.selected].enabled = !rules[self.rules_ui.selected].enabled;
                }
            KeyCode::Char('t')
                if count > 0 => {
                    let mut rules = self.rules.write().unwrap();
                    rules[self.rules_ui.selected].target = rules[self.rules_ui.selected].target.next();
                }
            KeyCode::Char('s')
                if count > 0 => {
                    let mut rules = self.rules.write().unwrap();
                    rules[self.rules_ui.selected].scope = rules[self.rules_ui.selected].scope.next();
                }
            KeyCode::Char('R')
                if count > 0 => {
                    let mut rules = self.rules.write().unwrap();
                    rules[self.rules_ui.selected].is_regex = !rules[self.rules_ui.selected].is_regex;
                }
            KeyCode::Char('n')
                if count > 0 => {
                    let rules = self.rules.read().unwrap();
                    self.rules_ui.edit_buffer = rules[self.rules_ui.selected].name.clone();
                    drop(rules);
                    self.rules_ui.editing_field = Some(RuleField::Name);
                }
            KeyCode::Char('p')
                if count > 0 => {
                    let rules = self.rules.read().unwrap();
                    self.rules_ui.edit_buffer = rules[self.rules_ui.selected].match_pattern.clone();
                    drop(rules);
                    self.rules_ui.editing_field = Some(RuleField::Pattern);
                }
            KeyCode::Char('e')
                if count > 0 => {
                    let rules = self.rules.read().unwrap();
                    self.rules_ui.edit_buffer = rules[self.rules_ui.selected].replacement.clone();
                    drop(rules);
                    self.rules_ui.editing_field = Some(RuleField::Replacement);
                }
            KeyCode::Up | KeyCode::Char('k')
                if self.rules_ui.selected > 0 => {
                    self.rules_ui.selected -= 1;
                }
            KeyCode::Down | KeyCode::Char('j')
                if count > 0 && self.rules_ui.selected < count - 1 => {
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

    fn handle_rules_import_editor_key(&mut self, key: KeyEvent) {
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
                        let mut rules = self.rules.write().unwrap();
                        rules.extend(imported);
                        self.status_message = Some((
                            format!("Imported {} rules from {}", count, expanded.display()),
                            std::time::Instant::now(),
                        ));
                    }
                    Err(e) => {
                        self.status_message = Some((
                            format!("Import failed: {}", e),
                            std::time::Instant::now(),
                        ));
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

    fn handle_rules_editor_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.rules_ui.editing_field = None;
                self.rules_ui.edit_buffer.clear();
            }
            KeyCode::Enter => {
                if let Some(field) = self.rules_ui.editing_field {
                    let mut rules = self.rules.write().unwrap();
                    if self.rules_ui.selected < rules.len() {
                        match field {
                            RuleField::Name => {
                                rules[self.rules_ui.selected].name = self.rules_ui.edit_buffer.clone();
                            }
                            RuleField::Pattern => {
                                rules[self.rules_ui.selected].match_pattern = self.rules_ui.edit_buffer.clone();
                            }
                            RuleField::Replacement => {
                                rules[self.rules_ui.selected].replacement = self.rules_ui.edit_buffer.clone();
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

    fn handle_tools_key(&mut self, key: KeyEvent) {
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
                        self.status_message = Some((
                            "Copied to clipboard".to_string(),
                            std::time::Instant::now(),
                        ));
                    }
                    Err(e) => {
                        self.status_message = Some((
                            format!("Clipboard error: {}", e),
                            std::time::Instant::now(),
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_tools_editor_key(&mut self, key: KeyEvent) {
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
            ToolsMode::UrlEncode => url_encode(&input),
            ToolsMode::UrlDecode => crate::http::url_decode(&input),
            ToolsMode::Base64Encode => base64::engine::general_purpose::STANDARD.encode(input.as_bytes()),
            ToolsMode::Base64Decode => {
                match base64::engine::general_purpose::STANDARD.decode(input.trim()) {
                    Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                    Err(e) => format!("Error: {}", e),
                }
            }
            ToolsMode::HexEncode => hex_encode(input.as_bytes()),
            ToolsMode::HexDecode => {
                match hex_decode(input.trim()) {
                    Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                    Err(e) => format!("Error: {}", e),
                }
            }
        }
    }

    fn handle_filter_key(&mut self, key: KeyEvent) {
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

    fn handle_repeater_key(&mut self, key: KeyEvent) {
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Enter) => {
                if self.macros.show {
                    self.macro_run();
                } else {
                    self.repeater_send();
                }
            }
            (KeyModifiers::NONE, KeyCode::Char('e'))
                if self.repeater.editor.has_content() => {
                    self.repeater.editing = true;
                    self.repeater.editor.cursor_line = 0;
                    self.repeater.editor.cursor_col = 0;
                    if self.editor_mode == EditorMode::Vim {
                        self.repeater.editor.vim_mode = crate::editor::VimMode::Insert;
                    }
                }
            (KeyModifiers::NONE, KeyCode::Char('d'))
                if !self.macros.show && self.repeater.original.is_some() => {
                    self.repeater.show_diff = !self.repeater.show_diff;
                }
            (KeyModifiers::NONE, KeyCode::Enter)
                if self.macros.show && !self.macros.steps.is_empty() && !self.macros.running => {
                    self.load_macro_step(true);
                }
            (KeyModifiers::NONE, KeyCode::Char('e'))
                if self.macros.show && !self.macros.steps.is_empty() && !self.macros.running => {
                    self.load_macro_step(false);
                }
            (KeyModifiers::SHIFT, KeyCode::Char('M')) => {
                self.macros.show = !self.macros.show;
            }
            (KeyModifiers::NONE, KeyCode::Char('j') | KeyCode::Down) => {
                if self.macros.show {
                    if !self.macros.steps.is_empty() && self.macros.selected < self.macros.steps.len() - 1 {
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
                if self.macros.show && !self.macros.steps.is_empty() && !self.macros.running => {
                    self.macros.steps.remove(self.macros.selected);
                    if self.macros.selected >= self.macros.steps.len() && !self.macros.steps.is_empty() {
                        self.macros.selected = self.macros.steps.len() - 1;
                    }
                }
            (KeyModifiers::NONE, KeyCode::Char('X')) | (KeyModifiers::SHIFT, KeyCode::Char('X'))
                if self.macros.show && !self.macros.running => {
                    self.macros.steps.clear();
                    self.macros.selected = 0;
                }
            _ => {}
        }
    }

    fn handle_editor_key(&mut self, key: KeyEvent, target: EditorTarget) {
        let editor = match target {
            EditorTarget::Intercept => &mut self.intercept_ui.editor,
            EditorTarget::Repeater => &mut self.repeater.editor,
        };

        let action = editor.handle_key(key);

        match action {
            EditorAction::Consumed => {}
            EditorAction::ExitEditor => {
                match target {
                    EditorTarget::Intercept => {
                        self.intercept_ui.editing = false;
                        self.intercept_ui.editor = TextEditor::new(vec![], self.editor_mode);
                    }
                    EditorTarget::Repeater => {
                        self.repeater.editing = false;
                    }
                }
            }
            EditorAction::Enter => {
                match target {
                    EditorTarget::Intercept => {
                        if let Some(original) = self.intercept_ui.queue.pop_front() {
                            let edited =
                                codec::lines_to_request(&self.intercept_ui.editor.lines, &original);
                            self.intercept_state
                                .resolve(original.id, InterceptDecision::ForwardEdited(edited));
                        }
                        self.intercept_ui.editing = false;
                        self.intercept_ui.editor = TextEditor::new(vec![], self.editor_mode);
                        self.intercept_ui.scroll = 0;
                    }
                    EditorTarget::Repeater => {
                        self.repeater.editor.insert_newline();
                    }
                }
            }
            EditorAction::CtrlEnter => {
                match target {
                    EditorTarget::Intercept => {
                        if let Some(original) = self.intercept_ui.queue.pop_front() {
                            let edited =
                                codec::lines_to_request(&self.intercept_ui.editor.lines, &original);
                            self.intercept_state
                                .resolve(original.id, InterceptDecision::ForwardEdited(edited));
                        }
                        self.intercept_ui.editing = false;
                        self.intercept_ui.editor = TextEditor::new(vec![], self.editor_mode);
                        self.intercept_ui.scroll = 0;
                    }
                    EditorTarget::Repeater => {
                        self.repeater.editing = false;
                        self.repeater_send();
                    }
                }
            }
            EditorAction::Custom(_) => {}
        }
    }

    fn send_to_repeater(&mut self) {
        let filtered = self.store.filtered_entries(&self.history.filter);
        if let Some(entry) = filtered.get(self.history.selected) {
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

    fn load_macro_step(&mut self, send: bool) {
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

    fn repeater_send(&mut self) {
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

    fn macro_run(&mut self) {
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
            self.status_message = Some((
                format!("Macro complete ({} steps)", self.macros.steps.len()),
                std::time::Instant::now(),
            ));
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

    pub fn handle_proxy_message(&mut self, msg: ProxyToUi) {
        match msg {
            ProxyToUi::RequestCaptured(req) => {
                self.store.insert(req);
                if !self.history.detail_open && self.store.len() > 1 {
                    self.history.selected = self.store.len() - 1;
                }
            }
            ProxyToUi::ResponseReceived(id, resp) => {
                if let Some(entry) = self.store.entries().iter().find(|e| e.request.id == id) {
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
                    self.status_message = Some((
                        format!("Macro stopped at step {} (error)", step_idx + 1),
                        std::time::Instant::now(),
                    ));
                }
            }
        }
    }

    pub fn render(&self, frame: &mut Frame) {
        let chunks = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(frame.area());

        self.render_tab_bar(frame, chunks[0]);
        self.render_active_tab(frame, chunks[1]);
        self.render_status_bar(frame, chunks[2]);

        if self.show_help {
            self.render_help_overlay(frame);
        }

        if self.show_cert_info {
            self.render_cert_overlay(frame);
        }

        if self.show_save_dialog {
            self.render_save_dialog(frame);
        }

        if self.show_quit_confirm {
            self.render_quit_confirm(frame);
        }
    }

    fn render_tab_bar(&self, frame: &mut Frame, area: Rect) {
        let titles: Vec<Line> = Tab::ALL
            .iter()
            .enumerate()
            .map(|(i, tab)| {
                let num = format!("{}", i + 1);
                let mut spans = vec![
                    Span::styled(num, Style::default().fg(Color::DarkGray)),
                    Span::raw(":"),
                    Span::raw(tab.title()),
                ];

                if *tab == Tab::Proxy && !self.intercept_ui.queue.is_empty() {
                    spans.push(Span::styled(
                        format!(" ({})", self.intercept_ui.queue.len()),
                        Style::default()
                            .fg(Color::Red)
                            .add_modifier(Modifier::BOLD),
                    ));
                }

                Line::from(spans)
            })
            .collect();

        let selected = Tab::ALL
            .iter()
            .position(|t| *t == self.active_tab)
            .unwrap_or(0);

        let version = env!("CARGO_PKG_VERSION");
        let tabs = Tabs::new(titles)
            .block(Block::default().borders(Borders::ALL).title(format!(" crowbar v{version} ")))
            .select(selected)
            .highlight_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            );

        frame.render_widget(tabs, area);
    }

    fn render_active_tab(&self, frame: &mut Frame, area: Rect) {
        match self.active_tab {
            Tab::History => history_tab::render(self, frame, area),
            Tab::Proxy => proxy_tab::render(self, frame, area),
            Tab::Repeater => repeater_tab::render(self, frame, area),
            Tab::Rules => rules_tab::render(self, frame, area),
            Tab::Tools => tools_tab::render(self, frame, area),
        }
    }

    fn render_status_bar(&self, frame: &mut Frame, area: Rect) {
        if let Some((msg, when)) = &self.status_message
            && when.elapsed() < std::time::Duration::from_secs(3) {
                let line = Line::from(Span::styled(
                    format!(" {} ", msg),
                    Style::default().fg(Color::Yellow),
                ));
                frame.render_widget(Paragraph::new(line), area);
                return;
            }

        let total = self.store.len();
        let complete = self
            .store
            .entries()
            .iter()
            .filter(|e| e.state == EntryState::Complete)
            .count();
        let errors = self
            .store
            .entries()
            .iter()
            .filter(|e| e.state == EntryState::Error)
            .count();

        let intercept_span = if !self.proxy_running {
            Span::styled(
                " NOT BOUND ",
                Style::default().bg(Color::Red).fg(Color::White).bold(),
            )
        } else if self.intercept_enabled() {
            Span::styled(
                " INTERCEPT ",
                Style::default().bg(Color::Red).fg(Color::White).bold(),
            )
        } else {
            Span::styled(
                " PROXY ",
                Style::default().bg(Color::Green).fg(Color::Black).bold(),
            )
        };

        let scope_patterns = self.scope.patterns();
        let scope_span = if scope_patterns.is_empty() {
            Span::styled(" ALL ", Style::default().fg(Color::DarkGray))
        } else {
            Span::styled(
                format!(" SCOPE:{} ", scope_patterns.join(",")),
                Style::default().fg(Color::Cyan),
            )
        };

        let editor_mode_span = if self.editor_mode == EditorMode::Vim {
            let active_editing = self.tools.editing || self.intercept_ui.editing || self.repeater.editing;
            if active_editing {
                let editor = if self.tools.editing {
                    &self.tools.editor
                } else if self.intercept_ui.editing {
                    &self.intercept_ui.editor
                } else {
                    &self.repeater.editor
                };
                let (label, bg) = match editor.vim_mode {
                    crate::editor::VimMode::Normal => ("NORMAL", Color::Blue),
                    crate::editor::VimMode::Insert => ("INSERT", Color::Green),
                };
                Span::styled(
                    format!(" {} ", label),
                    Style::default().bg(bg).fg(Color::Black).bold(),
                )
            } else {
                Span::styled(" VIM ", Style::default().fg(Color::DarkGray))
            }
        } else {
            Span::raw("")
        };

        let status = Line::from(vec![
            intercept_span,
            Span::raw(format!(" {} ", self.bind_addr)),
            scope_span,
            editor_mode_span,
            Span::raw("| "),
            Span::styled(
                format!("{} requests", total),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw(format!(" ({} complete", complete)),
            if errors > 0 {
                Span::styled(
                    format!(", {} errors", errors),
                    Style::default().fg(Color::Red),
                )
            } else {
                Span::raw("")
            },
            Span::raw(") | "),
            Span::styled("Tab", Style::default().fg(Color::DarkGray)),
            Span::raw(":switch "),
            Span::styled("?", Style::default().fg(Color::DarkGray)),
            Span::raw(":help "),
            Span::styled("q", Style::default().fg(Color::DarkGray)),
            Span::raw(":quit"),
        ]);

        frame.render_widget(Paragraph::new(status), area);
    }

    fn render_cert_overlay(&self, frame: &mut Frame) {
        let area = frame.area();
        let width = 72.min(area.width.saturating_sub(4));
        let height = 22.min(area.height.saturating_sub(4));
        let x = (area.width.saturating_sub(width)) / 2;
        let y = (area.height.saturating_sub(height)) / 2;
        let popup = Rect::new(x, y, width, height);

        let key = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
        let dim = Style::default().fg(Color::DarkGray);
        let section = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
        let path_style = Style::default().fg(Color::Green);

        let cert_path = dirs::home_dir()
            .unwrap_or_default()
            .join(".crowbar")
            .join("ca.pem");
        let cert_display = cert_path.display().to_string();

        let mut lines = vec![
            Line::from(Span::styled("CA Certificate", section)),
            Line::raw(""),
            Line::from(vec![
                Span::raw("  Location: "),
                Span::styled(&cert_display, path_style),
            ]),
            Line::raw(""),
            Line::from(Span::styled("Install in OS/Browser Trust Store", section)),
            Line::raw(""),
            Line::from(vec![
                Span::styled("  macOS:  ", key),
                Span::raw("security add-trusted-cert -d -r trustRoot \\"),
            ]),
            Line::from(Span::raw(format!(
                "            -k ~/Library/Keychains/login.keychain-db {}",
                cert_display
            ))),
            Line::raw(""),
            Line::from(vec![
                Span::styled("  Linux:  ", key),
                Span::raw(format!(
                    "sudo cp {} /usr/local/share/ca-certificates/crowbar.crt",
                    cert_display
                )),
            ]),
            Line::from(Span::raw(
                "            && sudo update-ca-certificates",
            )),
            Line::raw(""),
            Line::from(vec![
                Span::styled("  Firefox:", key),
                Span::raw(" Settings > Privacy & Security > Certificates > Import"),
            ]),
            Line::raw(""),
            Line::from(Span::styled("Export", section)),
            Line::raw(""),
        ];

        if self.cert_export_editing {
            lines.push(Line::from(vec![
                Span::raw("  Save to: "),
                Span::styled(&self.cert_export_buffer, Style::default().fg(Color::White)),
                Span::styled("\u{2588}", Style::default().fg(Color::Yellow)),
                Span::styled("  (Enter to confirm, Esc to cancel)", dim),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled("  s ", key),
                Span::raw("Quick export to ./crowbar-ca.pem    "),
                Span::styled("  p ", key),
                Span::raw("Export to custom path"),
            ]));
        }

        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled("  Esc/q to close", dim)));

        frame.render_widget(Clear, popup);
        let widget = Paragraph::new(Text::from(lines))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" CA Certificate Export ")
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(widget, popup);
    }

    fn render_save_dialog(&self, frame: &mut Frame) {
        let area = frame.area();
        let width = 64.min(area.width.saturating_sub(4));
        let height = 8.min(area.height.saturating_sub(2));
        let x = (area.width.saturating_sub(width)) / 2;
        let y = (area.height.saturating_sub(height)) / 2;
        let popup = Rect::new(x, y, width, height);

        let dim = Style::default().fg(Color::DarkGray);
        let count = self.store.len();

        let lines = vec![
            Line::raw(""),
            Line::from(format!(
                "  Saving {} request{}.",
                count,
                if count == 1 { "" } else { "s" }
            )),
            Line::raw(""),
            Line::from(vec![
                Span::raw("  Path: "),
                Span::styled(&self.save_buffer, Style::default().fg(Color::White)),
                Span::styled("\u{2588}", Style::default().fg(Color::Yellow)),
            ]),
            Line::from(Span::styled(
                "  Enter to save, Esc to cancel",
                dim,
            )),
        ];

        frame.render_widget(Clear, popup);
        let widget = Paragraph::new(Text::from(lines))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Save Session ")
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(widget, popup);
    }

    fn render_quit_confirm(&self, frame: &mut Frame) {
        let area = frame.area();
        let has_session = !self.store.is_empty();
        let width = 44.min(area.width.saturating_sub(4));
        let height = if has_session { 9 } else { 7 };
        let height = height.min(area.height.saturating_sub(2));
        let x = (area.width.saturating_sub(width)) / 2;
        let y = (area.height.saturating_sub(height)) / 2;
        let popup = Rect::new(x, y, width, height);

        let key = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
        let dim = Style::default().fg(Color::DarkGray);

        let mut lines = vec![Line::raw("")];

        if has_session {
            let count = self.store.len();
            lines.push(Line::from(format!(
                "  Session has {} request{}.",
                count,
                if count == 1 { "" } else { "s" }
            )));
            lines.push(Line::raw("  Save before quitting?"));
            lines.push(Line::raw(""));
            lines.push(Line::from(vec![
                Span::styled("  y", key),
                Span::raw(" Save & Quit   "),
                Span::styled("n", key),
                Span::raw(" Quit   "),
                Span::styled("Esc", key),
                Span::raw(" Cancel"),
            ]));
        } else {
            lines.push(Line::raw("  Quit Crowbar?"));
            lines.push(Line::raw(""));
            lines.push(Line::from(vec![
                Span::styled("  y", key),
                Span::raw(" Quit   "),
                Span::styled("Esc", key),
                Span::styled(" Cancel", dim),
            ]));
        }

        frame.render_widget(Clear, popup);
        let widget = Paragraph::new(Text::from(lines))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Quit ")
                    .border_style(Style::default().fg(Color::Red)),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(widget, popup);
    }

    fn render_help_overlay(&self, frame: &mut Frame) {
        let area = frame.area();
        let width = 52.min(area.width.saturating_sub(4));
        let height = 46.min(area.height.saturating_sub(4));
        let x = (area.width.saturating_sub(width)) / 2;
        let y = (area.height.saturating_sub(height)) / 2;
        let popup = Rect::new(x, y, width, height);

        let key = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
        let dim = Style::default().fg(Color::DarkGray);
        let section = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);

        let lines = vec![
            Line::from(Span::styled("Global", section)),
            Line::from(vec![Span::styled("  Tab/Shift+Tab  ", key), Span::raw("Cycle tabs")]),
            Line::from(vec![Span::styled("  1-5            ", key), Span::raw("Proxy/History/Repeater/Rules/Tools")]),
            Line::from(vec![Span::styled("  ?              ", key), Span::raw("Toggle this help")]),
            Line::from(vec![Span::styled("  Ctrl+S         ", key), Span::raw("Save session")]),
            Line::from(vec![Span::styled("  q / Ctrl+C     ", key), Span::raw("Quit")]),
            Line::raw(""),
            Line::from(Span::styled("Proxy (Intercept)", section)),
            Line::from(vec![Span::styled("  i              ", key), Span::raw("Toggle intercept on/off")]),
            Line::from(vec![Span::styled("  f              ", key), Span::raw("Forward intercepted request")]),
            Line::from(vec![Span::styled("  d              ", key), Span::raw("Drop intercepted request")]),
            Line::from(vec![Span::styled("  e              ", key), Span::raw("Edit intercepted request")]),
            Line::from(vec![Span::styled("  b              ", key), Span::raw("Change bind address")]),
            Line::from(vec![Span::styled("  s              ", key), Span::raw("Edit scope patterns")]),
            Line::from(vec![Span::styled("  C              ", key), Span::raw("Export CA certificate")]),
            Line::from(vec![Span::styled("  j/k            ", key), Span::raw("Scroll request")]),
            Line::raw(""),
            Line::from(Span::styled("History", section)),
            Line::from(vec![Span::styled("  j/k            ", key), Span::raw("Navigate / scroll")]),
            Line::from(vec![Span::styled("  g/G            ", key), Span::raw("Jump to first / last")]),
            Line::from(vec![Span::styled("  /              ", key), Span::raw("Filter by host, path, method, status")]),
            Line::from(vec![Span::styled("  Enter          ", key), Span::raw("Toggle detail view")]),
            Line::from(vec![Span::styled("  r              ", key), Span::raw("Send to repeater")]),
            Line::from(vec![Span::styled("  m              ", key), Span::raw("Add to macro sequence")]),
            Line::from(vec![Span::styled("  c              ", key), Span::raw("Export as curl")]),
            Line::from(vec![Span::styled("  w              ", key), Span::raw("Export as raw HTTP")]),
            Line::from(vec![Span::styled("  h              ", key), Span::raw("Export all as HAR")]),
            Line::raw(""),
            Line::from(Span::styled("Repeater", section)),
            Line::from(vec![Span::styled("  Ctrl+Enter     ", key), Span::raw("Send request")]),
            Line::from(vec![Span::styled("  e              ", key), Span::raw("Edit request")]),
            Line::from(vec![Span::styled("  d              ", key), Span::raw("Toggle diff view")]),
            Line::from(vec![Span::styled("  M              ", key), Span::raw("Toggle macro view")]),
            Line::from(vec![Span::styled("  j/k            ", key), Span::raw("Scroll request")]),
            Line::from(vec![Span::styled("  J/K            ", key), Span::raw("Scroll response")]),
            Line::raw(""),
            Line::from(Span::styled("Rules", section)),
            Line::from(vec![Span::styled("  a              ", key), Span::raw("Add rule")]),
            Line::from(vec![Span::styled("  x              ", key), Span::raw("Delete rule")]),
            Line::from(vec![Span::styled("  Enter          ", key), Span::raw("Toggle enabled")]),
            Line::from(vec![Span::styled("  n/p/e          ", key), Span::raw("Edit name / pattern / replacement")]),
            Line::from(vec![Span::styled("  t/s/R          ", key), Span::raw("Cycle target / scope / regex")]),
            Line::raw(""),
            Line::from(Span::styled("Tools", section)),
            Line::from(vec![Span::styled("  h/l            ", key), Span::raw("Switch tool")]),
            Line::from(vec![Span::styled("  e              ", key), Span::raw("Edit input")]),
            Line::from(vec![Span::styled("  j/k            ", key), Span::raw("Scroll output")]),
            Line::from(vec![Span::styled("  Ctrl+U         ", key), Span::raw("Clear input")]),
            Line::from(vec![Span::styled("  Ctrl+Y         ", key), Span::raw("Copy output to clipboard")]),
            Line::raw(""),
            Line::from(Span::styled("Editor", section)),
            Line::from(vec![Span::styled("  F2             ", key), Span::raw("Toggle vim/default mode")]),
            Line::from(vec![Span::styled("  Ctrl+Home/End  ", key), Span::raw("Jump to start/end of input")]),
            Line::from(vec![Span::styled("  Vim: Esc       ", key), Span::raw("Normal mode / exit edit")]),
            Line::from(vec![Span::styled("  Vim: i/a/o     ", key), Span::raw("Enter insert mode")]),
            Line::from(vec![Span::styled("  Vim: hjkl      ", key), Span::raw("Movement (normal mode)")]),
            Line::from(vec![Span::styled("  Vim: gg/G      ", key), Span::raw("Jump to start/end of input")]),
            Line::from(vec![Span::styled("  Vim: w/b       ", key), Span::raw("Word forward/backward")]),
            Line::from(vec![Span::styled("  Vim: dd/x/u    ", key), Span::raw("Delete line/char, undo")]),
            Line::raw(""),
            Line::from(Span::styled("Press any key to close", dim)),
        ];

        frame.render_widget(Clear, popup);
        let help = Paragraph::new(Text::from(lines))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Help ")
                    .border_style(Style::default().fg(Color::Yellow)),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(help, popup);
    }
}

enum EditorTarget {
    Intercept,
    Repeater,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleField {
    Name,
    Pattern,
    Replacement,
}

fn url_encode(input: &str) -> String {
    let mut result = String::new();
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}


fn hex_encode(input: &[u8]) -> String {
    input.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hex_decode(input: &str) -> Result<Vec<u8>, String> {
    let cleaned: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    if !cleaned.len().is_multiple_of(2) {
        return Err("Odd number of hex characters".into());
    }
    (0..cleaned.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&cleaned[i..i + 2], 16)
                .map_err(|e| format!("Invalid hex at position {}: {}", i, e))
        })
        .collect()
}
