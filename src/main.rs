// stickynote — a terminal-based sticky notes board.
//
// Keep a stack of coloured sticky notes, type into them,
// tag and filter them, navigate with keyboard or mouse,
// and persist everything to ~/.stickynote/board.json.

use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    poll, read,
};
use crossterm::execute;
use ratatui::DefaultTerminal;

mod app;
mod layout;
mod note;
mod persistence;
mod ui;

#[derive(Parser)]
#[command(
    name = "stickynote",
    version,
    about = "A terminal-based sticky notes board"
)]
struct Cli {
    /// Path to a custom board file (default: ~/.stickynote/board.json)
    #[arg(short, long, global = true)]
    board: Option<PathBuf>,

    /// Starting theme: dark, light, or mono
    #[arg(short, long, value_parser = ["dark", "light", "mono"])]
    theme: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Insert a note at the top without opening the TUI
    Add {
        /// Body text; if omitted, read piped UTF-8 stdin
        body: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long = "tag")]
        tags: Vec<String>,
    },
    /// Explicitly restore the validated .bak copy (also works for corrupt boards)
    Restore {
        /// Required confirmation: replaces the current board, not the backup
        #[arg(long, required = true)]
        yes: bool,
    },
}

fn run_command(
    command: Command,
    path: PathBuf,
    stdin: &mut impl Read,
    piped: bool,
) -> Result<(), String> {
    match command {
        Command::Restore { yes: true } => persistence::BoardStore::restore(path),
        Command::Restore { yes: false } => Err("restore requires --yes".into()),
        Command::Add { body, title, tags } => {
            let content = match body {
                Some(body) => body,
                None if piped => {
                    let mut body = String::new();
                    stdin
                        .read_to_string(&mut body)
                        .map_err(|e| format!("cannot read UTF-8 stdin: {e}"))?;
                    body
                }
                None => return Err("add requires body text or piped UTF-8 stdin".into()),
            };
            let (mut board, data) = persistence::BoardStore::open(path)?;
            let theme_idx = data.theme_idx;
            let mut notes = data.into_notes();
            let mut note = note::Note {
                content,
                title: title.unwrap_or_default(),
                ..note::Note::new()
            };
            for tag in tags {
                let tag = note::Note::normalize_tag(&tag);
                if !tag.is_empty() && !note.has_tag(&tag) {
                    note.tags.push(tag);
                }
            }
            notes.insert(0, note);
            board.save(&persistence::SaveData::from_notes(&notes, theme_idx))
        }
    }
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();
    let path = cli.board.unwrap_or_else(persistence::board_path);
    if let Some(command) = cli.command {
        if cli.theme.is_some() {
            return Err(io::Error::other("--theme is only supported for the TUI"));
        }
        let mut stdin = io::stdin();
        let piped = !stdin.is_terminal();
        return run_command(command, path, &mut stdin, piped).map_err(io::Error::other);
    }
    let (board, save_data) = persistence::BoardStore::open(path).map_err(io::Error::other)?;
    let mut app = app::App::new(save_data, board);

    if let Some(ref theme_name) = cli.theme
        && let Some(idx) = note::THEMES
            .iter()
            .position(|t| t.name == theme_name.as_str())
        && app.theme_idx != idx
    {
        app.theme_idx = idx;
        app.mark_dirty();
    }

    finish_run(&mut app, run_terminal)
}

/// Flush after an abnormal exit, but never resave after a confirmed discard.
fn finish_run(
    app: &mut app::App,
    run: impl FnOnce(&mut app::App) -> io::Result<()>,
) -> io::Result<()> {
    match run(app) {
        Ok(()) => Ok(()),
        Err(error) => match app.flush_save() {
            Ok(()) => Err(error),
            Err(save_error) => Err(io::Error::new(
                error.kind(),
                format!("{error}; unsaved changes: {save_error}"),
            )),
        },
    }
}

struct TerminalCleanup;

impl Drop for TerminalCleanup {
    fn drop(&mut self) {
        if let Err(error) = restore_terminal_with(&mut io::stdout(), ratatui::restore) {
            eprintln!("failed to disable terminal input modes: {error}");
        }
    }
}

fn restore_terminal_with(writer: &mut impl Write, restore: impl FnOnce()) -> io::Result<()> {
    let paste = execute!(writer, DisableBracketedPaste);
    let mouse = execute!(writer, DisableMouseCapture);
    let result = paste.and(mouse);
    restore();
    result
}

