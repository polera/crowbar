use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Tabs, Wrap};
use ratatui::Frame;
use tokio::sync::mpsc;

use crate::channel::ProxyToUi;
use crate::http::codec;
use crate::http::models::{EntryState, RequestData, RequestId, ResponseData};
use crate::http::store::InMemoryStore;
use crate::proxy::intercept::{InterceptDecision, InterceptState};
use crate::proxy::repeater;
use crate::tui::tabs::history_tab;
use crate::tui::tabs::proxy_tab;
use crate::tui::tabs::repeater_tab;
use crate::tui::tabs::Tab;

pub struct App {
    pub active_tab: Tab,
    pub should_quit: bool,
    pub store: InMemoryStore,
    pub bind_addr: SocketAddr,
    pub intercept_state: Arc<InterceptState>,
    pub ui_tx: mpsc::UnboundedSender<ProxyToUi>,

    // History tab state
    pub history_selected: usize,
    pub history_detail_open: bool,
    pub history_scroll: u16,

    // Proxy/Intercept tab state
    pub intercept_queue: VecDeque<RequestData>,
    pub intercept_scroll: u16,
    pub editing_intercept: bool,
    pub edit_buffer: Vec<String>,
    pub edit_cursor_line: usize,
    pub edit_cursor_col: usize,

    // Repeater tab state
    pub repeater_lines: Vec<String>,
    pub repeater_original: Option<RequestData>,
    pub repeater_response: Option<ResponseData>,
    pub repeater_error: Option<String>,
    pub repeater_pending: bool,
    pub repeater_editing: bool,
    pub repeater_cursor_line: usize,
    pub repeater_cursor_col: usize,
    pub repeater_req_scroll: u16,
    pub repeater_resp_scroll: u16,

    pub show_help: bool,
}

impl App {
    pub fn new(
        bind_addr: SocketAddr,
        intercept_state: Arc<InterceptState>,
        ui_tx: mpsc::UnboundedSender<ProxyToUi>,
    ) -> Self {
        Self {
            active_tab: Tab::History,
            should_quit: false,
            store: InMemoryStore::new(),
            bind_addr,
            intercept_state,
            ui_tx,
            history_selected: 0,
            history_detail_open: false,
            history_scroll: 0,
            intercept_queue: VecDeque::new(),
            intercept_scroll: 0,
            editing_intercept: false,
            edit_buffer: Vec::new(),
            edit_cursor_line: 0,
            edit_cursor_col: 0,
            repeater_lines: Vec::new(),
            repeater_original: None,
            repeater_response: None,
            repeater_error: None,
            repeater_pending: false,
            repeater_editing: false,
            repeater_cursor_line: 0,
            repeater_cursor_col: 0,
            repeater_req_scroll: 0,
            repeater_resp_scroll: 0,
            show_help: false,
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

            if self.editing_intercept {
                self.handle_editor_key(key, EditorTarget::Intercept);
                return;
            }

            if self.repeater_editing {
                self.handle_editor_key(key, EditorTarget::Repeater);
                return;
            }

            if self.handle_global_key(key) {
                return;
            }
            match self.active_tab {
                Tab::History => self.handle_history_key(key),
                Tab::Proxy => self.handle_proxy_key(key),
                Tab::Repeater => self.handle_repeater_key(key),
            }
        }
    }

    fn handle_global_key(&mut self, key: KeyEvent) -> bool {
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                self.should_quit = true;
                true
            }
            (KeyModifiers::NONE, KeyCode::Char('q')) => {
                if !self.history_detail_open {
                    self.should_quit = true;
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
            (KeyModifiers::NONE, KeyCode::Char('?')) => {
                self.show_help = true;
                true
            }
            _ => false,
        }
    }

