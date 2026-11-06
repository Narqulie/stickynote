# Stickynote

Ratatui/Crossterm sticky-notes TUI, Rust edition 2024, one binary crate and six modules. Minimum Rust is 1.88. Dependency versions remain pinned in `Cargo.lock`.

## Build and verify

```bash
cargo test --locked
cargo clippy --locked -- -D warnings
cargo fmt --check
cargo build --release --locked
cargo test --release --locked ui::tests
cargo test --release --locked layout::tests
python -B -m unittest discover -s .github/scripts -p 'test_*.py'
ruby -c stickynote.rb
```

Use pyenv or a virtualenv for local Python commands. Persistence and CLI tests use temporary boards, never the user's `~/.stickynote/board.json`. CLI tests execute the real binary. Release-helper tests substitute publishing tools and HTTP responses; they do not publish.

## Architecture

| File | Responsibility |
|---|---|
| `src/main.rs` | clap CLI/TUI routing, terminal cleanup, 50ms poll loop, frame preparation and autosave |
| `src/app.rs` | keyboard/mouse dispatch, editing/focus, selection/filter/search, bounded history, board switching and prompts |
| `src/note.rs` | notes/themes, Markdown, grapheme/word boundaries, character-index selections and atomic text replacement |
| `src/layout.rs` | shared card/tab/tag geometry, text source-to-cell projection and cursor-following viewports |
| `src/ui.rs` | rendering through the shared layouts, help/results/prompts/footer |
| `src/persistence.rs` | `BoardStore`, JSON conversion, locks, checked atomic save/backup/restore and exports |

Keep the existing module boundaries. Do not introduce a second editing dispatcher, persistence owner or layout calculation.

## Input and editing contracts

- Editing lives on each `Note`, not `InputMode`. `EditFocus` cycles Header → Content → Tags. Input modes are normal mode or tag/filter/search/export/board prompts.
- Key dispatch converts events to flat strings such as `^left`, `shift+tab`, and `enter`. Preserve this convention. Reject non-Press/Repeat events before history capture. Only a single non-control character may enter fallback text input; unsupported modifiers and named keys must not become text.
- Crossterm Shift+Tab is `KeyCode::BackTab`; also handle Tab with the SHIFT modifier.
- Use `toggle_edit()`/`start_editing()`/`stop_editing()`, never flip `note.editing` directly. Inline entry focuses Header; overlay entry focuses Content. Both reset cursors/selections/viewports and share dispatch/rendering. Printable `q` is text in editing.
- Ctrl+C copies a focused selection before quit routing, in inline and overlay editing. Without a focused selection it retains quit/cancel behavior. Cut must copy successfully before deleting and preserve text, selection, cursor and dirty state on failure.
- Retain the lazy `arboard::Clipboard` owner across operations and successful board switches. Clipboard errors and save errors have separate state. Never unwrap clipboard initialization or other fallible external I/O.
- Bracketed terminal paste and clipboard paste share `insert_paste()`. Content retains newlines; Header/Tags remove controls and newlines. Tags paste fills its draft, not committed chips. Export/board prompts accept path text without submitting; other prompts and normal mode ignore paste.
- Tags are trimmed/lowercased and deduplicated case-insensitively. Rendering, completion and popup exclusion use `tag_suggestions()`. Tab completes a partial suggestion once, then advances focus on an exact/no-change match. Enter commits without duplicates. Keep all matching global tags in suggestions, including already-committed tags.
- Double-click uses 350ms and a two-cell distance. Mouse drag selects within the starting Header or Content field. Borders, separators, footer and autocomplete popup cannot select underlying text.

## Text and geometry

- Cursor/selection positions remain Unicode **character indices**, never byte offsets. Convert with `char_idx_to_byte()` before string slicing/mutation; use `char_len()` for bounds.
- Left/Right and Backspace/Delete operate on extended graphemes. Selection ranges expand to whole graphemes; replacement is one atomic operation so removing text cannot relocate the original insertion point when neighboring clusters join. Store source text unchanged, without Unicode normalization.
- Ctrl+Left/Right and Ctrl+Backspace/Delete use the same documented word boundaries. Ctrl+Shift+Left/Right extends selection. Draft Backspace removes one grapheme.
- `cell_width()`/`TextLine` use the pinned segmentation/width versions. Source ranges survive cursor insertion, centering, clipping and horizontal windows. A wide continuation cell maps to its grapheme's first source character. Terminals/fonts may have different width policies.
- `App::prepare_frame()` runs before every draw, synchronizing dimensions and caret following. `NoteLayout`, `TagsLayout`, `TabStrip` and `ui::menu_rect()` drive both rendering and hit-testing. Do not use fixed mouse x/y origins. Menu bounds exclude both footer rows; overflow counters/gaps are not tab targets.
- `Note.viewport` stores logical `top_line` and display-cell `left_cell`. Offsets follow the Content caret only when it leaves the window, clamp after resize/mutation and reset on edit entry, duplication and load. Inactive focus retains the body window. Viewports are transient and never dirty the board.
- Content PageUp/PageDown moves by visible body height while preserving a cell column; Shift extends selection. `body_lines()` skips hidden rows before projecting visible glyphs. Drawing, clicks, drags and highlighting consume the same windowed projection.
- Editing shows literal, unwrapped Markdown. Reading uses styled soft wrapping without changing source/history. Clicking wrapped preview starts ordinary Header editing, not an invented wrapped-source mouse coordinate.
- Small terminals or unavailable focused areas show a resize prompt with inert underlying clicks. Help is modal and scrollable with arrows/j/k/wheel, page keys and Home/End; Esc/?/q closes help without quitting.
- Focus borders use thick black blocks and full-block cursors. Single-line focus areas need three rows; their adjacent separators are absorbed. Render `Clear` and an opaque background before overlays to prevent card bleed-through.
- `none` has no outer border even while editing. Legacy `hidden` remains borderless while reading and has a plain editing border. Preserve stored border/font compatibility.

