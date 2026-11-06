# Stickynote fix and feature roadmap

Baseline reviewed at version 0.3.1, commit `3905763`, on 2026-10-07. The Captain approved sequential implementation, then directed completion of the remaining list without per-batch interruptions. The recommended local defaults became the feature scope.

**All 38 tasks are complete:** 23 correctness/reliability tasks, 6 testing/distribution tasks and 9 additions. Source locations in the original findings refer to the audit baseline. Completion means implemented and locally verified within the stated limits, not installed, committed or published.

## Final delivery

- Safety, selection, shared layout, Unicode/grapheme editing, cursor-following viewports, active-tab windows and paged help are implemented. Autosave uses actual 200ms inactivity with bounded retries; schema defaults and tag completion match the docs.
- Ctrl+Z/Ctrl+Y provide bounded session history. `s` searches title/body/tags with a jump list, combined with exact-tag filtering. Bracketed paste inserts text rather than commands.
- One validated `.bak` and `restore --yes` provide explicit recovery. `E`/Ctrl+E export selected/visible notes to non-overwriting Markdown/text. CLI `add` accepts arguments or piped UTF-8. `B` switches existing boards while preserving unsaved work on failure. Reading wraps; editing keeps literal unwrapped source.
- Release publication requires an explicit version tag/manual request and immutable source checks. Matching completed destinations can be resumed; mismatches stop all writes. Permissions/actions/credentials are constrained, formula verification uses `--version`, minimum Rust is 1.88, and MIT licensing is included.
- Final review found backup-as-board ownership, missing-name export aliases and key-release history defects. The parent reproduced all three through four failing regressions, repaired their ownership/input boundaries and obtained a follow-up `verdict: ok`.
- Post-repair gates passed **127 unit + 4 CLI tests**, **28 release UI + 8 release layout tests**, **7 release-helper fixtures**, Clippy, formatting and locked release builds. Rust 1.88 builds/tests passed on macOS and Linux; Linux also passed 1.88 Clippy/formatting and real X11 clipboard ownership through switching/cut/undo/redo/save/exit. Native probes exercised case and Unicode-normalization filename aliases.

See [docs/VERIFICATION.md](docs/VERIFICATION.md) for environments, evidence and remaining limits. Wayland, Windows runtime, post-exit clipboard persistence, full power-loss durability, hosted publication and credential scopes are not claimed. No commit, staging, push, tag, installation, tap mutation or release occurred. The formula retains the actual published archive.

## System assessment

The current application is appropriately small: one Rust crate, six modules, a local JSON board, and a terminal interface. Keep that structure. It does not need a database, web service, plugin framework, or cloud synchronization to become reliable and useful.

| Subsystem | Responsibility | Audit-baseline weakness (resolved items recorded below) |
| --- | --- | --- |
| `src/main.rs` | CLI, terminal setup/cleanup, event loop, autosave | Cleanup and final-save errors; dimensions initialized only on resize; no terminal-paste events |
| `src/app.rs` | State, keyboard/mouse dispatch, note operations, filters, clipboard | Selection and focus invariants; command names mistaken for text; duplicated save handling |
| `src/note.rs` | Note data, character-index helpers, selection, themes, inline markdown | Mostly coherent, but renderer and input handlers do not consistently honor its cursor/selection contracts |
| `src/ui.rs` | Tabs, note sections, overlays, help, selection highlights | Unsafe bounds, no cursor-following viewport, hit-testing geometry duplicated elsewhere |
| `src/layout.rs` | Shared section geometry, grapheme/caret cell projection and tab/tag hit regions | Added after the audit to remove duplicated coordinate/width calculations |
| `src/persistence.rs` | JSON conversion and file access | Destructive writes, silent load failures, no same-board writer protection |
| Release workflow and formula | CI, crates.io, GitHub release, Homebrew | Fragile release detection/recovery and an incorrect formula test |

### Audit-baseline end-to-end flow

1. `main()` loads JSON; a failed load currently looks identical to a new empty board.
2. `App::new()` restores notes and resets transient state.
3. The event loop draws from App, then sends input to keyboard/mouse handlers.
4. Keyboard dispatch turns keys into flat strings, then chooses menu, overlay, prompt, editing, or normal handling.
5. Mutations mark the board dirty. The loop saves based on time since the previous save.
6. Saving rewrites the board directly. Quit handling and terminal cleanup attempt another flush.

Most defects arise at the boundaries between those steps. Fix their contracts rather than introducing a second implementation of the same behavior.

### Evidence and limitations

The existing release build, 31 unit tests, Clippy, and formatting passed during the audit. Those tests do not cover application dispatch or rendering.

A temporary harness against the audit baseline contained 19 failing intended-behavior checks. Failures reproduce the Unicode title panic, unsafe help/tab rendering, filter and mouse defects, selection errors, overlay behavior, invisible editing cursors, silent corrupt-board replacement, and competing-writer data loss. A separate copied-app harness injects only clipboard-write failure and reproduces destructive cuts.