    fn handle_proxy_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('i') => {
                let now_enabled = self.intercept_state.toggle();
                if !now_enabled {
                    self.intercept_state.forward_all();
                    self.intercept_queue.clear();
                }
            }
            KeyCode::Char('f') => {
                if let Some(req) = self.intercept_queue.pop_front() {
                    self.intercept_state
                        .resolve(req.id, InterceptDecision::Forward);
                    self.intercept_scroll = 0;
                }
            }
            KeyCode::Char('d') => {
                if let Some(req) = self.intercept_queue.pop_front() {
                    self.store.mark_dropped(req.id);
                    self.intercept_state
                        .resolve(req.id, InterceptDecision::Drop);
                    self.intercept_scroll = 0;
                }
            }
            KeyCode::Char('e') => {
                if let Some(req) = self.intercept_queue.front() {
                    self.edit_buffer = codec::request_to_lines(req);
                    self.edit_cursor_line = 0;
                    self.edit_cursor_col = 0;
                    self.editing_intercept = true;
                    self.intercept_scroll = 0;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.intercept_scroll = self.intercept_scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.intercept_scroll += 1;
            }
            _ => {}
        }
    }

    fn handle_history_key(&mut self, key: KeyEvent) {
        let entry_count = self.store.len();

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.history_detail_open {
                    self.history_scroll = self.history_scroll.saturating_sub(1);
                } else if self.history_selected > 0 {
                    self.history_selected -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.history_detail_open {
                    self.history_scroll += 1;
                } else if entry_count > 0 && self.history_selected < entry_count - 1 {
                    self.history_selected += 1;
                }
            }
            KeyCode::Home | KeyCode::Char('g') => {
                if !self.history_detail_open {
                    self.history_selected = 0;
                }
            }
            KeyCode::End | KeyCode::Char('G') => {
                if !self.history_detail_open && entry_count > 0 {
                    self.history_selected = entry_count - 1;
                }
            }
            KeyCode::Enter => {
                if self.history_detail_open {
                    self.history_detail_open = false;
                    self.history_scroll = 0;
                } else if entry_count > 0 {
                    self.history_detail_open = true;
                    self.history_scroll = 0;
                }
            }
            KeyCode::Esc => {
                if self.history_detail_open {
                    self.history_detail_open = false;
                    self.history_scroll = 0;
                }
            }
            KeyCode::Char('r') => {
                if entry_count > 0 {
                    self.send_to_repeater();
                }
            }
            _ => {}
        }
    }

    fn handle_repeater_key(&mut self, key: KeyEvent) {
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Enter) => {
                self.repeater_send();
            }
            (KeyModifiers::NONE, KeyCode::Char('e')) => {
                if !self.repeater_lines.is_empty() {
                    self.repeater_editing = true;
                    self.repeater_cursor_line = 0;
                    self.repeater_cursor_col = 0;
                }
            }
            (KeyModifiers::NONE, KeyCode::Char('j') | KeyCode::Down) => {
                self.repeater_req_scroll += 1;
            }
            (KeyModifiers::NONE, KeyCode::Char('k') | KeyCode::Up) => {
                self.repeater_req_scroll = self.repeater_req_scroll.saturating_sub(1);
            }
            (KeyModifiers::SHIFT, KeyCode::Char('J')) => {
                self.repeater_resp_scroll += 1;
            }
            (KeyModifiers::SHIFT, KeyCode::Char('K')) => {
                self.repeater_resp_scroll = self.repeater_resp_scroll.saturating_sub(1);
            }
            _ => {}
        }
    }

    fn handle_editor_key(&mut self, key: KeyEvent, target: EditorTarget) {
        let (lines, cursor_line, cursor_col) = match target {
            EditorTarget::Intercept => (
                &mut self.edit_buffer,
                &mut self.edit_cursor_line,
                &mut self.edit_cursor_col,
            ),
            EditorTarget::Repeater => (
                &mut self.repeater_lines,
                &mut self.repeater_cursor_line,
                &mut self.repeater_cursor_col,
            ),
        };

        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Enter) => {
                match target {
                    EditorTarget::Intercept => {
                        if let Some(original) = self.intercept_queue.pop_front() {
                            let edited =
                                codec::lines_to_request(&self.edit_buffer, &original);
                            self.intercept_state
                                .resolve(original.id, InterceptDecision::ForwardEdited(edited));
                        }
                        self.editing_intercept = false;
                        self.edit_buffer.clear();
                        self.intercept_scroll = 0;
                    }
                    EditorTarget::Repeater => {
                        self.repeater_editing = false;
                        self.repeater_send();
                    }
                }
                return;
            }
            (_, KeyCode::Esc) => {
                match target {
                    EditorTarget::Intercept => {
                        self.editing_intercept = false;
                        self.edit_buffer.clear();
                    }
                    EditorTarget::Repeater => {
                        self.repeater_editing = false;
                    }
                }
                return;
            }
            (_, KeyCode::Enter) => {
                match target {
                    EditorTarget::Intercept => {
                        if let Some(original) = self.intercept_queue.pop_front() {
                            let edited =
                                codec::lines_to_request(&self.edit_buffer, &original);
                            self.intercept_state
                                .resolve(original.id, InterceptDecision::ForwardEdited(edited));
                        }
                        self.editing_intercept = false;
                        self.edit_buffer.clear();
                        self.intercept_scroll = 0;
                    }
                    EditorTarget::Repeater => {
                        // In repeater editor, Enter inserts a newline
                        let col = (*cursor_col).min(lines[*cursor_line].len());
                        let rest = lines[*cursor_line][col..].to_string();
                        lines[*cursor_line].truncate(col);
                        *cursor_line += 1;
                        lines.insert(*cursor_line, rest);
                        *cursor_col = 0;
                    }
                }
                return;
            }
            _ => {}
        }

        match key.code {
            KeyCode::Up => {
                if *cursor_line > 0 {
                    *cursor_line -= 1;
                    *cursor_col = (*cursor_col).min(lines[*cursor_line].len());
                }
            }
            KeyCode::Down => {
                if *cursor_line + 1 < lines.len() {
                    *cursor_line += 1;
                    *cursor_col = (*cursor_col).min(lines[*cursor_line].len());
                }
            }
            KeyCode::Left => {
                if *cursor_col > 0 {
                    *cursor_col -= 1;
                }
            }
            KeyCode::Right => {
                let line_len = lines[*cursor_line].len();
                if *cursor_col < line_len {
                    *cursor_col += 1;
                }
            }
            KeyCode::Home => {
                *cursor_col = 0;
            }
            KeyCode::End => {
                *cursor_col = lines[*cursor_line].len();
            }
            KeyCode::Char(c) => {
                if *cursor_line < lines.len() {
                    let col = (*cursor_col).min(lines[*cursor_line].len());
                    lines[*cursor_line].insert(col, c);
                    *cursor_col = col + 1;
                }
            }
            KeyCode::Backspace => {
                if *cursor_col > 0 && *cursor_line < lines.len() {
                    *cursor_col -= 1;
                    lines[*cursor_line].remove(*cursor_col);
                } else if *cursor_col == 0 && *cursor_line > 0 {
                    let current = lines.remove(*cursor_line);
                    *cursor_line -= 1;
                    *cursor_col = lines[*cursor_line].len();
                    lines[*cursor_line].push_str(&current);
                }
            }
            KeyCode::Delete => {
                if *cursor_line < lines.len() {
                    let line_len = lines[*cursor_line].len();
                    if *cursor_col < line_len {
                        lines[*cursor_line].remove(*cursor_col);
                    } else if *cursor_line + 1 < lines.len() {
                        let next = lines.remove(*cursor_line + 1);
                        lines[*cursor_line].push_str(&next);
                    }
                }
            }
            _ => {}
        }
    }

    fn send_to_repeater(&mut self) {
        let entries = self.store.entries();
        if let Some(entry) = entries.get(self.history_selected) {
            let req = &entry.request;
            self.repeater_lines = codec::request_to_lines(req);
            self.repeater_original = Some(req.clone());
            self.repeater_response = None;
            self.repeater_error = None;
            self.repeater_pending = false;
            self.repeater_editing = false;
            self.repeater_cursor_line = 0;
            self.repeater_cursor_col = 0;
            self.repeater_req_scroll = 0;
            self.repeater_resp_scroll = 0;
            self.active_tab = Tab::Repeater;
        }
    }

    fn repeater_send(&mut self) {
        if self.repeater_lines.is_empty() || self.repeater_pending {
            return;
        }

        let original = self.repeater_original.clone().unwrap_or(RequestData {
            id: RequestId::next(),
            method: "GET".into(),
            uri: "/".into(),
            host: "localhost".into(),
            version: crate::http::models::HttpVersion::Http11,
            headers: Vec::new(),
            body: bytes::Bytes::new(),
            is_tls: false,
            timestamp: std::time::SystemTime::now(),
        });

        let request = codec::lines_to_request(&self.repeater_lines, &original);
        self.repeater_original = Some(request.clone());
        self.repeater_pending = true;
        self.repeater_response = None;
        self.repeater_error = None;
        self.repeater_resp_scroll = 0;

        let ui_tx = self.ui_tx.clone();
        tokio::spawn(async move {
            repeater::send_request(request, ui_tx).await;
        });
    }

    pub fn handle_proxy_message(&mut self, msg: ProxyToUi) {
        match msg {
            ProxyToUi::RequestCaptured(req) => {
                self.store.insert(req);
                if !self.history_detail_open && self.store.len() > 1 {
                    self.history_selected = self.store.len() - 1;
                }
            }
            ProxyToUi::ResponseReceived(id, resp) => {
                self.store.update_response(id, resp);
            }
            ProxyToUi::RequestError(id, err) => {
                self.store.mark_error(id, err);
            }
            ProxyToUi::InterceptedRequest(req) => {
                self.intercept_queue.push_back(req);
            }
            ProxyToUi::RepeaterResponse(resp) => {
                self.repeater_pending = false;
                self.repeater_response = Some(resp);
                self.repeater_error = None;
            }
            ProxyToUi::RepeaterError(err) => {
                self.repeater_pending = false;
                self.repeater_error = Some(err);
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

                if *tab == Tab::Proxy && !self.intercept_queue.is_empty() {
                    spans.push(Span::styled(
                        format!(" ({})", self.intercept_queue.len()),
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

        let tabs = Tabs::new(titles)
            .block(Block::default().borders(Borders::ALL).title(" crowbar "))
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
        }
    }

    fn render_status_bar(&self, frame: &mut Frame, area: Rect) {
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

        let intercept_span = if self.intercept_enabled() {
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

        let status = Line::from(vec![
            intercept_span,
            Span::raw(format!(" {} ", self.bind_addr)),
            Span::raw(" | "),
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

    fn render_help_overlay(&self, frame: &mut Frame) {
        let area = frame.area();
        let width = 52.min(area.width.saturating_sub(4));
        let height = 26.min(area.height.saturating_sub(4));
        let x = (area.width.saturating_sub(width)) / 2;
        let y = (area.height.saturating_sub(height)) / 2;
        let popup = Rect::new(x, y, width, height);

        let key = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
        let dim = Style::default().fg(Color::DarkGray);
        let section = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);

        let lines = vec![
            Line::from(Span::styled("Global", section)),
            Line::from(vec![Span::styled("  Tab/Shift+Tab  ", key), Span::raw("Cycle tabs")]),
            Line::from(vec![Span::styled("  1/2/3          ", key), Span::raw("Proxy / History / Repeater")]),
            Line::from(vec![Span::styled("  ?              ", key), Span::raw("Toggle this help")]),
            Line::from(vec![Span::styled("  q / Ctrl+C     ", key), Span::raw("Quit")]),
            Line::raw(""),
            Line::from(Span::styled("Proxy (Intercept)", section)),
            Line::from(vec![Span::styled("  i              ", key), Span::raw("Toggle intercept on/off")]),
            Line::from(vec![Span::styled("  f              ", key), Span::raw("Forward intercepted request")]),
            Line::from(vec![Span::styled("  d              ", key), Span::raw("Drop intercepted request")]),
            Line::from(vec![Span::styled("  e              ", key), Span::raw("Edit intercepted request")]),
            Line::from(vec![Span::styled("  j/k            ", key), Span::raw("Scroll request")]),
            Line::raw(""),
            Line::from(Span::styled("History", section)),
            Line::from(vec![Span::styled("  j/k            ", key), Span::raw("Navigate / scroll")]),
            Line::from(vec![Span::styled("  g/G            ", key), Span::raw("Jump to first / last")]),
            Line::from(vec![Span::styled("  Enter          ", key), Span::raw("Toggle detail view")]),
            Line::from(vec![Span::styled("  r              ", key), Span::raw("Send to repeater")]),
            Line::raw(""),
            Line::from(Span::styled("Repeater", section)),
            Line::from(vec![Span::styled("  Ctrl+Enter     ", key), Span::raw("Send request")]),
            Line::from(vec![Span::styled("  e              ", key), Span::raw("Edit request")]),
            Line::from(vec![Span::styled("  j/k            ", key), Span::raw("Scroll request")]),
            Line::from(vec![Span::styled("  J/K            ", key), Span::raw("Scroll response")]),
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
