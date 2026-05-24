# Stickynote — AGENTS.md

Ratatui + Crossterm sticky notes TUI. Rust edition 2024, single crate (5 source files).

## Build & Verify

```bash
cargo build --release          # binary at target/release/stickynote
cargo test                     # 25 tests (note.rs: 21, persistence.rs: 4)
cargo clippy                   # zero warnings — no #[allow(...)] without justification
cargo fmt                      # rustfmt defaults, no config
```

No tests in `app.rs` or `ui.rs` (immediate-mode rendering requires TTY).

## Architecture

| File | Role |
|------|------|
| `main.rs` | Entry point, clap `Cli` (`--board`/`-b`, `--theme`/`-t`), `ratatui::init/restore`, mouse capture, event loop (poll 50ms), debounced auto-save (200ms) |
| `app.rs` | `App` struct + keyboard/mouse dispatch, note ops, tab-navigable editing (Header/Content/Tags), delete + clear-tags confirmation, context menu, double-click (350ms), input modes |
| `note.rs` | `Note` (editing state per-note, not via InputMode), `Theme`, 10 colours, 5 border styles, `parse_md()` / `strip_md()` |
| `ui.rs` | `render()` — single-line tab bar, full note card with per-section focus borders, autocomplete popup, overlay, context menu, help, welcome, 2-line footer |
| `persistence.rs` | `SaveData`/`SavedNote`, serde JSON, `~/.stickynote/board.json`, graceful on missing/corrupt, field validation |

## Critical Design Details

- **Edition 2024** — let chains (`if let`), `$style_fn:expr` macro metavar.
- **EditFocus** — `Header`, `Content`, `Tags`. Tab cycles forward, Shift+Tab reverses.
- **InputMode** (`Normal`, `TagInput`, `FilterInput`) is for text input prompts only. Editing state lives on each `Note` (`note.editing`).
- **Mouse capture**: explicit `EnableMouseCapture` + `DisableMouseCapture` before `ratatui::restore()`.
- **Event loop**: `poll(50ms)` then `read()` (manual, not crossterm event stream) to support save debounce. Do not switch to `event::read()`.
- **Key dispatch**: converts `KeyEvent` to a flat string (`"c"`, `"^c"`, `"enter"`, `"shift+tab"`...), matches `ks.as_str()`. Preserve this pattern.
- **Double-click**: 350ms window, ≤2px distance. Tracked via `last_click: Instant`, `last_click_btn`, `last_click_x/y`.
- **Debounced save**: `dirty: bool`, saves after 200ms of inactivity via `last_save: Instant`.
- **Click to edit** must use `toggle_edit()` — not `note.editing = !note.editing`. `toggle_edit()` resets cursors and `edit_focus`.
- **Shift+Tab** = `KeyCode::BackTab` (crossterm sends `BackTab`, not `Tab+SHIFT`).
- **Tab-to-autofill in Tags focus**: If `tag_input` partially matches `all_tags`, Tab fills with first match (uses `.cloned()` to avoid borrow conflict). Tab only cycles focus when no match.
- **Focus border in edit mode**: Uses `Block::bordered()` with `BorderType::Thick` + black. Single-line sections (Header, Tags) expand to a 3-row rect (1 above, 1 below) so top/bottom borders have room. Adjacent separator lines are absorbed into the border and skipped. Content renders inside the block's `inner()` area (1-char inset). Cursors use `█` (full block) in all sections.
- **`focus_block` closure**: Defined as `let focus_block = || { Block::bordered()... }` (closure, not value) so it's constructed fresh each call — necessary because ratatui blocks are consumed by `render_widget()`.
- **Help overlay (`render_help`)**: Must render `Clear` on the help rect BEFORE the paragraph block. Block needs `.bg(theme.hint_bg)` or note card bleeds through.
- **Autocomplete popup height**: `popup_h = (suggestions.len() + 2).min(max)` — the `+2` accounts for `Block::bordered()` top/bottom. Guard `popup_h >= 3`.
- **Autocomplete filter**: ALL matching global tags shown (duplicate prevention at Enter-commit). Do not re-add exclusion filter `.filter(|t| !note.has_tag(t))`.
- **`render_welcome()`** and **`render_menu()`** take `&Theme` and use theme colors (not hardcoded greys).

