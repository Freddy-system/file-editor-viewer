//! The built-in editor: a modeless text box over the file being read, with
//! the usual save, undo, selection and clipboard keys.

use std::path::{Path, PathBuf};

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget;
use ratatui_textarea::{CursorMove, DataCursor, Scrolling, TextArea};

use crate::theme::Theme;

/// Lines the mouse wheel moves the view by.
const WHEEL_LINES: i16 = 3;
/// Columns a Tab key advances to the next multiple of.
const TAB_LENGTH: u8 = 4;

const KEYS: &[(&str, &str)] = &[
    ("Ctrl+S", "save"),
    ("Esc / Ctrl+Q", "close the editor"),
    ("Ctrl+Z / Ctrl+Y", "undo / redo"),
    ("Shift+arrows", "select"),
    ("Ctrl+A", "select all"),
    ("Ctrl+C / Ctrl+X", "copy / cut the selection"),
    ("Ctrl+V", "paste"),
    ("Ctrl+K", "delete the line"),
    ("Ctrl+←/→", "previous / next word"),
    ("Ctrl+Home/End", "start / end of the file"),
    ("PgUp / PgDn", "page up / down"),
];

/// What a key or mouse event did, for the viewer to react to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing the viewer needs to act on.
    None,
    /// The text changed (the viewer refreshes its live preview).
    Changed,
    /// The file was written to disk.
    Saved,
    /// The user asked to leave the editor. With unsaved changes this is only
    /// returned on a second, confirming request.
    Close,
    /// Text was copied or cut and should also go to the system clipboard.
    Copied(String),
    /// A short notice for the status line (errors, "unsaved changes" warning).
    Notice(String),
}

pub struct Editor {
    textarea: TextArea<'static>,
    path: PathBuf,
    /// The buffer as it is on disk, to tell whether there is anything to save.
    saved: Vec<String>,
    /// Line ending of the opened file, kept so saving does not rewrite it.
    newline: &'static str,
    trailing_newline: bool,
    /// Set by a close request on a dirty buffer; the next one goes through.
    pending_close: bool,
    /// Where the editor was last drawn, to place mouse clicks.
    area: Rect,
}

impl Editor {
    /// Opens `text` (the content of `path`) with the cursor on 1-based `line`.
    pub fn new(path: &Path, text: &str, line: usize, theme: &Theme) -> Editor {
        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let (body, trailing_newline) = match text.strip_suffix('\n') {
            Some(body) => (body, true),
            None => (text, false),
        };
        let lines: Vec<String> = body
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
            .collect();

        let mut textarea = TextArea::new(lines.clone());
        textarea.set_tab_length(TAB_LENGTH);
        textarea.set_hard_tab_indent(false);
        // Prose has long lines; wrapping keeps them readable without
        // horizontal scrolling.
        textarea.set_wrap_mode(ratatui_textarea::WrapMode::WordOrGlyph);
        textarea.remove_block();
        textarea.move_cursor(CursorMove::Jump(to_u16(line.saturating_sub(1)), 0));

        let mut editor = Editor {
            textarea,
            path: path.to_path_buf(),
            saved: lines,
            newline,
            trailing_newline,
            pending_close: false,
            area: Rect::default(),
        };
        editor.set_theme(theme);
        editor
    }

    /// Current buffer content with the file's own line endings, and a
    /// trailing newline if the original text had one.
    pub fn text(&self) -> String {
        let mut text = self.textarea.lines().join(self.newline);
        if self.trailing_newline {
            text.push_str(self.newline);
        }
        text
    }

    /// Whether the buffer differs from what is on disk.
    pub fn is_dirty(&self) -> bool {
        self.textarea.lines() != self.saved.as_slice()
    }