## Selection, search and history

- `App.selected: Option<usize>` is `None` with no matches. `visible_note_indices()` owns membership: exact case-insensitive tag AND case-insensitive substring search over title/body/tags. Navigation and selected actions must use that rule.
- Use selection/reconciliation helpers after membership mutations. Preserve a matching selection or choose a visible replacement. Losing selection ends editing/overlay; zero matches disable note actions. New notes insert at zero and clear both filters; duplicates retain source tags.
- Normal `s` opens the live search jump list. Search Esc/Ctrl+C clears only text search, tag-prompt Esc clears only tag filtering, normal Esc clears both. Query changes remain transient.
- Ctrl+Z/Ctrl+Y restore session-only board snapshots outside modal prompts/menus/help/failed-quit dialogs. Both stacks share 100 snapshots and an estimated 8 MiB allocation limit. Very large actions may have no history entry.
- Compare persisted `SaveData` for history/dirty decisions. Cursor/focus/draft/query/view changes and failed/identical operations are not actions. Consecutive typing in one field groups within 750ms; selection replacements, pastes and structural changes remain separate. New persisted edits invalidate redo; ignored key releases do not break a typing group.
- History restores selection, filters, focus and editing state, reconciles visibility, closes stale transients and calls the ordinary dirty/save path. It is not persisted or a substitute for backup.

## Persistence and file ownership

- Default board is `~/.stickynote/board.json`. `BoardStore::open()` runs before terminal setup; `App::new(data, store)` owns the store. Only a missing board starts empty. Invalid JSON, read or lock errors must not silently authorize replacement.
- Stable `.lock` sidecars hold fs2 exclusive locks until store drop. Keep the marker across sessions; deleting it can bypass a running lock. Lock acquisition uses fully qualified `fs2::FileExt` for Rust 1.88 compatibility.
- `.bak`/`.lock` filename suffixes, including Unicode case variants, are reserved for persistence. Validate requested and canonical paths. CLI capture/restore and board switching must never admit a sidecar as an editable board. To recover a backup, run `--board <original> restore --yes` or copy it to a normal board filename while closed.
- Saves compare disk bytes with the last observed bytes, serialize first, then use same-directory `NamedTempFile`, preserved permissions, complete write, file sync and atomic replacement. Advisory locks cannot serialize editors that ignore them. Parent-directory fsync/full power-loss durability is not promised.
- Before replacing an existing board, rotate only its validated prior bytes into one `.bak`. Backup failure cancels the save. Recheck native bytes after rotation. First save has no prior backup. Explicit confirmed restore validates/locks/rechecks and replaces only the original, never rotating damaged bytes into backup.
- `BoardStore::export()` owns export admission. Refuse persistence suffixes and active board destinations, including missing-name case/normalization aliases. Consult existing destination lock markers through filesystem lookup/locking, holding an acquired marker until no-clobber publication; never create markers for ordinary exports. Existing files/symlinks cannot be overwritten.
- `B` switches only to existing validated boards. Load/lock the destination and save current work before replacing App; failure retains current data/lock/history. Success transfers clipboard ownership and resets history, filters and editing transients.
- `SaveData { notes, theme_idx }` persists note content/title/color/font_style/border_style/tags only. Missing title defaults empty; missing legacy font_style defaults `normal`; other required fields remain required. Invalid palette/border/font values get the documented defaults. Keep `font_style` in schema/tests though it has no editing UI.
- Autosave waits 200ms after the last persisted edit, sampled by the 50ms loop. Failed writes keep dirty data and retry at most once per second. Ctrl+S explicitly flushes/retries. Do not dirty cursor/focus/draft/navigation operations.
- `QuitState::{Running, SaveFailed, ConfirmDiscard, Done}` retains dirty work after failed saves. Enter/r retries, Esc cancels, d then y confirms discard. Do not flush again after confirmed discard. Abnormal loop exit attempts a checked save and reports both failures.
- `TerminalCleanup` exists before fallible terminal setup. Independently attempt to disable bracketed paste and mouse capture and always restore the terminal even if disabling fails. Preserve polling rather than a blocking event read so debounce remains functional.

## Release and operational limits

CI checks main/master pushes and PRs. Publication requires an explicit `vX.Y.Z` tag or manual version request and checks one immutable commit. Never infer publication from a Cargo version bump.

`.github/scripts/release.py` preflights crates.io source identity, remote/local tag and GitHub archive, and Homebrew URL/checksum before any destination write. Matching completed destinations may be skipped on retry; conflicts fail closed. Tests exercise partial publication and retries without publishing. Workflow actions are SHA-pinned, checkout does not persist credentials, and only the publishing job has contents-write. Registry/tap credentials must reach only their required child tools.

A local change does not grant commit, push, tag, installation or release authority. The formula still targets the actual published archive until an approved release updates it. Linux X11 clipboard behavior is verified during application lifetime; Wayland, Windows runtime and post-exit clipboard persistence remain unverified. Hosted release execution and credential scope require an authorized release, not claims based on local fixtures.

## Conventions

- Keep explicit constructor defaults, not `#[derive(Default)]` on domain state.
- Keep `strip_md()` for tab previews and plain-text export.
- Do not reintroduce the old peek stack or add `t:tag` to the hint bar. The `t` binding still exists.
- Discover current shortcuts in README/help. Keep documentation and regression tests with behavioral changes.