Two newly identified bounds failures also occur in release-mode tests:

- Opening help in an 80x24 terminal accesses buffer coordinate `(12, 24)` outside the frame.
- Rendering several tabs at width 40 accesses `(40, 0)` outside the frame.

The separate 80x5 note-layout subtraction panic was debug-only in the tested reproduction. Do not describe that particular test as a verified release-mode crash.

Harnesses and logs currently live at `/tmp/stickynote-audit.WQRVRu/`; this roadmap preserves the durable task descriptions because temporary artifacts may disappear. Clipboard behavior on Linux, repository token permissions, dependency advisories, and actual publication were not tested.

## Historical delivery checkpoints

The following checkpoint notes preserve what was verified at each stage. Their pending-work statements and test counts describe that earlier stage; the final delivery above and checked tasks below are authoritative for current status.

## Safety batch completed

FIX-01 through FIX-08 are implemented and verified. FIX-13 was also completed as the prerequisite for keeping rendered menu bounds and mouse targets synchronized from startup.

- `BoardStore` owns the canonical board path, stable sidecar lock, last observed disk bytes, and atomic save operation. Missing files start empty; read/parse/lock errors stop startup before entering the TUI.
- Cuts copy before deletion; Unicode title cursor insertion converts character indices to byte offsets.
- Common help/tab crashes are fixed, small frames use a compact view, and menus share one geometry function for rendering and mouse dispatch with footer rows excluded.
- Failed quit offers retry, cancel, and separately confirmed discard. Abnormal terminal-loop exits report save failures; RAII cleanup restores terminal modes on errors.
- All 50 repository tests, Clippy, formatting, release build, and five release-mode renderer tests pass. A real pseudo-terminal check verifies corrupt-file refusal, second-instance refusal, Unicode editing, 80x24 help, 40-column tabs, mouse selection before resize, failed-quit confirmation, and restored terminal attributes.
- The independent reviewer found a resized-menu render/hit-test mismatch in the initial patch. The shared menu rectangle and its click/footer regression resolve that finding.

The JSON example is also corrected and the persistence/quit contracts are documented. Broader documentation/MSRV/release work and optional schema defaults remain pending. No version bump, commit, installation, or publication has been performed.

Known limits: file data is synced before replacement, but parent-directory power-loss durability is not promised. External file changes are compared before saving; non-cooperating external writers cannot be serialized by the advisory lock. Linux clipboard lifetime and Windows/Linux runtime behavior have not been checked in this batch. Help is now safely clipped, not yet scrollable.

## Editing and filtering batch completed

FIX-09 through FIX-12 are implemented and verified.

- Fallback text accepts only a single non-control character. Unsupported command names/modifiers cannot become text; Content Delete removes a character or selection using character indices.
- `Note::insert_title()` / `insert_content()` replace selections and clear collapsed anchors. Ctrl+A moves the active cursor to the selected end. Rendering maps source indices before inserting the caret, including cropped titles and multiline Unicode selections.
- Overlay entry starts in Content. Inline and overlay editing share keyboard/clipboard dispatch, focus rendering, tag suggestions, and contextual hints; printable `q` is text and Esc stops editing/closes the overlay.
- `App.selected: Option<usize>` explicitly represents no visible selection. Filter and tag changes reconcile selection, navigation wraps visible indices, and zero matches disable note actions and show an empty-result message. New notes clear the filter; duplicates retain matching tags.
- All 66 tests, Clippy, formatting, release build, and ten release renderer tests pass. A read-only reviewer found no issues. Real PTY checks verify the collapsed-anchor sequence, Unicode/selection Delete, overlay focus and `q`, filtering from a hidden selection, matching-note duplicate/delete, zero-match action guards, and new-note visibility. The previous safety PTY checks still pass.

Literal Markdown editing was necessary to make source-index selection truthful. Its mouse/cell acceptance was deferred at that stage and is now completed in the geometry batch below. Cursor-following viewports were still pending at that stage and are completed below. Tab/help overflow, autosave timing and autocomplete focus policy remain pending. No commit, installation, version bump or publication was performed.

## Mouse and Unicode layout batch completed

FIX-14, FIX-15 and FIX-18 are implemented. The resulting literal-source click/selection checks also complete FIX-19.

