// stickynote — application state and event dispatch.

use std::path::PathBuf;
use std::time::Instant;

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::note::{self, BORDER_STYLES, NOTE_COLORS, Note, abs_diff_u16, color_name, cycle_str};
use crate::persistence::SaveData;

// ── Input modes ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    TagInput,
    FilterInput,
}

// ── Edit focus (Tab-navigable areas within a card) ───────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditFocus {
    Header,
    Tags,
    Content,
}

// ── Menu ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Edit,
    Color,
    Border,
    Tag,
    Delete,
    NewNote,
    Close,
}

#[derive(Debug, Clone)]
pub struct MenuState {
    pub visible: bool,
    pub selected: usize,
    pub x: u16,
    pub y: u16,
    pub items: Vec<MenuAction>,
}

impl MenuState {
    fn new() -> Self {
        MenuState {
            visible: false,
            selected: 0,
            x: 0,
            y: 0,
            items: Vec::new(),
        }
    }
}

// ── App ───────────────────────────────────────────────────────────────────────

/// Application state holding all notes, UI mode, and interaction history.
pub struct App {
    pub notes: Vec<Note>,
    pub selected: usize,
    pub width: u16,
    pub height: u16,
    pub theme_idx: usize,
    pub menu: MenuState,

    pub mode: InputMode,
    pub input: String,
    pub show_help: bool,
    pub show_overlay: bool,
    pub should_quit: bool,

    pub filter_tag: String,
    pub all_tags: Vec<String>,

    // Double-click tracking.
    pub last_click: Instant,
    pub last_click_btn: Option<MouseButton>,
    pub last_click_x: u16,
    pub last_click_y: u16,

    // Save debounce.
    pub last_save: Instant,
    pub save_error: String,
    pub dirty: bool,
    pub confirm_delete: bool,
    pub confirm_clear_tags: bool,

    pub edit_focus: EditFocus,

    /// Custom board file path from CLI (None = default ~/.stickynote/board.json).
    pub board_path: Option<PathBuf>,

    /// True while the user is dragging the mouse to select text.
    pub mouse_dragging: bool,
}

impl App {
    pub fn new(save: SaveData) -> Self {
        let theme_idx = save.theme_idx.clamp(0, note::THEMES.len() - 1);
        let notes = save.into_notes();
        let mut app = App {
            notes,
            selected: 0,
            width: 0,
            height: 0,
            theme_idx,
            menu: MenuState::new(),
            mode: InputMode::Normal,
            input: String::new(),
            show_help: false,
            show_overlay: false,
            should_quit: false,
            filter_tag: String::new(),
            all_tags: Vec::new(),
            last_click: Instant::now(),
            last_click_btn: None,
            last_click_x: 0,
            last_click_y: 0,
            last_save: Instant::now(),
            save_error: String::new(),
            dirty: false,
            confirm_delete: false,
            confirm_clear_tags: false,
            edit_focus: EditFocus::Content,
            board_path: None,
            mouse_dragging: false,
        };
        if app.count() > 0 {
            app.selected = 0;
        }
        app.refresh_all_tags();
        app
    }

    // ── Count ────────────────────────────────────────────────────────────────

    /// Number of notes on the board.
    pub fn count(&self) -> usize {
        self.notes.len()
    }

    /// Indices of notes visible under the current tag filter (or all notes if none).
    pub fn visible_note_indices(&self) -> Vec<usize> {
        if self.filter_tag.is_empty() {
            (0..self.notes.len()).collect()
        } else {
            self.notes
                .iter()
                .enumerate()
                .filter(|(_, n)| n.has_tag(&self.filter_tag))
                .map(|(i, _)| i)
                .collect()
        }
    }

    /// Number of notes matching the current tag filter.
    pub fn visible_count(&self) -> usize {
        self.visible_note_indices().len()
    }

    // ── Keyboard dispatch ────────────────────────────────────────────────────