fn run_terminal(app: &mut app::App) -> io::Result<()> {
    // Covers partial initialization, mouse-capture errors, loop errors, and unwinding.
    let _cleanup = TerminalCleanup;
    let mut terminal = ratatui::try_init()?;
    execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    run_app(&mut terminal, app)
}

fn run_app(terminal: &mut DefaultTerminal, app: &mut app::App) -> io::Result<()> {
    while app.quit_state != app::QuitState::Done {
        terminal.draw(|frame| {
            let area = frame.area();
            app.prepare_frame(area.width, area.height);
            ui::render(app, frame);
        })?;

        if poll(Duration::from_millis(50))? {
            match read()? {
                Event::Key(key) => app.handle_key(key),
                Event::Mouse(mouse) => app.handle_mouse(mouse),
                Event::Paste(text) => app.insert_paste(&text),
                Event::Resize(w, h) => {
                    app.width = w;
                    app.height = h;
                }
                _ => {}
            }
        }

        if app.autosave_due(std::time::Instant::now()) {
            // The checked save path retains dirty state and exposes any error.
            let _ = app.flush_save();
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct BrokenOutput;

    impl Write for BrokenOutput {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "injected output failure",
            ))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn cli_no_arguments_remain_tui_and_capture_without_body_requires_a_pipe() {
        let cli = Cli::try_parse_from(["stickynote"]).unwrap();
        assert!(cli.command.is_none());
        assert!(Cli::try_parse_from(["stickynote", "restore"]).is_err());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("board.json");
        let error = run_command(
            Command::Add {
                body: None,
                title: None,
                tags: vec![],
            },
            path.clone(),
            &mut io::empty(),
            false,
        )
        .unwrap_err();
        assert!(error.contains("piped UTF-8"));
        assert!(!path.exists());
    }

    #[test]
    fn cleanup_disables_both_input_modes_even_if_the_first_write_fails() {
        struct FailOnce {
            failed: bool,
            bytes: Vec<u8>,
        }
        impl Write for FailOnce {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if !self.failed {
                    self.failed = true;
                    return Err(io::Error::other("first write fails"));
                }
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut writer = FailOnce {
            failed: false,
            bytes: Vec::new(),
        };
        let restored = Cell::new(false);
        assert!(restore_terminal_with(&mut writer, || restored.set(true)).is_err());
        assert!(restored.get());
        assert!(
            String::from_utf8(writer.bytes)
                .unwrap()
                .contains("\u{1b}[?1000l")
        );
        let mut bytes = Vec::new();
        restore_terminal_with(&mut bytes, || {}).unwrap();
        let output = String::from_utf8(bytes).unwrap();
        assert!(output.contains("\u{1b}[?2004l"));
        assert!(output.contains("\u{1b}[?1000l"));
    }

    #[test]
    fn terminal_restore_runs_even_when_disabling_mouse_capture_fails() {
        let restored = Cell::new(false);
        let error = restore_terminal_with(&mut BrokenOutput, || restored.set(true)).unwrap_err();
        assert!(restored.get());
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn abnormal_exit_reports_original_error_and_unsaved_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("board.json");
        let (board, data) = persistence::BoardStore::open(path.clone()).unwrap();
        let mut app = app::App::new(data, board);
        app.add_note();
        std::fs::write(path, b"external change").unwrap();
        let error =
            finish_run(&mut app, |_| Err(io::Error::other("injected event error"))).unwrap_err();
        assert!(error.to_string().contains("injected event error"));
        assert!(error.to_string().contains("unsaved changes"));
        assert!(app.dirty);
    }

    #[test]
    fn abnormal_exit_saves_pending_changes_before_reporting_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("board.json");
        let (board, data) = persistence::BoardStore::open(path.clone()).unwrap();
        let mut app = app::App::new(data, board);
        app.add_note();
        app.notes[0].content = "pending note".into();
        assert!(finish_run(&mut app, |_| Err(io::Error::other("event error"))).is_err());
        assert!(!app.dirty);
        drop(app);
        let (_board, data) = persistence::BoardStore::open(path).unwrap();
        assert_eq!(data.notes[0].content, "pending note");
    }
}