- `src/layout.rs` owns card/section rectangles, focus insets, centered/cropped title projection, autocomplete bounds and tab/tag hit regions. Drawing and mouse dispatch consume the same calculations. The superseded `Note::content_lines()` caret renderer is removed and its contract checks moved to `src/layout/tests.rs`.
- `TextLine` groups the displayed graphemes after inserting the caret and carries the original character ranges through cell clipping/centering. CJK/emoji continuation cells map to the same source index; combining sequences remain together. Stored cursors/selections are still character indices and string mutation still converts to byte boundaries.
- Editing clicks map the currently drawn text before changing focus. Header drags select title text; body drags select body text. Drags clamp to their own field, and clicks on borders/separators/footer or the visible autocomplete popup cannot edit underlying content. Tag-chip selection and keyboard deletion preserve body focus/cursor data.
- Wheel events outside editing navigate only visible notes, even when hovering over the footer. In Content editing, the wheel and vertical keys preserve display-cell columns; Header/Tags ignore wheel input.
- All 73 debug tests, Clippy, formatting, the release build, 15 release UI tests and 4 release layout tests pass. A read-only reviewer found no issues. Real PTY checks verify wide-glyph mouse placement in inline/overlay bodies, centered Unicode header placement and filtered wheel navigation over the footer. The earlier safety/editing smoke checks still pass.

Limits: normal reading clicks retain the existing `toggle_edit()` entry behavior. Selection styles a whole displayed grapheme while copying the actual character range. Character-based keyboard movement/deletion can still split a combining or emoji sequence; ADD-08 remains separate. Ratatui's cell widths match its pinned dependency versions, not every possible terminal font/width policy. Body viewports were deferred at that stage and are completed below; active-tab/help windows remain pending. No version bump, commit, installation or publication was performed.

## Cursor-following viewport batch completed

FIX-16 is implemented and verified.

- `Note.viewport: ContentViewport` stores transient logical-row and display-cell offsets. `App::prepare_frame()` follows the Content caret before every draw and clamps after resize or content changes. Offsets stay stable while the caret fits and reset on editing entry, load and duplication. View changes do not dirty the board or enter JSON.
- `TextLine::window()` applies the same horizontal offset to visible text, source selections and mouse hit-testing. A partially clipped wide glyph is omitted with cell padding rather than split. `body_lines()` scans source offsets before skipping hidden rows and projecting visible ones; the UI builds only the pane's visible lines.
- Content PageUp/PageDown moves caret and viewport by the pane height and clamps at the first/last logical line. Shift extends selection. Newlines, deletion, paste replacement and select-all continue to use source character indices.
- Long tag input reserves room for its caret before chips are clipped. Empty editing panes show a resize prompt, and clicks cannot hit hidden fields beneath that prompt. Autocomplete and selected-chip navigation policy remain FIX-23.
- All 83 debug tests, Clippy, formatting, release build, 19 release UI tests and 7 release layout tests pass. A read-only review found no issues. The parent subsequently moved glyph projection after skipped-row traversal and reran the gates. Real PTY checks verify scrolled Unicode mouse edits in inline/overlay mode, smaller-window hit-testing, and PageUp followed by editing the expected source line; earlier smoke checks still pass.

Limits: scrolling follows editing cursors, not the read-only preview. Soft wrapping, drag auto-scroll outside the pane, grapheme-aware keyboard movement and overflowed selected-tag visibility remain separate work. Tiny panes require resizing before further text entry. One post-build smoke attempt exceeded the harness's three-second process-start deadline on the corrupt-board check; the unchanged rerun passed all 17 checks. Its cause was not established. No commit, version bump, installation or publication was performed.

## Priorities and effort

- **P0:** Prevent data loss and common-operation crashes before adding features.
- **P1:** Make existing advertised interactions correct and predictable.
- **P2:** Improve ongoing reliability, distribution, and usability.
- **Proposed:** A new capability requiring an explicit scope decision.

Effort is relative: S is a localized change; M spans a few cooperating paths; L is a substantial subsystem change. These are not calendar estimates. Each fix includes its focused regression coverage; QA tasks do not postpone tests until the end.

## 1. Protect data and stop crashes

- [x] **FIX-01 · P0 · M · Make board writes atomic.**
  - Evidence: `src/persistence.rs:84-92` truncates the existing file before serialization completes.
  - Change: write a completed replacement in the same directory, check write/flush results, and atomically replace the original. Decide the fsync policy explicitly; atomic replacement and power-loss durability are different promises.
  - Done when: an injected write failure leaves the previous board readable, successful saves preserve all persisted fields, and temporary files are cleaned up appropriately.

- [x] **FIX-02 · P0 · M · Distinguish missing boards from failed loads.**
  - Evidence: `src/persistence.rs:47-69` returns an ordinary empty board for malformed JSON and read failures; adding a note then replaces the failed input.
  - Change: return a load error with path/context. Recommended first behavior is to refuse editable startup on an existing unreadable/corrupt board, report the problem before entering the TUI, and leave the file intact.
  - Done when: missing files start a new board; failed loads do not authorize overwriting an existing one; the error tells the user which file needs attention. Do not silently salvage a partial board by discarding notes that fail deserialization; retain the existing documented color/border validation.