    /// 1-based line and column of the cursor.
    pub fn cursor(&self) -> (usize, usize) {
        let DataCursor(row, col) = self.textarea.cursor();
        (row + 1, col + 1)
    }

    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        if key.kind == KeyEventKind::Release {
            return Outcome::None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        let closing = match key.code {
            KeyCode::Esc => true,
            KeyCode::Char(c) => ctrl && !alt && c.eq_ignore_ascii_case(&'q'),
            _ => false,
        };
        if closing {
            return self.request_close();
        }
        self.pending_close = false;

        match key.code {
            KeyCode::Char(c) if ctrl && !alt => self.shortcut(c.to_ascii_lowercase()),
            // Ctrl+Alt together is how AltGr characters arrive on some systems.
            KeyCode::Char(c) if ctrl == alt => {
                self.textarea.insert_char(c);
                Outcome::Changed
            }
            KeyCode::Enter => {
                self.textarea.insert_newline();
                Outcome::Changed
            }
            KeyCode::Tab => changed(self.textarea.insert_tab()),
            KeyCode::Backspace if ctrl || alt => changed(self.textarea.delete_word()),
            KeyCode::Backspace => changed(self.textarea.delete_char()),
            KeyCode::Delete if ctrl || alt => changed(self.textarea.delete_next_word()),
            KeyCode::Delete => changed(self.textarea.delete_next_char()),
            KeyCode::Left if ctrl => self.travel(CursorMove::WordBack, shift),
            KeyCode::Left => self.travel(CursorMove::Back, shift),
            KeyCode::Right if ctrl => self.travel(CursorMove::WordForward, shift),
            KeyCode::Right => self.travel(CursorMove::Forward, shift),
            KeyCode::Up => self.travel(CursorMove::Up, shift),
            KeyCode::Down => self.travel(CursorMove::Down, shift),
            KeyCode::Home if ctrl => self.travel(CursorMove::Jump(0, 0), shift),
            KeyCode::Home => self.travel(CursorMove::Head, shift),
            KeyCode::End if ctrl => self.travel(CursorMove::Jump(u16::MAX, u16::MAX), shift),
            KeyCode::End => self.travel(CursorMove::End, shift),
            KeyCode::PageUp => self.page(Scrolling::PageUp, shift),
            KeyCode::PageDown => self.page(Scrolling::PageDown, shift),
            _ => Outcome::None,
        }
    }

    /// `mouse` is in screen coordinates; uses the area last passed to render.
    pub fn mouse(&mut self, mouse: MouseEvent) -> Outcome {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.click(mouse.column, mouse.row),
            MouseEventKind::ScrollDown => self.textarea.scroll((WHEEL_LINES, 0)),
            MouseEventKind::ScrollUp => self.textarea.scroll((-WHEEL_LINES, 0)),
            _ => {}
        }
        Outcome::None
    }

    /// Inserts text at the cursor (bracketed paste), replacing the selection.
    pub fn paste(&mut self, text: &str) -> Outcome {
        self.pending_close = false;
        // Terminals send pasted line breaks as CR, LF or both.
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        changed(self.textarea.insert_str(text))
    }

    /// Re-applies colours after the viewer's theme switched.
    pub fn set_theme(&mut self, theme: &Theme) {
        self.textarea
            .set_line_number_style(Style::default().fg(theme.dim));
        self.textarea
            .set_cursor_line_style(Style::default().bg(theme.code_bg));
        self.textarea
            .set_selection_style(Style::default().fg(theme.search_fg).bg(theme.search_bg));
        self.textarea
            .set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));
    }

    /// Draws the editor (with line numbers) into `area`; no outer border.
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer) {
        self.area = area;
        Widget::render(&self.textarea, area, buffer);
    }

    /// Keys to show in the help window: (key, description) pairs.
    pub fn keys() -> &'static [(&'static str, &'static str)] {
        KEYS
    }

    /// Closes at once when clean; a dirty buffer needs asking twice in a row.
    fn request_close(&mut self) -> Outcome {
        if self.pending_close || !self.is_dirty() {
            self.pending_close = false;
            return Outcome::Close;
        }
        self.pending_close = true;
        Outcome::Notice("unsaved changes: Ctrl+S saves, Esc again discards".to_string())
    }

    /// A Ctrl+letter key. Letters without a meaning here do nothing, so the
    /// widget's emacs-style bindings never fire.
    fn shortcut(&mut self, letter: char) -> Outcome {
        match letter {
            's' => self.save(),
            'z' => changed(self.textarea.undo()),
            'y' => changed(self.textarea.redo()),
            'a' => {
                self.textarea.select_all();
                Outcome::None
            }
            'c' if self.has_selection() => {
                self.textarea.copy();
                Outcome::Copied(self.textarea.yank_text())
            }
            'x' if self.has_selection() => {
                self.textarea.cut();
                Outcome::Copied(self.textarea.yank_text())
            }
            'v' => changed(self.textarea.paste()),
            'k' => changed(self.delete_line()),
            _ => Outcome::None,
        }
    }

    fn save(&mut self) -> Outcome {
        match std::fs::write(&self.path, self.text()) {
            Ok(()) => {
                self.saved = self.textarea.lines().to_vec();
                Outcome::Saved
            }
            Err(error) => Outcome::Notice(format!("cannot save: {error}")),
        }
    }

    fn has_selection(&self) -> bool {
        self.textarea
            .selection_range()
            .is_some_and(|(start, end)| start != end)
    }

    /// Keeps the selection growing while Shift is held and drops it otherwise.
    fn anchor_selection(&mut self, extend: bool) {
        if !extend {
            self.textarea.cancel_selection();
        } else if !self.textarea.is_selecting() {
            self.textarea.start_selection();
        }
    }

    fn travel(&mut self, movement: CursorMove, extend: bool) -> Outcome {
        self.anchor_selection(extend);
        self.textarea.move_cursor(movement);
        Outcome::None
    }

    fn page(&mut self, scrolling: Scrolling, extend: bool) -> Outcome {
        self.anchor_selection(extend);
        self.textarea.scroll(scrolling);
        Outcome::None
    }

    /// Removes the cursor's line together with one of its line breaks.
    fn delete_line(&mut self) -> bool {
        self.textarea.cancel_selection();
        let DataCursor(row, _) = self.textarea.cursor();
        let lines = self.textarea.lines();
        let length = lines[row].chars().count();
        let last = lines.len() - 1;

        self.textarea.move_cursor(CursorMove::Head);
        if row < last {
            self.textarea.delete_str(length + 1)
        } else if row > 0 {
            // The last line has no break after it: take the one before.
            self.textarea.move_cursor(CursorMove::Back);
            self.textarea.delete_str(length + 1)
        } else {
            self.textarea.delete_str(length)
        }
    }

    /// Puts the cursor on the character drawn at a screen cell.
    fn click(&mut self, column: u16, row: u16) {
        let area = self.area;
        let inside = (area.left()..area.right()).contains(&column)
            && (area.top()..area.bottom()).contains(&row);
        if !inside {
            return;
        }
        self.pending_close = false;
        let (top_row, top_col) = self.textarea.scroll_offset();
        let gutter = usize::from(self.textarea.line_number_width());
        let screen_row = usize::from(top_row) + usize::from(row - area.y);
        // Scrolling sideways slides the line numbers out along with the text.
        let screen_col =
            (usize::from(column - area.x) + usize::from(top_col)).saturating_sub(gutter);
        let DataCursor(line, col) = self.textarea.screen_to_data(screen_row, screen_col);
        self.textarea.cancel_selection();
        self.textarea
            .move_cursor(CursorMove::Jump(to_u16(line), to_u16(col)));
    }
}

