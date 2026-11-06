// stickynote — application state and event dispatch.

use std::time::{Duration, Instant};

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::note::{
    self, BORDER_STYLES, NOTE_COLORS, Note, abs_diff_u16, char_idx_to_byte, char_len, color_name,
    cycle_str,
};
use crate::persistence::{BoardStore, ExportFormat, SaveData, check_board_path};

// ── Input modes ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    TagInput,
    FilterInput,
    SearchInput,
    ExportInput {
        visible: bool,
        format: Option<ExportFormat>,
    },
    BoardInput,
}

// ── Edit focus (Tab-navigable areas within a card) ───────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditFocus {
    Header,
    Tags,
    Content,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitState {
    Running,
    SaveFailed,
    ConfirmDiscard,
    Done,
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

// History is session-only: both stacks share a count and estimated allocation budget.
const HISTORY_LIMIT: usize = 100;
const HISTORY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone)]
struct BoardSnapshot {
    notes: Vec<Note>,
    theme_idx: usize,
    selected: Option<usize>,
    filter_tag: String,
    search_query: String,
    edit_focus: EditFocus,
    show_overlay: bool,
}

impl BoardSnapshot {
    fn bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.filter_tag.len()
            + self.search_query.len()
            + self
                .notes
                .iter()
                .map(|note| {
                    std::mem::size_of::<Note>()
                        + note.content.len()
                        + note.title.len()
                        + note.color.len()
                        + note.font_style.len()
                        + note.border_style.len()
                        + note.tag_input.len()
                        + note
                            .tags
                            .iter()
                            .map(|tag| std::mem::size_of::<String>() + tag.len())
                            .sum::<usize>()
                })
                .sum::<usize>()
    }
}

// ── App ───────────────────────────────────────────────────────────────────────

/// Application state holding all notes, UI mode, and interaction history.
pub struct App {
    pub notes: Vec<Note>,
    pub selected: Option<usize>,
    pub width: u16,
    pub height: u16,
    pub theme_idx: usize,
    pub menu: MenuState,

    pub mode: InputMode,
    pub input: String,
    pub show_help: bool,
    pub help_scroll: usize,
    pub show_overlay: bool,
    pub quit_state: QuitState,

    pub filter_tag: String,
    pub search_query: String,
    undo: Vec<BoardSnapshot>,
    redo: Vec<BoardSnapshot>,
    recording_history: bool,
    typing_group: Option<(usize, EditFocus, Instant)>,
    pub all_tags: Vec<String>,

    // Double-click tracking.
    pub last_click: Instant,
    pub last_click_btn: Option<MouseButton>,
    pub last_click_x: u16,
    pub last_click_y: u16,

    // Save debounce.
    pub last_edit: Instant,
    pub last_save_attempt: Option<Instant>,
    pub save_error: String,
    pub clipboard_error: String,
    pub file_message: String,
    clipboard: Option<arboard::Clipboard>,
    pub dirty: bool,
    pub confirm_delete: bool,
    pub confirm_clear_tags: bool,

    pub edit_focus: EditFocus,

    /// Owns the board file and its editing-session lock.
    board: BoardStore,

    /// True while the user is dragging the mouse to select text.
    pub mouse_dragging: bool,
}

impl App {
    pub fn new(save: SaveData, board: BoardStore) -> Self {
        let theme_idx = save.theme_idx.clamp(0, note::THEMES.len() - 1);
        let notes = save.into_notes();
        let selected = (!notes.is_empty()).then_some(0);
        let mut app = App {
            notes,
            selected,
            width: 0,
            height: 0,
            theme_idx,
            menu: MenuState::new(),
            mode: InputMode::Normal,
            input: String::new(),
            show_help: false,
            help_scroll: 0,
            show_overlay: false,
            quit_state: QuitState::Running,
            filter_tag: String::new(),
            search_query: String::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            recording_history: false,
            typing_group: None,
            all_tags: Vec::new(),
            last_click: Instant::now(),
            last_click_btn: None,
            last_click_x: 0,
            last_click_y: 0,
            last_edit: Instant::now(),
            last_save_attempt: None,
            save_error: String::new(),
            clipboard_error: String::new(),
            file_message: String::new(),
            clipboard: None,
            dirty: false,
            confirm_delete: false,
            confirm_clear_tags: false,
            edit_focus: EditFocus::Content,
            board,
            mouse_dragging: false,
        };
        app.refresh_all_tags();
        app
    }

    // ── Count ────────────────────────────────────────────────────────────────

    /// Number of notes on the board.
    pub fn count(&self) -> usize {
        self.notes.len()
    }

    /// One visibility rule: exact tag AND case-insensitive title/body/tag substring.
    pub fn visible_note_indices(&self) -> Vec<usize> {
        let query = self.search_query.to_lowercase();
        self.notes
            .iter()
            .enumerate()
            .filter(|(_, note)| {
                (self.filter_tag.is_empty() || note.has_tag(&self.filter_tag))
                    && (query.is_empty()
                        || note.title.to_lowercase().contains(&query)
                        || note.content.to_lowercase().contains(&query)
                        || note
                            .tags
                            .iter()
                            .any(|tag| tag.to_lowercase().contains(&query)))
            })
            .map(|(index, _)| index)
            .collect()
    }

    /// Number of notes matching the current tag filter.
    pub fn visible_count(&self) -> usize {
        self.visible_note_indices().len()
    }

    fn select_note(&mut self, selected: Option<usize>) {
        if self.selected == selected {
            return;
        }
        if let Some(previous) = self.selected {
            let note = &mut self.notes[previous];
            note.editing = false;
            note.clear_all_selections();
        }
        self.selected = selected;
        self.typing_group = None;
        self.show_overlay = false;
        self.mouse_dragging = false;
        self.confirm_delete = false;
        self.confirm_clear_tags = false;
        self.menu.visible = false;
    }

    fn reconcile_selection(&mut self) {
        let visible = self.visible_note_indices();
        let selected = self
            .selected
            .filter(|index| visible.contains(index))
            .or_else(|| visible.first().copied());
        self.select_note(selected);
    }

    fn set_filter(&mut self, tag: String) {
        self.filter_tag = tag;
        self.reconcile_selection();
    }

    // ── Keyboard dispatch ────────────────────────────────────────────────────

    /// Process a crossterm key event and update application state.
    pub fn handle_key(&mut self, key: KeyEvent) {
        self.handle_key_with_copy(key, copy_to_clipboard);
    }

