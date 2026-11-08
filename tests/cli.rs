//! Tests that run the binary from the outside.

use std::process::{Command, Stdio};

/// Does not panic when the reader closes the pipe first, as in `wifilog status | head -1`.
#[test]
fn closed_stdout_does_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_wifilog"))
        .arg("status")
        .env("WIFILOG_DIR", dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take()); // the reader closes without reading anything
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "panicked: {stderr}");
}

/// `wifilog completions zsh` prints a zsh completion script that knows the subcommands.
#[test]
fn prints_zsh_completion_script() {
    let out = Command::new(env!("CARGO_BIN_EXE_wifilog"))
        .args(["completions", "zsh"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let script = String::from_utf8(out.stdout).unwrap();
    assert!(script.starts_with("#compdef wifilog"), "{script}");
    for word in ["show-report", "--24h", "status"] {
        assert!(script.contains(word), "missing {word}");
    }
}
