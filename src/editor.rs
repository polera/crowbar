use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use serde::Deserialize;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditorMode {
    #[default]
    Default,
    Vim,
}

impl EditorMode {
    pub fn toggle(self) -> Self {
        match self {
            EditorMode::Default => EditorMode::Vim,
            EditorMode::Vim => EditorMode::Default,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            EditorMode::Default => "DEFAULT",
            EditorMode::Vim => "VIM",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimMode {
    Normal,
    Insert,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorAction {
    Consumed,
    ExitEditor,
    Enter,
    CtrlEnter,
    Custom(KeyEvent),
}

pub struct TextEditor {
    pub lines: Vec<String>,
    pub cursor_line: usize,
    pub cursor_col: usize,
    pub mode: EditorMode,
    pub vim_mode: VimMode,
    pending_key: Option<char>,
    undo_stack: std::collections::VecDeque<(Vec<String>, usize, usize)>,
}

impl TextEditor {
    pub fn new(lines: Vec<String>, mode: EditorMode) -> Self {
        let lines = if lines.is_empty() {
            vec![String::new()]
        } else {
            lines
        };
        Self {
            lines,
            cursor_line: 0,
            cursor_col: 0,
            mode,
            vim_mode: VimMode::Normal,
            pending_key: None,
            undo_stack: std::collections::VecDeque::new(),
        }
    }

    pub fn has_content(&self) -> bool {
        !(self.lines.is_empty() || self.lines.len() == 1 && self.lines[0].is_empty())
    }

    pub fn set_mode(&mut self, mode: EditorMode) {
        self.mode = mode;
        if mode == EditorMode::Vim {
            self.vim_mode = VimMode::Normal;
            self.clamp_cursor_normal();
        }
        self.pending_key = None;
    }

    pub fn mode_label(&self) -> &'static str {
        match self.mode {
            EditorMode::Default => "",
            EditorMode::Vim => match self.vim_mode {
                VimMode::Normal => "NORMAL",
                VimMode::Insert => "INSERT",
            },
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> EditorAction {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Enter {
            return EditorAction::CtrlEnter;
        }

        match self.mode {
            EditorMode::Default => self.handle_default_key(key),
            EditorMode::Vim => match self.vim_mode {
                VimMode::Normal => self.handle_vim_normal_key(key),
                VimMode::Insert => self.handle_vim_insert_key(key),
            },
        }
    }

    pub fn insert_newline(&mut self) {
        self.save_undo();
        let col = self.cursor_col.min(self.current_line_len());
        let rest = self.lines[self.cursor_line][col..].to_string();
        self.lines[self.cursor_line].truncate(col);
        self.cursor_line += 1;
        self.lines.insert(self.cursor_line, rest);
        self.cursor_col = 0;
    }

    pub fn clear(&mut self) {
        self.save_undo();
        self.lines = vec![String::new()];
        self.cursor_line = 0;
        self.cursor_col = 0;
    }

    pub fn gutter_width(&self) -> usize {
        format!("{}", self.lines.len()).len().max(2)
    }

    pub fn render_lines(&self, editing: bool) -> Vec<Line<'_>> {
        let gw = self.gutter_width();
        let num_style = Style::default().fg(Color::DarkGray);

        let mut result = Vec::with_capacity(self.lines.len());
        for (i, line) in self.lines.iter().enumerate() {
            let num_span = Span::styled(
                format!("{:>width$} ", i + 1, width = gw),
                num_style,
            );

            if editing && i == self.cursor_line {
                let char_count = line.chars().count();
                let col = self.cursor_col.min(char_count);
                let mut indices = line.char_indices();
                let col_byte = indices.nth(col).map(|(i, _)| i).unwrap_or(line.len());
                let before = &line[..col_byte];
                let (cursor_char, after) = if col < char_count {
                    let next_byte = line[col_byte..].chars().next().map(|c| col_byte + c.len_utf8()).unwrap_or(line.len());
                    (&line[col_byte..next_byte], &line[next_byte..])
                } else {
                    (" ", "")
                };
                result.push(Line::from(vec![
                    num_span,
                    Span::raw(before),
                    Span::styled(
                        cursor_char,
                        Style::default().bg(Color::White).fg(Color::Black),
                    ),
                    Span::raw(after),
                ]));
            } else {
                result.push(Line::from(vec![
                    num_span,
                    Span::raw(line.as_str()),
                ]));
            }
        }
        result
    }

    // --- Default mode ---

    fn handle_default_key(&mut self, key: KeyEvent) -> EditorAction {
        if key.code == KeyCode::Esc {
            return EditorAction::ExitEditor;
        }
        self.handle_editing_key(key)
    }

    // --- Vim insert mode ---

    fn handle_vim_insert_key(&mut self, key: KeyEvent) -> EditorAction {
        if key.code == KeyCode::Esc {
            self.vim_mode = VimMode::Normal;
            self.clamp_cursor_normal();
            self.pending_key = None;
            return EditorAction::Consumed;
        }
        self.handle_editing_key(key)
    }

    // --- Shared editing keys (default + vim insert) ---

    fn handle_editing_key(&mut self, key: KeyEvent) -> EditorAction {
        match (key.modifiers, key.code) {
            (_, KeyCode::Enter) => EditorAction::Enter,
            (_, KeyCode::Char(c)) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.save_undo();
                let col = self.cursor_col.min(self.current_line_len());
                self.lines[self.cursor_line].insert(col, c);
                self.cursor_col = col + 1;
                EditorAction::Consumed
            }
            (_, KeyCode::Backspace) => {
                self.handle_backspace();
                EditorAction::Consumed
            }
            (_, KeyCode::Delete) => {
                self.handle_delete();
                EditorAction::Consumed
            }
            (_, KeyCode::Up) => {
                self.move_up();
                EditorAction::Consumed
            }
            (_, KeyCode::Down) => {
                self.move_down();
                EditorAction::Consumed
            }
            (_, KeyCode::Left) => {
                self.move_left();
                EditorAction::Consumed
            }
            (_, KeyCode::Right) => {
                self.move_right();
                EditorAction::Consumed
            }
            (KeyModifiers::CONTROL, KeyCode::Home) => {
                self.cursor_line = 0;
                self.cursor_col = 0;
                EditorAction::Consumed
            }
            (KeyModifiers::CONTROL, KeyCode::End) => {
                self.cursor_line = self.lines.len().saturating_sub(1);
                self.cursor_col = self.current_line_len();
                EditorAction::Consumed
            }
            (_, KeyCode::Home) => {
                self.cursor_col = 0;
                EditorAction::Consumed
            }
            (_, KeyCode::End) => {
                self.cursor_col = self.current_line_len();
                EditorAction::Consumed
            }
            _ => EditorAction::Custom(key),
        }
    }

    // --- Vim normal mode ---

    fn handle_vim_normal_key(&mut self, key: KeyEvent) -> EditorAction {
        if let Some(pending) = self.pending_key.take() {
            return self.handle_vim_pending(pending, key);
        }

        match (key.modifiers, key.code) {
            (_, KeyCode::Esc) => EditorAction::ExitEditor,

            // Movement
            (_, KeyCode::Char('h') | KeyCode::Left) => {
                self.move_left();
                EditorAction::Consumed
            }
            (_, KeyCode::Char('j') | KeyCode::Down) => {
                self.move_down();
                self.clamp_cursor_normal();
                EditorAction::Consumed
            }
            (_, KeyCode::Char('k') | KeyCode::Up) => {
                self.move_up();
                self.clamp_cursor_normal();
                EditorAction::Consumed
            }
            (_, KeyCode::Char('l') | KeyCode::Right) => {
                self.move_right_normal();
                EditorAction::Consumed
            }
            (_, KeyCode::Home | KeyCode::Char('0')) => {
                self.cursor_col = 0;
                EditorAction::Consumed
            }
            (_, KeyCode::End) => {
                self.cursor_col = self.current_line_len().saturating_sub(1);
                EditorAction::Consumed
            }
            (_, KeyCode::Char('$')) => {
                self.cursor_col = self.current_line_len().saturating_sub(1);
                EditorAction::Consumed
            }
            (_, KeyCode::Char('^')) => {
                let line = &self.lines[self.cursor_line];
                self.cursor_col = line.len() - line.trim_start().len();
                EditorAction::Consumed
            }

            // Word motion
            (_, KeyCode::Char('w')) => {
                self.word_forward();
                self.clamp_cursor_normal();
                EditorAction::Consumed
            }
            (_, KeyCode::Char('b')) => {
                self.word_backward();
                EditorAction::Consumed
            }
            (_, KeyCode::Char('e')) => {
                self.word_end();
                EditorAction::Consumed
            }

            // Insert mode entry
            (_, KeyCode::Char('i')) => {
                self.vim_mode = VimMode::Insert;
                EditorAction::Consumed
            }
            (_, KeyCode::Char('a')) => {
                self.vim_mode = VimMode::Insert;
                let len = self.current_line_len();
                if len > 0 {
                    self.cursor_col = (self.cursor_col + 1).min(len);
                }
                EditorAction::Consumed
            }
            (KeyModifiers::SHIFT, KeyCode::Char('I')) => {
                self.vim_mode = VimMode::Insert;
                self.cursor_col = 0;
                EditorAction::Consumed
            }
            (KeyModifiers::SHIFT, KeyCode::Char('A')) => {
                self.vim_mode = VimMode::Insert;
                self.cursor_col = self.current_line_len();
                EditorAction::Consumed
            }
            (_, KeyCode::Char('o')) => {
                self.save_undo();
                self.cursor_line += 1;
                self.lines.insert(self.cursor_line, String::new());
                self.cursor_col = 0;
                self.vim_mode = VimMode::Insert;
                EditorAction::Consumed
            }
            (KeyModifiers::SHIFT, KeyCode::Char('O')) => {
                self.save_undo();
                self.lines.insert(self.cursor_line, String::new());
                self.cursor_col = 0;
                self.vim_mode = VimMode::Insert;
                EditorAction::Consumed
            }

            // Deletion
            (_, KeyCode::Char('x')) => {
                self.save_undo();
                let len = self.current_line_len();
                if len > 0 && self.cursor_col < len {
                    self.lines[self.cursor_line].remove(self.cursor_col);
                    self.clamp_cursor_normal();
                }
                EditorAction::Consumed
            }
            (KeyModifiers::SHIFT, KeyCode::Char('D')) => {
                self.save_undo();
                let col = self.cursor_col.min(self.current_line_len());
                self.lines[self.cursor_line].truncate(col);
                self.clamp_cursor_normal();
                EditorAction::Consumed
            }

            // Pending multi-key commands
            (_, KeyCode::Char('d')) => {
                self.pending_key = Some('d');
                EditorAction::Consumed
            }
            (_, KeyCode::Char('g')) => {
                self.pending_key = Some('g');
                EditorAction::Consumed
            }

            // Go to last line
            (KeyModifiers::SHIFT, KeyCode::Char('G')) => {
                self.cursor_line = self.lines.len().saturating_sub(1);
                self.clamp_cursor_normal();
                EditorAction::Consumed
            }

            // Undo
            (_, KeyCode::Char('u')) => {
                self.undo();
                EditorAction::Consumed
            }

            // Cancel / quit
            (_, KeyCode::Char('q')) => EditorAction::ExitEditor,

            _ => EditorAction::Consumed,
        }
    }

    fn handle_vim_pending(&mut self, pending: char, key: KeyEvent) -> EditorAction {
        match (pending, key.code) {
            // dd - delete line
            ('d', KeyCode::Char('d')) => {
                self.save_undo();
                if self.lines.len() > 1 {
                    self.lines.remove(self.cursor_line);
                    if self.cursor_line >= self.lines.len() {
                        self.cursor_line = self.lines.len().saturating_sub(1);
                    }
                } else {
                    self.lines[0].clear();
                }
                self.clamp_cursor_normal();
                EditorAction::Consumed
            }
            // dw - delete word
            ('d', KeyCode::Char('w')) => {
                self.save_undo();
                self.delete_word_forward();
                self.clamp_cursor_normal();
                EditorAction::Consumed
            }
            // d$ - delete to end of line
            ('d', KeyCode::Char('$')) => {
                self.save_undo();
                let col = self.cursor_col.min(self.current_line_len());
                self.lines[self.cursor_line].truncate(col);
                self.clamp_cursor_normal();
                EditorAction::Consumed
            }
            // gg - go to beginning of input
            ('g', KeyCode::Char('g')) => {
                self.cursor_line = 0;
                self.cursor_col = 0;
                EditorAction::Consumed
            }
            // Unknown continuation — discard both
            _ => EditorAction::Consumed,
        }
    }

    // --- Shared editing primitives ---

    fn handle_backspace(&mut self) {
        if self.cursor_col > 0 && self.cursor_line < self.lines.len() {
            self.save_undo();
            self.cursor_col -= 1;
            self.lines[self.cursor_line].remove(self.cursor_col);
        } else if self.cursor_col == 0 && self.cursor_line > 0 {
            self.save_undo();
            let current = self.lines.remove(self.cursor_line);
            self.cursor_line -= 1;
            self.cursor_col = self.lines[self.cursor_line].len();
            self.lines[self.cursor_line].push_str(&current);
        }
    }

    fn handle_delete(&mut self) {
        if self.cursor_line >= self.lines.len() {
            return;
        }
        let line_len = self.current_line_len();
        if self.cursor_col < line_len {
            self.save_undo();
            self.lines[self.cursor_line].remove(self.cursor_col);
        } else if self.cursor_line + 1 < self.lines.len() {
            self.save_undo();
            let next = self.lines.remove(self.cursor_line + 1);
            self.lines[self.cursor_line].push_str(&next);
        }
    }

    fn move_up(&mut self) {
        if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.cursor_col = self.cursor_col.min(self.current_line_len());
        }
    }

    fn move_down(&mut self) {
        if self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            self.cursor_col = self.cursor_col.min(self.current_line_len());
        }
    }

    fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        }
    }

    fn move_right(&mut self) {
        let len = self.current_line_len();
        if self.cursor_col < len {
            self.cursor_col += 1;
        }
    }

    fn move_right_normal(&mut self) {
        let max = self.current_line_len().saturating_sub(1);
        if self.cursor_col < max {
            self.cursor_col += 1;
        }
    }

    fn current_line_len(&self) -> usize {
        self.lines[self.cursor_line].len()
    }

    fn clamp_cursor_normal(&mut self) {
        let max = self.current_line_len().saturating_sub(1);
        if self.current_line_len() == 0 {
            self.cursor_col = 0;
        } else if self.cursor_col > max {
            self.cursor_col = max;
        }
    }

    // --- Word motion ---

    fn word_forward(&mut self) {
        let line: Vec<char> = self.lines[self.cursor_line].chars().collect();
        let mut col = self.cursor_col;

        if col < line.len() {
            let start_is_word = is_word_char(line[col]);
            while col < line.len()
                && is_word_char(line[col]) == start_is_word
                && !line[col].is_whitespace()
            {
                col += 1;
            }
            while col < line.len() && line[col].is_whitespace() {
                col += 1;
            }
        }

        if col >= line.len() && self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            let next_line: Vec<char> = self.lines[self.cursor_line].chars().collect();
            col = 0;
            while col < next_line.len() && next_line[col].is_whitespace() {
                col += 1;
            }
        }

        self.cursor_col = col;
    }

    fn word_backward(&mut self) {
        let line: Vec<char> = self.lines[self.cursor_line].chars().collect();
        let mut col = self.cursor_col;

        if col == 0 {
            if self.cursor_line > 0 {
                self.cursor_line -= 1;
                col = self.lines[self.cursor_line].len();
                let prev_line: Vec<char> = self.lines[self.cursor_line].chars().collect();
                while col > 0 && prev_line[col - 1].is_whitespace() {
                    col -= 1;
                }
                if col > 0 {
                    let is_word = is_word_char(prev_line[col - 1]);
                    while col > 0 && is_word_char(prev_line[col - 1]) == is_word && !prev_line[col - 1].is_whitespace() {
                        col -= 1;
                    }
                }
            }
            self.cursor_col = col;
            return;
        }

        while col > 0 && line[col - 1].is_whitespace() {
            col -= 1;
        }
        if col > 0 {
            let is_word = is_word_char(line[col - 1]);
            while col > 0 && is_word_char(line[col - 1]) == is_word && !line[col - 1].is_whitespace() {
                col -= 1;
            }
        }

        self.cursor_col = col;
    }

    fn word_end(&mut self) {
        let line: Vec<char> = self.lines[self.cursor_line].chars().collect();
        let mut col = self.cursor_col;

        if col + 1 < line.len() {
            col += 1;
        }
        while col < line.len() && line[col].is_whitespace() {
            col += 1;
        }
        if col < line.len() {
            let is_word = is_word_char(line[col]);
            while col + 1 < line.len()
                && is_word_char(line[col + 1]) == is_word
                && !line[col + 1].is_whitespace()
            {
                col += 1;
            }
        }

        self.cursor_col = col.min(self.current_line_len().saturating_sub(1));
    }

    fn delete_word_forward(&mut self) {
        let line: Vec<char> = self.lines[self.cursor_line].chars().collect();
        let start = self.cursor_col;
        let mut end = start;

        if end < line.len() {
            let start_is_word = is_word_char(line[end]);
            while end < line.len()
                && is_word_char(line[end]) == start_is_word
                && !line[end].is_whitespace()
            {
                end += 1;
            }
            while end < line.len() && line[end].is_whitespace() {
                end += 1;
            }
        }

        if end > start {
            let line_str = &mut self.lines[self.cursor_line];
            line_str.drain(start..end);
        }
    }

    // --- Undo ---

    fn save_undo(&mut self) {
        if self.undo_stack.len() >= 100 {
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back((
            self.lines.clone(),
            self.cursor_line,
            self.cursor_col,
        ));
    }

    fn undo(&mut self) {
        if let Some((lines, line, col)) = self.undo_stack.pop_back() {
            self.lines = lines;
            self.cursor_line = line;
            self.cursor_col = col;
        }
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