- [x] **FIX-03 · P0 · S · Correct Unicode title cursor rendering.**
  - Evidence: `src/ui.rs:247` inserts the cursor at a character index interpreted as a byte offset; typing `é` crashes.
  - Change: convert through `char_idx_to_byte()` before insertion; keep all stored cursor/selection offsets as character indices.
  - Done when: accented text and an emoji can be typed, navigated, selected, and rendered without panic in Header focus.

- [x] **FIX-04 · P0 · S · Make clipboard cuts transactional.**
  - Evidence: `src/app.rs:1239-1254` deletes before copying and only marks dirty on clipboard success.
  - Change: copy the selection first; delete and mark dirty only after success.
  - Done when: a failing clipboard preserves text, cursor, selection, and clean/dirty state; a successful cut saves the deletion normally.

- [x] **FIX-05 · P0 · M · Bound every rendered rectangle and text width.**
  - Evidence: `src/ui.rs:94-111,118-159,338,607-637,727-795`; help at 80x24 and tabs at width 40 panic in release-mode harnesses. Tiny note geometry also subtracts before saturating.
  - Change: compute all rectangles within the frame; never force a truncated label longer than its remaining width; bound help, menus, welcome, and overlay dimensions. Use safe arithmetic before computing section heights. Provide a compact too-small view where necessary.
  - Done when: the recorded help/tab reproductions render safely in debug and release tests; small geometry cannot produce an out-of-frame rectangle. Help paging is addressed separately in FIX-17.

- [x] **FIX-06 · P0 · M · Protect a board from competing writers.**
  - Evidence: two Apps loaded from the same path can save stale whole-board snapshots; the second silently erases the first instance's new note. No locking exists in persistence or startup.
  - Change: hold a board-scoped exclusive lock for the editing session, with a clear second-instance error. Protect against external file replacement by detecting a changed saved file before overwriting it. Start with refusal rather than automatic merging.
  - Done when: a second writer cannot silently replace the first session's edits, different board paths remain usable simultaneously, and an external edit produces an actionable conflict error. Coordinate lock identity with FIX-01's atomic replacement.

- [x] **FIX-07 · P0 · M · Preserve pending changes when the final save fails.**
  - Evidence: `src/app.rs:278,332,427,1481-1493` and `src/main.rs:69` record errors but still quit; the error can disappear with the TUI.
  - Change: make save results explicit. An ordinary quit should remain in the app after a failed save with retry/cancel choices; an unrecoverable event-loop exit should restore the terminal and report unsaved data with a nonzero exit status.
  - Done when: failed quit-time saving cannot look successful; dirty state is retained for retry; any discard action requires explicit confirmation.
  - Depends on: FIX-01 and FIX-02.

- [x] **FIX-08 · P1 · S · Restore terminal state on every exit path.**
  - Evidence: `src/main.rs:60,65-66` can return from a mouse-capture error before `ratatui::restore()`.
  - Change: make terminal restoration unconditional after initialization, including failures enabling/disabling optional terminal modes. Preserve the original operational error while reporting relevant cleanup failures.
  - Done when: injected setup/loop/cleanup errors do not leave raw mode or the alternate screen active. Apply the same ownership to future bracketed-paste mode.

## 2. Make editing, navigation, and rendering agree

- [x] **FIX-09 · P1 · S · Separate character input from command-key names.**
  - Evidence: fallback handlers at `src/app.rs:500-505,525-530,700-711,790-796,928-939` insert the first character of names such as `delete`, `up`, and control combinations.
  - Change: preserve flat-string dispatch, but accept fallback text only for an actual single non-control character. Implement Content Delete explicitly, including selected text.
  - Done when: Delete removes text rather than inserting `d`; unhandled navigation/control keys never mutate note or prompt text.

- [x] **FIX-10 · P1 · M · Keep selection anchors, cursors, and highlights coherent.**
  - Evidence: the collapsed-selection sequence in `src/app.rs:661-710,868-938` deletes unintended text. `src/ui.rs:845-975` has separate selection rendering; cropped title highlighting does not account for the skipped source offset.
  - Change: clear stale endpoints after mutations, keep the active selection end consistent with the cursor, render the cursor while selecting, and map cropped highlights from the correct source range.
  - Done when: the recorded `abc`/insert-X/select/delete sequence produces `aXc`; forward/backward selection and a cropped title highlight the text that copy/cut actually use.
  - Depends on: FIX-03 and FIX-09.

- [x] **FIX-11 · P1 · M · Unify inline and overlay editing behavior.**
  - Evidence: `src/app.rs:258-280,424-432,1201-1213` and `src/ui.rs:623` disagree on focus. Overlay Ctrl+C quits with a selection, and typing ordinary `q` also quits.
  - Change: choose a defined overlay-entry focus, preferably Content; render the actual subsequent focus and suggestions. Use the same editing and clipboard rules as inline mode. Ordinary printable keys remain text while editing.
  - Done when: Header/Content/Tags focus is both visible and operative; selected-text copy does not quit; `q` can be typed; Esc closes the overlay predictably.
  - Depends on: FIX-04, FIX-09, and FIX-10.