    /// Process a crossterm key event and update application state.
    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press && key.kind != KeyEventKind::Repeat {
            return;
        }
        let ks = match key.code {
            KeyCode::Char(c) => {
                let mut s = c.to_string();
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    s.insert(0, '^');
                }
                s
            }
            KeyCode::Enter => "enter".into(),
            KeyCode::Backspace => "backspace".into(),
            KeyCode::Esc => "esc".into(),
            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    "shift+tab".into()
                } else {
                    "tab".into()
                }
            }
            KeyCode::Up => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    "shift+up".into()
                } else {
                    "up".into()
                }
            }
            KeyCode::Down => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    "shift+down".into()
                } else {
                    "down".into()
                }
            }
            KeyCode::Left => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    "shift+left".into()
                } else {
                    "left".into()
                }
            }
            KeyCode::Right => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    "shift+right".into()
                } else {
                    "right".into()
                }
            }
            KeyCode::Home => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    "shift+home".into()
                } else {
                    "home".into()
                }
            }
            KeyCode::End => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    "shift+end".into()
                } else {
                    "end".into()
                }
            }
            KeyCode::Delete => "delete".into(),
            KeyCode::BackTab => "shift+tab".into(),
            _ => return,
        };

        // Help overlay: any key closes it.
        if self.show_help {
            self.show_help = false;
            return;
        }

        // Menu mode.
        if self.menu.visible {
            self.handle_menu_key(&ks);
            return;
        }

        // Overlay mode (full-screen editing).
        if self.show_overlay {
            self.handle_overlay_key(&ks);
            return;
        }

        // Ctrl+C: copy (if editing+selection) else quit (or exit mode).
        if ks == "^c" {
            if self.count() > 0
                && self.selected < self.count()
                && self.notes[self.selected].editing
                && self.notes[self.selected].any_selection()
            {
                self.copy_selection();
                return;
            }
            if self.mode != InputMode::Normal {
                self.mode = InputMode::Normal;
                return;
            }
            self.flush_save();
            self.should_quit = true;
            return;
        }

        // Input-mode-specific handling.
        match self.mode {
            InputMode::TagInput => {
                self.handle_tag_input(&ks);
                return;
            }
            InputMode::FilterInput => {
                self.handle_filter_input(&ks);
                return;
            }
            InputMode::Normal => {}
        }

        // Editing is tracked per-note (matching the Go pattern).
        if self.count() > 0 && self.selected < self.count() && self.notes[self.selected].editing {
            self.handle_edit_key(&ks);
            return;
        }

        // Confirm delete mode.
        if self.confirm_delete {
            if ks == "y" || ks == "Y" || ks == "enter" {
                self.confirm_delete = false;
                self.delete_selected();
            } else {
                self.confirm_delete = false;
            }
            return;
        }

        // Confirm clear-tags mode.
        if self.confirm_clear_tags {
            if ks == "y" || ks == "Y" || ks == "enter" {
                self.confirm_clear_tags = false;
                if self.count() > 0 {
                    let note = &mut self.notes[self.selected];
                    note.tags.clear();
                    self.refresh_all_tags();
                    self.mark_dirty();
                }
            } else {
                self.confirm_clear_tags = false;
            }
            return;
        }

        // Normal mode keybindings.
        match ks.as_str() {
            "q" => {
                self.flush_save();
                self.should_quit = true;
            }
            "?" => {
                self.show_help = true;
            }
            "n" => self.add_note(),
            "d" => {
                if self.count() > 0 {
                    self.confirm_delete = true;
                }
            }
            "^d" => {
                self.duplicate_selected();
            }
            "e" | "enter" => {
                self.toggle_edit();
                self.mark_dirty();
            }
            "c" => {
                self.cycle_color();
                self.mark_dirty();
            }
            "b" => {
                self.cycle_border();
                self.mark_dirty();
            }
            "t" => {
                if self.count() > 0 {
                    self.mode = InputMode::TagInput;
                    self.input.clear();
                }
            }
            "T" => {
                if !self.all_tags.is_empty() {
                    if self.filter_tag.is_empty() {
                        self.filter_tag = self.all_tags[0].clone();
                    } else {
                        self.filter_tag.clear();
                    }
                }
            }
            "^t" => {
                if self.count() > 0 {
                    let note = &self.notes[self.selected];
                    if !note.tags.is_empty() {
                        self.confirm_clear_tags = true;
                    }
                }
            }
            "/" => {
                self.mode = InputMode::FilterInput;
                self.input.clear();
            }
            "up" | "k" => self.select_prev(),
            "down" | "j" => self.select_next(),
            "left" => self.select_prev(),
            "right" => self.select_next(),
            "tab" => self.select_next(),
            "shift+tab" => self.select_prev(),
            "O" => {
                if self.count() > 0 {
                    self.toggle_overlay();
                }
            }
            "[" => {
                if self.count() >= 2 && self.selected < self.count() - 1 {
                    self.notes.swap(self.selected, self.selected + 1);
                    self.selected += 1;
                    self.mark_dirty();
                }
            }
            "]" => {
                if self.count() >= 2 && self.selected > 0 {
                    self.notes.swap(self.selected, self.selected - 1);
                    self.selected -= 1;
                    self.mark_dirty();
                }
            }
            "^r" => {
                self.theme_idx = (self.theme_idx + 1) % note::THEMES.len();
                self.mark_dirty();
            }
            "esc" => {
                self.filter_tag.clear();
            }
            _ => {}
        }
    }

    // ── Overlay key handling ─────────────────────────────────────────────────

    fn handle_overlay_key(&mut self, ks: &str) {
        match ks {
            "q" | "^c" => {
                self.flush_save();
                self.should_quit = true;
            }
            "esc" => {
                self.toggle_overlay();
                self.mark_dirty();
            }
            _ => {
                self.handle_edit_key(ks);
            }
        }
    }

    // ── Menu key handling ────────────────────────────────────────────────────

    fn handle_menu_key(&mut self, ks: &str) {
        match ks {
            "up" | "k" => {
                if self.menu.selected > 0 {
                    self.menu.selected -= 1;
                }
            }
            "down" | "j" => {
                if self.menu.selected + 1 < self.menu.items.len() {
                    self.menu.selected += 1;
                }
            }
            "enter" => self.activate_menu(),
            "esc" => self.menu.visible = false,
            _ => {}
        }
    }

    fn activate_menu(&mut self) {
        if self.menu.selected >= self.menu.items.len() {
            return;
        }
        match self.menu.items[self.menu.selected] {
            MenuAction::Edit => self.toggle_edit(),
            MenuAction::Color => self.cycle_color(),
            MenuAction::Border => self.cycle_border(),
            MenuAction::Tag => {
                self.mode = InputMode::TagInput;
                self.input.clear();
            }
            MenuAction::Delete => self.delete_selected(),
            MenuAction::NewNote => self.add_note(),
            MenuAction::Close => {}
        }
        self.menu.visible = false;
        self.mark_dirty();
    }

    // ── Tag input ────────────────────────────────────────────────────────────

    fn handle_tag_input(&mut self, ks: &str) {
        match ks {
            "esc" => self.mode = InputMode::Normal,
            "enter" => {
                let tag = note::Note::normalize_tag(&self.input);
                if !tag.is_empty() && self.count() > 0 {
                    let note = &mut self.notes[self.selected];
                    if !note.has_tag(&tag) {
                        note.tags.push(tag);
                        self.refresh_all_tags();
                        self.mark_dirty();
                    }
                }
                self.mode = InputMode::Normal;
            }
            "backspace" => {
                self.input.pop();
            }
            _ => {
                if let Some(c) = ks.chars().next()
                    && (c.is_ascii_graphic() || c == ' ')
                {
                    self.input.push(c);
                }
            }
        }
    }

    // ── Filter input ─────────────────────────────────────────────────────────

    fn handle_filter_input(&mut self, ks: &str) {
        match ks {
            "esc" => {
                self.filter_tag.clear();
                self.mode = InputMode::Normal;
            }
            "enter" => {
                self.filter_tag = self.input.trim().to_lowercase();
                self.mode = InputMode::Normal;
            }
            "backspace" => {
                self.input.pop();
            }
            _ => {
                if let Some(c) = ks.chars().next()
                    && (c.is_ascii_graphic() || c == ' ')
                {
                    self.input.push(c);
                }
            }
        }
    }

    // ── Edit key handling ────────────────────────────────────────────────────

    fn handle_edit_key(&mut self, ks: &str) {
        if self.selected >= self.count() {
            return;
        }

        // Focus-agnostic keys.
        let next_focus = |f: EditFocus| -> EditFocus {
            match f {
                EditFocus::Header => EditFocus::Content,
                EditFocus::Content => EditFocus::Tags,
                EditFocus::Tags => EditFocus::Header,
            }
        };
        let prev_focus = |f: EditFocus| -> EditFocus {
            match f {
                EditFocus::Header => EditFocus::Tags,
                EditFocus::Content => EditFocus::Header,
                EditFocus::Tags => EditFocus::Content,
            }
        };

        match ks {
            "esc" => {
                self.notes[self.selected].editing = false;
                self.mark_dirty();
                return;
            }
            "^v" => {
                self.paste_text();
                return;
            }
            "^x" => {
                self.cut_selection();
                return;
            }
            "^a" => {
                let note = &mut self.notes[self.selected];
                match self.edit_focus {
                    EditFocus::Header => {
                        if !note.title.is_empty() {
                            note.title_sel_start = Some(0);
                            note.title_sel_end = Some(note.title.len());
                        }
                    }
                    EditFocus::Content => {
                        if !note.content.is_empty() {
                            note.sel_start = Some(0);
                            note.sel_end = Some(note.content.len());
                        }
                    }
                    _ => {}
                }
                self.mark_dirty();
                return;
            }
            "tab" => {
                // In Tags focus with matching suggestions: autofill.
                if self.edit_focus == EditFocus::Tags {
                    let input = self.notes[self.selected].tag_input.to_lowercase();
                    if !input.is_empty()
                        && let Some(matched) = self
                            .all_tags
                            .iter()
                            .find(|t| t.to_lowercase().contains(&input))
                            .cloned()
                    {
                        self.notes[self.selected].tag_input = matched;
                        self.notes[self.selected].tag_cursor = None;
                        self.mark_dirty();
                        return;
                    }
                }
                self.edit_focus = next_focus(self.edit_focus);
                return;
            }
            "shift+tab" => {
                self.edit_focus = prev_focus(self.edit_focus);
                return;
            }
            _ => {}
        }

        // Dispatch based on current focus.
        match self.edit_focus {
            EditFocus::Header => {
                let note = &mut self.notes[self.selected];
                match ks {
                    "enter" | "down" => {
                        note.clear_title_selection();
                        // Move to content.
                        self.edit_focus = EditFocus::Content;
                    }
                    "backspace" => {
                        if note.has_title_selection() {
                            note.delete_selected_title();
                            self.mark_dirty();
                        } else if note.title_cursor > 0 {
                            note.title.remove(note.title_cursor - 1);
                            note.title_cursor = note.title_cursor.saturating_sub(1);
                            self.mark_dirty();
                        }
                    }
                    "delete" => {
                        if note.has_title_selection() {
                            note.delete_selected_title();
                            self.mark_dirty();
                        } else if note.title_cursor < note.title.len() {
                            note.title.remove(note.title_cursor);
                            self.mark_dirty();
                        }
                    }
                    "left" => {
                        if note.title_cursor > 0 {
                            note.title_cursor -= 1;
                        }
                        note.clear_title_selection();
                    }
                    "right" => {
                        if note.title_cursor < note.title.len() {
                            note.title_cursor += 1;
                        }
                        note.clear_title_selection();
                    }
                    "shift+left" => {
                        if note.title_cursor > 0 {
                            if note.title_sel_start.is_none() {
                                note.title_sel_start = Some(note.title_cursor);
                            }
                            note.title_cursor -= 1;
                            note.title_sel_end = Some(note.title_cursor);
                        }
                    }
                    "shift+right" => {
                        if note.title_cursor < note.title.len() {
                            if note.title_sel_start.is_none() {
                                note.title_sel_start = Some(note.title_cursor);
                            }
                            note.title_cursor += 1;
                            note.title_sel_end = Some(note.title_cursor);
                        }
                    }
                    "shift+home" => {
                        if note.title_sel_start.is_none() {
                            note.title_sel_start = Some(note.title_cursor);
                        }
                        note.title_cursor = 0;
                        note.title_sel_end = Some(0);
                    }
                    "shift+end" => {
                        if note.title_sel_start.is_none() {
                            note.title_sel_start = Some(note.title_cursor);
                        }
                        note.title_cursor = note.title.len();
                        note.title_sel_end = Some(note.title.len());
                    }
                    "home" => {
                        note.title_cursor = 0;
                        note.clear_title_selection();
                    }
                    "end" => {
                        note.title_cursor = note.title.len();
                        note.clear_title_selection();
                    }
                    _ => {
                        if let Some(c) = ks.chars().next()
                            && (c.is_ascii_graphic() || c == ' ')
                        {
                            if note.has_title_selection() {
                                note.delete_selected_title();
                            }
                            note.title.insert(note.title_cursor, c);
                            note.title_cursor += 1;
                            self.mark_dirty();
                        }
                    }
                }
            }
            EditFocus::Tags => {
                let mut tags_changed = false;
                {
                    let note = &mut self.notes[self.selected];
                    match ks {
                        "enter" => {
                            let tag = Note::normalize_tag(&note.tag_input);
                            if !tag.is_empty() && !note.has_tag(&tag) {
                                note.tags.push(tag);
                                tags_changed = true;
                            }
                            note.tag_input.clear();
                            note.tag_cursor = None;
                        }
                        "backspace" => {
                            if !note.tag_input.is_empty() {
                                note.tag_input.pop();
                            } else if let Some(i) = note.tag_cursor
                                && i < note.tags.len()
                            {
                                // Remove the selected tag.
                                note.tags.remove(i);
                                tags_changed = true;
                                note.tag_cursor = if note.tags.is_empty() {
                                    None
                                } else if i >= note.tags.len() {
                                    Some(note.tags.len() - 1)
                                } else {
                                    Some(i)
                                };
                            } else if !note.tags.is_empty() {
                                // No tag selected, remove last.
                                note.tags.pop();
                                tags_changed = true;
                            }
                        }
                        "delete" => {
                            if let Some(i) = note.tag_cursor
                                && i < note.tags.len()
                            {
                                note.tags.remove(i);
                                tags_changed = true;
                                note.tag_cursor = if note.tags.is_empty() {
                                    None
                                } else if i >= note.tags.len() {
                                    Some(note.tags.len() - 1)
                                } else {
                                    Some(i)
                                };
                            }
                        }
                        "left" => {
                            if note.tag_input.is_empty() && !note.tags.is_empty() {
                                note.tag_cursor = Some(match note.tag_cursor {
                                    None => note.tags.len() - 1,
                                    Some(0) => note.tags.len() - 1,
                                    Some(i) => i - 1,
                                });
                            } else if note.tag_input.is_empty() {
                                note.tag_cursor = None;
                            }
                        }
                        "right" => {
                            if note.tag_input.is_empty() && !note.tags.is_empty() {
                                note.tag_cursor = match note.tag_cursor {
                                    None => Some(0),
                                    Some(i) if i + 1 >= note.tags.len() => None,
                                    Some(i) => Some(i + 1),
                                };
                            } else if note.tag_input.is_empty() {
                                note.tag_cursor = None;
                            }
                        }
                        _ => {
                            if let Some(c) = ks.chars().next()
                                && (c.is_ascii_graphic() || c == ' ')
                            {
                                note.tag_input.push(c);
                                note.tag_cursor = None; // typing deselects
                            }
                        }
                    }
                }
                if tags_changed {
                    self.mark_dirty();
                    self.refresh_all_tags();
                }
            }
            EditFocus::Content => {
                let note = &mut self.notes[self.selected];
                match ks {
                    "enter" => {
                        if note.has_content_selection() {
                            note.delete_selected_content();
                        }
                        note.content.insert(note.cursor, '\n');
                        note.cursor += 1;
                        self.mark_dirty();
                    }
                    "backspace" => {
                        if note.has_content_selection() {
                            note.delete_selected_content();
                            self.mark_dirty();
                        } else if note.cursor > 0 {
                            note.content.remove(note.cursor - 1);
                            note.cursor = note.cursor.saturating_sub(1);
                            self.mark_dirty();
                        }
                    }
                    "left" => {
                        if note.cursor > 0 {
                            note.cursor -= 1;
                        }
                        note.clear_content_selection();
                    }
                    "right" => {
                        if note.cursor < note.content.len() {
                            note.cursor += 1;
                        }
                        note.clear_content_selection();
                    }
                    "up" => {
                        let (line, col) = note.cursor_pos();
                        if line > 0 {
                            note.cursor = note.pos_to_cursor(line - 1, col);
                        }
                        note.clear_content_selection();
                    }
                    "down" => {
                        let (line, col) = note.cursor_pos();
                        note.cursor = note.pos_to_cursor(line + 1, col);
                        note.clear_content_selection();
                    }
                    "home" => {
                        let before = &note.content[..note.cursor];
                        let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
                        note.cursor = line_start;
                        note.clear_content_selection();
                    }
                    "end" => {
                        let after = &note.content[note.cursor..];
                        let line_end = after
                            .find('\n')
                            .map(|i| note.cursor + i)
                            .unwrap_or(note.content.len());
                        note.cursor = line_end;
                        note.clear_content_selection();
                    }
                    "shift+left" => {
                        if note.cursor > 0 {
                            if note.sel_start.is_none() {
                                note.sel_start = Some(note.cursor);
                            }
                            note.cursor -= 1;
                            note.sel_end = Some(note.cursor);
                        }
                    }
                    "shift+right" => {
                        if note.cursor < note.content.len() {
                            if note.sel_start.is_none() {
                                note.sel_start = Some(note.cursor);
                            }
                            note.cursor += 1;
                            note.sel_end = Some(note.cursor);
                        }
                    }
                    "shift+up" => {
                        let (line, col) = note.cursor_pos();
                        if note.sel_start.is_none() {
                            note.sel_start = Some(note.cursor);
                        }
                        if line > 0 {
                            note.cursor = note.pos_to_cursor(line - 1, col);
                        } else {
                            note.cursor = 0;
                        }
                        note.sel_end = Some(note.cursor);
                    }
                    "shift+down" => {
                        let (line, col) = note.cursor_pos();
                        if note.sel_start.is_none() {
                            note.sel_start = Some(note.cursor);
                        }
                        note.cursor = note.pos_to_cursor(line + 1, col);
                        note.sel_end = Some(note.cursor);
                    }
                    "shift+home" => {
                        if note.sel_start.is_none() {
                            note.sel_start = Some(note.cursor);
                        }
                        let before = &note.content[..note.cursor];
                        let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
                        note.cursor = line_start;
                        note.sel_end = Some(note.cursor);
                    }
                    "shift+end" => {
                        if note.sel_start.is_none() {
                            note.sel_start = Some(note.cursor);
                        }
                        let after = &note.content[note.cursor..];
                        let line_end = after
                            .find('\n')
                            .map(|i| note.cursor + i)
                            .unwrap_or(note.content.len());
                        note.cursor = line_end;
                        note.sel_end = Some(note.cursor);
                    }
                    _ => {
                        if let Some(c) = ks.chars().next()
                            && (c.is_ascii_graphic() || c == ' ')
                        {
                            if note.has_content_selection() {
                                note.delete_selected_content();
                            }
                            note.content.insert(note.cursor, c);
                            note.cursor += 1;
                            self.mark_dirty();
                        }
                    }
                }
            }
        }
    }

    // ── Mouse dispatch ───────────────────────────────────────────────────────

    /// Process a crossterm mouse event and update application state.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        // Cancel delete/clear-tags confirmation on any mouse click.
        if self.confirm_delete && matches!(mouse.kind, MouseEventKind::Down(_)) {
            self.confirm_delete = false;
            return;
        }
        if self.confirm_clear_tags && matches!(mouse.kind, MouseEventKind::Down(_)) {
            self.confirm_clear_tags = false;
            return;
        }
        // Ignore mouse events in input modes.
        if self.mode != InputMode::Normal {
            return;
        }
        // While editing: route to mouse edit handler for positioning/selection.
        if !self.notes.is_empty()
            && self.selected < self.count()
            && self.notes[self.selected].editing
        {
            self.handle_mouse_edit(mouse);
            return;
        }
        if self.show_help {
            self.show_help = false;
            return;
        }
        if self.menu.visible {
            self.handle_menu_mouse(mouse);
            return;
        }
        if matches!(mouse.kind, MouseEventKind::Down(_)) {
            self.handle_mouse_press(mouse);
        }
    }

    fn handle_mouse_press(&mut self, mouse: MouseEvent) {
        let mx = mouse.column;
        let my = mouse.row;

        // Info bar click at row h-2.
        if self.height >= 2 && my == self.height - 2 {
            self.handle_status_bar_click(mx);
            return;
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.handle_left_click(mx, my);
            }
            MouseEventKind::Down(MouseButton::Right) => {
                // Tab bar click (row 0): select note.
                if my == 0 {
                    self.select_tab_at_x(mx);
                }
                self.show_context_menu(mx, my, self.count() > 0);
            }
            MouseEventKind::ScrollUp => self.select_prev(),
            MouseEventKind::ScrollDown => self.select_next(),
            MouseEventKind::Down(MouseButton::Middle) => {
                // Tab bar click (row 0): select + delete.
                if my == 0 && self.select_tab_at_x(mx) {
                    self.delete_selected();
                }
            }
            _ => {}
        }
    }

    /// Select a note by clicking on a tab at the given x-coordinate.
    /// Returns `true` if a tab was found and selection changed.
    fn select_tab_at_x(&mut self, mx: u16) -> bool {
        if let Some(idx) = crate::ui::note_index_at_tab_x(self, mx) {
            if self.selected < self.count() {
                self.notes[self.selected].editing = false;
            }
            self.selected = idx;
            true
        } else {
            false
        }
    }

    fn handle_left_click(&mut self, mx: u16, my: u16) {
        let now = Instant::now();
        let is_double = now.duration_since(self.last_click).as_millis() < 350
            && self.last_click_btn == Some(MouseButton::Left)
            && abs_diff_u16(mx, self.last_click_x) <= 2
            && abs_diff_u16(my, self.last_click_y) <= 2;

        self.last_click = now;
        self.last_click_btn = Some(MouseButton::Left);
        self.last_click_x = mx;
        self.last_click_y = my;

        // Click on tab bar (row 0): select. Double-click to enter edit.
        if my == 0 && self.count() > 0 {
            if let Some(idx) = crate::ui::note_index_at_tab_x(self, mx) {
                if self.selected < self.count() {
                    self.notes[self.selected].editing = false;
                }
                self.selected = idx;
                if is_double {
                    self.toggle_edit();
                }
            }
            return;
        }

        // Click in content area (below tab bar, above footer): toggle editing.
        if self.count() > 0 && my < self.height.saturating_sub(2) {
            self.toggle_edit();
            self.mark_dirty();
        }
    }

    fn handle_status_bar_click(&mut self, mx: u16) {
        if self.width == 0 {
            return;
        }
        if mx < self.width / 3 {
            self.theme_idx = (self.theme_idx + 1) % note::THEMES.len();
            self.mark_dirty();
        } else if self.count() > 0 && mx < 2 * self.width / 3 {
            self.cycle_color();
            self.mark_dirty();
        }
    }

    fn handle_menu_mouse(&mut self, mouse: MouseEvent) {
        if !matches!(mouse.kind, MouseEventKind::Down(_)) {
            return;
        }
        let mx = mouse.column;
        let my = mouse.row;
        let menu_w = 16u16;
        let menu_h = (self.menu.items.len() + 2) as u16;

        if mx >= self.menu.x
            && mx < self.menu.x + menu_w
            && my >= self.menu.y
            && my < self.menu.y + menu_h
        {
            let idx = (my - self.menu.y).saturating_sub(1) as usize;
            if idx < self.menu.items.len() {
                self.menu.selected = idx;
                self.activate_menu();
            }
        } else {
            self.menu.visible = false;
        }
    }

    // ── Context menu ─────────────────────────────────────────────────────────

    fn show_context_menu(&mut self, x: u16, y: u16, on_note: bool) {
        self.menu.items = if on_note {
            vec![
                MenuAction::Edit,
                MenuAction::Color,
                MenuAction::Border,
                MenuAction::Tag,
                MenuAction::Delete,
                MenuAction::Close,
            ]
        } else {
            vec![MenuAction::NewNote, MenuAction::Close]
        };

        let menu_w = 16u16;
        let menu_h = (self.menu.items.len() + 2) as u16;

        let mut x = x;
        let mut y = y;

        if x + menu_w > self.width.saturating_sub(2) {
            x = self.width.saturating_sub(menu_w + 2);
        }
        if y + menu_h > self.height.saturating_sub(2) {
            y = self.height.saturating_sub(menu_h + 2);
        }

        self.menu.x = x;
        self.menu.y = y;
        self.menu.selected = 0;
        self.menu.visible = true;
    }

    // ── Note operations ──────────────────────────────────────────────────────

    /// Insert a new blank note at the top of the stack.
    pub fn add_note(&mut self) {
        let color_idx = self.count() % NOTE_COLORS.len();
        let note = Note {
            color: NOTE_COLORS[color_idx].to_string(),
            font_style: "normal".to_string(),
            border_style: "rounded".to_string(),
            ..Note::new()
        };
        self.notes.insert(0, note);
        self.selected = 0;
        self.refresh_all_tags();
        self.mark_dirty();
    }

    /// Remove the currently selected note from the board.
    pub fn delete_selected(&mut self) {
        if self.count() == 0 || (self.selected < self.count() && self.notes[self.selected].editing)
        {
            return;
        }
        self.notes.remove(self.selected);
        if self.selected >= self.count() && self.count() > 0 {
            self.selected = self.count() - 1;
        }
        self.refresh_all_tags();
        self.mark_dirty();
    }

    /// Toggle editing mode on the selected note, resetting cursors and focus.
    pub fn toggle_edit(&mut self) {
        if self.count() == 0 {
            return;
        }
        let note = &mut self.notes[self.selected];
        note.editing = !note.editing;
        if note.editing {
            note.cursor = note.content.len();
            note.title_cursor = note.title.len();
            note.tag_input.clear();
            note.tag_cursor = None;
            note.clear_all_selections();
            self.edit_focus = EditFocus::Header;
        } else {
            note.clear_all_selections();
        }
    }

    /// Create a copy of the selected note inserted directly after it.
    pub fn duplicate_selected(&mut self) {
        if self.count() == 0 {
            return;
        }
        let mut note = self.notes[self.selected].clone();
        note.editing = false;
        note.cursor = 0;
        note.title_cursor = 0;
        note.tag_input.clear();
        self.notes.insert(self.selected + 1, note);
        self.selected += 1;
        self.refresh_all_tags();
        self.mark_dirty();
    }

    /// Toggle full-screen overlay mode for the selected note.
    pub fn toggle_overlay(&mut self) {
        if self.count() == 0 {
            return;
        }
        self.show_overlay = !self.show_overlay;
        if self.show_overlay {
            if self.selected < self.count() {
                self.notes[self.selected].editing = true;
                self.notes[self.selected].cursor = self.notes[self.selected].content.len();
            }
        } else if self.selected < self.count() {
            self.notes[self.selected].editing = false;
        }
    }

    // ── Clipboard operations ───────────────────────────────────────────────

    /// Copy selected text to the system clipboard.
    pub fn copy_selection(&mut self) {
        if self.selected >= self.count() {
            return;
        }
        let note = &self.notes[self.selected];
        let text = match self.edit_focus {
            EditFocus::Header => note.selected_title(),
            EditFocus::Content => note.selected_text(),
            EditFocus::Tags => return,
        };
        if text.is_empty() {
            return;
        }
        match copy_to_clipboard(&text) {
            Ok(()) => {}
            Err(e) => self.save_error = e,
        }
    }

    /// Cut selected text to the system clipboard.
    pub fn cut_selection(&mut self) {
        if self.selected >= self.count() {
            return;
        }
        let text = match self.edit_focus {
            EditFocus::Header => self.notes[self.selected].delete_selected_title(),
            EditFocus::Content => self.notes[self.selected].delete_selected_content(),
            EditFocus::Tags => return,
        };
        if text.is_empty() {
            return;
        }
        match copy_to_clipboard(&text) {
            Ok(()) => self.mark_dirty(),
            Err(e) => self.save_error = e,
        }
    }

    /// Paste text from the system clipboard at the cursor position.
    pub fn paste_text(&mut self) {
        if self.selected >= self.count() {
            return;
        }
        let text = match paste_from_clipboard() {
            Ok(t) => t,
            Err(e) => {
                self.save_error = e;
                return;
            }
        };
        // Filter: keep ASCII graphic, space, and newline only.
        let filtered: String = text
            .chars()
            .filter(|c| c.is_ascii_graphic() || *c == ' ' || *c == '\n')
            .collect();
        if filtered.is_empty() {
            return;
        }
        let note = &mut self.notes[self.selected];
        match self.edit_focus {
            EditFocus::Header => {
                if note.has_title_selection() {
                    note.delete_selected_title();
                }
                for c in filtered.chars() {
                    note.title.insert(note.title_cursor, c);
                    note.title_cursor += 1;
                }
                self.mark_dirty();
            }
            EditFocus::Content => {
                if note.has_content_selection() {
                    note.delete_selected_content();
                }
                for c in filtered.chars() {
                    note.content.insert(note.cursor, c);
                    note.cursor += 1;
                }
                self.mark_dirty();
            }
            EditFocus::Tags => {}
        }
    }

    // ── Mouse editing (cursor positioning + selection) ──────────────────────

    /// Handle mouse events while a note is being edited.
    fn handle_mouse_edit(&mut self, mouse: MouseEvent) {
        let mx = mouse.column;
        let my = mouse.row;

        // Right-click or middle-click during editing: stop editing.
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Right))
            || matches!(mouse.kind, MouseEventKind::Down(MouseButton::Middle))
        {
            if self.selected < self.count() {
                self.notes[self.selected].editing = false;
            }
            return;
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.mouse_dragging = false;
                self.mouse_edit_click(mx, my);
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if !self.mouse_dragging {
                    // Start drag selection.
                    self.mouse_dragging = true;
                    if self.selected < self.count() {
                        let note = &mut self.notes[self.selected];
                        if note.sel_start.is_none() {
                            note.sel_start = Some(note.cursor);
                        }
                    }
                }
                self.mouse_edit_drag(mx, my);
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.mouse_dragging = false;
            }
            MouseEventKind::ScrollUp => {
                // Scroll up during editing = go to previous line.
                if self.selected < self.count() {
                    let note = &mut self.notes[self.selected];
                    let (line, col) = note.cursor_pos();
                    if line > 0 {
                        note.cursor = note.pos_to_cursor(line - 1, col);
                    }
                }
            }
            MouseEventKind::ScrollDown => {
                if self.selected < self.count() {
                    let note = &mut self.notes[self.selected];
                    let (line, col) = note.cursor_pos();
                    note.cursor = note.pos_to_cursor(line + 1, col);
                }
            }
            _ => {}
        }
    }

    /// Position cursor from a mouse click in the content area.
    fn mouse_edit_click(&mut self, mx: u16, my: u16) {
        if self.selected >= self.count() {
            return;
        }
        let note = &mut self.notes[self.selected];

        // Determine which section was clicked based on y-coordinate.
        // Tab bar is row 0. Note card starts at row 1.
        // With border: inner_y = 2. Without: inner_y = 1.
        let has_border = note.border_style != "hidden" || note.editing;
        let inner_y = if has_border { 2u16 } else { 1u16 };
        // header starts at inner_y, header_sep at inner_y + 1, body at inner_y + 2.
        let header_y = inner_y;
        let body_y = inner_y + 2;

        if my == header_y {
            // Click in header area.
            let rel_x = mx.saturating_sub(2) as usize;
            let pos = rel_x.min(note.title.len());
            note.title_cursor = pos;
            note.clear_title_selection();
            self.edit_focus = EditFocus::Header;
        } else if my >= body_y {
            // Click in content body.
            let line = (my - body_y) as usize;
            let rel_x = mx.saturating_sub(2) as usize;
            let pos = note.pos_to_cursor(line, rel_x);
            note.cursor = pos;
            note.clear_content_selection();
            self.edit_focus = EditFocus::Content;
        }
    }

    /// Extend selection on mouse drag.
    fn mouse_edit_drag(&mut self, mx: u16, my: u16) {
        if self.selected >= self.count() {
            return;
        }
        let note = &mut self.notes[self.selected];

        let has_border = note.border_style != "hidden" || note.editing;
        let inner_y = if has_border { 2u16 } else { 1u16 };
        let body_y = inner_y + 2;

        if my >= body_y {
            let line = (my - body_y) as usize;
            let rel_x = mx.saturating_sub(2) as usize;
            let pos = note.pos_to_cursor(line, rel_x);
            if pos != note.cursor {
                note.cursor = pos;
                note.sel_end = Some(pos);
            }
        }
    }

    /// Cycle the selected note through the colour palette.
    pub fn cycle_color(&mut self) {
        if self.count() == 0 {
            return;
        }
        let note = &mut self.notes[self.selected];
        let next = cycle_str(&NOTE_COLORS, &note.color);
        note.color = next.to_string();
    }

    /// Cycle the selected note through the available border styles.
    pub fn cycle_border(&mut self) {
        if self.count() == 0 {
            return;
        }
        let note = &mut self.notes[self.selected];
        let next = cycle_str(&BORDER_STYLES, &note.border_style);
        note.border_style = next.to_string();
    }

    /// Select the previous note in the stack (wraps around).
    pub fn select_prev(&mut self) {
        if self.count() == 0 {
            return;
        }
        if self.selected < self.count() {
            self.notes[self.selected].editing = false;
        }
        self.selected = (self.selected + self.count() - 1) % self.count();
    }

    /// Select the next note in the stack (wraps around).
    pub fn select_next(&mut self) {
        if self.count() == 0 {
            return;
        }
        if self.selected < self.count() {
            self.notes[self.selected].editing = false;
        }
        self.selected = (self.selected + 1) % self.count();
    }

    // ── Tags ─────────────────────────────────────────────────────────────────

    /// Rebuild the global tag registry from all notes (deduplicated, sorted).
    pub fn refresh_all_tags(&mut self) {
        let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for note in &self.notes {
            for tag in &note.tags {
                set.insert(tag.to_lowercase());
            }
        }
        self.all_tags = set.into_iter().collect();
    }

    // ── Persistence helpers ──────────────────────────────────────────────────

    /// Mark the board as changed so it will be saved on the next debounce cycle.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Force an immediate save to disk if the board is dirty.
    pub fn flush_save(&mut self) {
        if self.dirty {
            let data = SaveData::from_notes(&self.notes, self.theme_idx);
            match crate::persistence::save_board(&data, self.board_path.as_ref()) {
                Ok(()) => {
                    self.dirty = false;
                    self.last_save = Instant::now();
                    self.save_error.clear();
                }
                Err(e) => {
                    self.save_error = e;
                }
            }
        }
    }

    /// Build a serialisable snapshot of the current board state.
    pub fn to_save_data(&self) -> SaveData {
        SaveData::from_notes(&self.notes, self.theme_idx)
    }

    // ── Status helpers (for the UI) ──────────────────────────────────────────

    /// Build the status-bar text showing theme, note info, and active filters.
    pub fn status_text(&self) -> String {
        if self.mode == InputMode::TagInput {
            return format!(" tag: {}", self.input);
        }
        if self.mode == InputMode::FilterInput {
            return format!(" filter by tag: {}", self.input);
        }

        let mut parts: Vec<String> = Vec::new();
        parts.push(format!("● {}", note::THEMES[self.theme_idx].name));

        if self.count() > 0 {
            let n = &self.notes[self.selected];
            let cname = color_name(&n.color);
            parts.push(format!("{}/{}", self.selected + 1, self.count()));
            parts.push(cname.to_string());
            parts.push(n.border_style.clone());
            if !n.tags.is_empty() {
                parts.push(n.tags.join(","));
            }
            if n.editing {
                parts.push("EDITING".into());
            }
        } else {
            parts.push("0 notes".into());
        }

        if !self.filter_tag.is_empty() {
            let visible = self.visible_count();
            parts.push(format!("◇ {} ({})", self.filter_tag, visible));
        }

        if !self.save_error.is_empty() {
            parts.push(format!("⚠ {}", self.save_error));
        }

        parts.join(" │ ")
    }

    /// Context-sensitive hint text for the bottom hint bar.
    pub fn hint_bar(&self) -> &'static str {
        if self.confirm_delete {
            return " Delete note?  y:yes  n:no  Esc:cancel";
        }
        if self.confirm_clear_tags {
            return " Clear ALL tags?  y:yes  Esc:cancel";
        }
        if self.mode == InputMode::TagInput {
            return " Enter:confirm  Esc:cancel  Backspace:delete";
        }
        if self.mode == InputMode::FilterInput {
            return " Enter:apply filter  Esc:cancel  Backspace:delete";
        }
        if self.count() > 0 && self.notes[self.selected].editing {
            return match self.edit_focus {
                EditFocus::Header => {
                    " Esc:stop  Tab:content  Enter:content  ^c:copy  ^x:cut  ^v:paste  ^a:all  Backspace:del"
                }
                EditFocus::Tags => {
                    " Esc:stop  Tab:header  Enter:add  ←/→:select tag  Backspace:del  [type tag name]"
                }
                EditFocus::Content => {
                    " Esc:stop  Tab:tags  Enter:newline  ^c:copy  ^x:cut  ^v:paste  ^a:all  ←/→/↑/↓:move"
                }
            };
        }
        " n:new  d:del  e:edit  c:color  b:border  ←/→:navigate  T:filter  /:search  O:overlay  ?:help  ^R:theme  q:quit"
    }
}

// ── Clipboard helpers ─────────────────────────────────────────────────────────

/// Copy text to the system clipboard.
fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let mut clip = arboard::Clipboard::new().map_err(|e| format!("clipboard: {e}"))?;
    clip.set_text(text).map_err(|e| format!("clipboard: {e}"))
}

/// Read text from the system clipboard.
fn paste_from_clipboard() -> Result<String, String> {
    let mut clip = arboard::Clipboard::new().map_err(|e| format!("clipboard: {e}"))?;
    clip.get_text().map_err(|e| format!("clipboard: {e}"))
}
