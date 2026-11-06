// stickynote — persistence layer: JSON save/load to ~/.stickynote/board.json

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::note::{BORDER_STYLES, NOTE_COLORS, Note};

// ── JSON types ────────────────────────────────────────────────────────────────

/// Top-level serialisable board state.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct SaveData {
    pub notes: Vec<SavedNote>,
    pub theme_idx: usize,
}

/// On-disk representation of a single note (transient editing state excluded).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct SavedNote {
    pub content: String,
    #[serde(default)]
    pub title: String,
    pub color: String,
    #[serde(default = "normal_font_style")]
    pub font_style: String,
    pub border_style: String,
    pub tags: Vec<String>,
}

fn normal_font_style() -> String {
    "normal".into()
}

// ── Path ──────────────────────────────────────────────────────────────────────

/// Default board file path: `~/.stickynote/board.json`.
pub fn board_path() -> PathBuf {
    let home = dirs::home_dir().expect("could not determine home directory");
    home.join(".stickynote").join("board.json")
}

// ── Board file ownership ──────────────────────────────────────────────────────

/// Owns the editing-session lock and the last observed on-disk contents.
/// The sidecar lock is stable across atomic replacement of the board itself.
pub struct BoardStore {
    path: PathBuf,
    _lock: fs::File,
    saved_bytes: Option<Vec<u8>>,
}

impl BoardStore {
    /// Open and lock a board before entering the terminal UI.
    /// Only a missing file creates an empty board; all other load errors propagate.
    pub fn open(path: PathBuf) -> Result<(Self, SaveData), String> {
        let store = Self::lock(path)?;
        let data = match store.saved_bytes.as_deref() {
            Some(bytes) => parse_board(bytes, &store.path)?,
            None => SaveData {
                notes: Vec::new(),
                theme_idx: 0,
            },
        };
        Ok((store, data))
    }

    /// Switching never creates a missing destination board.
    pub fn open_existing(path: PathBuf) -> Result<(Self, SaveData), String> {
        check_board_path(&path)?;
        let path = fs::canonicalize(&path)
            .map_err(|e| format!("cannot open existing board {}: {e}", path.display()))?;
        let (store, data) = Self::open(path)?;
        if store.saved_bytes.is_none() {
            return Err("destination board disappeared; switch cancelled".into());
        }
        Ok((store, data))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Recovery locks without accepting the damaged board as editable data.
    /// The validated backup is never replaced with the damaged input.
    pub fn restore(path: PathBuf) -> Result<(), String> {
        let store = Self::lock(path)?;
        let backup = sidecar(&store.path, ".bak");
        let bytes = read_board_bytes(&backup)?
            .ok_or_else(|| format!("no backup exists at {}", backup.display()))?;
        parse_board(&bytes, &backup)?;
        if read_board_bytes(&store.path)? != store.saved_bytes
            || read_board_bytes(&backup)?.as_deref() != Some(bytes.as_slice())
        {
            return Err("board or backup changed outside this session; restore cancelled".into());
        }
        write_atomic(&store.path, |file| file.write_all(&bytes))
            .map_err(|e| format!("cannot restore board {}: {e}", store.path.display()))
    }

    fn lock(path: PathBuf) -> Result<Self, String> {
        check_board_path(&path)?;
        let path = resolve_path(path)?;
        check_board_path(&path)?;
        let lock_path = sidecar(&path, ".lock");
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .map_err(|e| format!("cannot lock board {}: {e}", path.display()))?;
        fs2::FileExt::try_lock_exclusive(&lock).map_err(|e| {
            format!(
                "cannot lock board {} (another instance may be using it): {e}",
                path.display()
            )
        })?;

        let saved_bytes = read_board_bytes(&path)?;
        Ok(Self {
            path,
            _lock: lock,
            saved_bytes,
        })
    }

    /// Refuse stale writes, then replace the board with a complete, synced file.
    /// Non-cooperating external writers are checked, not locked, by this comparison.
    pub fn save(&mut self, data: &SaveData) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(data).map_err(|e| e.to_string())?;
        self.check_unchanged()?;
        if let Some(previous) = self.saved_bytes.as_deref() {
            // Only validated loaded/saved bytes enter the recovery copy.
            parse_board(previous, &self.path)?;
            let backup = sidecar(&self.path, ".bak");
            write_atomic(&backup, |file| {
                file.set_permissions(fs::metadata(&self.path)?.permissions())?;
                file.write_all(previous)
            })
            .map_err(|e| format!("cannot back up board {}: {e}", self.path.display()))?;
        }
        // Backup syncing can take time; recheck before the board replacement.
        self.check_unchanged()?;
        write_atomic(&self.path, |file| file.write_all(&bytes))
            .map_err(|e| format!("cannot save board {}: {e}", self.path.display()))?;
        self.saved_bytes = Some(bytes);
        Ok(())
    }

