//! 計測プロセスの起動・停止・状態確認。
//!
//! 「動いているか」は pid ファイルの有無ではなく、そのファイルへの排他ロックで判断する。
//! プロセスが落ちればロックは OS が外すので、古い pid ファイルが残っても次の起動を妨げない。

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{DateTime, FixedOffset, Local};

pub const PID_FILE: &str = "wifilog.pid";
pub const LOG_FILE: &str = "wifilog.log";

/// 持っている間ロックが続く。
pub struct Lock {
    _file: File,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Running { pid: u32, since: Option<DateTime<FixedOffset>> },
    Stopped,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StartOutcome {
    Started { pid: u32 },
    AlreadyRunning { pid: u32 },
}

fn open_pid_file(dir: &Path) -> Result<File> {
    std::fs::create_dir_all(dir).with_context(|| format!("{} を作れません", dir.display()))?;
    let path = dir.join(PID_FILE);
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("{} を開けません", path.display()))
}

/// ロックを取って pid と開始時刻を書く。すでに誰かが持っていれば `None`。
pub fn acquire(dir: &Path) -> Result<Option<Lock>> {
    let mut file = open_pid_file(dir)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(None),
        Err(TryLockError::Error(e)) => return Err(e).context("pid ファイルをロックできません"),
    }
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    write!(file, "{} {}", std::process::id(), Local::now().fixed_offset().to_rfc3339())?;
    file.flush()?;
    Ok(Some(Lock { _file: file }))
}

pub fn status(dir: &Path) -> Result<Status> {
    let mut file = open_pid_file(dir)?;
    match file.try_lock() {
        Ok(()) => return Ok(Status::Stopped), // ロックは file を閉じると外れる
        Err(TryLockError::WouldBlock) => {}
        Err(TryLockError::Error(e)) => return Err(e).context("pid ファイルを確認できません"),
    }
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let mut parts = text.split_whitespace();
    let pid = parts
        .next()
        .and_then(|p| p.parse().ok())
        .context("pid ファイルの中身が読めません")?;
    let since = parts.next().and_then(|s| DateTime::parse_from_rfc3339(s).ok());
    Ok(Status::Running { pid, since })
}

fn wait_until(timeout: Duration, mut ok: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if ok() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    ok()
}

/// 計測プロセスを裏で起動する。
pub fn start(dir: &Path, internet: &str) -> Result<StartOutcome> {
    if let Status::Running { pid, .. } = status(dir)? {
        return Ok(StartOutcome::AlreadyRunning { pid });
    }
    let log_path = dir.join(LOG_FILE);
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("{} を開けません", log_path.display()))?;
    let exe = std::env::current_exe().context("実行ファイルの場所が分かりません")?;
    let child = Command::new(exe)
        .args(["start", "--foreground", "--internet", internet])
        .env("WIFILOG_DIR", dir)
        // 新しいプロセスグループに入れる。端末を閉じても巻き込まれず、stop でグループごと止められる
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()
        .context("計測プロセスを起動できません")?;

    let started = wait_until(Duration::from_secs(2), || {
        matches!(status(dir), Ok(Status::Running { .. }))
    });
    if !started {
        anyhow::bail!("計測プロセスが起動しませんでした（{} を確認してください）", log_path.display());
    }
    Ok(StartOutcome::Started { pid: child.id() })
}

/// 計測プロセスをプロセスグループごと止める。止めたら `true`、もともと止まっていたら `false`。
pub fn stop(dir: &Path) -> Result<bool> {
    let Status::Running { pid, .. } = status(dir)? else {
        return Ok(false);
    };
    let pid = pid as libc::pid_t;
    // SAFETY: kill はシグナルを送るだけで、メモリには触れない
    let sent_to_group = unsafe { libc::kill(-pid, libc::SIGTERM) } == 0;
    if !sent_to_group {
        // launchd などから --foreground で動かしていて、自分がグループの長でない場合
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
    let stopped = wait_until(Duration::from_secs(5), || {
        matches!(status(dir), Ok(Status::Stopped))
    });
    if !stopped {
        anyhow::bail!("計測プロセス（pid {pid}）が止まりませんでした");
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_fails_while_first_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let first = acquire(dir.path()).unwrap();
        assert!(first.is_some());
        assert!(acquire(dir.path()).unwrap().is_none());
        drop(first);
        assert!(acquire(dir.path()).unwrap().is_some());
    }

    #[test]
    fn stale_pid_file_does_not_block_start() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(PID_FILE), "99999 2026-11-08T10:00:00+09:00").unwrap();
        assert_eq!(status(dir.path()).unwrap(), Status::Stopped);
        assert!(acquire(dir.path()).unwrap().is_some());
    }

    #[test]
    fn status_reports_running_with_pid() {
        let dir = tempfile::tempdir().unwrap();
        let _lock = acquire(dir.path()).unwrap().unwrap();
        let Status::Running { pid, .. } = status(dir.path()).unwrap() else {
            panic!("should be running")
        };
        assert_eq!(pid, std::process::id());
    }
}
