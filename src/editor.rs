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
    undo_stack: std::collections::VecDeque<UndoEntry>,
}

enum UndoEntry {
    Full {
        lines: Vec<String>,
        cursor_line: usize,
        cursor_col: usize,
    },
    Line {
        index: usize,
        content: String,
        cursor_line: usize,
        cursor_col: usize,
    },
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
        let byte_col = self.char_to_byte(col);
        let rest = self.lines[self.cursor_line][byte_col..].to_string();
        self.lines[self.cursor_line].truncate(byte_col);
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
            let num_span = Span::styled(format!("{:>width$} ", i + 1, width = gw), num_style);

            if editing && i == self.cursor_line {
                let char_count = line.chars().count();
                let col = self.cursor_col.min(char_count);
                let mut indices = line.char_indices();
                let col_byte = indices.nth(col).map(|(i, _)| i).unwrap_or(line.len());
                let before = &line[..col_byte];
                let (cursor_char, after) = if col < char_count {
                    let next_byte = line[col_byte..]
                        .chars()
                        .next()
                        .map(|c| col_byte + c.len_utf8())
                        .unwrap_or(line.len());
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
                result.push(Line::from(vec![num_span, Span::raw(line.as_str())]));
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
                self.save_line_undo();
                let col = self.cursor_col.min(self.current_line_len());
                let byte_col = self.char_to_byte(col);
                self.lines[self.cursor_line].insert(byte_col, c);
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
                self.cursor_col = line.chars().take_while(|c| c.is_whitespace()).count();
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
                self.save_line_undo();
                let len = self.current_line_len();
                if len > 0 && self.cursor_col < len {
                    let byte_col = self.char_to_byte(self.cursor_col);
                    let next_byte = self.lines[self.cursor_line][byte_col..]
                        .chars()
                        .next()
                        .map(|c| byte_col + c.len_utf8())
                        .unwrap_or(byte_col);
                    self.lines[self.cursor_line].drain(byte_col..next_byte);
                    self.clamp_cursor_normal();
                }
                EditorAction::Consumed
            }
            (KeyModifiers::SHIFT, KeyCode::Char('D')) => {
                self.save_line_undo();
                let col = self.cursor_col.min(self.current_line_len());
                let byte_col = self.char_to_byte(col);
                self.lines[self.cursor_line].truncate(byte_col);
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
                if self.lines.len() > 1 {
                    self.save_undo();
                    self.lines.remove(self.cursor_line);
                    if self.cursor_line >= self.lines.len() {
                        self.cursor_line = self.lines.len().saturating_sub(1);
                    }
                } else {
                    self.save_line_undo();
                    self.lines[0].clear();
                }
                self.clamp_cursor_normal();
                EditorAction::Consumed
            }
            // dw - delete word
            ('d', KeyCode::Char('w')) => {
                self.save_line_undo();
                self.delete_word_forward();
                self.clamp_cursor_normal();
                EditorAction::Consumed
            }
            // d$ - delete to end of line
            ('d', KeyCode::Char('$')) => {
                self.save_line_undo();
                let col = self.cursor_col.min(self.current_line_len());
                let byte_col = self.char_to_byte(col);
                self.lines[self.cursor_line].truncate(byte_col);
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
            self.save_line_undo();
            self.cursor_col -= 1;
            let byte_col = self.char_to_byte(self.cursor_col);
            let next_byte = self.lines[self.cursor_line][byte_col..]
                .chars()
                .next()
                .map(|c| byte_col + c.len_utf8())
                .unwrap_or(byte_col);
            self.lines[self.cursor_line].drain(byte_col..next_byte);
        } else if self.cursor_col == 0 && self.cursor_line > 0 {
            self.save_undo();
            let current = self.lines.remove(self.cursor_line);
            self.cursor_line -= 1;
            self.cursor_col = self.current_line_len();
            self.lines[self.cursor_line].push_str(&current);
        }
    }

