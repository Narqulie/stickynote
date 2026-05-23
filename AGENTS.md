# Stickynote — AGENTS.md

Ratatui + Crossterm sticky notes TUI. Rust edition 2024, single crate.

## Build & Verify

```bash
cargo build --release          # binary at target/release/stickynote
cargo test                     # 25 tests (note.rs: 21, persistence.rs: 4)
cargo clippy                   # zero warnings expected
cargo fmt                      # uses rustfmt defaults, no config
```

No tests in `app.rs` or `ui.rs` (immediate-mode rendering requires TTY).

## Architecture

| File | Role |
|------|------|
| `main.rs` | Entry point, clap `Cli` struct (`--board`/`-b`, `--theme`/`-t`), `ratatui::init/restore`, crossterm mouse capture, event loop (poll 50ms), debounced auto-save (200ms) |
| `app.rs` | `App` struct + keyboard/mouse dispatch, note ops, tab-navigable editing (Header/Content/Tags), delete + clear-tags confirmation, context menu, double-click (350ms), input modes |
| `note.rs` | `Note` struct (content, title, tags, color, border, editing state, cursor/title_cursor/tag_input/tag_cursor), `Theme`, 10 colour hexes/names, 5 border styles, `parse_md()` / `strip_md()` |
| `ui.rs` | `render()` — single-line tab bar (1 row), full note with Header/Content/Tags sections + separators, per-section focus borders, autocomplete popup, overlay, context menu, help, welcome, 2-line footer |
| `persistence.rs` | `SaveData`/`SavedNote` with serde, `~/.stickynote/board.json`, `load_board_from()` / `save_board()` accept custom paths, graceful on missing/corrupt, field validation on load |

### Critical Design Details

- **Edition 2024** — let chains (`if let`), `&raw` not needed here, but `try_capture!` macro uses edition 2024 `$style_fn:expr` metavar.
- **EditFocus enum** — `Header`, `Content`, `Tags`. When editing, Tab cycles Header→Content→Tags→Header. Shift+Tab reverses. Each area has its own editing behaviour (title text, content, tag input + tag cursor).
- **Editing tracked per-note** (`Note.editing` field), not via `InputMode`. The `InputMode` enum (`Normal`, `TagInput`, `FilterInput`) is for text input prompts only.
- **Mouse capture** requires explicit `EnableMouseCapture` in `main.rs` + `DisableMouseCapture` before `ratatui::restore()`.
- **Event loop**: `poll(50ms)` then `read()`. This is manual (not crossterm event stream) to support save debounce. Don't switch to `event::read()` blocking.
- **Key dispatch**: converts `KeyEvent` to a string (`"c"`, `"^c"`, `"enter"`, `"esc"`, `"up"`, `"shift+tab"`...), then matches `ks.as_str()`. This pattern must be preserved for consistency.
- **Double-click**: 350ms window, ≤2px distance. Tracked via `last_click: Instant`, `last_click_btn`, `last_click_x/y` on the `App`.
- **Debounced save**: marks dirty, saves after 200ms of inactivity. `last_save: Instant`, `dirty: bool`.
- **Delete confirmation**: `d` sets `confirm_delete`, `y`/Enter confirms, any other key cancels.
- **Clear-tags confirmation**: `^t` sets `confirm_clear_tags`, `y`/Enter confirms, any other key cancels.

### UI Layout

- **Tab bar** (line 0): colored tabs side-by-side showing note titles. Tagged notes show `#` marker; untagged show `●`. Active tab highlighted with theme's selection border color. Click to select, double-click to edit. Overflow shown as `+N`.
- **Full note card** (below tab bar): Header (title, center-justified) → `─` separator → Content → `─` separator → Tags (right-aligned as `[bracketed]` chips). All three editable when editing.
- **Per-section focus borders**: The focused section (Header, Content, or Tags) gets a thin black bordered frame around it. Content area is inset by 1 char inside the border.
- **Autocomplete popup**: appears above the tags area during inline tag editing, showing up to 5 matching tags from the global registry.
- **Footer** (last 2 lines): status bar + context-sensitive hint bar.

### Key Bindings

| Key | Normal Mode | Edit Mode |
|-----|------------|-----------|
| `←/→` / `↑/↓` / `j/k` | Navigate tabs | Move cursor (Content); select tag (Tags) |
| `Tab` / `Shift+Tab` | Navigate tabs | Cycle focus (Header→Content→Tags) |
| `e` / `Enter` | Toggle edit | Focus-dependent (Enter: newline/commit-tag/focus-switch) |
| `n` | New note | — |
| `d` | Delete confirm | — |
| `^d` | Duplicate note | — |
| `c` | Cycle color | — |
| `b` | Cycle border | — |
| `t` | Tag input prompt | — |
| `T` | Toggle tag filter | — |
| `^t` | Clear-tags confirm | — |
| `/` | Filter by tag | — |
| `O` | Full-screen overlay | — |
| `[` / `]` | Move note down/up | — |
| `Esc` | Cancel filter | Stop editing |
| `^R` | Cycle theme | — |
| `?` | Toggle help | — |
| `q` | Quit | — |

