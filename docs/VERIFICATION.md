# Reviewed working-tree verification

Verified 2026-10-07 at package version 0.3.1, following the audit of commit `3905763`. This records local verification, not a published release. All 38 roadmap tasks are implemented; historical checkpoint counts in ROADMAP.md describe earlier stages.

## Final checks

The main session checked the managed workers' changes, resolved the review findings, and tested a frozen post-repair source tree in separate build targets.

| Environment | Observed result |
|---|---|
| macOS aarch64, Rust/Cargo 1.99 | 127 unit tests and 4 real-binary CLI tests; Clippy with warnings denied; formatting; locked release build |
| macOS aarch64, official checksum-verified Rust/Cargo 1.88 | Same 131 Rust tests and locked release build |
| Linux aarch64, digest-pinned official Rust 1.88 Debian image | Same 131 Rust tests; Clippy with warnings denied; formatting; locked release build |
| macOS and Linux release builds | 28 UI tests and 8 layout tests |
| macOS and Linux Python | 7 mocked release-helper tests, including interrupted publication, retry and source/checksum mismatch refusal |
| Local formula/metadata | Ruby syntax, CLI `--version`, workflow YAML/permissions and package/license inclusion checked |

The Linux image used disposable boards and Xvfb, with all container capabilities dropped and no real home board or host clipboard mounted. A separate X11 process read copied text while the TUI remained open. Clipboard ownership survived switching boards, cut, undo/redo and saving; both native boards contained the expected independent data. Exit restored the pseudo-terminal attributes. These checks passed again after the review repairs.

Earlier macOS pseudo-terminal checks exercised corrupt startup, competing writers, Unicode title entry, narrow tabs/help, failed-save retry/discard, mouse placement, filtering and scrolled editing. Final Rust tests cover those contracts and all subsequent feature paths.

## Review findings and repairs

A read-only reviewer examined the cumulative Rust, release-script, workflow and documentation changes. It found three issues in the final feature integration:

1. Switching to the active `.bak` loaded a destination that the old board's save could overwrite. `check_board_path()` now reserves persistence suffixes at requested/canonical board admission and before switching. Backups are recovery files, not editable boards.
2. Export reservations compared missing names lexically, allowing case/normalization aliases of a native board or backup. `BoardStore::export()` now owns admission, rejects sidecar suffixes and consults existing destination lock markers through filesystem lookup/exclusive locking. It creates no markers for ordinary exports and keeps any acquired marker until no-clobber publication.
3. Ignored key-release events broke consecutive typing undo groups. Dispatch now rejects them before history capture.

Four focused regressions failed before those repairs and passed afterward. Native filesystem probes confirmed that the macOS case-alias and combined Unicode-normalization/case branches actually executed. A Kelvin-sign spelling of `.bak` resolved to the same backup entry and is covered by suffix validation. The follow-up review returned `verdict: ok` with no remaining findings; it did not run commands. The main session ran the post-repair checks above.

## Dependency disposition

The advisory check identified two `lru` 0.12.5 soundness advisories and the unmaintained `paste` warning. The affected LRU mutable-iteration and panic-unsafe `pop` APIs are not used by the app's Ratatui layout-cache path. Its keys have non-panicking equality/hash implementations. Existing locked dependency versions remain; this is an exposure assessment, not an advisory-free claim. A future upstream migration should repeat the check.

## Limits and authority

- Wayland, Windows runtime and post-exit clipboard persistence are unverified. Linux clipboard managers may retain contents after exit; Stickynote does not promise that.
- File sync plus atomic replacement does not promise parent-directory/full power-loss durability. Advisory locks cannot serialize an editor that ignores them; close the app before manual edits.
- History is session-only, bounded to 100 snapshots and an estimated 8 MiB. It is not a hard process-memory limit or a persistent archive.
- Reading wraps; editing remains unwrapped source. Tiny focused panes require resizing.
- Release helpers passed local mocked fixtures. Hosted workflow execution, live publication and actual registry/tap credential scopes need an authorized release. Maintainers must restrict the registry token to this crate and the tap PAT to its intended repository.
- No commit, staging, push, tag, tap mutation, installed-binary replacement or release occurred. The formula keeps the actual published 0.3.0 archive; the local 0.3.1 working tree is not an installation/publication claim.
- Temporary Rust 1.88 tools and disposable Linux images were used for checks. Installed host toolchains/configuration were not replaced. Docker Desktop was started for the local container checks.

## Evidence retention

Detailed source snapshots, reproduction harnesses and logs are under `/tmp/stickynote-audit.WQRVRu/`. Post-repair supervised-job logs are under `~/.pi/agent/long-jobs/` with labels for reviewed native gates, Linux MSRV/X11, and macOS minimum Rust. These temporary artifacts may disappear; this document and ROADMAP.md retain the results and limits. Managed review results with no prose are available in their run's structured-output record, not inferred from an empty text artifact.
