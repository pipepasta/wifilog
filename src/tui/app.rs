//! Screen loops for `view` (live) and `show-report` (static).

use std::path::Path;
use std::time::Duration as StdDuration;

use anyhow::Result;
use chrono::{DateTime, Duration, FixedOffset, Local};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::{Mode, chart_buckets, draw};
use crate::daemon::{self, Status};
use crate::record::Record;
use crate::series::{self, Window};
use crate::store::{self, Tail};

/// Redraw interval for `view`.
const REFRESH: StdDuration = StdDuration::from_secs(2);
/// Time spans for `view` (switched with ←/→).
const SPANS: [(i64, &str); 3] = [(5, "last 5 min"), (15, "last 15 min"), (60, "last 1 hour")];
const DEFAULT_SPAN: usize = 1;

fn now() -> DateTime<FixedOffset> {
    Local::now().fixed_offset()
}

fn is_quit(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
}

/// Always restores the terminal before returning.
fn with_terminal(f: impl FnOnce(&mut DefaultTerminal) -> Result<()>) -> Result<()> {
    let mut terminal = ratatui::init();
    let result = f(&mut terminal);
    ratatui::restore();
    result
}

/// The oldest record time kept in memory.
fn retain_cutoff(
    now: DateTime<FixedOffset>,
    paused_at: Option<DateTime<FixedOffset>>,
    keep: Duration,
) -> DateTime<FixedOffset> {
    // While paused, anchor on the pause time (so the graph being viewed is not trimmed)
    paused_at.unwrap_or(now) - keep - Duration::minutes(1)
}

pub fn run_view(dir: &Path, internet: &str) -> Result<()> {
    if daemon::status(dir)? == Status::Stopped {
        daemon::start(dir, internet)?;
    }
    let keep = Duration::minutes(SPANS[SPANS.len() - 1].0);
    let mut tail = Tail::new(dir.to_path_buf());
    let started = now();
    let mut records: Vec<Record> = store::load_range(dir, started - keep, started)?;
    let mut span = DEFAULT_SPAN;
    let mut paused_at: Option<DateTime<FixedOffset>> = None;

    with_terminal(|terminal| {
        loop {
            records.extend(tail.poll()?);
            let cutoff = retain_cutoff(now(), paused_at, keep);
            records.retain(|r| r.t() >= cutoff);

            let (minutes, span_label) = SPANS[span];
            let at = paused_at.unwrap_or_else(now);
            terminal.draw(|f| {
                let window = Window { from: at - Duration::minutes(minutes), to: at };
                let model = series::build(&records, window, chart_buckets(f.area().width));
                let mode = Mode::Live { paused: paused_at.is_some(), span_label };
                draw(f, &model, &mode, now());
            })?;

            if !event::poll(REFRESH)? {
                continue;
            }
            let Event::Key(key) = event::read()? else { continue };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if is_quit(&key) {
                return Ok(());
            }
            match key.code {
                KeyCode::Left => span = span.saturating_sub(1),
                KeyCode::Right => span = (span + 1).min(SPANS.len() - 1),
                KeyCode::Char(' ') => {
                    paused_at = match paused_at {
                        Some(_) => None,
                        None => Some(now()),
                    }
                }
                _ => {}
            }
        }
    })
}

pub fn run_report(dir: &Path, span: Duration, label: &'static str) -> Result<()> {
    let to = now();
    let window = Window { from: to - span, to };
    let records = store::load_range(dir, window.from, window.to)?;
    if records.is_empty() {
        println!("No records in this period (start recording with `wifilog start`)");
        return Ok(());
    }

    with_terminal(|terminal| {
        loop {
            terminal.draw(|f| {
                let model = series::build(&records, window, chart_buckets(f.area().width));
                draw(f, &model, &Mode::Report { span_label: label }, to);
            })?;
            // Redraw on every event (resize etc.) and exit on the quit key
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press && is_quit(&key) {
                    return Ok(());
                }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_follows_the_paused_time() {
        let now = DateTime::parse_from_rfc3339("2026-11-08T12:00:00+09:00").unwrap();
        let paused_at = now - Duration::minutes(30);
        assert_eq!(
            retain_cutoff(now, Some(paused_at), Duration::hours(1)),
            paused_at - Duration::hours(1) - Duration::minutes(1)
        );
        assert_eq!(
            retain_cutoff(now, None, Duration::hours(1)),
            now - Duration::hours(1) - Duration::minutes(1)
        );
    }
}
