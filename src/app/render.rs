use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Tabs, Wrap};
use ratatui::Frame;

use crate::editor::EditorMode;
use crate::http::models::EntryState;
use crate::tui::tabs::history_tab;
use crate::tui::tabs::proxy_tab;
use crate::tui::tabs::repeater_tab;
use crate::tui::tabs::rules_tab;
use crate::tui::tabs::tools_tab;
use crate::tui::tabs::Tab;

use super::App;

impl App {
    pub fn prepare_render(&mut self) {
        self.store.refresh_filter_cache(&self.history.filter);
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
        let (complete, errors) = self.store.entries().iter().fold((0, 0), |(c, e), entry| {
            match entry.state {
                EntryState::Complete => (c + 1, e),
                EntryState::Error => (c, e + 1),
                _ => (c, e),
            }
        });

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
