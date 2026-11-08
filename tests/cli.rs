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
