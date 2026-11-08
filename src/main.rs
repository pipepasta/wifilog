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

/// Record your Mac's Wi-Fi quality and watch how it changes in the terminal
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start recording (keeps running in the background)
    Start {
        /// Record in the foreground instead of detaching (for launchd)
        #[arg(long)]
        foreground: bool,
        /// Internet ping target (default: the WIFILOG_INTERNET environment variable, or 1.1.1.1)
        #[arg(long, value_name = "HOST")]
        internet: Option<String>,
    },
    /// Stop recording
    Stop,
    /// Show whether recording is running and when the last records were written
    Status,
    /// Watch live (starts recording if stopped; recording continues after you close it)
    View,
    /// View a past period (default: past 1 hour)
    ShowReport(ReportArgs),
}

#[derive(clap::Args)]
#[group(multiple = false)]
struct ReportArgs {
    /// Past 5 minutes
    #[arg(long = "5m")]
    m5: bool,
    /// Past 1 hour
    #[arg(long = "1h")]
    h1: bool,
    /// Past 24 hours
    #[arg(long = "24h")]
    h24: bool,
    /// Past 7 days
    #[arg(long = "7d")]
    d7: bool,
}

fn report_span(a: &ReportArgs) -> (chrono::Duration, &'static str) {
    if a.m5 {
        (chrono::Duration::minutes(5), "past 5 min")
    } else if a.h24 {
        (chrono::Duration::hours(24), "past 24 hours")
    } else if a.d7 {
        (chrono::Duration::days(7), "past 7 days")
    } else {
        (chrono::Duration::hours(1), "past 1 hour")
    }
}

const DEFAULT_INTERNET: &str = "1.1.1.1";

/// The internet target. Precedence: flag > `WIFILOG_INTERNET` environment variable > default.
fn internet_target(flag: Option<String>, env: Option<String>) -> String {
    flag.or(env).unwrap_or_else(|| DEFAULT_INTERNET.to_string())
}

fn main() -> Result<()> {
    // Rust ignores SIGPIPE, so when the reader closes first, as in `wifilog status | head -1`,
    // println! panics. Exit quietly instead, like any ordinary CLI.
    // SAFETY: only restores the default signal disposition, and runs before any threads are spawned
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
                    bail!("Already recording (see `wifilog status`)");
                };
                return collector::run_foreground(&dir, &internet);
            }
            match daemon::start(&dir, &internet)? {
                StartOutcome::Started { pid } => println!("Started recording (pid {pid})"),
                StartOutcome::AlreadyRunning { pid } => println!("Already recording (pid {pid})"),
            }
            Ok(())
        }
        Cmd::Stop => {
            if daemon::stop(&dir)? {
                println!("Stopped recording");
            } else {
                println!("Not running");
            }
            Ok(())
        }
        Cmd::Status => print_status(&dir),
        Cmd::View => {
            let internet = internet_target(None, std::env::var("WIFILOG_INTERNET").ok());
            tui::app::run_view(&dir, &internet)
        }
        Cmd::ShowReport(args) => {
            let (span, label) = report_span(&args);
            tui::app::run_report(&dir, span, label)
        }
    }
}

fn print_status(dir: &Path) -> Result<()> {
    match daemon::status(dir)? {
        Status::Running { pid, since } => {
            let since = since.map_or("unknown".to_string(), |s| s.format("%m/%d %H:%M").to_string());
            println!("Recording (pid {pid}, since {since})");
        }
        Status::Stopped => println!("Stopped"),
    }

    let now = Local::now().fixed_offset();
    let recent = store::load_range(dir, now - chrono::Duration::minutes(10), now)?;
    let last = |pick: fn(&Record) -> bool| {
        recent
            .iter()
            .rev()
            .find(|r| pick(r))
            .map_or("none".to_string(), |r| r.t().format("%H:%M:%S").to_string())
    };
    println!(
        "Last records (past 10 min): radio {} / ping {}",
        last(|r| matches!(r, Record::Lqm(_))),
        last(|r| matches!(r, Record::Ping(_))),
    );
    println!("Data: {}", dir.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report_args(argv: &[&str]) -> Result<ReportArgs, clap::Error> {
        let mut full = vec!["wifilog", "show-report"];
        full.extend_from_slice(argv);
        match Cli::try_parse_from(full)?.command {
            Cmd::ShowReport(a) => Ok(a),
            _ => panic!("show-report expected"),
        }
    }

    #[test]
    fn report_span_defaults_to_one_hour() {
        assert_eq!(report_span(&report_args(&[]).unwrap()), (chrono::Duration::hours(1), "past 1 hour"));
    }

    #[test]
    fn report_span_flags() {
        assert_eq!(report_span(&report_args(&["--5m"]).unwrap()), (chrono::Duration::minutes(5), "past 5 min"));
        assert_eq!(report_span(&report_args(&["--24h"]).unwrap()), (chrono::Duration::hours(24), "past 24 hours"));
        assert_eq!(report_span(&report_args(&["--7d"]).unwrap()), (chrono::Duration::days(7), "past 7 days"));
    }

    #[test]
    fn report_span_flags_are_exclusive() {
        assert!(report_args(&["--5m", "--1h"]).is_err());
    }

    #[test]
    fn internet_target_precedence() {
        assert_eq!(internet_target(Some("8.8.8.8".into()), Some("9.9.9.9".into())), "8.8.8.8");
        assert_eq!(internet_target(None, Some("9.9.9.9".into())), "9.9.9.9");
        assert_eq!(internet_target(None, None), "1.1.1.1");
    }
}