### Mouse Actions

| Action | Behaviour |
|--------|-----------|
| Left-click tab | Select note |
| Double-click tab | Select + edit |
| Right-click tab | Context menu |
| Middle-click tab | Delete note |
| Click content area | Toggle editing |
| Click status bar (left third) | Cycle theme |
| Click status bar (mid third) | Cycle color |
| Scroll wheel | Navigate tabs |

## Naming & Style

- Files: `snake_case.rs` (standard Rust)
- Types, enums: `PascalCase` — `SaveData`, `MenuAction`, `InputMode`, `Theme`, `EditFocus`
- Fns, vars, fields: `snake_case` — `handle_left_click()`, `last_click_btn`, `board_path()`
- Consts: `SCREAMING_SNAKE_CASE` — `NOTE_COLORS`, `BORDER_STYLES`, `THEMES`
- Modules: `mod app; mod note; mod persistence; mod ui;` — no subdirectories
- Order inside files: structs → impl blocks → free functions → tests

## Convention Notes

- No `use` re-exports from module root — caller imports via `crate::note::parse_md`.
- Clippy: zero warnings enforced. No `#[allow(...)]` without justification.
- `unwrap()` tolerated only where failure is impossible (`dirs::home_dir()` in a TUI app).
- New notes insert at index 0 (top of stack). `select_prev/next` wraps with modulo.
- Filtering uses `visible_note_indices()` which returns filtered indices or full range.
- Markdown: `parse_md()` returns `Vec<Span>`, `strip_md()` returns plain `String` for preview display. Base style inherits note's foreground color.
- Tags are stored lowercase and matched case-insensitively (`has_tag()` uses `to_lowercase()`).

## Persistence Format

- Path: `~/.stickynote/board.json`
- `SaveData { notes: Vec<SavedNote>, theme_idx: usize }`
- `SavedNote { content, title (#[serde(default)]), color, font_style, border_style, tags }`
- Loading validates all fields: invalid colour → first in palette, invalid border → `"rounded"`, invalid font_style → `"normal"`.
- `title` is optional (`#[serde(default)]`) for backward compatibility with older saves.
- `font_style` field kept in persistence for backward compat but font cycling UI removed. New notes always `"normal"`.
- Transient editing fields (`editing`, `cursor`, `title_cursor`, `tag_input`, `tag_cursor`) are NOT persisted.

## Gotchas

- **`font_style` field is dead UI code** — stored/loaded from JSON for backward compat, but no way to change it from UI. Keep field in struct/tests/persistence.
- **No `#[derive(Default)]`** anywhere — all defaults explicit in `Note::new()` and `App::new()`.
- **Edition 2024** (`edition = "2024"` in Cargo.toml) — requires Rust 1.85+. Changes let-chain syntax to `if let` without `&&` prefix.
- **`$style_fn:expr`** macro metavar syntax only works in edition 2024.
- **Shift+Tab requires `KeyCode::BackTab`** — crossterm sends `BackTab` (not `Tab+SHIFT`) for Shift+Tab.
- **Tab bar vs peek stack**: The old peek stack (2 rows/note) has been replaced with a single-line tab bar. All references to "peek" are outdated.
- **Click to edit must use `toggle_edit()`** — not manual `note.editing = !note.editing`. `toggle_edit()` also resets `title_cursor`, `tag_input`, and `edit_focus`.
- **Custom board paths**: `App.board_path: Option<PathBuf>` stores the CLI `--board` flag. `load_board_from(&Path)` and `save_board(data, custom_path: Option<&PathBuf>)` accept custom paths. The `save_board()` function resolves custom vs default internally; callers pass `app.board_path.as_ref()`.
- **Autocomplete popup height**: `popup_h = (suggestions.len() + 2).min(max)` — the `+2` accounts for `Block::bordered()` top/bottom rows. If the popup block is changed to unbordered, remove the +2. Guard `popup_h >= 3` ensures at least one content row is visible.
- **Autocomplete filter**: The suggestion filter was once `.filter(|t| !note.has_tag(t))` which hid tags already on the note. This was removed — now ALL matching global tags are shown; duplicate prevention happens at Enter-commit time. Do not re-add the exclusion filter.
- **Tab-to-autofill**: When Tags focus is active and `tag_input` has a partial match against `all_tags`, Tab fills `tag_input` with the first matching tag (instead of cycling focus to Header). Uses `.cloned()` on the iterator to avoid borrow conflicts between `self.all_tags` (immutable ref) and `self.notes` (mutable). Tab only cycles focus when there's no match.
- **Help overlay (`render_help`)**: Must call `frame.render_widget(Clear, ...)` on the help rect BEFORE rendering the paragraph, or the underlying note card bleeds through. The block must have `.style(Style::new().bg(theme.hint_bg))` for a solid background. Theme is NOT `_theme` — it's actively used.
- **`t:tag` removed from footer hint bar**: The `t` keybinding still works for adding tags, but `t:tag` was intentionally removed from the default hint bar string in `App::hint_bar()`. Do not add it back.
