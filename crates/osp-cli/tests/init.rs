//! `osp init` asks whether to share prefetched copies (ADR 0012) and whether
//! to be listed by directories (ADR 0015), and creates nothing without an
//! answer. A new node has Tor on unless told otherwise.

use std::{
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

use osp_node::{Config, Home};

/// Runs `osp --home <home> init <args>`, typing `input`.
fn osp_init(home: &Path, args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_osp"))
        .arg("--home")
        .arg(home)
        .arg("init")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

/// The config of the node made in `home`.
fn config(home: &Path) -> Config {
    Config::load(&Home::new(home).config()).unwrap()
}

/// Whether the node made in `home` shares what it prefetches.
fn shares(home: &Path) -> bool {
    config(home).swarm.share_prefetched
}

#[test]
fn init_asks_whether_to_share_and_needs_an_answer() {
    let dir = tempfile::tempdir().unwrap();
    let new = ["--skip-word-check", "--no-list-me"];

    // No answer: nothing is made, and the words were never shown.
    let home = dir.path().join("none");
    let out = osp_init(&home, &new, "");
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--share or --no-share"), "{err}");
    assert!(!home.exists());
    assert!(!String::from_utf8_lossy(&out.stdout).contains("recovery words"));

    // Asked, recommending yes; something else is asked again.
    let home = dir.path().join("yes");
    let out = osp_init(&home, &new, "maybe\nyes\n");
    assert!(out.status.success(), "{out:?}");
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains("Recommended: yes"), "{said}");
    assert!(said.contains("Please answer yes or no"), "{said}");
    assert!(shares(&home));

    let home = dir.path().join("no");
    assert!(osp_init(&home, &new, "no\n").status.success());
    assert!(!shares(&home));

    // A flag answers without asking.
    for (flag, share) in [("--share", true), ("--no-share", false)] {
        let home = dir.path().join(flag);
        let out = osp_init(&home, &[&new[..], &[flag]].concat(), "");
        assert!(out.status.success(), "{out:?}");
        assert!(!String::from_utf8_lossy(&out.stdout).contains("Recommended"));
        assert_eq!(shares(&home), share);
    }
}

#[test]
fn restoring_asks_too() {
    let dir = tempfile::tempdir().unwrap();
    let words = osp_node::backup::new_identity_words().join(" ");
    let restore = ["--restore", "--retired", "--no-list-me"];

    let home = dir.path().join("asked");
    let out = osp_init(&home, &restore, &format!("{words}\n"));
    assert!(!out.status.success());
    assert!(!home.exists());

    let out = osp_init(&home, &restore, &format!("{words}\nyes\n"));
    assert!(out.status.success(), "{out:?}");
    assert!(shares(&home));

    let home = dir.path().join("flag");
    let args = [&restore[..], &["--no-share"]].concat();
    let out = osp_init(&home, &args, &format!("{words}\n"));
    assert!(out.status.success(), "{out:?}");
    assert!(!shares(&home));
}

#[test]
fn a_new_node_has_tor_on_and_is_asked_about_being_listed() {
    let dir = tempfile::tempdir().unwrap();
    let new = ["--skip-word-check", "--share"];

    // No answer: nothing is made.
    let home = dir.path().join("none");
    let out = osp_init(&home, &new, "");
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--list-me or --no-list-me"), "{err}");
    assert!(!home.exists());

    // Asked, saying what it shows, with no recommendation.
    let home = dir.path().join("yes");
    let out = osp_init(&home, &new, "maybe\nyes\n");
    assert!(out.status.success(), "{out:?}");
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains("anyone who reads one"), "{said}");
    assert!(said.contains("Please answer yes or no"), "{said}");
    assert!(!said.contains("Recommended"), "{said}");
    let made = config(&home);
    assert!(made.directory.list_me);
    assert!(made.tor.enabled && made.endpoints.enabled);

    let home = dir.path().join("flag");
    let out = osp_init(&home, &[&new[..], &["--no-list-me"]].concat(), "");
    assert!(out.status.success(), "{out:?}");
    assert!(!config(&home).directory.list_me);

    // Without Tor there are no directories to be listed by, so it isn't asked.
    let home = dir.path().join("no-tor");
    let out = osp_init(&home, &[&new[..], &["--no-tor"]].concat(), "");
    assert!(out.status.success(), "{out:?}");
    let made = config(&home);
    assert!(!made.directory.list_me);
    assert!(!made.tor.enabled && !made.endpoints.enabled);
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(!said.contains("Directories"), "{said}");
    assert!(!said.contains("machine name"), "{said}");

    let home = dir.path().join("both");
    let out = osp_init(&home, &[&new[..], &["--no-tor", "--list-me"]].concat(), "");
    assert!(!out.status.success());
    assert!(!home.exists());
}
