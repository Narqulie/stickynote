use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn run(path: &Path, args: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_stickynote"))
        .arg("--board")
        .arg(path)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn cli_version_matches_formula_contract_without_opening_or_creating_a_board() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("must-not-open.json");
    let output = run(&path, &["--version"], b"");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        format!("stickynote {}", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn cli_capture_preserves_multiline_unicode_order_title_and_normalized_repeatable_tags() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("board.json");
    let output = run(&path, &["add", "first"], b"");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let first = fs::read(&path).unwrap();
    let output = run(
        &path,
        &[
            "add", "--title", "标题", "--tag", " Work ", "--tag", "WORK", "--tag", "todo",
        ],
        "é🙂\nline two\n".as_bytes(),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let data: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(data["notes"][0]["content"], "é🙂\nline two\n");
    assert_eq!(data["notes"][0]["title"], "标题");
    assert_eq!(
        data["notes"][0]["tags"],
        serde_json::json!(["work", "todo"])
    );
    assert_eq!(data["notes"][1]["content"], "first");
    assert_eq!(fs::read(dir.path().join("board.json.bak")).unwrap(), first);
}

#[test]
fn cli_restore_requires_confirmation_and_recovers_corruption_without_rotating_good_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("custom.json");
    assert!(run(&path, &["add", "good"], b"").status.success());
    let good = fs::read(&path).unwrap();
    assert!(run(&path, &["add", "later"], b"").status.success());
    fs::write(&path, b"corrupt").unwrap();
    let refused = run(&path, &["restore"], b"");
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--yes"));
    assert_eq!(fs::read(&path).unwrap(), b"corrupt");
    assert!(run(&path, &["restore", "--yes"], b"").status.success());
    assert_eq!(fs::read(&path).unwrap(), good);
    assert_eq!(fs::read(dir.path().join("custom.json.bak")).unwrap(), good);
}

#[test]
fn cli_invalid_utf8_corruption_and_live_writer_are_nonzero_and_non_destructive() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("board.json");
    assert!(!run(&path, &["add"], &[0xff]).status.success());
    assert!(!path.exists());
    assert!(run(&path, &["add", "good"], b"").status.success());
    let good = fs::read(&path).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.path().join("board.json.lock"))
        .unwrap();
    fs2::FileExt::try_lock_exclusive(&lock).unwrap();
    assert!(!run(&path, &["add", "blocked"], b"").status.success());
    assert!(!run(&path, &["restore", "--yes"], b"").status.success());
    assert_eq!(fs::read(&path).unwrap(), good);
    drop(lock);
    fs::write(&path, b"broken").unwrap();
    assert!(
        !run(&path, &["add", "must not overwrite"], b"")
            .status
            .success()
    );
    assert_eq!(fs::read(&path).unwrap(), b"broken");
}
