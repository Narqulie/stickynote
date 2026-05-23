// stickynote — a terminal-based sticky notes board.
//
// Keep a stack of coloured sticky notes, type into them,
// tag and filter them, navigate with keyboard or mouse,
// and persist everything to ~/.stickynote/board.json.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event, poll, read};
use crossterm::execute;
use ratatui::DefaultTerminal;

mod app;
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
    #[arg(short, long)]
    board: Option<PathBuf>,

    /// Starting theme: dark, light, or mono
    #[arg(short, long, value_parser = ["dark", "light", "mono"])]
    theme: Option<String>,
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    // Load persisted board state (custom path or default).
    let save_data = if let Some(ref path) = cli.board {
        persistence::load_board_from(path)
    } else {
        persistence::load_board()
    };
    let mut app = app::App::new(save_data);
    app.board_path = cli.board;

    // Override theme from CLI flag.
    if let Some(ref theme_name) = cli.theme
        && let Some(idx) = note::THEMES
            .iter()
            .position(|t| t.name == theme_name.as_str())
    {
        app.theme_idx = idx;
    }

    // Initialise terminal (raw mode + alternate screen).
    let mut terminal = ratatui::init();
    execute!(io::stdout(), EnableMouseCapture)?;

    let result = run_app(&mut terminal, &mut app);

    // Cleanup: disable mouse capture and restore terminal.
    execute!(io::stdout(), DisableMouseCapture)?;
    ratatui::restore();

    // Flush any pending save on exit.
    app.flush_save();

    result
}

fn run_app(terminal: &mut DefaultTerminal, app: &mut app::App) -> io::Result<()> {
    let save_threshold = Duration::from_millis(200);

    while !app.should_quit {
        // Render the current state.
        terminal.draw(|frame| ui::render(app, frame))?;

        // Poll for events with a short timeout so we can check save debounce.
        if poll(Duration::from_millis(50))? {
            match read()? {
                Event::Key(key) => app.handle_key(key),
                Event::Mouse(mouse) => app.handle_mouse(mouse),
                Event::Resize(w, h) => {
                    app.width = w;
                    app.height = h;
                }
                _ => {}
            }
        }

        // Debounced auto-save: write to disk if dirty and enough time has passed.
        if app.dirty && app.last_save.elapsed() >= save_threshold {
            let data = app.to_save_data();
            persistence::save_board(&data, app.board_path.as_ref());
            app.dirty = false;
            app.last_save = std::time::Instant::now();
        }
    }

    Ok(())
}