    fn handle_delete(&mut self) {
        if self.cursor_line >= self.lines.len() {
            return;
        }
        let line_len = self.current_line_len();
        if self.cursor_col < line_len {
            self.save_line_undo();
            let byte_col = self.char_to_byte(self.cursor_col);
            let next_byte = self.lines[self.cursor_line][byte_col..]
                .chars()
                .next()
                .map(|c| byte_col + c.len_utf8())
                .unwrap_or(byte_col);
            self.lines[self.cursor_line].drain(byte_col..next_byte);
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
        self.lines[self.cursor_line].chars().count()
    }

    fn char_to_byte(&self, char_col: usize) -> usize {
        self.lines[self.cursor_line]
            .char_indices()
            .nth(char_col)
            .map(|(i, _)| i)
            .unwrap_or(self.lines[self.cursor_line].len())
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
        let line = &self.lines[self.cursor_line];
        let line_len = line.chars().count();
        let mut col = self.cursor_col;
        let mut chars = line.chars().skip(col).peekable();

        if let Some(&first) = chars.peek() {
            let start_is_word = is_word_char(first);
            while chars
                .next_if(|ch| is_word_char(*ch) == start_is_word && !ch.is_whitespace())
                .is_some()
            {
                col += 1;
            }
            while chars.next_if(|ch| ch.is_whitespace()).is_some() {
                col += 1;
            }
        }

        if col >= line_len && self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            col = 0;
            for ch in self.lines[self.cursor_line].chars() {
                if !ch.is_whitespace() {
                    break;
                }
                col += 1;
            }
        }

        self.cursor_col = col;
    }

    fn word_backward(&mut self) {
        let mut col = self.cursor_col;

        if col == 0 {
            if self.cursor_line > 0 {
                self.cursor_line -= 1;
                col = self.lines[self.cursor_line].chars().count();
                col = word_start_before(&self.lines[self.cursor_line], col);
            }
            self.cursor_col = col;
            return;
        }

        self.cursor_col = word_start_before(&self.lines[self.cursor_line], col);
    }

    fn word_end(&mut self) {
        let line = &self.lines[self.cursor_line];
        let line_len = line.chars().count();
        let mut col = self.cursor_col;

        if col + 1 < line_len {
            col += 1;
        }
        let mut chars = line.chars().skip(col).peekable();
        while chars.next_if(|ch| ch.is_whitespace()).is_some() {
            col += 1;
        }
        if let Some(first) = chars.next() {
            let is_word = is_word_char(first);
            while chars
                .next_if(|ch| is_word_char(*ch) == is_word && !ch.is_whitespace())
                .is_some()
            {
                col += 1;
            }
        }

        self.cursor_col = col.min(self.current_line_len().saturating_sub(1));
    }

    fn delete_word_forward(&mut self) {
        let start = self.cursor_col;
        let mut end = start;
        let mut chars = self.lines[self.cursor_line].chars().skip(start).peekable();

        if let Some(&first) = chars.peek() {
            let start_is_word = is_word_char(first);
            while chars
                .next_if(|ch| is_word_char(*ch) == start_is_word && !ch.is_whitespace())
                .is_some()
            {
                end += 1;
            }
            while chars.next_if(|ch| ch.is_whitespace()).is_some() {
                end += 1;
            }
        }

        if end > start {
            let byte_start = self.char_to_byte(start);
            let byte_end = self.char_to_byte(end);
            self.lines[self.cursor_line].drain(byte_start..byte_end);
        }
    }

    // --- Undo ---

    fn save_undo(&mut self) {
        self.push_undo(UndoEntry::Full {
            lines: self.lines.clone(),
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
        });
    }

    fn save_line_undo(&mut self) {
        self.push_undo(UndoEntry::Line {
            index: self.cursor_line,
            content: self.lines[self.cursor_line].clone(),
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
        });
    }