- [x] **FIX-12 · P1 · M · Make visible selection an invariant under filtering.**
  - Evidence: `src/app.rs:364-373,512-521,1439-1458` and `src/ui.rs:48-49` leave unmatched notes selected and actionable.
  - Change: navigate `visible_note_indices()`; reconcile selection after applying a filter and after mutations that change membership. Represent no visible selection explicitly for a zero-match board instead of retaining a hidden actionable note.
  - Done when: arrows, Tab, clicks, editing, delete, duplicate, and tag changes operate only on visible selected notes; zero matches display a useful empty result and allow clearing the filter.
  - Implemented policy: creating a new note clears the filter and selects the new note. Duplicates retain the source tags and remain in the filtered view.

- [x] **FIX-13 · P1 · S · Initialize interaction geometry on startup.**
  - Evidence: `src/app.rs:117-118` initializes dimensions to zero; `src/main.rs:86-88` changes them only on Resize.
  - Change: synchronize terminal size before first rendering/input and on subsequent resize.
  - Done when: tab, body, menu, and status-bar clicks work immediately after launch without resizing the terminal.

- [x] **FIX-14 · P1 · L · Give rendering and hit-testing one layout calculation.**
  - Evidence: `src/app.rs:1363-1415` assumes fixed offsets that differ from `src/ui.rs:255-256,368-369,619-623`.
  - Change: calculate section rectangles, focused insets, title padding, and overlay origins once; use those results for rendering and click/drag mapping. Ignore clicks outside the relevant text area.
  - Done when: clicking a displayed character positions the cursor at that character in normal and overlay layouts; header dragging selects header text; clicking footer/borders cannot move the text cursor.
  - Depends on: FIX-05 and FIX-13. Extend the same calculation for FIX-16 and FIX-18 rather than creating another geometry model.

- [x] **FIX-15 · P1 · S · Route normal-mode wheel navigation.**
  - Evidence: `src/app.rs:979-981` excludes scroll events from a handler containing the ScrollUp/ScrollDown branches.
  - Change: dispatch those events explicitly while retaining the distinct editing behavior.
  - Done when: scrolling selects the previous/next visible note in normal mode and does not escape the active filter.
  - Depends on: FIX-12.

- [x] **FIX-16 · P1 · L · Keep editing text and cursor in view.**
  - Evidence: `src/ui.rs:395-410` always starts at line zero and truncates long lines; a 30-line note has an invisible end cursor at 80x24.
  - Change: add vertical and horizontal editing offsets, follow cursor movement/selection, and apply the same offsets to mouse mapping. Add PageUp/PageDown once viewport behavior is defined.
  - Done when: the end of a long note and the end of a long line can be edited visibly, resizing keeps the cursor reachable, and selection/hit-testing still refer to the displayed text.
  - Depends on: FIX-10, FIX-14, and FIX-18.

- [x] **FIX-17 · P1 · M · Make the tab strip and help usable when content overflows.**
  - Evidence: `src/ui.rs:118-159` starts from the first note, does not keep the active tab visible, and counts filtered-out notes as tab overflow. Help contains more rows than a common terminal and has no scrolling/paging.
  - Change: use an active-tab-following window, count only genuinely omitted visible tabs, and make bounded help scrollable or paginated with clear close controls.
  - Done when: navigation always reveals the active tab; overflow counts exclude filtered notes; every help section can be read at 80x24.
  - Depends on: FIX-05 and FIX-12.

- [x] **FIX-18 · P1 · M · Distinguish terminal cells from text offsets.**
  - Evidence: tab/title layout uses `char_len()` as a width; mouse positions are terminal columns. CJK, emoji, and combining text do not all occupy one cell per character.
  - Change: use display-cell width for clipping, padding, and mouse-coordinate conversion. Keep stored editing offsets as character indices and byte conversion at string operations; do not silently convert the whole editor to byte indices.
  - Done when: one wide-character title and one combining-character title have correct centering, tab hit regions, and cursor placement. Account for the terminal's rendering limitations explicitly.
  - Depends on: FIX-03 and FIX-14. This is separate from optional grapheme-aware editing in ADD-08.

- [x] **FIX-19 · P1 · M · Keep editable markdown aligned with its source.**
  - Evidence: `src/ui.rs:395-404,845-887` formats/strips markdown while editing offsets still refer to the raw source. Source offsets and displayed positions diverge.
  - Recommended change: display literal markdown in Content while editing; retain formatted preview outside editing. This is simpler than maintaining a rich-text source-to-display mapping.
  - Done when: a click/selection in `**bold**` edits or copies the source characters shown; moving the cursor through delimiters never changes geometry unexpectedly.
  - Literal-source rendering/keyboard selection landed with FIX-10. FIX-14/18 now verify actual drawn-cell clicks and delimiter selection in inline/overlay editing, completing the remaining mouse/cell contract.