    fn handle_key_with_copy(
        &mut self,
        key: KeyEvent,
        copy: impl FnOnce(&mut Option<arboard::Clipboard>, &str) -> Result<(), String>,
    ) {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }
        let typing = self.selected.and_then(|index| {
            let note = &self.notes[index];
            (note.editing
                && self.mode == InputMode::Normal
                && !self.show_help
                && !self.menu.visible
                && !key.modifiers.intersects(
                    KeyModifiers::CONTROL
                        | KeyModifiers::ALT
                        | KeyModifiers::SUPER
                        | KeyModifiers::HYPER
                        | KeyModifiers::META,
                )
                && (matches!(key.code, KeyCode::Char(c) if !c.is_control())
                    || (key.code == KeyCode::Enter && self.edit_focus == EditFocus::Content))
                && match self.edit_focus {
                    EditFocus::Header => !note.has_title_selection(),
                    EditFocus::Content => !note.has_content_selection(),
                    EditFocus::Tags => false,
                })
            .then_some((index, self.edit_focus))
        });
        let before = self.begin_history();
        self.handle_key_inner(key, copy);
        self.finish_history(before, typing);
    }

    fn handle_key_inner(
        &mut self,
        key: KeyEvent,
        copy: impl FnOnce(&mut Option<arboard::Clipboard>, &str) -> Result<(), String>,
    ) {
        if key.modifiers.intersects(
            KeyModifiers::ALT | KeyModifiers::SUPER | KeyModifiers::HYPER | KeyModifiers::META,
        ) || (key.modifiers.contains(KeyModifiers::CONTROL)
            && !matches!(
                key.code,
                KeyCode::Char(_)
                    | KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Backspace
                    | KeyCode::Delete
            ))
            || (key
                .modifiers
                .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT)
                && matches!(key.code, KeyCode::Backspace | KeyCode::Delete))
        {
            return;
        }
        let mut ks: String = match key.code {
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
            KeyCode::PageUp => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    "shift+pageup".into()
                } else {
                    "pageup".into()
                }
            }
            KeyCode::PageDown => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    "shift+pagedown".into()
                } else {
                    "pagedown".into()
                }
            }
            KeyCode::Delete => "delete".into(),
            KeyCode::BackTab => "shift+tab".into(),
            _ => return,
        };

        if key.modifiers.contains(KeyModifiers::CONTROL) && !matches!(key.code, KeyCode::Char(_)) {
            ks.insert(0, '^');
        }

        if self.quit_state != QuitState::Running {
            self.handle_quit_key(&ks);
            return;
        }

        if self.show_help {
            self.handle_help_key(&ks);
            return;
        }
        if ks == "^s" {
            let _ = self.flush_save();
            return;
        }
        if matches!(ks.as_str(), "^z" | "^y")
            && !self.menu.visible
            && self.mode == InputMode::Normal
        {
            self.restore_history(ks == "^y");
            return;
        }

        // Menu mode.
        if self.menu.visible {
            self.handle_menu_key(&ks);
            return;
        }

        // Inline and overlay editing share clipboard and key dispatch.
        // Ctrl+C: copy the focused selection, else quit (or exit a prompt).
        if ks == "^c" {
            if self.selected.is_some_and(|selected| {
                let note = &self.notes[selected];
                note.editing
                    && match self.edit_focus {
                        EditFocus::Header => note.has_title_selection(),
                        EditFocus::Content => note.has_content_selection(),
                        EditFocus::Tags => false,
                    }
            }) {
                let mut clipboard = self.clipboard.take();
                self.copy_selection_with(|text| copy(&mut clipboard, text));
                self.clipboard = clipboard;
                return;
            }
            if self.mode != InputMode::Normal {
                if self.mode == InputMode::SearchInput {
                    self.set_search(String::new());
                    self.input.clear();
                }
                self.mode = InputMode::Normal;
                return;
            }
            self.request_quit();
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
            InputMode::SearchInput => {
                self.handle_search_input(&ks);
                return;
            }
            InputMode::ExportInput { .. } | InputMode::BoardInput => {
                self.handle_file_input(&ks);
                return;
            }
            InputMode::Normal => {}
        }

        // Editing is tracked per-note (matching the Go pattern).
        if self
            .selected
            .is_some_and(|selected| self.notes[selected].editing)
        {
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
                if let Some(selected) = self.selected {
                    let note = &mut self.notes[selected];
                    if !note.tags.is_empty() {
                        note.tags.clear();
                        self.refresh_all_tags();
                        self.mark_dirty();
                    }
                }
            } else {
                self.confirm_clear_tags = false;
            }
            return;
        }

        // Normal mode keybindings.
        match ks.as_str() {
            "q" => self.request_quit(),
            "E" if self.selected.is_some() => self.start_file_input(false),
            "^e" if self.visible_count() > 0 => self.start_file_input(true),
            "B" => {
                self.mode = InputMode::BoardInput;
                self.input.clear();
                self.file_message.clear();
            }
            "?" => {
                self.show_help = true;
                self.help_scroll = 0;
            }
            "n" => self.add_note(),
            "d" => {
                if self.selected.is_some() {
                    self.confirm_delete = true;
                }
            }
            "^d" => {
                self.duplicate_selected();
            }
            "e" | "enter" if self.selected.is_some() => {
                self.toggle_edit();
            }
            "c" if self.selected.is_some() => {
                self.cycle_color();
            }
            "b" if self.selected.is_some() => {
                self.cycle_border();
            }
            "t" => {
                if self.selected.is_some() {
                    self.mode = InputMode::TagInput;
                    self.input.clear();
                }
            }
            "T" => {
                if !self.filter_tag.is_empty() {
                    self.set_filter(String::new());
                } else if let Some(tag) = self.all_tags.first().cloned() {
                    self.set_filter(tag);
                }
            }
            "^t" => {
                if let Some(selected) = self.selected {
                    let note = &self.notes[selected];
                    if !note.tags.is_empty() {
                        self.confirm_clear_tags = true;
                    }
                }
            }
            "/" => {
                self.mode = InputMode::FilterInput;
                self.input.clear();
            }
            "s" => {
                self.mode = InputMode::SearchInput;
                self.input = self.search_query.clone();
            }
            "up" | "k" => self.select_prev(),
            "down" | "j" => self.select_next(),
            "left" => self.select_prev(),
            "right" => self.select_next(),
            "tab" => self.select_next(),
            "shift+tab" => self.select_prev(),
            "O" => self.toggle_overlay(),
            "[" => {
                if let Some(selected) = self.selected
                    && selected + 1 < self.count()
                {
                    self.notes.swap(selected, selected + 1);
                    self.selected = Some(selected + 1);
                    self.mark_dirty();
                }
            }
            "]" => {
                if let Some(selected) = self.selected
                    && selected > 0
                {
                    self.notes.swap(selected, selected - 1);
                    self.selected = Some(selected - 1);
                    self.mark_dirty();
                }
            }
            "^r" => {
                self.theme_idx = (self.theme_idx + 1) % note::THEMES.len();
                self.mark_dirty();
            }
            "esc" => {
                self.search_query.clear();
                self.set_filter(String::new());
            }
            _ => {}
        }
    }

    fn start_file_input(&mut self, visible: bool) {
        self.mode = InputMode::ExportInput {
            visible,
            format: None,
        };
        self.input.clear();
        self.file_message.clear();
    }

    fn handle_file_input(&mut self, key: &str) {
        match key {
            "esc" => {
                self.mode = InputMode::Normal;
                self.input.clear();
                self.file_message.clear();
            }
            "backspace" => {
                note::pop_grapheme(&mut self.input);
            }
            "tab" => {
                if let InputMode::ExportInput { format, .. } = &mut self.mode {
                    *format = match format {
                        None => Some(ExportFormat::Markdown),
                        Some(ExportFormat::Markdown) => Some(ExportFormat::Text),
                        Some(ExportFormat::Text) => None,
                    };
                }
            }
            "enter" => {
                let path = std::path::PathBuf::from(&self.input);
                let result = match self.mode {
                    InputMode::BoardInput => self.switch_board(&path),
                    InputMode::ExportInput { visible, format } => {
                        let format = format
                            .map(Ok)
                            .unwrap_or_else(|| ExportFormat::from_path(&path));
                        format.and_then(|format| {
                            let indices = if visible {
                                self.visible_note_indices()
                            } else {
                                self.selected.into_iter().collect()
                            };
                            let notes: Vec<&Note> =
                                indices.iter().map(|&i| &self.notes[i]).collect();
                            self.board.export(&path, &notes, format)
                        })
                    }
                    _ => return,
                };
                match result {
                    Ok(()) => {
                        self.mode = InputMode::Normal;
                        self.input.clear();
                        self.file_message = format!("Completed: {}", path.display());
                    }
                    Err(error) => self.file_message = error,
                }
            }
            _ => {
                if let Some(c) = input_character(key) {
                    self.input.push(c);
                }
            }
        }
    }

    pub fn board_path(&self) -> &std::path::Path {
        self.board.path()
    }

    /// Keep the old store/lock and work until both destination validation/locking
    /// and saving the current board succeed. No registry or missing-file creation.
    fn switch_board(&mut self, path: &std::path::Path) -> Result<(), String> {
        check_board_path(path)?;
        let path = std::fs::canonicalize(path)
            .map_err(|e| format!("cannot open existing board {}: {e}", path.display()))?;
        if path == self.board.path() {
            return self.flush_save();
        }
        let (board, data) = BoardStore::open_existing(path)?;
        self.flush_save()?;
        let mut next = Self::new(data, board);
        next.width = self.width;
        next.height = self.height;
        next.clipboard = self.clipboard.take();
        *self = next;
        // App::new resets recording_history too: the key boundary must not push
        // the previous board into the new session's undo stack.
        Ok(())
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
    }

    // ── Tag input ────────────────────────────────────────────────────────────

    fn handle_tag_input(&mut self, ks: &str) {
        match ks {
            "esc" => self.mode = InputMode::Normal,
            "enter" => {
                let tag = note::Note::normalize_tag(&self.input);
                if !tag.is_empty()
                    && let Some(selected) = self.selected
                {
                    let note = &mut self.notes[selected];
                    if !note.has_tag(&tag) {
                        note.tags.push(tag);
                        self.refresh_all_tags();
                        self.mark_dirty();
                    }
                }
                self.mode = InputMode::Normal;
            }
            "backspace" => {
                note::pop_grapheme(&mut self.input);
            }
            _ => {
                if let Some(c) = input_character(ks) {
                    self.input.push(c);
                }
            }
        }
    }

    // ── Filter input ─────────────────────────────────────────────────────────

    fn handle_filter_input(&mut self, ks: &str) {
        match ks {
            "esc" => {
                self.set_filter(String::new());
                self.mode = InputMode::Normal;
            }
            "enter" => {
                self.set_filter(self.input.trim().to_lowercase());
                self.mode = InputMode::Normal;
            }
            "backspace" => {
                note::pop_grapheme(&mut self.input);
            }
            _ => {
                if let Some(c) = input_character(ks) {
                    self.input.push(c);
                }
            }
        }
    }

    fn set_search(&mut self, query: String) {
        self.search_query = query;
        self.reconcile_selection();
    }

    fn handle_search_input(&mut self, key: &str) {
        match key {
            "esc" => {
                self.set_search(String::new());
                self.input.clear();
                self.mode = InputMode::Normal;
            }
            "enter" => self.mode = InputMode::Normal,
            "up" | "shift+tab" => self.select_prev(),
            "down" | "tab" => self.select_next(),
            "backspace" => {
                note::pop_grapheme(&mut self.input);
                self.set_search(self.input.clone());
            }
            _ => {
                if let Some(c) = input_character(key) {
                    self.input.push(c);
                    self.set_search(self.input.clone());
                }
            }
        }
    }

    // ── Edit key handling ────────────────────────────────────────────────────

    fn handle_edit_key(&mut self, ks: &str) {
        let Some(selected) = self.selected else {
            return;
        };

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
                self.stop_editing();
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
                let note = &mut self.notes[selected];
                match self.edit_focus {
                    EditFocus::Header => {
                        note.title_cursor = char_len(&note.title);
                        if !note.title.is_empty() {
                            note.title_sel_start = Some(0);
                            note.title_sel_end = Some(char_len(&note.title));
                        }
                    }
                    EditFocus::Content if !note.content.is_empty() => {
                        note.cursor = char_len(&note.content);
                        note.sel_start = Some(0);
                        note.sel_end = Some(char_len(&note.content));
                    }
                    _ => {}
                }
                return;
            }
            "tab" => {
                // In Tags focus with matching suggestions: autofill.
                if self.edit_focus == EditFocus::Tags {
                    let input = Note::normalize_tag(&self.notes[selected].tag_input);
                    if !input.is_empty()
                        && let Some(matched) = self
                            .tag_suggestions()
                            .first()
                            .filter(|tag| **tag != input)
                            .map(|tag| (*tag).to_string())
                    {
                        self.notes[selected].tag_input = matched;
                        self.notes[selected].tag_cursor = None;
                        return;
                    }
                }
                self.notes[selected].clear_all_selections();
                self.edit_focus = next_focus(self.edit_focus);
                return;
            }
            "shift+tab" => {
                self.notes[selected].clear_all_selections();
                self.edit_focus = prev_focus(self.edit_focus);
                return;
            }
            _ => {}
        }

        // Dispatch based on current focus.
        match self.edit_focus {
            EditFocus::Header => {
                let note = &mut self.notes[selected];
                match ks {
                    "enter" | "down" => {
                        note.clear_title_selection();
                        // Move to content.
                        self.edit_focus = EditFocus::Content;
                    }
                    "backspace" | "delete" | "^backspace" | "^delete" => {
                        if note.delete_title(ks.ends_with("backspace"), ks.starts_with('^')) {
                            self.mark_dirty();
                        }
                    }
                    "left" | "right" | "shift+left" | "shift+right" | "^left" | "^right"
                    | "^shift+left" | "^shift+right" => {
                        if ks.contains("shift+") {
                            note.title_sel_start.get_or_insert(note.title_cursor);
                        } else {
                            note.clear_title_selection();
                        }
                        note.title_cursor = horizontal_target(&note.title, note.title_cursor, ks);
                        if ks.contains("shift+") {
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
                        note.title_cursor = char_len(&note.title);
                        note.title_sel_end = Some(char_len(&note.title));
                    }
                    "home" => {
                        note.title_cursor = 0;
                        note.clear_title_selection();
                    }
                    "end" => {
                        note.title_cursor = char_len(&note.title);
                        note.clear_title_selection();
                    }
                    _ => {
                        if let Some(c) = input_character(ks)
                            && note.insert_title(&c.to_string())
                        {
                            self.mark_dirty();
                        }
                    }
                }
            }
            EditFocus::Tags => {
                let mut tags_changed = false;
                {
                    let note = &mut self.notes[selected];
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
                                note::pop_grapheme(&mut note.tag_input);
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
                            if let Some(c) = input_character(ks) {
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
                let page_height = self
                    .note_layout()
                    .map_or(0, |layout| layout.body.text.height as usize);
                let note = &mut self.notes[selected];
                match ks {
                    "enter" => {
                        if note.insert_content("\n") {
                            self.mark_dirty();
                        }
                    }
                    "backspace" | "delete" | "^backspace" | "^delete" => {
                        if note.delete_content(ks.ends_with("backspace"), ks.starts_with('^')) {
                            self.mark_dirty();
                        }
                    }
                    "left" | "right" | "shift+left" | "shift+right" | "^left" | "^right"
                    | "^shift+left" | "^shift+right" => {
                        if ks.contains("shift+") {
                            note.sel_start.get_or_insert(note.cursor);
                        } else {
                            note.clear_content_selection();
                        }
                        note.cursor = horizontal_target(&note.content, note.cursor, ks);
                        if ks.contains("shift+") {
                            note.sel_end = Some(note.cursor);
                        }
                    }
                    "up" | "down" => {
                        Self::move_content_vertical(note, if ks == "up" { -1 } else { 1 });
                        note.clear_content_selection();
                    }
                    "pageup" | "pagedown" | "shift+pageup" | "shift+pagedown" => {
                        let selecting = ks.starts_with("shift+");
                        if selecting && note.sel_start.is_none() {
                            note.sel_start = Some(note.cursor);
                        }
                        let step = if ks.ends_with("pageup") {
                            -(page_height as isize)
                        } else {
                            page_height
                                .min(note.content.split('\n').count() - 1 - note.cursor_pos().0)
                                as isize
                        };
                        Self::move_content_vertical(note, step);
                        note.viewport.top_line = note.viewport.top_line.saturating_add_signed(step);
                        if selecting {
                            note.sel_end = Some(note.cursor);
                        } else {
                            note.clear_content_selection();
                        }
                    }
                    "home" => {
                        let byte_pos = char_idx_to_byte(&note.content, note.cursor);
                        let before = &note.content[..byte_pos];
                        let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
                        note.cursor = note.content[..line_start].chars().count();
                        note.clear_content_selection();
                    }
                    "end" => {
                        let byte_pos = char_idx_to_byte(&note.content, note.cursor);
                        let after = &note.content[byte_pos..];
                        let line_end = after
                            .find('\n')
                            .map(|i| byte_pos + i)
                            .unwrap_or(note.content.len());
                        note.cursor = note::grapheme_floor(
                            &note.content,
                            char_len(&note.content[..line_end]),
                        );
                        note.clear_content_selection();
                    }
                    "shift+up" | "shift+down" => {
                        if note.sel_start.is_none() {
                            note.sel_start = Some(note.cursor);
                        }
                        if ks == "shift+up" && note.cursor_pos().0 == 0 {
                            note.cursor = 0;
                        } else {
                            Self::move_content_vertical(
                                note,
                                if ks == "shift+up" { -1 } else { 1 },
                            );
                        }
                        note.sel_end = Some(note.cursor);
                    }
                    "shift+home" => {
                        if note.sel_start.is_none() {
                            note.sel_start = Some(note.cursor);
                        }
                        let byte_pos = char_idx_to_byte(&note.content, note.cursor);
                        let before = &note.content[..byte_pos];
                        let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
                        note.cursor = note.content[..line_start].chars().count();
                        note.sel_end = Some(note.cursor);
                    }
                    "shift+end" => {
                        if note.sel_start.is_none() {
                            note.sel_start = Some(note.cursor);
                        }
                        let byte_pos = char_idx_to_byte(&note.content, note.cursor);
                        let after = &note.content[byte_pos..];
                        let line_end = after
                            .find('\n')
                            .map(|i| byte_pos + i)
                            .unwrap_or(note.content.len());
                        note.cursor = note::grapheme_floor(
                            &note.content,
                            char_len(&note.content[..line_end]),
                        );
                        note.sel_end = Some(note.cursor);
                    }
                    _ => {
                        if let Some(c) = input_character(ks)
                            && note.insert_content(&c.to_string())
                        {
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
        let before = self.begin_history();
        self.handle_mouse_inner(mouse);
        self.finish_history(before, None);
    }

    fn handle_mouse_inner(&mut self, mouse: MouseEvent) {
        if self.quit_state != QuitState::Running {
            return;
        }
        if self.show_help {
            match mouse.kind {
                MouseEventKind::ScrollUp => self.handle_help_key("up"),
                MouseEventKind::ScrollDown => self.handle_help_key("down"),
                _ => {}
            }
            return;
        }
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
        if self
            .selected
            .is_some_and(|selected| self.notes[selected].editing)
        {
            self.handle_mouse_edit(mouse);
            return;
        }
        if self.menu.visible {
            self.handle_menu_mouse(mouse);
            return;
        }
        if matches!(
            mouse.kind,
            MouseEventKind::Down(_) | MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ) {
            self.handle_mouse_press(mouse);
        }
    }

    fn handle_mouse_press(&mut self, mouse: MouseEvent) {
        let mx = mouse.column;
        let my = mouse.row;

        // Info bar click at row h-2.
        if matches!(mouse.kind, MouseEventKind::Down(_))
            && self.height >= 2
            && my == self.height - 2
        {
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
                self.show_context_menu(mx, my, self.selected.is_some());
            }
            MouseEventKind::ScrollUp => self.select_prev(),
            MouseEventKind::ScrollDown => self.select_next(),
            MouseEventKind::Down(MouseButton::Middle) if my == 0 && self.select_tab_at_x(mx) => {
                // Tab bar click (row 0): select + delete.
                self.delete_selected();
            }
            _ => {}
        }
    }

    /// Select a note by clicking on a tab at the given x-coordinate.
    /// Returns `true` if a tab was found and selection changed.
    fn select_tab_at_x(&mut self, mx: u16) -> bool {
        if let Some(idx) = crate::ui::note_index_at_tab_x(self, mx) {
            self.select_note(Some(idx));
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
                self.select_note(Some(idx));
                if is_double {
                    self.toggle_edit();
                }
            }
            return;
        }

        // Read-only Markdown is wrapped and has no source-cell mapping. This click
        // only enters editing via toggle_edit (Header, cursors at field ends).
        // Never feed the wrapped row to mouse_edit_click's unwrapped projection.
        // Outer borders, separators and gutters are not editable fields.
        if self.note_layout().is_some_and(|layout| {
            layout
                .focus_at(ratatui::layout::Position::new(mx, my))
                .is_some()
        }) {
            self.toggle_edit();
        }
    }

    fn handle_status_bar_click(&mut self, mx: u16) {
        if self.width == 0 {
            return;
        }
        if mx < self.width / 3 {
            self.theme_idx = (self.theme_idx + 1) % note::THEMES.len();
            self.mark_dirty();
        } else if self.selected.is_some() && mx < 2 * self.width / 3 {
            self.cycle_color();
        }
    }

    fn handle_menu_mouse(&mut self, mouse: MouseEvent) {
        if !matches!(mouse.kind, MouseEventKind::Down(_)) {
            return;
        }
        let mx = mouse.column;
        let my = mouse.row;
        let rect = crate::ui::menu_rect(
            self,
            ratatui::layout::Rect::new(0, 0, self.width, self.height),
        );
        if rect.contains(ratatui::layout::Position::new(mx, my)) {
            if mx > rect.x && mx < rect.right() - 1 && my > rect.y && my < rect.bottom() - 1 {
                let idx = (my - rect.y - 1) as usize;
                if idx < self.menu.items.len() {
                    self.menu.selected = idx;
                    self.activate_menu();
                }
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

        self.menu.x = x;
        self.menu.y = y;
        self.menu.selected = 0;
        self.menu.visible = true;
    }

    // ── Note operations ──────────────────────────────────────────────────────

    /// Insert a new blank note at the top of the stack.
    pub fn add_note(&mut self) {
        let before = self.begin_history();
        self.add_note_inner();
        self.finish_history(before, None);
    }

    fn add_note_inner(&mut self) {
        let color_idx = self.count() % NOTE_COLORS.len();
        let note = Note {
            color: NOTE_COLORS[color_idx].to_string(),
            font_style: "normal".to_string(),
            border_style: "rounded".to_string(),
            ..Note::new()
        };
        self.select_note(None);
        self.filter_tag.clear();
        self.search_query.clear();
        self.notes.insert(0, note);
        self.selected = Some(0);
        self.refresh_all_tags();
        self.mark_dirty();
    }

    /// Remove the currently selected note from the board.
    pub fn delete_selected(&mut self) {
        let before = self.begin_history();
        self.delete_selected_inner();
        self.finish_history(before, None);
    }

    fn delete_selected_inner(&mut self) {
        let Some(selected) = self.selected else {
            return;
        };
        if self.notes[selected].editing {
            return;
        }
        self.select_note(None);
        self.notes.remove(selected);
        self.refresh_all_tags();
        self.mark_dirty();
    }

    /// Toggle editing mode on the selected note, resetting cursors and focus.
    pub fn toggle_edit(&mut self) {
        let Some(selected) = self.selected else {
            return;
        };
        if self.notes[selected].editing {
            self.stop_editing();
        } else {
            self.start_editing(EditFocus::Header);
        }
    }

    fn start_editing(&mut self, focus: EditFocus) {
        self.typing_group = None;
        let Some(selected) = self.selected else {
            return;
        };
        let note = &mut self.notes[selected];
        note.editing = true;
        note.viewport = crate::note::ContentViewport::new();
        note.cursor = char_len(&note.content);
        note.title_cursor = char_len(&note.title);
        note.tag_input.clear();
        note.tag_cursor = None;
        note.clear_all_selections();
        self.edit_focus = focus;
    }

    fn stop_editing(&mut self) {
        self.typing_group = None;
        if let Some(selected) = self.selected {
            self.notes[selected].editing = false;
            self.notes[selected].clear_all_selections();
        }
        self.show_overlay = false;
        self.mouse_dragging = false;
    }

    /// Create a copy of the selected note inserted directly after it.
    pub fn duplicate_selected(&mut self) {
        let before = self.begin_history();
        self.duplicate_selected_inner();
        self.finish_history(before, None);
    }

    fn duplicate_selected_inner(&mut self) {
        let Some(selected) = self.selected else {
            return;
        };
        let mut note = self.notes[selected].clone();
        note.editing = false;
        note.viewport = crate::note::ContentViewport::new();
        note.cursor = 0;
        note.title_cursor = 0;
        note.tag_input.clear();
        note.tag_cursor = None;
        note.clear_all_selections();
        self.select_note(None);
        self.notes.insert(selected + 1, note);
        self.selected = Some(selected + 1);
        self.refresh_all_tags();
        self.mark_dirty();
    }

    /// Toggle full-screen overlay mode for the selected note.
    pub fn toggle_overlay(&mut self) {
        if self.selected.is_none() {
            return;
        }
        if self.show_overlay {
            self.stop_editing();
        } else {
            self.start_editing(EditFocus::Content);
            self.show_overlay = true;
        }
    }

    // ── Clipboard operations ───────────────────────────────────────────────

    /// Copy selected text to the system clipboard.
    fn copy_selection_with(&mut self, copy: impl FnOnce(&str) -> Result<(), String>) {
        let Some(selected) = self.selected else {
            return;
        };
        let note = &self.notes[selected];
        let text = match self.edit_focus {
            EditFocus::Header => note.selected_title(),
            EditFocus::Content => note.selected_text(),
            EditFocus::Tags => return,
        };
        if text.is_empty() {
            return;
        }
        match copy(&text) {
            Ok(()) => self.clipboard_error.clear(),
            Err(e) => self.clipboard_error = e,
        }
    }

    /// Cut selected text to the system clipboard.
    pub fn cut_selection(&mut self) {
        let mut clipboard = self.clipboard.take();
        self.cut_selection_with(|text| copy_to_clipboard(&mut clipboard, text));
        self.clipboard = clipboard;
    }

    fn cut_selection_with(&mut self, copy: impl FnOnce(&str) -> Result<(), String>) {
        let before = self.begin_history();
        self.cut_selection_with_inner(copy);
        self.finish_history(before, None);
    }

    fn cut_selection_with_inner(&mut self, copy: impl FnOnce(&str) -> Result<(), String>) {
        let Some(selected) = self.selected else {
            return;
        };
        let note = &self.notes[selected];
        let text = match self.edit_focus {
            EditFocus::Header => note.selected_title(),
            EditFocus::Content => note.selected_text(),
            EditFocus::Tags => return,
        };
        if text.is_empty() {
            return;
        }
        if let Err(e) = copy(&text) {
            self.clipboard_error = e;
            return;
        }
        self.clipboard_error.clear();
        match self.edit_focus {
            EditFocus::Header => {
                self.notes[selected].delete_selected_title();
            }
            EditFocus::Content => {
                self.notes[selected].delete_selected_content();
            }
            EditFocus::Tags => unreachable!(),
        }
        self.mark_dirty();
    }

    /// Paste text from the system clipboard at the cursor position.
    pub fn paste_text(&mut self) {
        if self.selected.is_none() {
            return;
        }
        let text = match clipboard(&mut self.clipboard)
            .and_then(|clip| clip.get_text().map_err(|e| format!("clipboard: {e}")))
        {
            Ok(t) => t,
            Err(e) => {
                self.clipboard_error = e;
                return;
            }
        };
        self.clipboard_error.clear();
        self.insert_paste(&text);
    }

    /// Shared terminal/clipboard insertion: paste is data, never commands.
    pub fn insert_paste(&mut self, text: &str) {
        let before = self.begin_history();
        self.insert_paste_inner(text);
        self.finish_history(before, None);
    }

    fn insert_paste_inner(&mut self, text: &str) {
        if self.quit_state != QuitState::Running || self.show_help || self.menu.visible {
            return;
        }
        if matches!(
            self.mode,
            InputMode::BoardInput | InputMode::ExportInput { .. }
        ) {
            self.input.extend(text.chars().filter(|c| !c.is_control()));
            return;
        }
        let Some(selected) = self.selected.filter(|&i| self.notes[i].editing) else {
            return;
        };
        if self.mode != InputMode::Normal {
            return;
        }
        let filtered: String = text
            .chars()
            .filter(|c| !c.is_control() || (*c == '\n' && self.edit_focus == EditFocus::Content))
            .collect();
        if filtered.is_empty() {
            return;
        }
        let note = &mut self.notes[selected];
        let changed = match self.edit_focus {
            EditFocus::Header => note.insert_title(&filtered),
            EditFocus::Content => note.insert_content(&filtered),
            EditFocus::Tags => {
                note.tag_input.push_str(&filtered);
                note.tag_cursor = None;
                false
            }
        };
        if changed {
            self.mark_dirty();
        }
    }

    // ── Mouse editing (cursor positioning + selection) ──────────────────────

    /// Handle mouse events while a note is being edited.
    fn handle_mouse_edit(&mut self, mouse: MouseEvent) {
        let Some(selected) = self.selected else {
            return;
        };
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Right | MouseButton::Middle) => self.stop_editing(),
            MouseEventKind::Down(MouseButton::Left) => {
                self.mouse_dragging = false;
                self.mouse_edit_click(mouse.column, mouse.row);
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if !self.mouse_dragging {
                    self.mouse_dragging = true;
                    let note = &mut self.notes[selected];
                    match self.edit_focus {
                        EditFocus::Header => {
                            note.title_sel_start = Some(note.title_cursor);
                            note.title_sel_end = Some(note.title_cursor);
                        }
                        EditFocus::Content => {
                            note.sel_start = Some(note.cursor);
                            note.sel_end = Some(note.cursor);
                        }
                        EditFocus::Tags => return,
                    }
                }
                self.mouse_edit_drag(mouse.column, mouse.row);
            }
            MouseEventKind::Up(MouseButton::Left) => self.mouse_dragging = false,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                if self.edit_focus == EditFocus::Content =>
            {
                Self::move_content_vertical(
                    &mut self.notes[selected],
                    if mouse.kind == MouseEventKind::ScrollUp {
                        -1
                    } else {
                        1
                    },
                );
                self.notes[selected].clear_content_selection();
            }
            _ => {}
        }
    }

    /// Synchronize transient viewport state once, before drawing the next frame.
    pub fn prepare_frame(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.help_scroll = self.help_scroll.min(crate::ui::help_max_scroll(height));
        if let Some(selected) = self.selected
            && self.notes[selected].editing
            && let Some(layout) = self.note_layout()
        {
            crate::layout::follow_body(
                &mut self.notes[selected],
                layout.body.text,
                self.edit_focus == EditFocus::Content,
            );
        }
    }

    fn note_layout(&self) -> Option<crate::layout::NoteLayout> {
        let note = &self.notes[self.selected?];
        let rect = crate::layout::card_rect(self.width, self.height, self.show_overlay)?;
        Some(crate::layout::NoteLayout::new(
            rect,
            note,
            note.editing.then_some(self.edit_focus),
        ))
    }

    /// Map the cells currently shown, before changing focus or moving the caret.
    fn mouse_edit_click(&mut self, mx: u16, my: u16) {
        let Some(selected) = self.selected else {
            return;
        };
        let Some(layout) = self.note_layout() else {
            return;
        };
        if !layout.editing_visible(self.edit_focus) {
            return;
        }
        let point = ratatui::layout::Position::new(mx, my);
        if self.edit_focus == EditFocus::Tags && !self.notes[selected].tag_input.is_empty() {
            let count = self.tag_suggestions().len();
            if layout.popup(count).is_some_and(|rect| rect.contains(point)) {
                return;
            }
        }
        let Some(focus) = layout.focus_at(point) else {
            return;
        };
        let note = &mut self.notes[selected];
        match focus {
            EditFocus::Header => {
                let line = crate::layout::header_line(
                    note,
                    self.edit_focus == EditFocus::Header,
                    layout.header.text.width,
                );
                note.title_cursor = line
                    .index_at_cell((mx - layout.header.text.x) as usize)
                    .min(char_len(&note.title));
            }
            EditFocus::Content => {
                let (line, start) = crate::layout::body_line(
                    note,
                    (my - layout.body.text.y) as usize,
                    layout.body.text.width,
                    self.edit_focus == EditFocus::Content,
                );
                note.cursor = note::grapheme_floor(
                    &note.content,
                    start + line.index_at_cell((mx - layout.body.text.x) as usize),
                );
            }
            EditFocus::Tags => {
                let tags = crate::layout::TagsLayout::new(
                    note,
                    layout.tags.as_ref().unwrap().text,
                    self.edit_focus == EditFocus::Tags,
                );
                note.tag_cursor = tags
                    .chips
                    .iter()
                    .find(|chip| chip.rect.contains(point))
                    .map(|chip| chip.index);
            }
        }
        note.clear_all_selections();
        self.edit_focus = focus;
    }

    /// A drag stays in its starting field and clamps at the visible text edges.
    fn mouse_edit_drag(&mut self, mx: u16, my: u16) {
        let Some(selected) = self.selected else {
            return;
        };
        let Some(layout) = self.note_layout() else {
            return;
        };
        let rect = match self.edit_focus {
            EditFocus::Header => layout.header.text,
            EditFocus::Content => layout.body.text,
            EditFocus::Tags => return,
        };
        if rect.is_empty() {
            return;
        }
        let x = mx.clamp(rect.x, rect.right()) - rect.x;
        let y = my.clamp(rect.y, rect.bottom() - 1) - rect.y;
        let note = &mut self.notes[selected];
        match self.edit_focus {
            EditFocus::Header => {
                note.title_cursor = crate::layout::header_line(note, true, rect.width)
                    .index_at_cell(x as usize)
                    .min(char_len(&note.title));
                note.title_sel_end = Some(note.title_cursor);
            }
            EditFocus::Content => {
                let (line, start) = crate::layout::body_line(note, y as usize, rect.width, true);
                note.cursor =
                    note::grapheme_floor(&note.content, start + line.index_at_cell(x as usize));
                note.sel_end = Some(note.cursor);
            }
            EditFocus::Tags => unreachable!(),
        }
    }

    fn move_content_vertical(note: &mut Note, step: isize) {
        let (row, col) = note.cursor_pos();
        let target = row.saturating_add_signed(step);
        if target == row {
            return;
        }
        let text = note.content.split('\n').nth(row).unwrap_or("");
        let cell = crate::layout::cell_width(&text[..char_idx_to_byte(text, col)]);
        let Some(text) = note.content.split('\n').nth(target) else {
            note.cursor = char_len(&note.content);
            return;
        };
        let col = crate::layout::TextLine::source(text).index_at_cell(cell);
        note.cursor = note::grapheme_floor(&note.content, note.pos_to_cursor(target, col));
    }

    /// Cycle the selected note through the colour palette.
    pub fn cycle_color(&mut self) {
        let before = self.begin_history();
        self.cycle_color_inner();
        self.finish_history(before, None);
    }

    fn cycle_color_inner(&mut self) {
        let Some(selected) = self.selected else {
            return;
        };
        let note = &mut self.notes[selected];
        let next = cycle_str(&NOTE_COLORS, &note.color);
        if note.color != next {
            note.color = next.to_string();
            self.mark_dirty();
        }
    }

    /// Cycle the selected note through the available border styles.
    pub fn cycle_border(&mut self) {
        let before = self.begin_history();
        self.cycle_border_inner();
        self.finish_history(before, None);
    }

    fn cycle_border_inner(&mut self) {
        let Some(selected) = self.selected else {
            return;
        };
        let note = &mut self.notes[selected];
        let next = cycle_str(&BORDER_STYLES, &note.border_style);
        if note.border_style != next {
            note.border_style = next.to_string();
            self.mark_dirty();
        }
    }

    /// Select the previous visible note, wrapping around.
    pub fn select_prev(&mut self) {
        self.select_relative(-1);
    }

    /// Select the next visible note, wrapping around.
    pub fn select_next(&mut self) {
        self.select_relative(1);
    }

    fn select_relative(&mut self, step: isize) {
        let visible = self.visible_note_indices();
        if let Some(position) = self
            .selected
            .and_then(|selected| visible.iter().position(|&index| index == selected))
        {
            let next = (position as isize + step).rem_euclid(visible.len() as isize) as usize;
            self.select_note(Some(visible[next]));
        }
    }

    // ── Tags ─────────────────────────────────────────────────────────────────

    /// Exact matches lead the popup; Tab and rendering use the same completion.
    pub fn tag_suggestions(&self) -> Vec<&str> {
        let Some(selected) = self.selected else {
            return Vec::new();
        };
        let input = Note::normalize_tag(&self.notes[selected].tag_input);
        if input.is_empty() {
            return Vec::new();
        }
        self.all_tags
            .iter()
            .filter(|tag| **tag == input)
            .chain(
                self.all_tags
                    .iter()
                    .filter(|tag| **tag != input && tag.contains(&input)),
            )
            .take(5)
            .map(String::as_str)
            .collect()
    }

    /// Rebuild the global tag registry from all notes (deduplicated, sorted).
    pub fn refresh_all_tags(&mut self) {
        let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for note in &self.notes {
            for tag in &note.tags {
                set.insert(tag.to_lowercase());
            }
        }
        self.all_tags = set.into_iter().collect();
        self.reconcile_selection();
    }

    fn snapshot(&self) -> BoardSnapshot {
        BoardSnapshot {
            notes: self.notes.clone(),
            theme_idx: self.theme_idx,
            selected: self.selected,
            filter_tag: self.filter_tag.clone(),
            search_query: self.search_query.clone(),
            edit_focus: self.edit_focus,
            show_overlay: self.show_overlay,
        }
    }

    // Keyboard/mouse/paste entrypoints and direct note operations share this
    // narrow capture boundary; nested owners cannot record the same edit twice.
    fn begin_history(&mut self) -> Option<BoardSnapshot> {
        if self.recording_history {
            return None;
        }
        self.recording_history = true;
        Some(self.snapshot())
    }

    fn finish_history(
        &mut self,
        before: Option<BoardSnapshot>,
        typing: Option<(usize, EditFocus)>,
    ) {
        let Some(before) = before else {
            return;
        };
        if !self.recording_history {
            return; // Undo/redo already consumed the boundary, not a new action.
        }
        self.recording_history = false;
        if SaveData::from_notes(&before.notes, before.theme_idx) == self.to_save_data() {
            self.typing_group = None;
            return;
        }
        let now = Instant::now();
        let grouped = !self.undo.is_empty()
            && typing.is_some_and(|(index, focus)| {
                self.typing_group
                    .is_some_and(|(previous, previous_focus, time)| {
                        previous == index
                            && previous_focus == focus
                            && now.saturating_duration_since(time) <= Duration::from_millis(750)
                    })
            });
        if !grouped {
            self.undo.push(before);
        }
        self.redo.clear();
        self.typing_group = typing.map(|(index, focus)| (index, focus, now));
        self.trim_history();
        self.reconcile_selection();
    }

    fn trim_history(&mut self) {
        while self.undo.len() + self.redo.len() > HISTORY_LIMIT
            || self
                .undo
                .iter()
                .chain(&self.redo)
                .map(BoardSnapshot::bytes)
                .sum::<usize>()
                > HISTORY_BYTES
        {
            if !self.undo.is_empty() {
                self.undo.remove(0);
            } else if !self.redo.is_empty() {
                self.redo.remove(0);
            } else {
                break;
            }
        }
    }

    fn restore_history(&mut self, redo: bool) {
        self.recording_history = false;
        self.typing_group = None;
        let target = if redo {
            self.redo.pop()
        } else {
            self.undo.pop()
        };
        let Some(target) = target else {
            return;
        };
        let current = self.snapshot();
        if redo {
            self.undo.push(current);
        } else {
            self.redo.push(current);
        }
        self.notes = target.notes;
        self.theme_idx = target.theme_idx;
        self.selected = target.selected;
        self.filter_tag = target.filter_tag;
        self.search_query = target.search_query;
        self.edit_focus = target.edit_focus;
        self.show_overlay = target.show_overlay;
        self.mode = InputMode::Normal;
        self.input.clear();
        self.menu.visible = false;
        self.confirm_delete = false;
        self.confirm_clear_tags = false;
        self.mouse_dragging = false;
        self.last_click_btn = None;
        self.refresh_all_tags();
        self.mark_dirty();
        self.trim_history();
    }

    // ── Persistence helpers ──────────────────────────────────────────────────

    /// Mark the board as changed so it will be saved on the next debounce cycle.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
        self.last_edit = Instant::now();
    }

    /// Force an immediate save to disk if the board is dirty.
    pub fn flush_save(&mut self) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }
        self.last_save_attempt = Some(Instant::now());
        let data = self.to_save_data();
        match self.board.save(&data) {
            Ok(()) => {
                self.dirty = false;
                self.save_error.clear();
                Ok(())
            }
            Err(e) => {
                self.save_error = e.clone();
                Err(e)
            }
        }
    }

    /// Debounce persisted edits; retry failed saves at most once per second.
    pub fn autosave_due(&self, now: Instant) -> bool {
        self.quit_state == QuitState::Running
            && self.dirty
            && now.saturating_duration_since(self.last_edit) >= Duration::from_millis(200)
            && (self.save_error.is_empty()
                || self.last_save_attempt.is_none_or(|attempt| {
                    now.saturating_duration_since(attempt) >= Duration::from_secs(1)
                }))
    }

    fn handle_help_key(&mut self, key: &str) {
        let page = crate::ui::help_page_height(self.height);
        let max = crate::ui::help_max_scroll(self.height);
        self.help_scroll = match key {
            "esc" | "?" | "q" => {
                self.show_help = false;
                return;
            }
            "up" | "k" => self.help_scroll.saturating_sub(1),
            "down" | "j" => self.help_scroll.saturating_add(1),
            "pageup" => self.help_scroll.saturating_sub(page),
            "pagedown" => self.help_scroll.saturating_add(page),
            "home" => 0,
            "end" => max,
            _ => self.help_scroll,
        }
        .min(max);
    }

    fn request_quit(&mut self) {
        self.quit_state = match self.flush_save() {
            Ok(()) => QuitState::Done,
            Err(_) => QuitState::SaveFailed,
        };
    }

    fn handle_quit_key(&mut self, key: &str) {
        match self.quit_state {
            QuitState::SaveFailed => match key {
                "r" | "enter" => self.request_quit(),
                "esc" => self.quit_state = QuitState::Running,
                "d" => self.quit_state = QuitState::ConfirmDiscard,
                _ => {}
            },
            QuitState::ConfirmDiscard => match key {
                "y" | "Y" => self.quit_state = QuitState::Done,
                "n" | "N" | "esc" => self.quit_state = QuitState::SaveFailed,
                _ => {}
            },
            QuitState::Running | QuitState::Done => {}
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
        if self.mode == InputMode::SearchInput {
            return format!(" search: {} ({} results)", self.input, self.visible_count());
        }
        if self.mode == InputMode::FilterInput {
            return format!(" filter by tag: {}", self.input);
        }

        if let InputMode::ExportInput { visible, format } = self.mode {
            return format!(
                " Export {} [{}]: {}",
                if visible { "visible" } else { "selected" },
                format.map(ExportFormat::name).unwrap_or("by extension"),
                self.input
            );
        }
        if self.mode == InputMode::BoardInput {
            return format!(" Open existing board: {}", self.input);
        }

        let mut parts: Vec<String> = Vec::new();
        parts.push(format!("● {}", note::THEMES[self.theme_idx].name));
        parts.push(if self.dirty { "UNSAVED" } else { "SAVED" }.into());
        if !self.clipboard_error.is_empty() {
            parts.push(format!("⚠ {}", self.clipboard_error));
        }
        if !self.save_error.is_empty() {
            parts.push(format!("⚠ SAVE ERROR (^S:retry): {}", self.save_error));
        }

        parts.push(format!(
            "board: {} ({})",
            self.board_path()
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            self.board_path().display()
        ));
        if !self.file_message.is_empty() {
            parts.push(self.file_message.clone());
        }

        if let Some(selected) = self.selected {
            let n = &self.notes[selected];
            let cname = color_name(&n.color);
            parts.push(format!("{}/{}", selected + 1, self.count()));
            parts.push(cname.to_string());
            parts.push(n.border_style.clone());
            if !n.tags.is_empty() {
                parts.push(n.tags.join(","));
            }
            if n.editing {
                parts.push("EDITING".into());
            }
        } else {
            parts.push(if self.count() == 0 {
                "0 notes".into()
            } else {
                "0 matches".into()
            });
        }

        if !self.filter_tag.is_empty() {
            let visible = self.visible_count();
            parts.push(format!("◇ {} ({})", self.filter_tag, visible));
        }

        if !self.search_query.is_empty() {
            parts.push(format!(
                "search: {} ({})",
                self.search_query,
                self.visible_count()
            ));
        }
        parts.join(" │ ")
    }

    /// Context-sensitive hint text for the bottom hint bar.
    pub fn hint_bar(&self) -> &'static str {
        if matches!(self.mode, InputMode::ExportInput { .. }) {
            return " Enter:export (no overwrite)  Tab:auto/md/txt  Esc:cancel";
        }
        if self.mode == InputMode::BoardInput {
            return " Enter:save and switch  Esc:cancel  Existing files only";
        }
        if self.show_help {
            return " ↑/↓:scroll  PgUp/PgDn:page  Home/End:jump  Esc/?/q:close help";
        }
        if self.confirm_delete {
            return " Delete note?  y:yes  n:no  Esc:cancel";
        }
        if self.confirm_clear_tags {
            return " Clear ALL tags?  y:yes  Esc:cancel";
        }
        if self.mode == InputMode::TagInput {
            return " Enter:confirm  Esc:cancel  Backspace:delete";
        }
        if self.mode == InputMode::SearchInput {
            return " ↑/↓ or Tab:result  Enter:jump  Esc:clear search  Backspace:delete";
        }
        if self.mode == InputMode::FilterInput {
            return " Enter:apply filter  Esc:cancel  Backspace:delete";
        }
        if self
            .selected
            .is_some_and(|selected| self.notes[selected].editing)
        {
            return match self.edit_focus {
                EditFocus::Header => {
                    " Esc:stop  Tab:content  ^Z:undo  ^Y:redo  Enter:content  ^c:copy  ^x:cut  ^v:paste  ^a:all  Backspace:del"
                }
                EditFocus::Tags => {
                    " Esc:stop  Tab:complete/next  ^Z:undo  ^Y:redo  ^v:paste  Enter:add  ←/→:select tag  Backspace:del  [type tag name]"
                }
                EditFocus::Content => {
                    " Esc:stop  Tab:tags  ^Z:undo  ^Y:redo  PgUp/PgDn:page  Enter:newline  ^c:copy  ^x:cut  ^v:paste  ^a:all  ←/→/↑/↓:move"
                }
            };
        }
        if self.selected.is_none() && (!self.filter_tag.is_empty() || !self.search_query.is_empty())
        {
            return " Esc:clear filters  /:tag  s:search  ^Z:undo  ^Y:redo  n:new  ?:help  q:quit";
        }
        " E:export  ^E:visible export  B:board  n:new  d:del  e:edit  c:color  b:border  ←/→:navigate  T:filter  /:tag  s:search  ^Z:undo  ^Y:redo  O:overlay  ?:help  ^R:theme  q:quit"
    }
}

/// Horizontal commands return character indices on extended-grapheme boundaries.
fn horizontal_target(text: &str, cursor: usize, key: &str) -> usize {
    match (key.ends_with("right"), key.starts_with('^')) {
        (true, true) => note::next_word(text, cursor),
        (false, true) => note::previous_word(text, cursor),
        (true, false) => note::next_grapheme(text, cursor),
        (false, false) => note::previous_grapheme(text, cursor),
    }
}

/// Command names and modified-key strings are never fallback text input.
fn input_character(key: &str) -> Option<char> {
    let mut chars = key.chars();
    let c = chars.next()?;
    (chars.next().is_none() && !c.is_control()).then_some(c)
}

// ── Clipboard helpers ─────────────────────────────────────────────────────────

/// Retain the owner while the app runs (important on X11/Wayland).
fn clipboard(owner: &mut Option<arboard::Clipboard>) -> Result<&mut arboard::Clipboard, String> {
    if owner.is_none() {
        *owner = Some(arboard::Clipboard::new().map_err(|e| format!("clipboard: {e}"))?);
    }
    Ok(owner.as_mut().expect("clipboard initialized above"))
}

fn copy_to_clipboard(owner: &mut Option<arboard::Clipboard>, text: &str) -> Result<(), String> {
    clipboard(owner)?
        .set_text(text)
        .map_err(|e| format!("clipboard: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_board_switches_retain_work_history_current_lock_and_release_destination_lock() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("current.json");
        let target = dir.path().join("target.json");
        let mut app = app_at(current.clone());
        app.add_note();
        app.notes[0].content = "pending é🙂".into();
        let pending = app.to_save_data();
        let history = app.undo.len();
        assert!(app.switch_board(&target).is_err());
        assert!(!target.exists());
        std::fs::write(&target, b"broken").unwrap();
        assert!(app.switch_board(&target).is_err());
        let (mut target_store, _) = BoardStore::open(dir.path().join("valid.json")).unwrap();
        target_store
            .save(&SaveData::from_notes(
                &[Note {
                    title: "target".into(),
                    ..Note::new()
                }],
                0,
            ))
            .unwrap();
        let valid = target_store.path().to_path_buf();
        assert!(app.switch_board(&valid).is_err()); // Target writer still owns it.
        drop(target_store);
        std::fs::write(&current, b"external").unwrap();
        assert!(
            app.switch_board(&valid)
                .unwrap_err()
                .contains("changed outside")
        );
        assert!(app.dirty);
        assert_eq!(app.to_save_data(), pending);
        assert_eq!(app.undo.len(), history);
        assert_eq!(app.board_path(), std::fs::canonicalize(&current).unwrap());
        assert!(BoardStore::open(current).err().unwrap().contains("lock"));
        assert!(BoardStore::open(valid).is_ok());
    }

    #[test]
    fn board_prompt_saves_switches_and_resets_history_filters_and_transients() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("current.json");
        let target = dir.path().join("target.json");
        let (mut store, _) = BoardStore::open(target.clone()).unwrap();
        store
            .save(&SaveData::from_notes(
                &[Note {
                    title: "next".into(),
                    ..Note::new()
                }],
                1,
            ))
            .unwrap();
        drop(store);
        let mut app = app_at(current.clone());
        app.prepare_frame(80, 24);
        app.add_note();
        app.notes[0].content = "saved on switch".into();
        app.filter_tag = "old".into();
        app.search_query = "old query".into();
        key(&mut app, KeyCode::Char('B'), KeyModifiers::NONE);
        app.insert_paste(target.to_str().unwrap());
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.board_path(), std::fs::canonicalize(&target).unwrap());
        assert_eq!(app.notes[0].title, "next");
        assert_eq!(app.theme_idx, 1);
        assert!(!app.dirty);
        assert!(app.undo.is_empty() && app.redo.is_empty());
        assert!(app.filter_tag.is_empty() && app.search_query.is_empty());
        assert_eq!(app.mode, InputMode::Normal);
        assert!(!app.show_overlay && !app.notes[0].editing);
        assert_eq!((app.width, app.height), (80, 24));
        assert!(app.status_text().contains(target.to_str().unwrap()));
        assert!(BoardStore::open(target).is_err());
        let (_, data) = BoardStore::open(current).unwrap();
        assert_eq!(data.notes[0].content, "saved on switch");
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(app.notes[0].title, "next"); // Old board cannot be undone here.
    }

    #[test]
    fn selected_and_visible_exports_follow_one_visibility_rule_without_mutating_session() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = filtered_app(dir.path().join("board.json"));
        for (index, note) in app.notes.iter_mut().enumerate() {
            note.content = format!("body {index} é🙂");
        }
        app.set_filter("work".into());
        let visible = app.visible_note_indices();
        assert!(!visible.is_empty());
        let before = app.to_save_data();
        let dirty = app.dirty;
        let history = app.undo.len();
        let selected = app.selected.unwrap();
        let md = dir.path().join("selected.md");
        key(&mut app, KeyCode::Char('E'), KeyModifiers::NONE);
        app.insert_paste(md.to_str().unwrap());
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let text = std::fs::read_to_string(&md).unwrap();
        assert!(text.contains(&app.notes[selected].content));
        assert_eq!(text.matches("# ").count(), 1);
        let txt = dir.path().join("visible.custom");
        key(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL);
        app.insert_paste(txt.to_str().unwrap());
        key(&mut app, KeyCode::Tab, KeyModifiers::NONE); // Explicit Markdown.
        key(&mut app, KeyCode::Tab, KeyModifiers::NONE); // Explicit text.
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let text = std::fs::read_to_string(&txt).unwrap();
        for &index in &visible {
            assert!(text.contains(&app.notes[index].content));
        }
        for index in 0..app.count() {
            if !visible.contains(&index) {
                assert!(!text.contains(&app.notes[index].content));
            }
        }
        assert!(
            text.find(&app.notes[visible[0]].content).unwrap()
                < text.find(&app.notes[visible[1]].content).unwrap()
        );
        app.set_search("body 2".into());
        let search_export = dir.path().join("search.md");
        key(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL);
        app.insert_paste(search_export.to_str().unwrap());
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let search_text = std::fs::read_to_string(search_export).unwrap();
        assert!(search_text.contains("body 2"));
        assert!(!search_text.contains("body 0") && !search_text.contains("body 1"));
        key(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL);
        app.insert_paste(md.to_str().unwrap());
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(app.mode, InputMode::ExportInput { .. }));
        assert!(app.file_message.contains("choose a new path"));
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(app.input, md.to_str().unwrap());
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.to_save_data(), before);
        assert_eq!(app.dirty, dirty);
        assert_eq!(app.undo.len(), history);
    }

    #[test]
    fn context_menu_and_theme_history_are_separate_and_modal_keys_cannot_restore() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_at(dir.path().join("structural-history.json"));
        app.add_note();
        app.flush_save().unwrap();
        let original = app.to_save_data();
        app.show_context_menu(1, 1, true);
        app.menu.selected = 1; // Color.
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.undo.len(), 2);
        app.cycle_border();
        key(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL);
        assert_eq!(app.undo.len(), 4);
        key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(app.undo.len(), 4);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(app.undo.len(), 4);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        app.quit_state = QuitState::SaveFailed;
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(app.undo.len(), 4);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        for _ in 0..3 {
            key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        }
        assert_eq!(app.to_save_data(), original);
        // Context-menu deletion does not need a keyboard confirmation, but can recover.
        app.show_context_menu(1, 1, true);
        app.menu.selected = 4;
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.count(), 0);
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(app.to_save_data(), original);
        assert_eq!(app.selected, Some(0));
        assert!(!app.menu.visible);
    }

    #[test]
    fn deletion_history_restores_filtered_selection_and_saves_undo_redo() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("delete-history.json");
        let mut app = filtered_app(path.clone());
        apply_filter(&mut app, "WORK");
        app.select_note(Some(2));
        app.flush_save().unwrap();
        // Immediate mouse deletion records the same single structural action.
        app.prepare_frame(80, 24);
        let tab = crate::layout::tabs(&app, 80)
            .tabs
            .into_iter()
            .find(|tab| tab.index == 2)
            .unwrap();
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Middle),
            column: tab.rect.x,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.count(), 2);
        assert_eq!(app.undo.len(), 1);
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(app.count(), 3);
        assert_eq!(app.selected, Some(2));
        assert_eq!(app.filter_tag, "work");
        assert!(app.dirty);
        assert!(!app.confirm_delete && !app.menu.visible);
        app.flush_save().unwrap();
        let saved: SaveData = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved.notes.len(), 3);
        key(&mut app, KeyCode::Char('y'), KeyModifiers::CONTROL);
        assert_eq!(app.count(), 2);
        assert_eq!(app.selected, Some(0));
        app.flush_save().unwrap();
        let saved: SaveData = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(saved.notes.len(), 2);
        assert!(app.redo.is_empty());
    }

    #[test]
    fn replacement_history_restores_unicode_selection_and_inline_or_overlay_focus() {
        let dir = tempfile::tempdir().unwrap();
        for focus in [EditFocus::Header, EditFocus::Content] {
            for overlay in [false, true] {
                let mut app = editing_app(
                    dir.path().join(format!("{focus:?}-{overlay}.json")),
                    focus,
                    "aé🙂中",
                );
                app.show_overlay = overlay;
                app.undo.clear();
                key(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
                key(&mut app, KeyCode::Char('X'), KeyModifiers::SHIFT);
                assert_eq!(app.undo.len(), 1);
                key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
                let note = &app.notes[0];
                assert_eq!(note.title, "aé🙂中");
                assert_eq!(note.content, "aé🙂中");
                assert!(note.editing && app.dirty);
                assert_eq!(app.edit_focus, focus);
                assert_eq!(app.show_overlay, overlay);
                assert_eq!(
                    if focus == EditFocus::Header {
                        note.title_selection_range()
                    } else {
                        note.content_selection_range()
                    },
                    Some((0, 4))
                );
                key(&mut app, KeyCode::Char('y'), KeyModifiers::CONTROL);
                let note = &app.notes[0];
                assert_eq!(
                    if focus == EditFocus::Header {
                        &note.title
                    } else {
                        &note.content
                    },
                    "X"
                );
                assert!(!note.has_title_selection() && !note.has_content_selection());
            }
        }
    }

    #[test]
    fn key_releases_do_not_split_typing_history() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = editing_app(dir.path().join("release.json"), EditFocus::Content, "");
        app.undo.clear();
        for c in ['a', 'b'] {
            key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
            app.handle_key(KeyEvent::new_with_kind(
                KeyCode::Char(c),
                KeyModifiers::NONE,
                KeyEventKind::Release,
            ));
        }
        app.handle_key(KeyEvent::new_with_kind(
            KeyCode::Char('b'),
            KeyModifiers::NONE,
            KeyEventKind::Repeat,
        ));
        assert_eq!(app.notes[0].content, "abb");
        assert_eq!(app.undo.len(), 1);
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert!(app.notes[0].content.is_empty());
    }

    #[test]
    fn switching_to_a_backup_never_flushes_or_opens_it_as_a_board() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("board.json");
        let backup = dir.path().join("board.json.bak");
        let mut app = app_at(path.clone());
        app.add_note();
        app.notes[0].content = "S0".into();
        app.flush_save().unwrap();
        app.notes[0].content = "S1".into();
        app.mark_dirty();
        app.flush_save().unwrap();
        let original = std::fs::read(&path).unwrap();
        let recovery = std::fs::read(&backup).unwrap();
        app.notes[0].content = "S2".into();
        app.mark_dirty();
        assert!(app.switch_board(&backup).is_err());
        assert_eq!(app.notes[0].content, "S2");
        assert!(app.dirty);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(std::fs::read(&backup).unwrap(), recovery);
        assert!(!dir.path().join("board.json.bak.lock").exists());
        assert!(
            BoardStore::open(path).is_err(),
            "original lock remains held"
        );
    }

    #[test]
    fn typing_groups_by_field_time_and_navigation_but_paste_is_one_separate_action() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = editing_app(dir.path().join("groups.json"), EditFocus::Content, "");
        app.undo.clear();
        for c in "abc".chars() {
            key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }
        assert_eq!(app.undo.len(), 1);
        app.insert_paste("é🙂\n中");
        assert_eq!(app.undo.len(), 2);
        for c in "xy".chars() {
            key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }
        assert_eq!(app.undo.len(), 3);
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(app.notes[0].content, "abcé🙂\n中");
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(app.notes[0].content, "abc");
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert!(app.notes[0].content.is_empty());
        for _ in 0..3 {
            key(&mut app, KeyCode::Char('y'), KeyModifiers::CONTROL);
        }
        assert_eq!(app.notes[0].content, "abcé🙂\n中xy");
        key(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
        let (index, focus, time) = app.typing_group.unwrap();
        app.typing_group = Some((index, focus, time - Duration::from_secs(1)));
        key(&mut app, KeyCode::Char('b'), KeyModifiers::NONE);
        assert_eq!(app.undo.len(), 5);
        key(&mut app, KeyCode::Left, KeyModifiers::NONE);
        key(&mut app, KeyCode::Char('c'), KeyModifiers::NONE);
        assert_eq!(app.undo.len(), 6);
        key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
        assert_eq!(app.undo.len(), 7);
    }

    #[test]
    fn new_persisted_edits_invalidate_redo_while_transient_and_failed_cut_preserve_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = editing_app(dir.path().join("redo.json"), EditFocus::Content, "abc");
        app.undo.clear();
        app.insert_paste("d");
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(app.redo.len(), 1);
        key(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
        app.cut_selection_with(|_| Err("unavailable".into()));
        assert_eq!(app.redo.len(), 1);
        app.prepare_frame(40, 12);
        app.flush_save().unwrap();
        assert_eq!(app.redo.len(), 1);
        app.cut_selection_with(|text| {
            assert_eq!(text, "abc");
            Ok(())
        });
        assert!(app.redo.is_empty());
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(app.notes[0].content, "abc");
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        app.cycle_color();
        assert!(app.redo.is_empty());
    }

    #[test]
    fn history_bounds_both_stacks_by_count_and_estimated_allocations() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_at(dir.path().join("bounded.json"));
        app.add_note();
        for _ in 0..HISTORY_LIMIT + 5 {
            app.cycle_color();
        }
        assert_eq!(app.undo.len(), HISTORY_LIMIT);
        for _ in 0..HISTORY_LIMIT + 2 {
            key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        }
        assert_eq!(app.count(), 1);
        assert_eq!(app.redo.len(), HISTORY_LIMIT);
        app.start_editing(EditFocus::Content);
        app.insert_paste(&"x".repeat(HISTORY_BYTES + 1));
        key(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
        assert!(app.undo.is_empty() && app.redo.is_empty());
    }

    #[test]
    fn search_membership_and_tag_clear_history_restore_consistent_editing_and_filters() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = filtered_app(dir.path().join("reconcile-history.json"));
        app.notes[0].content = "needle".into();
        app.set_filter("work".into());
        app.set_search("needle".into());
        app.start_editing(EditFocus::Content);
        key(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
        app.insert_paste("lost");
        assert_eq!(app.selected, None);
        assert!(!app.notes[0].editing && !app.show_overlay);
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(app.selected, Some(0));
        assert!(app.notes[0].editing);
        assert_eq!(app.search_query, "needle");
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        key(&mut app, KeyCode::Char('t'), KeyModifiers::CONTROL);
        key(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
        assert_eq!(app.selected, None);
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(app.selected, Some(0));
        assert_eq!(app.notes[0].tags, ["work"]);
        key(&mut app, KeyCode::Char('y'), KeyModifiers::CONTROL);
        assert!(app.notes[0].tags.is_empty() && app.selected.is_none());
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        app.duplicate_selected();
        assert_eq!(app.visible_note_indices(), [0, 1]);
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(app.count(), 3);
        key(&mut app, KeyCode::Char('['), KeyModifiers::NONE);
        assert_eq!(app.selected, Some(1));
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(app.notes[0].title, "A");
        assert_eq!(app.selected, Some(0));
    }

    #[test]
    fn phrase_search_combines_with_exact_tag_and_jump_clear_restore_navigation() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = filtered_app(dir.path().join("search.json"));
        app.notes[0].content = "A Phrase in Body, not the title".into();
        app.notes[1].content = "phrase in body".into();
        app.notes[2].content = "Second PHRASE IN BODY".into();
        apply_filter(&mut app, "work");
        app.flush_save().unwrap();
        key(&mut app, KeyCode::Char('s'), KeyModifiers::NONE);
        for c in "pHrAsE iN bOdY".chars() {
            key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }
        assert_eq!(app.visible_note_indices(), [0, 2]);
        key(&mut app, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(app.selected, Some(2));
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.mode, InputMode::Normal);
        assert_eq!(app.notes[app.selected.unwrap()].title, "C");
        assert!(!app.dirty && app.undo.is_empty());
        key(&mut app, KeyCode::Char('s'), KeyModifiers::NONE);
        assert_eq!(app.input, "pHrAsE iN bOdY");
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.search_query.is_empty());
        assert_eq!(app.filter_tag, "work");
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.visible_note_indices(), [0, 1, 2]);
        app.select_next();
        assert_eq!(app.selected, Some(0));
        app.set_search("ÅÄÖ".into());
        assert_eq!(app.selected, None);
        app.notes[1].title = "åäö".into();
        app.reconcile_selection();
        assert_eq!(app.selected, Some(1));
        app.set_search("oThEr".into());
        assert_eq!(app.visible_note_indices(), [1]);
        app.set_filter("work".into());
        assert_eq!(app.selected, None);
        app.add_note();
        assert!(app.search_query.is_empty() && app.filter_tag.is_empty());
        assert_eq!(app.selected, Some(0));
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(app.count(), 3);
        assert_eq!(app.selected, None);
        assert_eq!(app.search_query, "oThEr");
        assert_eq!(app.filter_tag, "work");
    }

    #[test]
    fn clipboard_results_never_overwrite_save_errors() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_at(dir.path().join("errors.json"));
        app.add_note();
        app.start_editing(EditFocus::Content);
        app.insert_paste("abc");
        key(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
        app.save_error = "save failure".into();
        app.copy_selection_with(|_| Err("clipboard failure".into()));
        assert_eq!(app.save_error, "save failure");
        assert_eq!(app.clipboard_error, "clipboard failure");
        app.copy_selection_with(|_| Ok(()));
        assert_eq!(app.save_error, "save failure");
        assert!(app.clipboard_error.is_empty());
    }

    #[test]
    fn autosave_debounces_persisted_edits_and_bounds_failure_retries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("debounce.json");
        let mut app = app_at(path.clone());
        app.add_note();
        let first = app.last_edit;
        assert!(!app.autosave_due(first + Duration::from_millis(199)));
        assert!(app.autosave_due(first + Duration::from_millis(200)));
        app.start_editing(EditFocus::Content);
        key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(app.last_edit >= first);
        let edited = app.last_edit;
        assert!(!app.autosave_due(edited + Duration::from_millis(199)));
        assert!(app.autosave_due(edited + Duration::from_millis(200)));
        std::fs::write(&path, b"external change").unwrap();
        assert!(app.flush_save().is_err());
        let attempt = app.last_save_attempt.unwrap();
        assert!(app.dirty);
        assert!(!app.autosave_due(attempt + Duration::from_millis(999)));
        assert!(app.autosave_due(attempt + Duration::from_secs(1)));
        std::fs::remove_file(&path).unwrap();
        app.clipboard_error = "clipboard unavailable".into();
        key(&mut app, KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert!(!app.dirty && app.save_error.is_empty());
        assert_eq!(app.clipboard_error, "clipboard unavailable");
        assert!(app.status_text().contains("SAVED"));
    }

    #[test]
    fn transient_interactions_and_identical_replacement_never_dirty_the_board() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_at(dir.path().join("transient.json"));
        app.add_note();
        app.notes[0].title = "title".into();
        app.notes[0].content = "中é".into();
        app.notes[0].tags = vec!["work".into()];
        app.refresh_all_tags();
        app.flush_save().unwrap();
        let edited = app.last_edit;
        let saved = serde_json::to_string(&app.to_save_data()).unwrap();
        for (code, modifiers) in [
            (KeyCode::Char('e'), KeyModifiers::NONE),
            (KeyCode::Char('a'), KeyModifiers::CONTROL),
            (KeyCode::Tab, KeyModifiers::NONE),
            (KeyCode::Char('a'), KeyModifiers::CONTROL),
        ] {
            key(&mut app, code, modifiers);
        }
        app.insert_paste("中é");
        key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        app.insert_paste("wo");
        key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.notes[0].tag_input, "work");
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE); // duplicate
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        app.show_context_menu(1, 1, true);
        app.menu.selected = 6; // Close
        app.activate_menu();
        assert!(!app.dirty);
        assert_eq!(app.last_edit, edited);
        assert_eq!(serde_json::to_string(&app.to_save_data()).unwrap(), saved);
        app.cycle_color();
        assert!(app.dirty);
        app.flush_save().unwrap();
        app.cycle_border();
        assert!(app.dirty);
    }

    #[test]
    fn completion_advances_after_partial_or_exact_match_and_enter_prevents_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_at(dir.path().join("completion.json"));
        app.add_note();
        app.notes[0].tags = vec!["afoo".into(), "foo".into(), "Åland".to_lowercase()];
        app.refresh_all_tags();
        app.flush_save().unwrap();
        app.start_editing(EditFocus::Tags);
        app.insert_paste("fo");
        let suggestion = app.tag_suggestions()[0].to_string();
        key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.notes[0].tag_input, suggestion);
        assert_eq!(app.edit_focus, EditFocus::Tags);
        key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.edit_focus, EditFocus::Header);
        app.edit_focus = EditFocus::Tags;
        app.notes[0].tag_input = " FOO ".into();
        assert_eq!(app.tag_suggestions()[0], "foo");
        key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.edit_focus, EditFocus::Header);
        app.edit_focus = EditFocus::Tags;
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.notes[0].tags.len(), 3);
        assert!(!app.dirty);
        app.insert_paste("new");
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.dirty && app.notes[0].has_tag("new"));
    }

    #[test]
    fn terminal_paste_is_data_only_and_uses_field_specific_sanitizing() {
        let dir = tempfile::tempdir().unwrap();
        for overlay in [false, true] {
            let mut app = app_at(dir.path().join(format!("paste-{overlay}.json")));
            app.insert_paste("qn\n\u{1b}\u{3}");
            assert_eq!(app.count(), 0);
            assert_eq!(app.quit_state, QuitState::Running);
            app.add_note();
            app.flush_save().unwrap();
            app.insert_paste("qn\n\u{1b}");
            assert!(!app.dirty && app.notes[0].content.is_empty());
            if overlay {
                app.toggle_overlay();
            } else {
                app.start_editing(EditFocus::Content);
            }
            app.insert_paste("qn\n中é\u{1b}\t\r\u{7f}");
            assert_eq!(app.notes[0].content, "qn\n中é");
            key(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
            app.insert_paste("Å\n🦀");
            assert_eq!(app.notes[0].content, "Å\n🦀");
            assert!(!app.notes[0].has_content_selection());
            assert_eq!(app.notes[0].cursor, 3);
            app.edit_focus = EditFocus::Header;
            app.insert_paste("title\n中\r\t\u{1b}");
            assert_eq!(app.notes[0].title, "title中");
            app.edit_focus = EditFocus::Tags;
            app.flush_save().unwrap();
            app.insert_paste("tag\nÅ\t\u{1b}");
            assert_eq!(app.notes[0].tag_input, "tagÅ");
            assert!(!app.dirty);
            let input = app.notes[0].tag_input.clone();
            app.quit_state = QuitState::SaveFailed;
            app.insert_paste("ignored");
            assert_eq!(app.notes[0].tag_input, input);
        }
    }

    #[test]
    fn help_navigation_is_modal_bounded_and_has_explicit_close_controls() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_at(dir.path().join("help.json"));
        app.prepare_frame(80, 24);
        key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);
        key(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
        assert!(app.show_help && app.notes.is_empty());
        key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
        assert_eq!(app.help_scroll, crate::ui::help_page_height(24));
        wheel(&mut app, MouseEventKind::ScrollUp);
        assert_eq!(app.help_scroll, crate::ui::help_page_height(24) - 1);
        key(&mut app, KeyCode::End, KeyModifiers::NONE);
        assert_eq!(app.help_scroll, crate::ui::help_max_scroll(24));
        key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
        assert_eq!(app.help_scroll, crate::ui::help_max_scroll(24));
        app.prepare_frame(80, 80);
        assert_eq!(app.help_scroll, 0);
        key(&mut app, KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(!app.show_help && app.quit_state == QuitState::Running && !app.dirty);
    }

    fn app_at(path: std::path::PathBuf) -> App {
        let (board, data) = BoardStore::open(path).unwrap();
        App::new(data, board)
    }

    fn key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        app.handle_key(KeyEvent::new(code, modifiers));
    }

    fn wheel(app: &mut App, kind: MouseEventKind) {
        app.handle_mouse(MouseEvent {
            kind,
            column: 12,
            row: app.height - 2,
            modifiers: KeyModifiers::NONE,
        });
    }

    #[test]
    fn wheel_navigation_stays_filtered_and_never_cycles_footer_theme_or_color() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_at(dir.path().join("wheel.json"));
        app.width = 80;
        app.height = 24;
        app.notes = vec![
            Note {
                tags: vec!["work".into()],
                ..Note::new()
            },
            Note::new(),
            Note {
                tags: vec!["work".into()],
                ..Note::new()
            },
        ];
        app.filter_tag = "work".into();
        app.reconcile_selection();
        let color = app.notes[0].color.clone();
        let theme = app.theme_idx;
        wheel(&mut app, MouseEventKind::ScrollDown);
        assert_eq!(app.selected, Some(2));
        wheel(&mut app, MouseEventKind::ScrollUp);
        assert_eq!(app.selected, Some(0));
        wheel(&mut app, MouseEventKind::ScrollUp);
        assert_eq!(app.selected, Some(2));
        assert_eq!(app.theme_idx, theme);
        assert_eq!(app.notes[0].color, color);
        app.filter_tag = "missing".into();
        app.reconcile_selection();
        wheel(&mut app, MouseEventKind::ScrollDown);
        assert_eq!(app.selected, None);
    }

    #[test]
    fn page_keys_move_by_visible_height_keep_columns_and_extend_selection() {
        for overlay in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut app = app_at(dir.path().join("paging.json"));
            let mut note = Note {
                content: vec!["中abc"; 50].join("\n"),
                editing: true,
                ..Note::new()
            };
            note.cursor = note.pos_to_cursor(40, 1);
            app.notes = vec![note];
            app.selected = Some(0);
            app.edit_focus = EditFocus::Content;
            app.show_overlay = overlay;
            app.prepare_frame(80, 24);
            let page = app.note_layout().unwrap().body.text.height as usize;
            let top = app.notes[0].viewport.top_line;
            key(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
            app.prepare_frame(80, 24);
            assert_eq!(app.notes[0].cursor_pos(), (40 - page, 1));
            assert_eq!(app.notes[0].viewport.top_line, top.saturating_sub(page));
            let anchor = app.notes[0].cursor;
            key(&mut app, KeyCode::PageDown, KeyModifiers::SHIFT);
            app.prepare_frame(80, 24);
            assert_eq!(app.notes[0].cursor_pos(), (40, 1));
            assert_eq!(app.notes[0].viewport.top_line, top);
            assert_eq!(
                app.notes[0].content_selection_range(),
                Some((anchor, app.notes[0].cursor))
            );
            assert_eq!(app.notes[0].selected_text().chars().count(), page * 5);
            app.notes[0].cursor = app.notes[0].pos_to_cursor(48, 1);
            app.prepare_frame(80, 24);
            key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
            app.prepare_frame(80, 24);
            assert_eq!(app.notes[0].cursor_pos(), (49, 1));
            assert!(!app.notes[0].has_content_selection());
            assert!(!app.dirty);
            for _ in 0..50_usize.div_ceil(page) {
                key(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
                app.prepare_frame(80, 24);
            }
            assert_eq!(app.notes[0].cursor_pos().0, 0);
            assert_eq!(app.notes[0].viewport.top_line, 0);
        }
    }

    #[test]
    fn multiline_paste_replacement_and_new_editing_sessions_reset_or_follow_viewports() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_at(dir.path().join("viewport-paste.json"));
        app.notes = vec![Note {
            content: "中🦀".repeat(80),
            ..Note::new()
        }];
        app.selected = Some(0);
        app.toggle_overlay();
        app.prepare_frame(40, 12);
        assert!(app.notes[0].viewport.left_cell > 0);
        app.insert_paste(&format!("\n{}\nend", vec!["pasted"; 30].join("\n")));
        app.prepare_frame(40, 12);
        assert_eq!(app.notes[0].cursor_pos(), (31, 3));
        assert!(app.notes[0].viewport.top_line > 0);
        assert_eq!(app.notes[0].viewport.left_cell, 0);
        key(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
        app.insert_paste("small");
        app.prepare_frame(40, 12);
        assert_eq!(app.notes[0].content, "small");
        assert_eq!(app.notes[0].viewport.top_line, 0);
        assert_eq!(app.notes[0].viewport.left_cell, 0);
        app.stop_editing();
        app.notes[0].viewport.top_line = 7;
        app.notes[0].viewport.left_cell = 90;
        app.start_editing(EditFocus::Header);
        assert_eq!(app.notes[0].viewport.top_line, 0);
        assert_eq!(app.notes[0].viewport.left_cell, 0);
        app.notes[0].viewport.left_cell = 90;
        app.stop_editing();
        app.duplicate_selected();
        assert_eq!(app.notes[app.selected.unwrap()].viewport.left_cell, 0);
    }

    #[test]
    fn editing_wheel_and_vertical_keys_use_cells_without_changing_notes_or_hidden_fields() {
        let dir = tempfile::tempdir().unwrap();
        for overlay in [false, true] {
            let mut app = app_at(dir.path().join(format!("edit-wheel-{overlay}.json")));
            app.width = 80;
            app.height = 24;
            app.notes = vec![
                Note {
                    content: "中a\nxy".into(),
                    cursor: 1,
                    editing: true,
                    ..Note::new()
                },
                Note::new(),
            ];
            app.selected = Some(0);
            app.show_overlay = overlay;
            app.edit_focus = EditFocus::Content;
            wheel(&mut app, MouseEventKind::ScrollDown);
            assert_eq!(app.notes[0].cursor, 5);
            assert_eq!(app.selected, Some(0));
            wheel(&mut app, MouseEventKind::ScrollUp);
            assert_eq!(app.notes[0].cursor, 1);
            key(&mut app, KeyCode::Down, KeyModifiers::SHIFT);
            assert_eq!(app.notes[0].content_selection_range(), Some((1, 5)));
            key(&mut app, KeyCode::Up, KeyModifiers::NONE);
            assert_eq!(app.notes[0].cursor, 1);
            assert!(!app.notes[0].has_content_selection());
            for focus in [EditFocus::Header, EditFocus::Tags] {
                app.edit_focus = focus;
                wheel(&mut app, MouseEventKind::ScrollDown);
                assert_eq!(app.notes[0].cursor, 1);
                assert_eq!(app.edit_focus, focus);
                assert_eq!(app.selected, Some(0));
            }
        }
    }

    #[test]
    fn failed_cut_preserves_text_cursor_selection_and_dirty_state() {
        let dir = tempfile::tempdir().unwrap();
        for focus in [EditFocus::Header, EditFocus::Content] {
            for dirty in [false, true] {
                let mut app = app_at(dir.path().join(format!("{focus:?}-{dirty}.json")));
                let mut note = Note {
                    title: "hé🙂llo".into(),
                    content: "hé🙂llo".into(),
                    ..Note::new()
                };
                note.title_cursor = 3;
                note.cursor = 3;
                note.title_sel_start = Some(1);
                note.title_sel_end = Some(3);
                note.sel_start = Some(1);
                note.sel_end = Some(3);
                app.notes.push(note);
                app.reconcile_selection();
                app.edit_focus = focus;
                app.dirty = dirty;
                app.cut_selection_with(|text| {
                    assert_eq!(text, "é🙂");
                    Err("clipboard unavailable".into())
                });
                let note = &app.notes[0];
                assert_eq!(note.title, "hé🙂llo");
                assert_eq!(note.content, "hé🙂llo");
                assert_eq!((note.title_cursor, note.cursor), (3, 3));
                assert_eq!(
                    (note.title_sel_start, note.title_sel_end),
                    (Some(1), Some(3))
                );
                assert_eq!((note.sel_start, note.sel_end), (Some(1), Some(3)));
                assert_eq!(app.dirty, dirty);
                assert_eq!(app.clipboard_error, "clipboard unavailable");
            }
        }
    }

    #[test]
    fn successful_cut_copies_before_deleting_and_marks_dirty() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_at(dir.path().join("board.json"));
        app.notes.push(Note {
            content: "hé🙂llo".into(),
            cursor: 3,
            sel_start: Some(1),
            sel_end: Some(3),
            ..Note::new()
        });
        app.reconcile_selection();
        app.cut_selection_with(|text| {
            assert_eq!(text, "é🙂");
            Ok(())
        });
        assert_eq!(app.notes[0].content, "hllo");
        assert_eq!(app.notes[0].cursor, 1);
        assert!(!app.notes[0].has_content_selection());
        assert!(app.dirty);
    }

    #[test]
    fn failed_quit_keeps_changes_and_can_retry_or_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("board.json");
        let mut app = app_at(path.clone());
        app.add_note();
        app.flush_save().unwrap();
        let saved = std::fs::read(&path).unwrap();
        app.notes[0].content = "pending change".into();
        app.mark_dirty();
        std::fs::write(&path, b"external change").unwrap();
        key(&mut app, KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(app.quit_state, QuitState::SaveFailed);
        assert!(app.dirty);
        key(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
        assert_eq!(
            app.count(),
            1,
            "quit dialog must not dispatch normal commands"
        );
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.quit_state, QuitState::Running);
        assert_eq!(app.notes[0].content, "pending change");
        key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(app.quit_state, QuitState::SaveFailed);
        std::fs::write(&path, saved).unwrap();
        key(&mut app, KeyCode::Char('r'), KeyModifiers::NONE);
        assert_eq!(app.quit_state, QuitState::Done);
        assert!(!app.dirty);
        drop(app);
        let (_board, data) = BoardStore::open(path).unwrap();
        assert_eq!(data.notes[0].content, "pending change");
    }

    #[test]
    fn resized_menu_uses_the_drawn_rectangle_and_excludes_the_footer() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_at(dir.path().join("board.json"));
        app.notes.push(Note::new());
        app.reconcile_selection();
        app.width = 80;
        app.height = 24;
        app.show_context_menu(10, 14, true);
        app.height = 21;
        let rect = crate::ui::menu_rect(&app, ratatui::layout::Rect::new(0, 0, 80, 21));
        assert!(rect.bottom() <= 19);
        let initial_color = app.notes[0].color.clone();
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x + 2,
            row: rect.y + 2,
            modifiers: KeyModifiers::NONE,
        });
        assert_ne!(
            app.notes[0].color, initial_color,
            "displayed Color must dispatch Color"
        );
        assert!(!app.notes[0].editing);
        app.show_context_menu(10, 14, true);
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 12,
            row: 19,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.count(), 1, "footer click must not delete the note");
        assert!(!app.menu.visible);
    }

    #[test]
    fn discarding_failed_save_requires_a_second_explicit_confirmation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("board.json");
        let mut app = app_at(path.clone());
        app.add_note();
        app.toggle_overlay();
        std::fs::write(&path, b"external change").unwrap();
        key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(app.quit_state, QuitState::SaveFailed);
        key(&mut app, KeyCode::Char('d'), KeyModifiers::NONE);
        assert_eq!(app.quit_state, QuitState::ConfirmDiscard);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.quit_state, QuitState::SaveFailed);
        key(&mut app, KeyCode::Char('d'), KeyModifiers::NONE);
        key(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
        assert_eq!(app.quit_state, QuitState::Done);
        assert!(app.dirty);
        assert_eq!(std::fs::read(path).unwrap(), b"external change");
    }

    fn editing_app(path: std::path::PathBuf, focus: EditFocus, text: &str) -> App {
        let mut app = app_at(path);
        app.add_note();
        app.notes[0].title = text.into();
        app.notes[0].content = text.into();
        app.toggle_edit();
        app.edit_focus = focus;
        app.dirty = false;
        app
    }

    fn filtered_app(path: std::path::PathBuf) -> App {
        let mut app = app_at(path);
        app.notes = vec![
            Note {
                title: "A".into(),
                tags: vec!["work".into()],
                ..Note::new()
            },
            Note {
                title: "B".into(),
                tags: vec!["other".into()],
                ..Note::new()
            },
            Note {
                title: "C".into(),
                tags: vec!["work".into()],
                ..Note::new()
            },
        ];
        app.refresh_all_tags();
        app
    }

    fn apply_filter(app: &mut App, tag: &str) {
        key(app, KeyCode::Char('/'), KeyModifiers::NONE);
        for c in tag.chars() {
            key(app, KeyCode::Char(c), KeyModifiers::NONE);
        }
        key(app, KeyCode::Enter, KeyModifiers::NONE);
    }

    #[test]
    fn header_and_content_move_select_copy_and_delete_whole_graphemes() {
        let dir = tempfile::tempdir().unwrap();
        for focus in [EditFocus::Header, EditFocus::Content] {
            for (i, cluster) in ["e\u{301}", "👩🏽‍💻", "🇫🇮", "👍🏽", "中"].iter().enumerate()
            {
                let source = format!("{cluster}Z");
                let mut app = editing_app(
                    dir.path().join(format!("{focus:?}{i}.json")),
                    focus,
                    &source,
                );
                key(&mut app, KeyCode::Home, KeyModifiers::NONE);
                key(&mut app, KeyCode::Right, KeyModifiers::SHIFT);
                let note = &app.notes[0];
                let copied = if focus == EditFocus::Header {
                    note.selected_title()
                } else {
                    note.selected_text()
                };
                assert_eq!(copied, *cluster);
                app.handle_key_with_copy(
                    KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                    |_, text| {
                        assert_eq!(text, *cluster);
                        Ok(())
                    },
                );
                assert!(!app.dirty);
                app.cut_selection_with(|text| {
                    assert_eq!(text, *cluster);
                    Err("injected failure".into())
                });
                let note = &app.notes[0];
                assert_eq!(
                    if focus == EditFocus::Header {
                        note.selected_title()
                    } else {
                        note.selected_text()
                    },
                    *cluster
                );
                assert!(!app.dirty);
                key(&mut app, KeyCode::Left, KeyModifiers::NONE);
                key(&mut app, KeyCode::Delete, KeyModifiers::NONE);
                let text = if focus == EditFocus::Header {
                    &app.notes[0].title
                } else {
                    &app.notes[0].content
                };
                assert_eq!(text, "Z");
                key(&mut app, KeyCode::Home, KeyModifiers::NONE);
                app.insert_paste(cluster);
                key(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
                let text = if focus == EditFocus::Header {
                    &app.notes[0].title
                } else {
                    &app.notes[0].content
                };
                assert_eq!(text, "Z");
            }
        }
    }

    #[test]
    fn control_word_keys_share_boundaries_and_reject_unsupported_modifiers() {
        let dir = tempfile::tempdir().unwrap();
        let source = "café_2...  中华\n👩🏽‍💻!";
        for focus in [EditFocus::Header, EditFocus::Content] {
            let mut app = editing_app(
                dir.path().join(format!("word{focus:?}.json")),
                focus,
                source,
            );
            // Header Home starts at field start; body starts on the last logical row.
            app.notes[0].title_cursor = 0;
            app.notes[0].cursor = 0;
            for target in [6, 11, 14, 19] {
                key(&mut app, KeyCode::Right, KeyModifiers::CONTROL);
                assert_eq!(
                    if focus == EditFocus::Header {
                        app.notes[0].title_cursor
                    } else {
                        app.notes[0].cursor
                    },
                    target
                );
            }
            for target in [14, 11, 6, 0] {
                key(&mut app, KeyCode::Left, KeyModifiers::CONTROL);
                assert_eq!(
                    if focus == EditFocus::Header {
                        app.notes[0].title_cursor
                    } else {
                        app.notes[0].cursor
                    },
                    target
                );
            }
            key(
                &mut app,
                KeyCode::Right,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            );
            key(&mut app, KeyCode::Backspace, KeyModifiers::CONTROL);
            let text = if focus == EditFocus::Header {
                &app.notes[0].title
            } else {
                &app.notes[0].content
            };
            assert_eq!(text, "...  中华\n👩🏽‍💻!");
            key(&mut app, KeyCode::Delete, KeyModifiers::CONTROL);
            let text = if focus == EditFocus::Header {
                &app.notes[0].title
            } else {
                &app.notes[0].content
            };
            assert_eq!(text, "中华\n👩🏽‍💻!");
            key(&mut app, KeyCode::Right, KeyModifiers::CONTROL);
            key(&mut app, KeyCode::Backspace, KeyModifiers::CONTROL);
            let text = if focus == EditFocus::Header {
                &app.notes[0].title
            } else {
                &app.notes[0].content
            };
            assert_eq!(text, "👩🏽‍💻!"); // Backward word deletion consumes preceding whitespace.
            let before = app.to_save_data();
            for (code, modifiers) in [
                (KeyCode::Delete, KeyModifiers::CONTROL | KeyModifiers::ALT),
                (
                    KeyCode::Backspace,
                    KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                ),
                (KeyCode::Up, KeyModifiers::CONTROL),
                (KeyCode::Right, KeyModifiers::CONTROL | KeyModifiers::SUPER),
            ] {
                key(&mut app, code, modifiers);
            }
            assert_eq!(app.to_save_data(), before);
        }
    }

    #[test]
    fn single_line_drafts_backspace_graphemes_without_mutating_the_board() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = editing_app(dir.path().join("drafts.json"), EditFocus::Tags, "body");
        let before = app.to_save_data();
        app.notes[0].tag_input = "e\u{301}👩🏽‍💻".into();
        key(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(app.notes[0].tag_input, "e\u{301}");
        for mode in [
            InputMode::TagInput,
            InputMode::FilterInput,
            InputMode::SearchInput,
            InputMode::BoardInput,
            InputMode::ExportInput {
                visible: false,
                format: None,
            },
        ] {
            app.mode = mode;
            app.input = "Ae\u{301}🇫🇮".into();
            key(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
            assert_eq!(app.input, "Ae\u{301}");
            key(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
            assert_eq!(app.input, "A");
        }
        assert_eq!(app.to_save_data(), before);
        assert!(!app.dirty);
    }

    #[test]
    fn delete_removes_unicode_or_selection_instead_of_inserting_its_key_name() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = editing_app(dir.path().join("board.json"), EditFocus::Content, "aé🙂b");
        key(&mut app, KeyCode::Home, KeyModifiers::NONE);
        key(&mut app, KeyCode::Right, KeyModifiers::NONE);
        key(&mut app, KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(app.notes[0].content, "a🙂b");
        assert_eq!(app.notes[0].cursor, 1);
        key(&mut app, KeyCode::Right, KeyModifiers::SHIFT);
        key(&mut app, KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(app.notes[0].content, "ab");
        assert_eq!(app.notes[0].cursor, 1);
        key(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
        key(&mut app, KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(app.notes[0].content, "");
        assert_eq!(app.notes[0].cursor, 0);
        assert_eq!((app.notes[0].sel_start, app.notes[0].sel_end), (None, None));
        app.dirty = false;
        key(&mut app, KeyCode::Delete, KeyModifiers::NONE);
        assert!(!app.dirty);
    }

    #[test]
    fn unhandled_commands_never_become_edit_or_prompt_text() {
        let dir = tempfile::tempdir().unwrap();
        for focus in [EditFocus::Header, EditFocus::Content, EditFocus::Tags] {
            let mut app = editing_app(dir.path().join(format!("{focus:?}.json")), focus, "x");
            for (code, modifiers) in [
                (KeyCode::Char('w'), KeyModifiers::CONTROL),
                (KeyCode::Char('x'), KeyModifiers::ALT),
                (KeyCode::Home, KeyModifiers::CONTROL),
                (
                    KeyCode::Backspace,
                    KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                ),
                (KeyCode::Up, KeyModifiers::NONE),
                (KeyCode::F(1), KeyModifiers::NONE),
            ] {
                key(&mut app, code, modifiers);
            }
            assert_eq!(app.notes[0].title, "x");
            assert_eq!(app.notes[0].content, "x");
            assert_eq!(app.notes[0].tag_input, "");
            assert!(!app.dirty);
            key(&mut app, KeyCode::Char('é'), KeyModifiers::NONE);
            match focus {
                EditFocus::Header => assert_eq!(app.notes[0].title, "xé"),
                EditFocus::Content => assert_eq!(app.notes[0].content, "xé"),
                EditFocus::Tags => assert_eq!(app.notes[0].tag_input, "é"),
            }
        }
        for mode in [
            InputMode::TagInput,
            InputMode::FilterInput,
            InputMode::SearchInput,
        ] {
            let mut app = app_at(dir.path().join(format!("{mode:?}.json")));
            app.add_note();
            app.mode = mode;
            app.input = "tag".into();
            for (code, modifiers) in [
                (KeyCode::Left, KeyModifiers::NONE),
                (KeyCode::Delete, KeyModifiers::NONE),
                (KeyCode::Char('z'), KeyModifiers::CONTROL),
                (KeyCode::Backspace, KeyModifiers::CONTROL),
            ] {
                key(&mut app, code, modifiers);
            }
            assert_eq!(app.input, "tag");
            key(&mut app, KeyCode::Char('é'), KeyModifiers::NONE);
            assert_eq!(app.input, "tagé");
        }
    }

    #[test]
    fn typing_after_collapsed_selection_starts_the_next_selection_at_the_new_cursor() {
        let dir = tempfile::tempdir().unwrap();
        for focus in [EditFocus::Header, EditFocus::Content] {
            let mut app = editing_app(dir.path().join(format!("{focus:?}.json")), focus, "abc");
            for (code, modifiers) in [
                (KeyCode::Home, KeyModifiers::NONE),
                (KeyCode::Right, KeyModifiers::NONE),
                (KeyCode::Right, KeyModifiers::SHIFT),
                (KeyCode::Left, KeyModifiers::SHIFT),
                (KeyCode::Char('X'), KeyModifiers::NONE),
                (KeyCode::Right, KeyModifiers::SHIFT),
                (KeyCode::Backspace, KeyModifiers::NONE),
            ] {
                key(&mut app, code, modifiers);
            }
            let note = &app.notes[0];
            let (text, cursor) = match focus {
                EditFocus::Header => (&note.title, note.title_cursor),
                EditFocus::Content => (&note.content, note.cursor),
                EditFocus::Tags => unreachable!(),
            };
            assert_eq!(text, "aXc");
            assert_eq!(cursor, 2);
            assert!(!note.has_title_selection() && !note.has_content_selection());
        }
    }

    #[test]
    fn select_all_moves_the_active_cursor_and_unicode_paste_resets_the_anchor() {
        let dir = tempfile::tempdir().unwrap();
        for focus in [EditFocus::Header, EditFocus::Content] {
            let mut app = editing_app(dir.path().join(format!("{focus:?}.json")), focus, "hé🙂llo");
            key(&mut app, KeyCode::Home, KeyModifiers::NONE);
            key(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
            let cursor = if focus == EditFocus::Header {
                app.notes[0].title_cursor
            } else {
                app.notes[0].cursor
            };
            assert_eq!(cursor, 6);
            app.insert_paste("Å🐦");
            key(&mut app, KeyCode::Left, KeyModifiers::SHIFT);
            let note = &app.notes[0];
            let (text, selected) = if focus == EditFocus::Header {
                (&note.title, note.selected_title())
            } else {
                (&note.content, note.selected_text())
            };
            assert_eq!(text, "Å🐦");
            assert_eq!(selected, "🐦");
        }
    }

    #[test]
    fn newline_resets_a_collapsed_content_selection() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = editing_app(dir.path().join("board.json"), EditFocus::Content, "abc");
        for (code, modifiers) in [
            (KeyCode::Home, KeyModifiers::NONE),
            (KeyCode::Right, KeyModifiers::NONE),
            (KeyCode::Right, KeyModifiers::SHIFT),
            (KeyCode::Left, KeyModifiers::SHIFT),
            (KeyCode::Enter, KeyModifiers::NONE),
            (KeyCode::Right, KeyModifiers::SHIFT),
        ] {
            key(&mut app, code, modifiers);
        }
        assert_eq!(app.notes[0].content, "a\nbc");
        assert_eq!(app.notes[0].selected_text(), "b");
    }

    #[test]
    fn overlay_starts_in_content_and_uses_the_same_editing_focus_and_escape() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = editing_app(dir.path().join("board.json"), EditFocus::Tags, "text");
        app.notes[0].sel_start = Some(0);
        app.notes[0].sel_end = Some(1);
        app.toggle_overlay();
        assert_eq!(app.edit_focus, EditFocus::Content);
        assert!(!app.notes[0].has_content_selection());
        key(&mut app, KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(app.notes[0].content, "textq");
        assert_eq!(app.quit_state, QuitState::Running);
        key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.edit_focus, EditFocus::Tags);
        for c in "work".chars() {
            key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.notes[0].tags, ["work"]);
        key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.edit_focus, EditFocus::Header);
        key(&mut app, KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(app.notes[0].title, "textq");
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!app.show_overlay && !app.notes[0].editing);
        assert_eq!(app.quit_state, QuitState::Running);
    }

    #[test]
    fn ctrl_c_copies_the_focused_selection_in_inline_and_overlay_editing() {
        let dir = tempfile::tempdir().unwrap();
        for overlay in [false, true] {
            for focus in [EditFocus::Header, EditFocus::Content] {
                let mut app = editing_app(
                    dir.path().join(format!("{overlay}-{focus:?}.json")),
                    focus,
                    "éx",
                );
                if overlay {
                    app.toggle_overlay();
                    app.edit_focus = focus;
                }
                key(&mut app, KeyCode::Home, KeyModifiers::NONE);
                key(&mut app, KeyCode::Right, KeyModifiers::SHIFT);
                let mut copied = false;
                app.handle_key_with_copy(
                    KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                    |_, text| {
                        assert_eq!(text, "é");
                        copied = true;
                        Ok(())
                    },
                );
                assert!(copied);
                assert_eq!(app.quit_state, QuitState::Running);
                assert_eq!(app.show_overlay, overlay);
                assert!(!app.dirty);
            }
        }
    }

    #[test]
    fn filtering_navigation_and_membership_mutations_keep_selection_visible() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = filtered_app(dir.path().join("board.json"));
        app.select_note(Some(1));
        apply_filter(&mut app, "WORK");
        assert_eq!(app.selected, Some(0));
        assert_eq!(app.visible_note_indices(), [0, 2]);
        for (code, selected) in [
            (KeyCode::Down, 2),
            (KeyCode::Tab, 0),
            (KeyCode::Up, 2),
            (KeyCode::BackTab, 0),
        ] {
            key(&mut app, code, KeyModifiers::NONE);
            assert_eq!(app.selected, Some(selected));
        }
        app.toggle_overlay();
        key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        key(&mut app, KeyCode::Left, KeyModifiers::NONE);
        key(&mut app, KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(app.selected, Some(2));
        assert!(!app.notes[0].editing && !app.show_overlay);
        app.duplicate_selected();
        assert_eq!(app.selected, Some(3));
        assert!(app.notes[3].has_tag("work"));
        app.delete_selected();
        assert_eq!(app.selected, Some(2));
        app.delete_selected();
        assert_eq!(app.selected, None);
        assert_eq!(app.notes[1].title, "B");
        assert_eq!(app.visible_count(), 0);
    }

    #[test]
    fn filtered_mouse_tabs_never_target_hidden_notes() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = filtered_app(dir.path().join("board.json"));
        app.width = 80;
        app.height = 24;
        apply_filter(&mut app, "work");
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 8,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.selected, Some(2));
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Middle),
            column: 2,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.selected, Some(1));
        assert_eq!(app.notes[0].title, "B");
        assert_eq!(app.notes[1].title, "C");
    }

    #[test]
    fn zero_matches_disable_note_actions_and_a_new_note_clears_the_filter() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = filtered_app(dir.path().join("board.json"));
        apply_filter(&mut app, "missing");
        let before = serde_json::to_string(&app.to_save_data()).unwrap();
        for (code, modifiers) in [
            (KeyCode::Char('d'), KeyModifiers::NONE),
            (KeyCode::Char('y'), KeyModifiers::NONE),
            (KeyCode::Char('d'), KeyModifiers::CONTROL),
            (KeyCode::Char('e'), KeyModifiers::NONE),
            (KeyCode::Enter, KeyModifiers::NONE),
            (KeyCode::Char('c'), KeyModifiers::NONE),
            (KeyCode::Char('b'), KeyModifiers::NONE),
            (KeyCode::Char('t'), KeyModifiers::NONE),
            (KeyCode::Char('t'), KeyModifiers::CONTROL),
            (KeyCode::Char('O'), KeyModifiers::NONE),
            (KeyCode::Down, KeyModifiers::NONE),
            (KeyCode::Tab, KeyModifiers::NONE),
            (KeyCode::Char('['), KeyModifiers::NONE),
            (KeyCode::Char(']'), KeyModifiers::NONE),
        ] {
            key(&mut app, code, modifiers);
        }
        app.width = 80;
        app.height = 24;
        for (button, column, row) in [
            (MouseButton::Left, 4, 5),
            (MouseButton::Middle, 2, 0),
            (MouseButton::Left, 40, 22),
        ] {
            app.handle_mouse(MouseEvent {
                kind: MouseEventKind::Down(button),
                column,
                row,
                modifiers: KeyModifiers::NONE,
            });
        }
        assert_eq!(app.selected, None);
        assert_eq!(app.mode, InputMode::Normal);
        assert!(!app.confirm_delete && !app.confirm_clear_tags && !app.show_overlay && !app.dirty);
        assert_eq!(serde_json::to_string(&app.to_save_data()).unwrap(), before);
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: 4,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.menu.items, [MenuAction::NewNote, MenuAction::Close]);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.selected, Some(0));
        apply_filter(&mut app, "missing");
        key(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
        assert_eq!(app.filter_tag, "");
        assert_eq!(app.selected, Some(0));
        assert_eq!(app.count(), 4);
        assert_eq!(app.notes[1].title, "A");
    }

    #[test]
    fn clearing_tags_and_reapplying_filters_reconciles_selection() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = filtered_app(dir.path().join("board.json"));
        app.select_note(Some(2));
        apply_filter(&mut app, "work");
        assert_eq!(app.selected, Some(2), "keep a selected matching note");
        key(&mut app, KeyCode::Char('t'), KeyModifiers::CONTROL);
        key(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
        assert_eq!(app.selected, Some(0));
        assert!(app.notes[2].tags.is_empty());
        key(&mut app, KeyCode::Char('t'), KeyModifiers::CONTROL);
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.selected, None);
        key(&mut app, KeyCode::Char('T'), KeyModifiers::NONE);
        assert_eq!(app.filter_tag, "");
        assert_eq!(app.selected, Some(0));
    }
}