    fn push_undo(&mut self, entry: UndoEntry) {
        if self.undo_stack.len() >= 100 {
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(entry);
    }

    fn undo(&mut self) {
        match self.undo_stack.pop_back() {
            Some(UndoEntry::Full {
                lines,
                cursor_line,
                cursor_col,
            }) => {
                self.lines = lines;
                self.cursor_line = cursor_line;
                self.cursor_col = cursor_col;
            }
            Some(UndoEntry::Line {
                index,
                content,
                cursor_line,
                cursor_col,
            }) => {
                self.lines[index] = content;
                self.cursor_line = cursor_line;
                self.cursor_col = cursor_col;
            }
            None => {}
        }
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn word_start_before(line: &str, col: usize) -> usize {
    let byte_col = line
        .char_indices()
        .nth(col)
        .map_or(line.len(), |(index, _)| index);
    let mut chars = line[..byte_col].chars().rev().peekable();
    let mut col = col;

    while chars.next_if(|ch| ch.is_whitespace()).is_some() {
        col -= 1;
    }
    if let Some(&last) = chars.peek() {
        let is_word = is_word_char(last);
        while chars
            .next_if(|ch| is_word_char(*ch) == is_word && !ch.is_whitespace())
            .is_some()
        {
            col -= 1;
        }
    }
    col
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn shift_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    fn ctrl_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn editor(text: &str) -> TextEditor {
        let lines: Vec<String> = text.lines().map(String::from).collect();
        TextEditor::new(
            if lines.is_empty() {
                vec![String::new()]
            } else {
                lines
            },
            EditorMode::Default,
        )
    }

    fn vim_editor(text: &str) -> TextEditor {
        let lines: Vec<String> = text.lines().map(String::from).collect();
        TextEditor::new(
            if lines.is_empty() {
                vec![String::new()]
            } else {
                lines
            },
            EditorMode::Vim,
        )
    }

    // --- Constructor / basic state ---

    #[test]
    fn new_empty_lines_gets_one_empty() {
        let ed = TextEditor::new(vec![], EditorMode::Default);
        assert_eq!(ed.lines, vec![""]);
        assert_eq!(ed.cursor_line, 0);
        assert_eq!(ed.cursor_col, 0);
    }

    #[test]
    fn has_content_empty() {
        let ed = editor("");
        assert!(!ed.has_content());
    }

    #[test]
    fn has_content_with_text() {
        let ed = editor("hello");
        assert!(ed.has_content());
    }

    // --- EditorMode ---

    #[test]
    fn editor_mode_toggle() {
        assert_eq!(EditorMode::Default.toggle(), EditorMode::Vim);
        assert_eq!(EditorMode::Vim.toggle(), EditorMode::Default);
    }

    #[test]
    fn editor_mode_label() {
        assert_eq!(EditorMode::Default.label(), "DEFAULT");
        assert_eq!(EditorMode::Vim.label(), "VIM");
    }

    #[test]
    fn mode_label_default() {
        let ed = editor("test");
        assert_eq!(ed.mode_label(), "");
    }

    #[test]
    fn mode_label_vim_normal() {
        let ed = vim_editor("test");
        assert_eq!(ed.mode_label(), "NORMAL");
    }

    #[test]
    fn mode_label_vim_insert() {
        let mut ed = vim_editor("test");
        ed.vim_mode = VimMode::Insert;
        assert_eq!(ed.mode_label(), "INSERT");
    }

    // --- Default mode editing ---

    #[test]
    fn insert_char() {
        let mut ed = editor("");
        ed.handle_key(key(KeyCode::Char('a')));
        assert_eq!(ed.lines[0], "a");
        assert_eq!(ed.cursor_col, 1);
    }

    #[test]
    fn insert_multiple_chars() {
        let mut ed = editor("");
        for c in "hello".chars() {
            ed.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(ed.lines[0], "hello");
        assert_eq!(ed.cursor_col, 5);
    }

    #[test]
    fn backspace_removes_char() {
        let mut ed = editor("abc");
        ed.cursor_col = 3;
        ed.handle_key(key(KeyCode::Backspace));
        assert_eq!(ed.lines[0], "ab");
        assert_eq!(ed.cursor_col, 2);
    }

    #[test]
    fn backspace_at_start_joins_lines() {
        let mut ed = editor("hello\nworld");
        ed.cursor_line = 1;
        ed.cursor_col = 0;
        ed.handle_key(key(KeyCode::Backspace));
        assert_eq!(ed.lines.len(), 1);
        assert_eq!(ed.lines[0], "helloworld");
        assert_eq!(ed.cursor_col, 5);
    }

    #[test]
    fn delete_removes_char() {
        let mut ed = editor("abc");
        ed.cursor_col = 0;
        ed.handle_key(key(KeyCode::Delete));
        assert_eq!(ed.lines[0], "bc");
    }

    #[test]
    fn delete_at_end_joins_lines() {
        let mut ed = editor("hello\nworld");
        ed.cursor_col = 5;
        ed.handle_key(key(KeyCode::Delete));
        assert_eq!(ed.lines.len(), 1);
        assert_eq!(ed.lines[0], "helloworld");
    }

    #[test]
    fn insert_newline_splits() {
        let mut ed = editor("hello world");
        ed.cursor_col = 5;
        ed.insert_newline();
        assert_eq!(ed.lines, vec!["hello", " world"]);
        assert_eq!(ed.cursor_line, 1);
        assert_eq!(ed.cursor_col, 0);
    }

    #[test]
    fn clear_resets() {
        let mut ed = editor("some\ntext\nhere");
        ed.cursor_line = 2;
        ed.cursor_col = 3;
        ed.clear();
        assert_eq!(ed.lines, vec![""]);
        assert_eq!(ed.cursor_line, 0);
        assert_eq!(ed.cursor_col, 0);
    }

    // --- Arrow key movement ---

    #[test]
    fn move_up_down() {
        let mut ed = editor("line1\nline2\nline3");
        ed.handle_key(key(KeyCode::Down));
        assert_eq!(ed.cursor_line, 1);
        ed.handle_key(key(KeyCode::Down));
        assert_eq!(ed.cursor_line, 2);
        ed.handle_key(key(KeyCode::Down)); // at bottom
        assert_eq!(ed.cursor_line, 2);
        ed.handle_key(key(KeyCode::Up));
        assert_eq!(ed.cursor_line, 1);
    }

    #[test]
    fn move_left_right() {
        let mut ed = editor("abc");
        ed.handle_key(key(KeyCode::Right));
        assert_eq!(ed.cursor_col, 1);
        ed.handle_key(key(KeyCode::Right));
        ed.handle_key(key(KeyCode::Right));
        assert_eq!(ed.cursor_col, 3);
        ed.handle_key(key(KeyCode::Right)); // at end
        assert_eq!(ed.cursor_col, 3);
        ed.handle_key(key(KeyCode::Left));
        assert_eq!(ed.cursor_col, 2);
    }

    #[test]
    fn home_end_keys() {
        let mut ed = editor("hello world");
        ed.cursor_col = 5;
        ed.handle_key(key(KeyCode::Home));
        assert_eq!(ed.cursor_col, 0);
        ed.handle_key(key(KeyCode::End));
        assert_eq!(ed.cursor_col, 11);
    }

    #[test]
    fn ctrl_home_end() {
        let mut ed = editor("line1\nline2\nline3");
        ed.cursor_line = 1;
        ed.cursor_col = 3;
        ed.handle_key(ctrl_key(KeyCode::Home));
        assert_eq!(ed.cursor_line, 0);
        assert_eq!(ed.cursor_col, 0);
        ed.handle_key(ctrl_key(KeyCode::End));
        assert_eq!(ed.cursor_line, 2);
        assert_eq!(ed.cursor_col, 5);
    }

    #[test]
    fn cursor_clamps_on_line_change() {
        let mut ed = editor("long line here\nhi");
        ed.cursor_col = 14;
        ed.handle_key(key(KeyCode::Down));
        assert_eq!(ed.cursor_col, 2);
    }

    // --- Esc exits default mode ---

    #[test]
    fn esc_exits_default() {
        let mut ed = editor("test");
        assert_eq!(ed.handle_key(key(KeyCode::Esc)), EditorAction::ExitEditor);
    }

    // --- Ctrl+Enter ---

    #[test]
    fn ctrl_enter() {
        let mut ed = editor("test");
        assert_eq!(
            ed.handle_key(ctrl_key(KeyCode::Enter)),
            EditorAction::CtrlEnter
        );
    }

    // --- Gutter width ---

    #[test]
    fn gutter_width_small() {
        let ed = editor("a\nb");
        assert_eq!(ed.gutter_width(), 2);
    }

    #[test]
    fn gutter_width_100_lines() {
        let lines: Vec<String> = (0..100).map(|i| format!("line {}", i)).collect();
        let ed = TextEditor::new(lines, EditorMode::Default);
        assert_eq!(ed.gutter_width(), 3);
    }

    // --- Undo ---

    #[test]
    fn undo_restores_state() {
        let mut ed = editor("hello");
        ed.cursor_col = 5;
        ed.handle_key(key(KeyCode::Char('!')));
        assert_eq!(ed.lines[0], "hello!");
        ed.handle_key(key(KeyCode::Char('!')));
        assert_eq!(ed.lines[0], "hello!!");
        // Each char insert saves undo, so undo once should restore "hello!"
        let mut ed2 = editor("hello");
        ed2.cursor_col = 5;
        ed2.handle_key(key(KeyCode::Char('!')));
        ed2.handle_key(key(KeyCode::Char('!')));
        ed2.undo();
        assert_eq!(ed2.lines[0], "hello!");
    }

    #[test]
    fn undo_line_edit_preserves_other_lines() {
        let mut ed = editor("first\nsecond");
        ed.cursor_line = 1;
        ed.cursor_col = 6;
        ed.handle_key(key(KeyCode::Char('!')));
        ed.undo();
        assert_eq!(ed.lines, vec!["first", "second"]);
    }

    #[test]
    fn undo_restores_structural_edit() {
        let mut ed = editor("hello world");
        ed.cursor_col = 5;
        ed.insert_newline();
        ed.undo();
        assert_eq!(ed.lines, vec!["hello world"]);
        assert_eq!((ed.cursor_line, ed.cursor_col), (0, 5));
    }

    // --- Vim normal mode ---

    #[test]
    fn vim_hjkl_movement() {
        let mut ed = vim_editor("hello\nworld");
        assert_eq!(ed.vim_mode, VimMode::Normal);

        ed.handle_key(key(KeyCode::Char('l')));
        assert_eq!(ed.cursor_col, 1);
        ed.handle_key(key(KeyCode::Char('l')));
        assert_eq!(ed.cursor_col, 2);
        ed.handle_key(key(KeyCode::Char('h')));
        assert_eq!(ed.cursor_col, 1);
        ed.handle_key(key(KeyCode::Char('j')));
        assert_eq!(ed.cursor_line, 1);
        ed.handle_key(key(KeyCode::Char('k')));
        assert_eq!(ed.cursor_line, 0);
    }

    #[test]
    fn vim_i_enters_insert() {
        let mut ed = vim_editor("hello");
        ed.handle_key(key(KeyCode::Char('i')));
        assert_eq!(ed.vim_mode, VimMode::Insert);
    }

    #[test]
    fn vim_a_enters_insert_after() {
        let mut ed = vim_editor("hello");
        ed.cursor_col = 2;
        ed.handle_key(key(KeyCode::Char('a')));
        assert_eq!(ed.vim_mode, VimMode::Insert);
        assert_eq!(ed.cursor_col, 3);
    }

    #[test]
    fn vim_shift_i_insert_at_start() {
        let mut ed = vim_editor("hello");
        ed.cursor_col = 3;
        ed.handle_key(shift_key(KeyCode::Char('I')));
        assert_eq!(ed.vim_mode, VimMode::Insert);
        assert_eq!(ed.cursor_col, 0);
    }

    #[test]
    fn vim_shift_a_insert_at_end() {
        let mut ed = vim_editor("hello");
        ed.handle_key(shift_key(KeyCode::Char('A')));
        assert_eq!(ed.vim_mode, VimMode::Insert);
        assert_eq!(ed.cursor_col, 5);
    }

    #[test]
    fn vim_esc_from_insert_to_normal() {
        let mut ed = vim_editor("hello");
        ed.handle_key(key(KeyCode::Char('i')));
        assert_eq!(ed.vim_mode, VimMode::Insert);
        ed.handle_key(key(KeyCode::Esc));
        assert_eq!(ed.vim_mode, VimMode::Normal);
    }

    #[test]
    fn vim_o_open_line_below() {
        let mut ed = vim_editor("hello\nworld");
        ed.handle_key(key(KeyCode::Char('o')));
        assert_eq!(ed.lines.len(), 3);
        assert_eq!(ed.cursor_line, 1);
        assert_eq!(ed.lines[1], "");
        assert_eq!(ed.vim_mode, VimMode::Insert);
    }

    #[test]
    fn vim_shift_o_open_line_above() {
        let mut ed = vim_editor("hello\nworld");
        ed.cursor_line = 1;
        ed.handle_key(shift_key(KeyCode::Char('O')));
        assert_eq!(ed.lines.len(), 3);
        assert_eq!(ed.cursor_line, 1);
        assert_eq!(ed.lines[1], "");
        assert_eq!(ed.vim_mode, VimMode::Insert);
    }

    #[test]
    fn vim_x_delete_char() {
        let mut ed = vim_editor("hello");
        ed.cursor_col = 1;
        ed.handle_key(key(KeyCode::Char('x')));
        assert_eq!(ed.lines[0], "hllo");
    }

    #[test]
    fn vim_shift_d_delete_to_end() {
        let mut ed = vim_editor("hello world");
        ed.cursor_col = 5;
        ed.handle_key(shift_key(KeyCode::Char('D')));
        assert_eq!(ed.lines[0], "hello");
    }

    #[test]
    fn vim_dd_delete_line() {
        let mut ed = vim_editor("line1\nline2\nline3");
        ed.cursor_line = 1;
        ed.handle_key(key(KeyCode::Char('d')));
        ed.handle_key(key(KeyCode::Char('d')));
        assert_eq!(ed.lines, vec!["line1", "line3"]);
    }

    #[test]
    fn vim_dd_last_line_clears() {
        let mut ed = vim_editor("only line");
        ed.handle_key(key(KeyCode::Char('d')));
        ed.handle_key(key(KeyCode::Char('d')));
        assert_eq!(ed.lines, vec![""]);
    }

    #[test]
    fn vim_dw_delete_word() {
        let mut ed = vim_editor("hello world");
        ed.cursor_col = 0;
        ed.handle_key(key(KeyCode::Char('d')));
        ed.handle_key(key(KeyCode::Char('w')));
        assert_eq!(ed.lines[0], "world");
    }

    #[test]
    fn vim_d_dollar_delete_to_eol() {
        let mut ed = vim_editor("hello world");
        ed.cursor_col = 5;
        ed.handle_key(key(KeyCode::Char('d')));
        ed.handle_key(key(KeyCode::Char('$')));
        assert_eq!(ed.lines[0], "hello");
    }

    #[test]
    fn vim_gg_go_to_top() {
        let mut ed = vim_editor("a\nb\nc");
        ed.cursor_line = 2;
        ed.handle_key(key(KeyCode::Char('g')));
        ed.handle_key(key(KeyCode::Char('g')));
        assert_eq!(ed.cursor_line, 0);
        assert_eq!(ed.cursor_col, 0);
    }

    #[test]
    fn vim_shift_g_go_to_bottom() {
        let mut ed = vim_editor("a\nb\nc");
        ed.handle_key(shift_key(KeyCode::Char('G')));
        assert_eq!(ed.cursor_line, 2);
    }

    #[test]
    fn vim_0_go_to_start() {
        let mut ed = vim_editor("hello");
        ed.cursor_col = 3;
        ed.handle_key(key(KeyCode::Char('0')));
        assert_eq!(ed.cursor_col, 0);
    }

    #[test]
    fn vim_dollar_go_to_end() {
        let mut ed = vim_editor("hello");
        ed.handle_key(key(KeyCode::Char('$')));
        assert_eq!(ed.cursor_col, 4); // last char in normal mode
    }

    #[test]
    fn vim_caret_first_non_whitespace() {
        let mut ed = vim_editor("   hello");
        ed.handle_key(key(KeyCode::Char('^')));
        assert_eq!(ed.cursor_col, 3);
    }

    #[test]
    fn vim_w_word_forward() {
        let mut ed = vim_editor("hello world foo");
        ed.handle_key(key(KeyCode::Char('w')));
        assert_eq!(ed.cursor_col, 6);
    }

    #[test]
    fn vim_b_word_backward() {
        let mut ed = vim_editor("hello world");
        ed.cursor_col = 8;
        ed.handle_key(key(KeyCode::Char('b')));
        assert_eq!(ed.cursor_col, 6);
    }

    #[test]
    fn vim_e_word_end() {
        let mut ed = vim_editor("hello world");
        ed.handle_key(key(KeyCode::Char('e')));
        assert_eq!(ed.cursor_col, 4);
    }

    #[test]
    fn vim_word_motions_preserve_character_columns() {
        let mut ed = vim_editor("héllo 世界");
        ed.handle_key(key(KeyCode::Char('w')));
        assert_eq!(ed.cursor_col, 6);
        ed.handle_key(key(KeyCode::Char('e')));
        assert_eq!(ed.cursor_col, 7);
        ed.handle_key(key(KeyCode::Char('b')));
        assert_eq!(ed.cursor_col, 6);
    }

    #[test]
    fn vim_u_undo() {
        let mut ed = vim_editor("hello");
        ed.handle_key(key(KeyCode::Char('x'))); // delete 'h'
        assert_eq!(ed.lines[0], "ello");
        ed.handle_key(key(KeyCode::Char('u')));
        assert_eq!(ed.lines[0], "hello");
    }

    #[test]
    fn vim_q_exits() {
        let mut ed = vim_editor("test");
        assert_eq!(
            ed.handle_key(key(KeyCode::Char('q'))),
            EditorAction::ExitEditor
        );
    }

    #[test]
    fn vim_esc_normal_exits() {
        let mut ed = vim_editor("test");
        assert_eq!(ed.handle_key(key(KeyCode::Esc)), EditorAction::ExitEditor);
    }

    // --- set_mode ---

    #[test]
    fn set_mode_to_vim_resets_to_normal() {
        let mut ed = editor("test");
        ed.set_mode(EditorMode::Vim);
        assert_eq!(ed.mode, EditorMode::Vim);
        assert_eq!(ed.vim_mode, VimMode::Normal);
    }

    // --- Word char detection ---

    #[test]
    fn word_char_classification() {
        assert!(is_word_char('a'));
        assert!(is_word_char('Z'));
        assert!(is_word_char('0'));
        assert!(is_word_char('_'));
        assert!(!is_word_char(' '));
        assert!(!is_word_char('.'));
        assert!(!is_word_char('-'));
    }

    // --- UTF-8 handling ---

    #[test]
    fn insert_and_delete_multibyte() {
        let mut ed = editor("");
        for c in "café".chars() {
            ed.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(ed.lines[0], "café");
        assert_eq!(ed.cursor_col, 4);

        ed.handle_key(key(KeyCode::Backspace));
        assert_eq!(ed.lines[0], "caf");
        assert_eq!(ed.cursor_col, 3);
    }

    #[test]
    fn vim_x_on_multibyte() {
        let mut ed = vim_editor("héllo");
        ed.cursor_col = 1;
        ed.handle_key(key(KeyCode::Char('x')));
        assert_eq!(ed.lines[0], "hllo");
    }

    // --- Render ---

    #[test]
    fn render_lines_non_editing() {
        let ed = editor("hello\nworld");
        let lines = ed.render_lines(false);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn render_lines_editing_shows_cursor() {
        let ed = editor("hello");
        let lines = ed.render_lines(true);
        assert_eq!(lines.len(), 1);
        // Should have 4 spans: gutter, before cursor, cursor char, after cursor
        assert_eq!(lines[0].spans.len(), 4);
    }

    // --- Enter returns Enter action (not consumed) ---

    #[test]
    fn enter_returns_enter_action() {
        let mut ed = editor("test");
        assert_eq!(ed.handle_key(key(KeyCode::Enter)), EditorAction::Enter);
    }
}