- [x] **FIX-20 · P1 · S · Align the persisted schema with documented hand-editing.**
  - Evidence: `src/persistence.rs:20-28` requires `font_style`; README's example omits it and loads as zero notes.
  - Change: retain `font_style` for compatibility and define its missing value as `normal`, or correct the example if strict presence is deliberately required. Recommended behavior is the explicit default because the field has no current UI.
  - Done when: the documented example loads one note, existing saved style values survive a round trip, and unsupported required-data errors remain visible through FIX-02. Do not drop the compatibility field.

## 3. Improve operational reliability and tag interaction

- [x] **FIX-21 · P2 · M · Make autosave timing and dirty state truthful.**
  - Evidence: `src/main.rs:95-106` measures time since the last successful save; `src/app.rs:1476` does not timestamp edits. Selection/edit-mode changes also mark dirty even though they are not persisted. Failures retry every loop.
  - Change: define the promised 200ms inactivity debounce using persisted-data edit time; mark dirty only for persisted changes; share the save-result path used by autosave and quit. Use a bounded retry interval and a visible retry action rather than retrying at rendering frequency.
  - Done when: typing saves after inactivity, navigation/select-all causes no write, failed saves retain pending data, and the UI distinguishes unsaved, saving-error, and saved state without erasing unrelated clipboard errors.
  - Depends on: FIX-01 and FIX-07.

- [x] **FIX-22 · P2 · M · Verify clipboard lifetime on supported Linux desktops.**
  - Evidence: `src/app.rs:1578-1586` creates and drops Clipboard per operation. Pinned arboard 3.6.1 documentation states that dropping the last instance can make copied content unavailable on X11/Wayland. This is a documented platform risk, not a locally reproduced Linux failure.
  - Change: test the claimed Linux targets; retain the clipboard owner for the necessary lifetime if required. Continue treating unavailable clipboard access as an ordinary non-destructive error.
  - Done when: copied text can be pasted into another app while Stickynote is running; behavior after exit is documented; macOS behavior and failure handling remain correct.
  - Depends on: FIX-04 and an available Linux test environment.

- [x] **FIX-23 · P2 · M · Complete tag-field interaction.**
  - Evidence: `src/app.rs:1363-1393` treats the tags row as Content; autocomplete uses the first matching tag and a complete matching value can keep Tab in autocomplete instead of advancing focus.
  - Change: route tag-chip/input clicks using FIX-14's layout; make the committed/input/suggestion selection behavior explicit. Recommended behavior is first Tab completes a partial suggestion, then Tab advances focus when completion would not change the input. Preserve Enter's duplicate prevention and case-insensitive matching.
  - Done when: a clicked tag can be selected/deleted without changing Content focus, Tags is keyboard-accessible without getting stuck, and displayed suggestions match the committed value.
  - Completed: shared chip/input hit-testing, selected-chip windows, caret following and exact/partial Tab completion. The visible popup remains inert rather than editing the body underneath it.
  - Depends on: FIX-09 and FIX-14. Confirm the revised Tab behavior because current project instructions describe autocomplete taking precedence whenever a match exists.

## 4. Make verification and distribution dependable

- [x] **QA-01 · P1 · M · Add focused application, rendering, and file regressions.**
  - Evidence: existing tests are limited to `src/note.rs` and `src/persistence.rs`; `test_load_board_never_panics` reads the user's real default board.
  - Change: turn the confirmed reproduction sequences into repository tests as each fix lands, use Ratatui TestBackend for bounds/layout behavior, and use temporary files plus narrow failure injection for persistence/clipboard checks. Replace the home-directory-dependent test.
  - Done when: tests never touch the user's board and fail on the original observed defects. Keep coverage tied to actual changed contracts; no new generic test framework is needed.

- [x] **QA-02 · P2 · S/M · Declare and verify the real supported toolchain/platforms.**
  - Evidence: README says Rust 1.85+, while current locked `image`, `darling`, and `instability` require 1.88; Cargo lacks `rust-version`.
  - Change: declare a verified minimum of at least 1.88, align documentation, use the lockfile in CI, and run the relevant behavior checks on Linux and macOS if both are supported. Do not claim Windows clipboard/terminal compatibility without checking it.
  - Done when: the declared minimum toolchain builds the locked project and the supported-platform list reflects observed behavior. Broader platform expansion is optional.

- [x] **QA-03 · P2 · M · Make releases explicit and resumable.**
  - Evidence: `.github/workflows/release.yml:34-42` checks HEAD against HEAD~1; publication at :57-72 cannot resume cleanly after crates.io succeeds and a later step fails.
  - Recommended change: release from an explicit version tag or manual release request with version/tag checks, rather than inspecting the last two commits. Check each destination's existing version/release before proceeding; mismatch must stop rather than be ignored.
  - Done when: a bump followed by another commit is not missed, an interrupted release can continue safely, and an existing version cannot publish different source. Actual publication still requires user authorization.
  - Depends on: the selected P0/P1 fixes and QA-01. Choose the trigger before changing the workflow.