    fn check_unchanged(&self) -> Result<(), String> {
        if read_board_bytes(&self.path)? != self.saved_bytes {
            return Err(format!(
                "board {} changed outside this session; save cancelled",
                self.path.display()
            ));
        }
        Ok(())
    }
}

/// Recovery and lock files belong to persistence, never another editable board.
pub(crate) fn check_board_path(path: &Path) -> Result<(), String> {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    if name.ends_with(".bak") || name.ends_with(".lock") {
        return Err(format!(
            "reserved persistence sidecar {}: choose a filename without .bak or .lock",
            path.display()
        ));
    }
    Ok(())
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn parse_board(bytes: &[u8], path: &Path) -> Result<SaveData, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("cannot load board {}: {e}", path.display()))
}

fn resolve_path(path: PathBuf) -> Result<PathBuf, String> {
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    match fs::canonicalize(&path) {
        Ok(path) => Ok(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            // Do not replace a broken symlink as if it were a new board.
            if path.is_symlink() {
                return Err(format!("cannot load board {}: {e}", path.display()));
            }
            let parent = path
                .parent()
                .ok_or_else(|| "board path has no parent".to_string())?;
            fs::create_dir_all(parent)
                .map_err(|e| format!("cannot open board {}: {e}", path.display()))?;
            let name = path
                .file_name()
                .ok_or_else(|| "board path has no filename".to_string())?;
            Ok(fs::canonicalize(parent)
                .map_err(|e| e.to_string())?
                .join(name))
        }
        Err(e) => Err(format!("cannot open board {}: {e}", path.display())),
    }
}

fn read_board_bytes(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read board {}: {e}", path.display())),
    }
}

/// Sync file data before renaming. Parent-directory power-loss durability is
/// not promised; the replacement is atomic, and incomplete writes stay temporary.
fn write_atomic(
    path: &Path,
    write: impl FnOnce(&mut fs::File) -> io::Result<()>,
) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("board path has no parent"))?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    match fs::metadata(path) {
        Ok(metadata) => file.as_file().set_permissions(metadata.permissions())?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    write(file.as_file_mut())?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}

// ── Portable exports ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Markdown,
    Text,
}

impl ExportFormat {
    pub fn from_path(path: &Path) -> Result<Self, String> {
        match path.extension().and_then(|extension| extension.to_str()) {
            Some(extension) if extension.eq_ignore_ascii_case("md") => Ok(Self::Markdown),
            Some(extension) if extension.eq_ignore_ascii_case("txt") => Ok(Self::Text),
            _ => Err("use a .md or .txt extension, or Tab to select a format".into()),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Markdown => "Markdown",
            Self::Text => "plain text",
        }
    }
}