### Key Bindings

| Key | Normal Mode | Edit Mode |
|-----|------------|-----------|
| `←/→` / `↑/↓` / `j/k` | Navigate tabs | Move cursor (Content); select tag (Tags) |
| `Tab` / `Shift+Tab` | Navigate tabs | Cycle focus (Header→Content→Tags) |
| `e` / `Enter` | Toggle edit | Focus-dependent (newline/commit-tag/focus-switch) |
| `n` | New note at position 0 | — |
| `d` / `y` | Delete confirm / confirm | — |
| `^d` | Duplicate note | — |
| `c` | Cycle color | — |
| `b` | Cycle border | — |
| `t` | Tag input prompt | — |
| `^t` / `y` | Clear-tags confirm / confirm | — |
| `T` | Toggle tag filter | — |
| `/` | Filter by tag | — |
| `[` / `]` | Move note down/up | — |
| `O` | Full-screen overlay | — |
| `Esc` | Cancel filter / deselect | Stop editing |
| `^R` | Cycle theme (dark → light → mono) | — |
| `?` | Toggle help | — |
| `q` / `^C` | Quit | — |

### Mouse

| Action | Behaviour |
|--------|-----------|
| Left-click tab | Select note |
| Double-click tab | Select + edit (via `toggle_edit()`) |
| Right-click tab | Context menu |
| Middle-click tab | Delete note |
| Click content area | Toggle editing |
| Click status bar (left/mid third) | Cycle theme / color |
| Scroll wheel | Navigate tabs |

## Naming & Conventions

- **Types**: `PascalCase` — `SaveData`, `MenuAction`, `EditFocus`, `Theme`
- **Fns, fields**: `snake_case` — `handle_left_click()`, `last_click_x`, `board_path()`
- **Consts**: `SCREAMING_SNAKE_CASE` — `NOTE_COLORS`, `BORDER_STYLES`, `THEMES`
- **Order inside files**: structs → impl blocks → free functions → tests
- **No `#[derive(Default)]`** — all defaults explicit in `Note::new()` and `App::new()`.
- **No `use` re-exports** from module root — callers import via `crate::note::parse_md`.
- **`unwrap()`** tolerated only where failure is impossible (`dirs::home_dir()` in a TUI).
- **`font_style` field is dead UI code** — stored/loaded from JSON for backward compat, but no UI to change it. Keep field in struct/tests/persistence. All new notes get `"normal"`.
- **`save_board()`** returns `Result<(), String>`. Errors propagate to `App.save_error` via the auto-save loop and `flush_save()`. Displayed as `⚠ {msg}` in the status bar.
- **`strip_md()`** is live — used in `tab_label()` so markdown syntax doesn't appear in tab previews. Returns `String`.

## Persistence

- Path: `~/.stickynote/board.json` (or custom via `--board`)
- `SaveData { notes: Vec<SavedNote>, theme_idx: usize }`
- `SavedNote { content, title (#[serde(default)]), color, font_style, border_style, tags }`
- Validation on load: invalid colour → first palette entry, invalid border → `"rounded"`, invalid `font_style` → `"normal"`.
- Transient fields (`editing`, `cursor`, `title_cursor`, `tag_input`, `tag_cursor`) are NOT persisted.
- Custom board path stored as `App.board_path: Option<PathBuf>` from CLI. `save_board()` resolves default vs custom internally; callers pass `app.board_path.as_ref()`.

## Gotchas

- **Tag matching**: Tags stored lowercase, matched case-insensitively via `has_tag()` (`.to_lowercase()`).
- **New notes** insert at index 0. `select_prev/next` wraps with modulo.
- **Filtering** uses `visible_note_indices()` returning filtered or full range.
- **`t:tag` removed from hint bar**: The `t` keybinding still works but was intentionally removed from `App::hint_bar()`. Do not add it back.
- **Tab bar** (single-line) replaced the old peek stack (2 rows/note). No references to "peek" remain.
- **Help overlay**: Theme is NOT `_theme` — it's actively used as `theme`.