- [x] **QA-04 · P2 · S · Repair Homebrew verification and artifact download checks.**
  - Evidence: `stickynote.rb:15` expects version text from `--help`, which is absent; workflow :80 hashes HTTP responses without failing on HTTP errors.
  - Change: assert `--version`; fail/retry failed tarball downloads and verify the intended artifact before computing its checksum. Check the separately maintained tap against the selected release.
  - Done when: the local formula test matches actual CLI output and HTTP errors cannot produce a committed formula checksum. Test/install or tap publication must be explicitly authorized.
  - Depends on: QA-03 for the final release path.

- [x] **QA-05 · P2 · S · Narrow CI publication authority and check dependencies.**
  - Evidence: workflow uses mutable action refs and relies on unspecified repository token defaults; no dependency-advisory scan has been run.
  - Change: use explicit minimal job permissions, immutable action SHAs, and a narrowly scoped tap credential. Run a dependency advisory check and disposition actual findings rather than upgrading every package blindly.
  - Done when: PR checks have no publication authority, release jobs have only required permissions, credential handling stays out of tracked files, and the advisory result is recorded. No direct fork-PR secret exposure was observed in this audit.

- [x] **QA-06 · P2 · S · Reconcile documentation, help, licensing, and project instructions.**
  - Evidence: README claims preserved entry focus/cursors, hover-specific markdown, and context-menu duplication that do not match current behavior. AGENTS.md describes workflow branches as main although the workflow uses master. No LICENSE file is tracked despite MIT metadata.
  - Change: document actual accepted behavior and keybindings, correct the JSON example/MSRV, include the intended license text, and update AGENTS.md for verified conventions after implementation. Decide whether `none` should mean no border or be renamed plain; it currently renders a bordered Plain block.
  - Done when: examples and documented commands are executable, help agrees with dispatch, licensing is complete, and compatibility fields/invariants are accurately described. Keep reset-on-entry cursor behavior unless explicitly choosing to change it.

## 5. Approved functionality

These additions were originally proposals, then included by the Captain's instruction to complete the remaining list. Their initial scope and acceptance criteria remain below; all are implemented.

- [x] **ADD-01 · Recommended · L · Add bounded undo/redo.**
  - Value: recover accidental edits, deletes, tag clearing, and reordering, including immediate mouse/context-menu deletion.
  - Initial scope: in-memory board changes with bounded history, grouped typing, one action for a paste, and a clear redo-invalidation rule. Restoring a change marks persisted data dirty. Do not persist an unlimited edit history.
  - Done when: an accidental note deletion and a text replacement can be undone/redone without corrupting selection, filter, or saved state.
  - Depends on: FIX-09, FIX-10, FIX-12, and FIX-21. Choose shortcuts that do not conflict with terminal behavior.

- [x] **ADD-02 · Recommended · M · Add full-text search and a note jump list.**
  - Value: find notes by title/body as well as exact tags, which is all `/` currently supports.
  - Initial scope: case-insensitive search over title, content, and tags, a visible result list, and next/previous result navigation. Reuse one visibility/selection model instead of introducing independent filter paths. No indexing service or embeddings.
  - Done when: a phrase in a note body finds that note, selecting a result reveals it, and clearing the query restores ordinary navigation.
  - Depends on: FIX-12 and FIX-17. Define how text queries combine with tag filters before implementation.

- [x] **ADD-03 · Recommended · M · Support safe terminal paste.**
  - Value: terminal paste should behave like a single text insertion, not a stream of command keystrokes; `src/main.rs` currently neither enables bracketed paste nor handles Event::Paste.
  - Initial scope: bracketed-paste setup/cleanup and one shared insertion operation for terminal/system-clipboard text. Preserve newlines in Content, define a single-line policy for Header/Tags, and never execute pasted text as normal-mode commands.
  - Done when: pasted text containing `q`, `n`, newlines, or escape characters cannot quit or operate on notes unexpectedly; selection replacement works; invalid controls are filtered without destroying ordinary Unicode.
  - Depends on: FIX-08, FIX-09, FIX-10, and FIX-11.

- [x] **ADD-04 · Recommended · M · Keep one last-known-good backup and an explicit restore path.**
  - Value: recover from accidental whole-board edits or external corruption after atomic writes are fixed.
  - Initial scope: one board-scoped backup of a validated previous save and a documented restore action. Do not overwrite the recovery copy with a failed load or partial save; never restore silently over the user's file.
  - Done when: the previous board can be inspected/restored, a corrupt input does not destroy the backup, and custom board paths have independent recovery files.
  - Depends on: FIX-01, FIX-02, and FIX-06. Confirm retention and restore UX before implementation.