/// Create a complete export without replacing any existing file, including
/// symlinks. Native board/sidecar paths are reserved even before the first save.
impl BoardStore {
    pub fn export(&self, path: &Path, notes: &[&Note], format: ExportFormat) -> Result<(), String> {
        if notes.is_empty() {
            return Err("no matching notes to export".into());
        }
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent = fs::canonicalize(parent)
            .map_err(|e| format!("cannot export to {}: {e}", path.display()))?;
        let name = path
            .file_name()
            .ok_or_else(|| "export path needs a filename".to_string())?;
        let destination = parent.join(name);
        check_board_path(&destination)?;
        if destination == self.path {
            return Err("export cannot replace the active board".into());
        }
        // A board's marker exists even before its first save. Filesystem lookup and
        // locking resolve case, normalization and symlink aliases without guessing
        // volume rules. Never create a marker for an ordinary export destination.
        let _destination_lock = match fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(sidecar(&destination, ".lock"))
        {
            Ok(lock) => {
                fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| {
                    format!(
                        "export destination {} is reserved by an active board",
                        destination.display()
                    )
                })?;
                Some(lock)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(format!(
                    "cannot check export destination {}: {error}",
                    destination.display()
                ));
            }
        };
        let mut text = String::new();
        for (index, note) in notes.iter().enumerate() {
            if index > 0 {
                text.push_str("\n---\n\n");
            }
            let title = if note.title.is_empty() {
                "Untitled"
            } else {
                &note.title
            };
            match format {
                ExportFormat::Markdown => {
                    text.push_str(&format!("# {title}\n\n{}\n", note.content))
                }
                ExportFormat::Text => text.push_str(&format!(
                    "{title}\n\n{}\n",
                    crate::note::strip_md(&note.content)
                )),
            }
            if !note.tags.is_empty() {
                text.push_str(&format!("\nTags: {}\n", note.tags.join(", ")));
            }
        }
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        file.as_file().sync_all().map_err(|e| e.to_string())?;
        file.persist_noclobber(&destination).map_err(|e| {
            format!(
                "cannot create export {} (choose a new path): {}",
                destination.display(),
                e.error
            )
        })?;
        Ok(())
    }
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
                    viewport: crate::note::ContentViewport::new(),
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
    fn backup_tracks_only_validated_prior_saves_and_corrupt_restore_keeps_the_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("work.json");
        let backup = sidecar(&path, ".bak");
        let (mut store, _) = BoardStore::open(path.clone()).unwrap();
        let first = SaveData::from_notes(
            &[Note {
                content: "good é🙂".into(),
                ..Note::new()
            }],
            1,
        );
        store.save(&first).unwrap();
        assert!(!backup.exists());
        let original = fs::read(&path).unwrap();
        let second = SaveData::from_notes(
            &[Note {
                content: "second".into(),
                ..Note::new()
            }],
            2,
        );
        store.save(&second).unwrap();
        assert_eq!(fs::read(&backup).unwrap(), original);
        assert!(
            BoardStore::restore(path.clone())
                .unwrap_err()
                .contains("lock")
        );
        fs::write(&path, b"corrupt board").unwrap();
        assert!(store.save(&second).is_err());
        assert_eq!(fs::read(&backup).unwrap(), original);
        drop(store);
        assert!(BoardStore::open(path.clone()).is_err());
        assert_eq!(fs::read(&backup).unwrap(), original);
        BoardStore::restore(path.clone()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read(&backup).unwrap(), original);
        let (store, restored) = BoardStore::open(path.clone()).unwrap();
        assert_eq!(restored, first);
        drop(store);
        fs::remove_file(&path).unwrap();
        BoardStore::restore(path.clone()).unwrap();
        assert_eq!(fs::read(path).unwrap(), original);
        let personal = dir.path().join("personal.json");
        let (mut store, _) = BoardStore::open(personal.clone()).unwrap();
        store.save(&second).unwrap();
        let personal_prior = fs::read(&personal).unwrap();
        store.save(&first).unwrap();
        assert_eq!(
            fs::read(sidecar(&personal, ".bak")).unwrap(),
            personal_prior
        );
        assert_eq!(fs::read(backup).unwrap(), original);
    }

    #[test]
    fn missing_or_invalid_backup_never_authorizes_restore_and_backup_failure_cancels_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("board.json");
        fs::write(&path, b"valuable corrupt input").unwrap();
        assert!(
            BoardStore::restore(path.clone())
                .unwrap_err()
                .contains("no backup")
        );
        let backup = sidecar(&path, ".bak");
        fs::write(&backup, b"also corrupt").unwrap();
        assert!(BoardStore::restore(path.clone()).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"valuable corrupt input");
        assert_eq!(fs::read(&backup).unwrap(), b"also corrupt");
        fs::remove_file(&path).unwrap();
        fs::remove_file(&backup).unwrap();
        let (mut store, _) = BoardStore::open(path.clone()).unwrap();
        let data = SaveData::from_notes(&[Note::new()], 0);
        store.save(&data).unwrap();
        let before = fs::read(&path).unwrap();
        fs::create_dir(&backup).unwrap();
        assert!(
            store
                .save(&SaveData {
                    notes: vec![],
                    theme_idx: 1
                })
                .unwrap_err()
                .contains("back up")
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(backup.is_dir());
    }

    #[test]
    fn persistence_sidecars_cannot_be_admitted_as_editable_boards() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = serde_json::to_vec(&SaveData::from_notes(&[Note::new()], 0)).unwrap();
        for suffix in [".bak", ".BAK", ".baK", ".lock", ".LOCK"] {
            let path = dir.path().join(format!("board.json{suffix}"));
            fs::write(&path, &bytes).unwrap();
            assert!(BoardStore::open(path.clone()).is_err());
            assert!(BoardStore::open_existing(path.clone()).is_err());
            assert!(BoardStore::restore(path.clone()).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert!(!sidecar(&path, ".lock").exists());
        }
        #[cfg(unix)]
        {
            let alias = dir.path().join("alias.json");
            std::os::unix::fs::symlink(dir.path().join("board.json.bak"), &alias).unwrap();
            assert!(BoardStore::open(alias).is_err());
        }
    }

    #[test]
    fn exports_reserve_active_native_and_sidecar_filesystem_names_before_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("board.json");
        let (mut store, _) = BoardStore::open(path.clone()).unwrap();
        let note = Note::new();
        let alias = dir.path().join("BOARD.JSON");
        if sidecar(&alias, ".lock").exists() {
            assert!(store.export(&alias, &[&note], ExportFormat::Text).is_err());
            assert!(!path.exists());
        }
        let unicode_path = dir.path().join("bóard.json");
        let (unicode_store, _) = BoardStore::open(unicode_path.clone()).unwrap();
        let unicode_alias = dir.path().join("BO\u{301}ARD.JSON");
        if sidecar(&unicode_alias, ".lock").exists() {
            assert!(
                unicode_store
                    .export(&unicode_alias, &[&note], ExportFormat::Text)
                    .is_err()
            );
            assert!(!unicode_path.exists());
        }
        for suffix in [".bak", ".BAK", ".baK", ".lock", ".LOCK"] {
            let alias = dir.path().join(format!("BOARD.JSON{suffix}"));
            assert!(store.export(&alias, &[&note], ExportFormat::Text).is_err());
        }
        store.save(&SaveData::from_notes(&[note], 0)).unwrap();
        assert!(!sidecar(&path, ".bak").exists());
        let (other, _) = BoardStore::open(dir.path().join("other.json")).unwrap();
        assert!(
            store
                .export(other.path(), &[&Note::new()], ExportFormat::Text)
                .is_err()
        );
        assert!(!other.path().exists());
    }

    #[test]
    fn exports_unicode_in_requested_format_in_order_without_overwrite_or_board_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("native.md");
        let (store, _) = BoardStore::open(board.clone()).unwrap();
        let notes = [
            Note {
                title: "标题".into(),
                content: "**bold** é🙂\nnext".into(),
                tags: vec!["todo".into()],
                ..Note::new()
            },
            Note {
                content: "second".into(),
                ..Note::new()
            },
        ];
        let refs: Vec<_> = notes.iter().collect();
        let md = dir.path().join("notes.md");
        store.export(&md, &refs, ExportFormat::Markdown).unwrap();
        assert_eq!(
            fs::read_to_string(&md).unwrap(),
            "# 标题\n\n**bold** é🙂\nnext\n\nTags: todo\n\n---\n\n# Untitled\n\nsecond\n"
        );
        assert!(store.export(&md, &refs, ExportFormat::Text).is_err());
        assert!(fs::read_to_string(&md).unwrap().contains("**bold**"));
        let txt = dir.path().join("notes.txt");
        store.export(&txt, &refs, ExportFormat::Text).unwrap();
        assert!(fs::read_to_string(txt).unwrap().contains("bold é🙂"));
        assert!(store.export(&board, &refs, ExportFormat::Text).is_err());
        assert!(!board.exists());
        assert!(ExportFormat::from_path(Path::new("notes.MD")).is_ok());
        assert!(ExportFormat::from_path(Path::new("notes.json")).is_err());
        assert!(
            store
                .export(&dir.path().join("empty.txt"), &[], ExportFormat::Text)
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn backup_preserves_board_permissions_and_export_refuses_existing_or_dangling_symlinks() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("board.json");
        let (mut store, _) = BoardStore::open(path.clone()).unwrap();
        let data = SaveData::from_notes(&[Note::new()], 0);
        store.save(&data).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        store.save(&data).unwrap();
        assert_eq!(
            fs::metadata(sidecar(&path, ".bak"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let good = fs::read(&path).unwrap();
        let note = Note::new();
        let alias = dir.path().join("alias.txt");
        symlink(&path, &alias).unwrap();
        assert!(store.export(&alias, &[&note], ExportFormat::Text).is_err());
        assert!(alias.is_symlink());
        assert_eq!(fs::read(&path).unwrap(), good);
        let dangling = dir.path().join("dangling.txt");
        symlink(dir.path().join("missing.txt"), &dangling).unwrap();
        assert!(
            store
                .export(&dangling, &[&note], ExportFormat::Text)
                .is_err()
        );
        assert!(dangling.is_symlink());
        assert!(!dir.path().join("missing.txt").exists());
    }

    #[test]
    fn documented_json_loads_and_missing_font_style_defaults_without_relaxing_required_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("example.json");
        let example = include_str!("../README.md")
            .split("```json\n")
            .nth(1)
            .unwrap()
            .split("```")
            .next()
            .unwrap();
        let mut json: serde_json::Value = serde_json::from_str(example).unwrap();
        json["notes"][0]
            .as_object_mut()
            .unwrap()
            .remove("font_style");
        fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        let (mut board, data) = BoardStore::open(path.clone()).unwrap();
        assert_eq!(data.notes.len(), 1);
        assert_eq!(data.notes[0].font_style, "normal");
        board.save(&data).unwrap();
        drop(board);
        let (board, data) = BoardStore::open(path.clone()).unwrap();
        assert_eq!(data.notes[0].font_style, "normal");
        drop(board);
        json["notes"][0].as_object_mut().unwrap().remove("content");
        let invalid = serde_json::to_vec(&json).unwrap();
        fs::write(&path, &invalid).unwrap();
        assert!(
            BoardStore::open(path.clone())
                .err()
                .unwrap()
                .contains("content")
        );
        assert_eq!(fs::read(path).unwrap(), invalid);
    }

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
    fn missing_board_starts_empty_without_writing_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("board.json");
        let (_store, data) = BoardStore::open(path.clone()).unwrap();
        assert!(data.notes.is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn failed_load_preserves_file_and_releases_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("board.json");
        let invalid = b"{\"notes\":[{\"content\":\"valuable note\"}]";
        fs::write(&path, invalid).unwrap();
        for _ in 0..2 {
            let error = BoardStore::open(path.clone()).err().unwrap();
            assert!(error.contains("cannot load board"));
            assert!(error.contains(path.to_str().unwrap()));
            assert_eq!(fs::read(&path).unwrap(), invalid);
        }
    }

    #[test]
    fn unreadable_board_is_not_an_empty_board() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("board.json");
        fs::create_dir(&path).unwrap();
        let error = BoardStore::open(path).err().unwrap();
        assert!(error.contains("cannot read board"));
    }

    #[test]
    fn failed_atomic_write_keeps_original_and_removes_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("board.json");
        fs::write(&path, b"previous board").unwrap();
        let result = write_atomic(&path, |file| {
            file.write_all(b"partial replacement")?;
            Err(io::Error::other("injected disk failure"))
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"previous board");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn saves_round_trip_and_lock_survives_atomic_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("board.json");
        let (mut store, _) = BoardStore::open(path.clone()).unwrap();
        let note = Note {
            title: "é 🦀".into(),
            content: "saved body".into(),
            ..Note::new()
        };
        store.save(&SaveData::from_notes(&[note], 2)).unwrap();
        assert!(BoardStore::open(path.clone()).is_err());
        drop(store);
        let (_store, data) = BoardStore::open(path).unwrap();
        assert_eq!(data.theme_idx, 2);
        assert_eq!(data.notes[0].title, "é 🦀");
        assert_eq!(data.notes[0].content, "saved body");
    }

    #[test]
    fn locks_are_board_scoped_and_aliases_share_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("board.json");
        let (_first, _) = BoardStore::open(path.clone()).unwrap();
        let alias = dir.path().join(".").join("board.json");
        assert!(BoardStore::open(alias).is_err());
        assert!(BoardStore::open(dir.path().join("other.json")).is_ok());
    }

    #[test]
    fn external_changes_are_preserved_and_cancel_the_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("board.json");
        let (mut store, _) = BoardStore::open(path.clone()).unwrap();
        let data = SaveData::from_notes(&[Note::new()], 0);
        store.save(&data).unwrap();
        fs::write(&path, b"external replacement").unwrap();
        assert!(store.save(&data).unwrap_err().contains("changed outside"));
        assert_eq!(fs::read(&path).unwrap(), b"external replacement");
    }

    #[test]
    fn viewport_offsets_are_transient_and_reset_on_load() {
        let mut note = Note::new();
        note.content = "unsaved viewport state".into();
        note.viewport.top_line = 50;
        note.viewport.left_cell = 100;
        let saved = SaveData::from_notes(&[note], 0);
        let value = serde_json::to_value(&saved).unwrap();
        assert!(value["notes"][0].get("viewport").is_none());
        let loaded = saved.into_notes();
        assert_eq!(loaded[0].viewport.top_line, 0);
        assert_eq!(loaded[0].viewport.left_cell, 0);
    }

    #[test]
    fn test_board_path_is_sensible() {
        let path = board_path();
        assert!(path.to_string_lossy().contains(".stickynote/board.json"));
    }
}
