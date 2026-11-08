//! `view`（リアルタイム）と `show-report`（固定表示）の画面の動かし方。

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

/// `view` の再描画の間隔。
const REFRESH: StdDuration = StdDuration::from_secs(2);
/// `view` の時間幅（←/→ で切り替え）。
const SPANS: [(i64, &str); 3] = [(5, "直近5分"), (15, "直近15分"), (60, "直近1時間")];
const DEFAULT_SPAN: usize = 1;

fn now() -> DateTime<FixedOffset> {
    Local::now().fixed_offset()
}

fn is_quit(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
}

/// 端末を必ず元に戻してから戻る。
fn with_terminal(f: impl FnOnce(&mut DefaultTerminal) -> Result<()>) -> Result<()> {
    let mut terminal = ratatui::init();
    let result = f(&mut terminal);
    ratatui::restore();
    result
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
            let cutoff = now() - keep - Duration::minutes(1);
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
        println!("この期間の記録はありません（`wifilog start` で計測を始めます）");
        return Ok(());
    }

    with_terminal(|terminal| {
        loop {
            terminal.draw(|f| {
                let model = series::build(&records, window, chart_buckets(f.area().width));
                draw(f, &model, &Mode::Report { span_label: label }, to);
            })?;
            // サイズ変更などのイベントのたびに描き直し、終了キーで抜ける
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press && is_quit(&key) {
                    return Ok(());
                }
        }
    })
}