- [x] **ADD-05 · Useful · M · Export a selected note or visible notes.**
  - Value: turn working notes into portable Markdown/text without reconstructing them by hand.
  - Initial scope: explicit output path, title/body/tags, predictable ordering, and no silent overwrite. The existing whole-board JSON already remains the native backup format.
  - Done when: exporting the filtered view includes only matching notes and produces readable Unicode text without editing the board.
  - Depends on: FIX-12 and QA-06. Choose formats before adding flags.

- [x] **ADD-06 · Useful · M · Add CLI quick capture.**
  - Value: append a note from a shell command or stdin without opening the TUI.
  - Initial scope: a small `add` command with body, optional title/tags, and the existing `--board` selection. Reuse the same persistence validation/locking rather than creating a second writer implementation.
  - Done when: piped multiline text becomes one note, errors have nonzero exit status, and an active writer/conflicting file is respected.
  - Depends on: FIX-01, FIX-02, FIX-06, FIX-20, and QA-02. Preserve ordinary no-argument TUI startup.

- [x] **ADD-07 · Optional · M · Add a board picker for frequent multi-board use.**
  - Value: switch between existing board files without restarting with a different `--board` argument.
  - Implemented scope: choose an existing file and display the active board path. Validate/lock the destination, save current work, then replace the session; any failure retains the old board and its lock. No second board-storage registry.
  - Done when: work/personal boards cannot overwrite each other and a failed switch preserves the current session.
  - Depends on: FIX-02, FIX-06, and FIX-07. Implemented as the `B` path prompt, preserving `--board` startup.

- [x] **ADD-08 · Optional · M · Add word movement and grapheme-aware text editing.**
  - Value: efficient editing of longer notes and correct user-perceived movement through combined emoji/combining marks.
  - Initial scope: tested word navigation/deletion and movement between grapheme boundaries, translated back to existing character-index storage. Review portable terminal shortcuts before choosing bindings.
  - Done when: the chosen word commands behave consistently and editing a combined grapheme does not strand half of a user-perceived character.
  - Depends on: FIX-09, FIX-10, and FIX-18. Do not confuse this with the mandatory UTF-8 crash fix.

- [x] **ADD-09 · Optional · L · Add soft-wrapped reading.**
  - Value: read paragraphs without horizontal truncation or manually inserted newlines.
  - Initial scope: wrapped non-editing preview first. Keep source text unchanged; add wrapped editing only if source-to-screen mapping is explicitly implemented and tested.
  - Done when: resizing reflows the preview without altering stored content and switching into editing preserves cursor/selection correctness.
  - Depends on: FIX-14, FIX-16, FIX-18, and FIX-19. Do not bolt wrapping onto the current fixed-offset mouse model.

## Recommended delivery sequence

1. **Safety pass:** FIX-01 through FIX-08, with their reproductions becoming real tests. Include bounds regressions for ordinary help/tab usage before another release.
2. **Existing-behavior pass:** FIX-09 through FIX-20. Coordinate FIX-14 and FIX-18 before viewport work; avoid independent geometry patches. Land localized fixes without waiting for a grand refactor.
3. **Operational/distribution pass:** FIX-21 through FIX-23 and QA-01 through QA-06. QA-01 progresses alongside earlier fixes, not only here.
4. **First feature pass, if approved:** ADD-01 undo/redo, ADD-02 search, ADD-03 paste, and ADD-04 recovery backup. Each is independently useful; do not make delivery depend on all four.
5. **Later convenience work:** select from ADD-05 through ADD-09 based on actual daily use.

For changed Rust behavior, discover/select the relevant focused tests first, then use the repository gates proportionately:

```bash
cargo test --locked
cargo clippy --locked -- -D warnings
cargo fmt --check
cargo build --release --locked
```

Use a focused release-mode renderer check for the confirmed bounds crashes. Implementation of this list is complete. Publication, installing a tap artifact, dependency upgrades and any additional feature scope remain separate decisions.

## Design guardrails

- Keep `InputMode` for prompts and `EditFocus` for editing sections; do not recreate a boolean mode maze.
- Keep flat-string key dispatch, but validate character input at its boundary.
- Keep character-index cursor storage and explicit byte conversion. Add terminal-cell/grapheme mapping only where required.
- Use one selection/visibility rule, one layout calculation, and one checked save path.
- Retain persisted `font_style` compatibility; do not remove it as dead code.
- Do not introduce stable note IDs, schema migrations, or generic command/history frameworks until a selected feature actually needs them.
- Avoid speculative optimization. The observed problems are correctness and UX, not demonstrated performance limits.
- Defer archive/pin/reminder semantics until the desired note lifecycle is clear. Defer cloud sync, SQLite, accounts, encryption, and plugin infrastructure unless requirements change.
