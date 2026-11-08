//! 実行ファイルを外から動かして確かめるテスト。

use std::process::{Command, Stdio};

/// `wifilog status | head -1` のように、読み手が先にパイプを閉じてもパニックしない。
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
    drop(child.stdout.take()); // 読み手が何も読まずに閉じる
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "panicked: {stderr}");
}