fn changed(modified: bool) -> Outcome {
    if modified {
        Outcome::Changed
    } else {
        Outcome::None
    }
}

fn to_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(text: &str) -> Editor {
        Editor::new(Path::new("note.md"), text, 1, &Theme::dark())
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(letter: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(letter), KeyModifiers::CONTROL)
    }

    fn shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("fev-editor-{}-{name}.md", std::process::id()))
    }

    #[test]
    fn typing_changes_text_and_marks_dirty() {
        let mut editor = editor("one\ntwo\n");
        assert!(!editor.is_dirty());
        assert_eq!(editor.key(press(KeyCode::Char('x'))), Outcome::Changed);
        assert_eq!(editor.text(), "xone\ntwo\n");
        assert!(editor.is_dirty());
    }

    #[test]
    fn enter_splits_the_line() {
        let mut editor = editor("ab");
        editor.key(press(KeyCode::Right));
        assert_eq!(editor.key(press(KeyCode::Enter)), Outcome::Changed);
        assert_eq!(editor.text(), "a\nb");
        assert_eq!(editor.cursor(), (2, 1));
    }

    #[test]
    fn tab_inserts_spaces_up_to_the_next_stop() {
        let mut editor = editor("ab");
        editor.key(press(KeyCode::Right));
        assert_eq!(editor.key(press(KeyCode::Tab)), Outcome::Changed);
        assert_eq!(editor.text(), "a   b");
    }

    #[test]
    fn movement_keys_change_nothing() {
        let mut editor = editor("one\ntwo");
        for code in [KeyCode::Down, KeyCode::End, KeyCode::Left, KeyCode::Home] {
            assert_eq!(editor.key(press(code)), Outcome::None);
        }
        assert_eq!(editor.cursor(), (2, 1));
        assert!(!editor.is_dirty());
    }

    #[test]
    fn unbound_control_keys_do_nothing() {
        let mut editor = editor("one\ntwo");
        for letter in ['h', 'd', 'w', 'j', 'u', 'r', 'e', 'n'] {
            assert_eq!(editor.key(ctrl(letter)), Outcome::None, "Ctrl+{letter}");
        }
        assert_eq!(editor.text(), "one\ntwo");
        assert_eq!(editor.cursor(), (1, 1));
    }

    #[test]
    fn text_round_trips_with_trailing_newline() {
        assert_eq!(editor("a\nb\n").text(), "a\nb\n");
        assert_eq!(editor("a\n\n").text(), "a\n\n");
        assert_eq!(editor("\n").text(), "\n");
    }

    #[test]
    fn text_round_trips_without_trailing_newline() {
        assert_eq!(editor("a\nb").text(), "a\nb");
        assert_eq!(editor("").text(), "");
    }

    #[test]
    fn windows_line_endings_are_kept() {
        let mut editor = editor("a\r\nb\r\n");
        assert_eq!(editor.text(), "a\r\nb\r\n");
        editor.key(press(KeyCode::Char('x')));
        assert_eq!(editor.text(), "xa\r\nb\r\n");
    }

    #[test]
    fn opening_at_a_line_puts_the_cursor_there() {
        let editor = Editor::new(Path::new("note.md"), "a\nb\nc\n", 2, &Theme::dark());
        assert_eq!(editor.cursor(), (2, 1));
    }

    #[test]
    fn opening_past_the_end_clamps_to_the_last_line() {
        let editor = Editor::new(Path::new("note.md"), "a\nb\nc\n", 99, &Theme::dark());
        assert_eq!(editor.cursor(), (3, 1));
        let editor = Editor::new(Path::new("note.md"), "a\nb\nc\n", 0, &Theme::dark());
        assert_eq!(editor.cursor(), (1, 1));
    }

    #[test]
    fn saving_writes_the_file_and_clears_dirty() {
        let path = temp_file("save");
        let mut editor = Editor::new(&path, "one\ntwo\n", 1, &Theme::dark());
        editor.key(press(KeyCode::Char('x')));
        assert!(editor.is_dirty());

        let outcome = editor.key(ctrl('s'));
        let written = std::fs::read_to_string(&path);
        let _ = std::fs::remove_file(&path);

        assert_eq!(outcome, Outcome::Saved);
        assert_eq!(written.unwrap(), "xone\ntwo\n");
        assert!(!editor.is_dirty());
    }

    #[test]
    fn failed_save_gives_a_notice_and_stays_dirty() {
        let path = temp_file("missing-dir").join("note.md");
        let mut editor = Editor::new(&path, "one\n", 1, &Theme::dark());
        editor.key(press(KeyCode::Char('x')));
        match editor.key(ctrl('s')) {
            Outcome::Notice(notice) => assert!(notice.starts_with("cannot save: "), "{notice}"),
            other => panic!("expected a notice, got {other:?}"),
        }
        assert!(editor.is_dirty());
    }

    #[test]
    fn escape_on_a_clean_buffer_closes() {
        let mut editor = editor("one\n");
        assert_eq!(editor.key(press(KeyCode::Esc)), Outcome::Close);
        assert_eq!(editor.key(ctrl('q')), Outcome::Close);
    }

    #[test]
    fn escape_on_a_dirty_buffer_warns_then_closes() {
        let mut editor = editor("one\n");
        editor.key(press(KeyCode::Char('x')));
        assert!(matches!(
            editor.key(press(KeyCode::Esc)),
            Outcome::Notice(_)
        ));
        assert_eq!(editor.key(ctrl('q')), Outcome::Close);
    }

    #[test]
    fn typing_after_the_warning_disarms_the_close() {
        let mut editor = editor("one\n");
        editor.key(press(KeyCode::Char('x')));
        assert!(matches!(
            editor.key(press(KeyCode::Esc)),
            Outcome::Notice(_)
        ));
        assert_eq!(editor.key(press(KeyCode::Char('y'))), Outcome::Changed);
        assert!(matches!(
            editor.key(press(KeyCode::Esc)),
            Outcome::Notice(_)
        ));
        assert_eq!(editor.key(press(KeyCode::Esc)), Outcome::Close);
    }

    #[test]
    fn undo_restores_the_text_and_redo_reapplies_it() {
        let mut editor = editor("one\n");
        editor.key(press(KeyCode::Char('x')));
        assert_eq!(editor.key(ctrl('z')), Outcome::Changed);
        assert_eq!(editor.text(), "one\n");
        assert!(!editor.is_dirty());
        assert_eq!(editor.key(ctrl('z')), Outcome::None);

        assert_eq!(editor.key(ctrl('y')), Outcome::Changed);
        assert_eq!(editor.text(), "xone\n");
        assert_eq!(editor.key(ctrl('y')), Outcome::None);
    }

    #[test]
    fn select_all_then_copy_returns_the_whole_text() {
        let mut editor = editor("one\ntwo");
        assert_eq!(editor.key(ctrl('a')), Outcome::None);
        assert_eq!(
            editor.key(ctrl('c')),
            Outcome::Copied("one\ntwo".to_string())
        );
        assert_eq!(editor.text(), "one\ntwo");
    }

    #[test]
    fn copy_without_a_selection_does_nothing() {
        let mut editor = editor("one\ntwo");
        assert_eq!(editor.key(ctrl('c')), Outcome::None);
        assert_eq!(editor.key(ctrl('x')), Outcome::None);
        assert_eq!(editor.text(), "one\ntwo");
    }

    #[test]
    fn shift_arrows_select_and_a_plain_arrow_cancels() {
        let mut editor = editor("alpha\nbravo");
        editor.key(shift(KeyCode::Right));
        editor.key(shift(KeyCode::Right));
        assert_eq!(editor.key(ctrl('c')), Outcome::Copied("al".to_string()));

        editor.key(shift(KeyCode::End));
        editor.key(press(KeyCode::Left));
        assert_eq!(editor.key(ctrl('c')), Outcome::None);
    }

    #[test]
    fn cut_removes_the_selection_and_paste_puts_it_back() {
        let mut editor = editor("alpha\nbravo");
        editor.key(shift(KeyCode::Right));
        editor.key(shift(KeyCode::Right));
        assert_eq!(editor.key(ctrl('x')), Outcome::Copied("al".to_string()));
        assert_eq!(editor.text(), "pha\nbravo");

        editor.key(press(KeyCode::End));
        assert_eq!(editor.key(ctrl('v')), Outcome::Changed);
        assert_eq!(editor.text(), "phaal\nbravo");
    }

    #[test]
    fn typing_replaces_the_selection() {
        let mut editor = editor("one\ntwo");
        editor.key(ctrl('a'));
        editor.key(press(KeyCode::Char('x')));
        assert_eq!(editor.text(), "x");
    }

    #[test]
    fn paste_inserts_at_the_cursor() {
        let mut editor = editor("ab");
        editor.key(press(KeyCode::Right));
        assert_eq!(editor.paste("X\r\nY"), Outcome::Changed);
        assert_eq!(editor.text(), "aX\nYb");
        assert_eq!(editor.cursor(), (2, 2));
        assert_eq!(editor.paste(""), Outcome::None);
    }

    #[test]
    fn control_k_deletes_the_current_line() {
        let mut editor = editor("one\ntwo\nthree\n");
        editor.key(press(KeyCode::Down));
        editor.key(press(KeyCode::Right));
        assert_eq!(editor.key(ctrl('k')), Outcome::Changed);
        assert_eq!(editor.text(), "one\nthree\n");
        assert_eq!(editor.cursor(), (2, 1));

        assert_eq!(editor.key(ctrl('k')), Outcome::Changed);
        assert_eq!(editor.text(), "one\n");
        assert_eq!(editor.key(ctrl('k')), Outcome::Changed);
        assert_eq!(editor.text(), "\n");
        assert_eq!(editor.key(ctrl('k')), Outcome::None);
    }

    #[test]
    fn click_moves_the_cursor_to_the_clicked_cell() {
        let mut editor = editor("alpha\nbravo\ncharlie");
        let area = Rect::new(4, 2, 40, 10);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 60, 20));
        editor.render(area, &mut buffer);

        let gutter = 3;
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: area.x + gutter + 2,
            row: area.y + 1,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(editor.mouse(click), Outcome::None);
        assert_eq!(editor.cursor(), (2, 3));

        let outside = MouseEvent {
            column: 0,
            row: 0,
            ..click
        };
        editor.mouse(outside);
        assert_eq!(editor.cursor(), (2, 3));
    }

    #[test]
    fn wheel_scrolls_three_lines() {
        let text: Vec<String> = (1..=40).map(|n| n.to_string()).collect();
        let mut editor = editor(&text.join("\n"));
        let area = Rect::new(0, 0, 20, 10);
        let mut buffer = Buffer::empty(area);
        editor.render(area, &mut buffer);

        let wheel = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(editor.mouse(wheel), Outcome::None);
        assert_eq!(editor.cursor(), (4, 1));
        assert!(!editor.is_dirty());
    }

    #[test]
    fn help_lists_the_save_and_close_keys() {
        let keys = Editor::keys();
        assert!(keys.iter().any(|(key, _)| *key == "Ctrl+S"));
        assert!(keys.iter().any(|(key, _)| key.contains("Esc")));
    }
}
