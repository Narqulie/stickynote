// stickynote — persistence layer: JSON save/load to ~/.stickynote/board.json

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use crate::note::{BORDER_STYLES, NOTE_COLORS, Note};

// ── JSON types ────────────────────────────────────────────────────────────────

/// Top-level serialisable board state.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SaveData {
    pub notes: Vec<SavedNote>,
    pub theme_idx: usize,
}

/// On-disk representation of a single note (transient editing state excluded).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SavedNote {
    pub content: String,
    #[serde(default)]
    pub title: String,
    pub color: String,
    pub font_style: String,
    pub border_style: String,
    pub tags: Vec<String>,
}

// ── Path ──────────────────────────────────────────────────────────────────────

/// Default board file path: `~/.stickynote/board.json`.
pub fn board_path() -> PathBuf {
    let home = dirs::home_dir().expect("could not determine home directory");
    home.join(".stickynote").join("board.json")
}

// ── Load ──────────────────────────────────────────────────────────────────────

/// Load board state from the default path, returning an empty board on error.
pub fn load_board() -> SaveData {
    load_board_from(&board_path())
}

/// Load board state from a custom path, returning an empty board on error.
pub fn load_board_from(path: &Path) -> SaveData {
    if !path.exists() {
        return SaveData {
            notes: Vec::new(),
            theme_idx: 0,
        };
    }
    match fs::File::open(path) {
        Ok(file) => {
            let reader = BufReader::new(file);
            match serde_json::from_reader(reader) {
                Ok(data) => data,
                Err(_) => SaveData {
                    notes: Vec::new(),
                    theme_idx: 0,
                },
            }
        }
        Err(_) => SaveData {
            notes: Vec::new(),
            theme_idx: 0,
        },
    }
}

// ── Save ──────────────────────────────────────────────────────────────────────

/// Save board state to disk, using a custom path or the default.
/// Returns an error string if writing fails.
pub fn save_board(data: &SaveData, custom_path: Option<&PathBuf>) -> Result<(), String> {
    let default = board_path();
    let path: &Path = custom_path
        .map(|p| p.as_path())
        .unwrap_or_else(|| default.as_path());
    save_board_at(data, path)
}

fn save_board_at(data: &SaveData, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let file = fs::File::create(path).map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(file, data).map_err(|e| e.to_string())?;
    Ok(())
}

// ── Conversion ────────────────────────────────────────────────────────────────

impl SaveData {
    /// Convert saved notes into full `Note` instances, applying defaults for
    /// missing or invalid fields.
    pub fn into_notes(self) -> Vec<Note> {
        self.notes
            .into_iter()
            .map(|sn| {
                let color = if !sn.color.is_empty() && NOTE_COLORS.contains(&sn.color.as_str()) {
                    sn.color
                } else {
                    NOTE_COLORS[0].to_string()
                };
                let font_style = match sn.font_style.as_str() {
                    "normal" | "bold" | "italic" => sn.font_style,
                    _ => "normal".to_string(),
                };
                let border_style = if BORDER_STYLES.contains(&sn.border_style.as_str()) {
                    sn.border_style
                } else {
                    "rounded".to_string()
                };
                Note {
                    content: sn.content,
                    tags: sn.tags,
                    title: sn.title,
                    title_cursor: 0,
                    color,
                    font_style,
                    border_style,
                    editing: false,
                    cursor: 0,
                    tag_input: String::new(),
                    tag_cursor: None,
                    sel_start: None,
                    sel_end: None,
                    title_sel_start: None,
                    title_sel_end: None,
                }
            })
            .collect()
    }

    /// Build `SaveData` from a slice of `Note` references.
    pub fn from_notes(notes: &[Note], theme_idx: usize) -> Self {
        SaveData {
            notes: notes
                .iter()
                .map(|n| SavedNote {
                    content: n.content.clone(),
                    title: n.title.clone(),
                    color: n.color.clone(),
                    font_style: n.font_style.clone(),
                    border_style: n.border_style.clone(),
                    tags: n.tags.clone(),
                })
                .collect(),
            theme_idx,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_save_data_round_trip() {
        let notes = vec![
            Note {
                content: "hello world".into(),
                color: NOTE_COLORS[0].to_string(),
                font_style: "bold".into(),
                border_style: "double".into(),
                tags: vec!["urgent".into(), "todo".into()],
                ..Note::new()
            },
            Note {
                content: "second note\nmulti-line".into(),
                color: NOTE_COLORS[3].to_string(),
                font_style: "italic".into(),
                border_style: "hidden".into(),
                tags: vec!["done".into()],
                ..Note::new()
            },
        ];

        let save = SaveData::from_notes(&notes, 1);
        let json = serde_json::to_string(&save).expect("serialize");
        let restored: SaveData = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(restored.theme_idx, 1);
        assert_eq!(restored.notes.len(), 2);
        assert_eq!(restored.notes[0].content, "hello world");
        assert_eq!(restored.notes[0].font_style, "bold");
        assert_eq!(restored.notes[1].content, "second note\nmulti-line");
        assert_eq!(restored.notes[1].color, NOTE_COLORS[3]);
    }

    #[test]
    fn test_into_notes_applies_defaults() {
        let save = SaveData {
            notes: vec![SavedNote {
                content: "ok".into(),
                title: String::new(),
                color: "".into(),
                font_style: "weird".into(),
                border_style: "".into(),
                tags: vec![],
            }],
            theme_idx: 0,
        };

        let notes = save.into_notes();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].color, NOTE_COLORS[0]);
        assert_eq!(notes[0].font_style, "normal");
        assert_eq!(notes[0].border_style, "rounded");
        assert!(!notes[0].editing);
    }

    #[test]
    fn test_load_board_never_panics() {
        let save = load_board(); // reads ~/.stickynote/board.json; may or may not exist
        // Should never panic regardless of whether file exists
        assert!(save.theme_idx < 10); // theme_idx is always sane
    }

    #[test]
    fn test_board_path_is_sensible() {
        let path = board_path();
        assert!(path.to_string_lossy().contains(".stickynote/board.json"));
    }
}
