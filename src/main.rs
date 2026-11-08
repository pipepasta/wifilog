mod collector;
mod daemon;
mod lqm;
mod net;
mod ping;
mod record;
mod series;
mod store;
mod tui;

use std::path::Path;

use anyhow::{Result, bail};
use chrono::Local;
use clap::{Parser, Subcommand};

use daemon::{StartOutcome, Status};
use record::Record;

/// Mac の Wi-Fi の品質を記録して、ターミナルで推移を見る
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 計測を始める（裏で動き続ける）
    Start {
        /// 裏に回らず、その場で計測し続ける（launchd から使う）
        #[arg(long)]
        foreground: bool,
        /// インターネット側の ping の宛先（既定: 環境変数 WIFILOG_INTERNET、なければ 1.1.1.1）
        #[arg(long, value_name = "HOST")]
        internet: Option<String>,
    },
    /// 計測を止める
    Stop,
    /// 計測が動いているか、最後にいつ記録したかを表示する
    Status,
}

const DEFAULT_INTERNET: &str = "1.1.1.1";

/// インターネット側の宛先。フラグ > 環境変数 `WIFILOG_INTERNET` > 既定値 の順。
fn internet_target(flag: Option<String>, env: Option<String>) -> String {
    flag.or(env).unwrap_or_else(|| DEFAULT_INTERNET.to_string())
}

fn main() -> Result<()> {
    // Rust は SIGPIPE を無視するので、`wifilog status | head -1` のように読み手が先に閉じると
    // println! がパニックする。普通の CLI と同じく、静かに終了するようにしておく。
    // SAFETY: シグナルの扱いを既定に戻すだけで、スレッドを立てる前に呼んでいる
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    let dir = store::data_dir()?;
    match cli.command {
        Cmd::Start { foreground, internet } => {
            let internet = internet_target(internet, std::env::var("WIFILOG_INTERNET").ok());
            if foreground {
                let Some(_lock) = daemon::acquire(&dir)? else {
                    bail!("すでに計測中です（`wifilog status` で確認できます）");
                };
                return collector::run_foreground(&dir, &internet);
            }
            match daemon::start(&dir, &internet)? {
                StartOutcome::Started { pid } => println!("計測を開始しました（pid {pid}）"),
                StartOutcome::AlreadyRunning { pid } => println!("すでに計測中です（pid {pid}）"),
            }
            Ok(())
        }
        Cmd::Stop => {
            if daemon::stop(&dir)? {
                println!("計測を止めました");
            } else {
                println!("計測は動いていません");
            }
            Ok(())
        }
        Cmd::Status => print_status(&dir),
    }
}

fn print_status(dir: &Path) -> Result<()> {
    match daemon::status(dir)? {
        Status::Running { pid, since } => {
            let since = since.map_or("不明".to_string(), |s| s.format("%m/%d %H:%M").to_string());
            println!("計測中（pid {pid}、{since} から）");
        }
        Status::Stopped => println!("停止中"),
    }

    let now = Local::now().fixed_offset();
    let recent = store::load_range(dir, now - chrono::Duration::minutes(10), now)?;
    let last = |pick: fn(&Record) -> bool| {
        recent
            .iter()
            .rev()
            .find(|r| pick(r))
            .map_or("記録なし".to_string(), |r| r.t().format("%H:%M:%S").to_string())
    };
    println!(
        "最後の記録（直近10分）: 電波 {} / ping {}",
        last(|r| matches!(r, Record::Lqm(_))),
        last(|r| matches!(r, Record::Ping(_))),
    );
    println!("データ: {}", dir.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internet_target_precedence() {
        assert_eq!(internet_target(Some("8.8.8.8".into()), Some("9.9.9.9".into())), "8.8.8.8");
        assert_eq!(internet_target(None, Some("9.9.9.9".into())), "9.9.9.9");
        assert_eq!(internet_target(None, None), "1.1.1.1");
    }
}
