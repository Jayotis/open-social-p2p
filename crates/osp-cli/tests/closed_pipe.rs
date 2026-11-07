//! Output nobody reads any more (`osp feed | head`) ends `osp` the way a
//! closed pipe ends other programs, not with a panic.

use std::process::{Command, Stdio};

#[test]
fn a_reader_that_goes_away_ends_it_quietly() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("unread");
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let out = Command::new(env!("CARGO_BIN_EXE_osp"))
        .arg("--home")
        .arg(&home)
        .args(["init", "--skip-word-check", "--share"])
        .stdin(Stdio::null())
        .stdout(writer)
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(141), "{err}");
    assert!(!err.contains("panicked"), "{err}");
    // The words were the first thing it printed, and nobody saw them.
    assert!(!home.exists());
}
