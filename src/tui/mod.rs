//! 画面の描画。`view` と `show-report` で共通。
//!
//! グラフを縦に 5 段積み、すべての段で時刻の軸（x 軸）をそろえる。

use chrono::{DateTime, Duration, FixedOffset};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::symbols::Marker;
use ratatui::text::Line;
use ratatui::widgets::{Axis, Block, Chart, Dataset, GraphType, Paragraph};

use crate::record::Target;
use crate::series::{Metric, Model};

pub mod app;

pub enum Mode {
    Live { paused: bool, span_label: &'static str },
    Report { span_label: &'static str },
}

/// 5 段の見出し（上から順）。
pub const PANEL_TITLES: [&str; 5] =
    ["RSSI / SNR", "遅延（ルーター / インターネット）", "チャンネル混雑度", "送信速度", "再送・失敗"];

/// 記録が止まったとみなすまでの時間。
const STALE: Duration = Duration::seconds(30);
/// 1 段の最低の高さ（枠の上下 2 行 ＋ グラフ 1 行）。
const MIN_PANEL_HEIGHT: u16 = 3;

const PANELS: [&[Metric]; 5] = [
    &[Metric::Rssi, Metric::Snr],
    &[Metric::RouterMs, Metric::InternetMs],
    &[Metric::Cca],
    &[Metric::Tx],
    &[Metric::Retx, Metric::Fail],
];

fn name(m: Metric) -> &'static str {
    match m {
        Metric::Rssi => "RSSI",
        Metric::Snr => "SNR",
        Metric::RouterMs => "ルーター",
        Metric::InternetMs => "ネット",
        Metric::Cca => "混雑",
        Metric::Tx => "送信",
        Metric::Retx => "再送",
        Metric::Fail => "失敗",
    }
}

fn unit(m: Metric) -> &'static str {
    match m {
        Metric::Rssi => " dBm",
        Metric::Snr => " dB",
        Metric::RouterMs | Metric::InternetMs => " ms",
        Metric::Cca => "%",
        Metric::Tx => " Mbps",
        Metric::Retx | Metric::Fail => "",
    }
}

fn fmt(m: Metric, v: f64) -> String {
    match m {
        Metric::RouterMs | Metric::InternetMs => format!("{v:.1}"),
        _ => format!("{v:.0}"),
    }
}

fn color(m: Metric) -> Color {
    match m {
        Metric::Rssi => Color::Cyan,
        Metric::Snr => Color::Green,
        Metric::RouterMs => Color::Yellow,
        Metric::InternetMs => Color::Magenta,
        Metric::Cca => Color::Rgb(209, 154, 102),
        Metric::Tx => Color::Blue,
        Metric::Retx => Color::Red,
        Metric::Fail => Color::LightMagenta,
    }
}

/// `aa:bb:cc:dd:ee:12` → `aa:bb:…:12`
fn short_mac(mac: Option<&str>) -> String {
    let Some(mac) = mac else { return "不明".into() };
    let p: Vec<&str> = mac.split(':').collect();
    if p.len() != 6 {
        return mac.to_string();
    }
    format!("{}:{}:…:{}", p[0], p[1], p[5])
}

fn fmt_secs(secs: i64) -> String {
    let (h, m) = (secs / 3600, secs % 3600 / 60);
    if h > 0 { format!("{h}時間{m:02}分") } else { format!("{m}分") }
}

/// グラフの横のドット数（点字マーカーは 1 文字が横 2 ドット）。y 軸の目盛りと枠の分を引く。
pub fn chart_buckets(area_width: u16) -> usize {
    (area_width.saturating_sub(10) as usize * 2).max(1)
}

pub fn draw(frame: &mut Frame, model: &Model, mode: &Mode, now: DateTime<FixedOffset>) {
    let area = frame.area();
    let legend = model.networks.len() >= 2;
    let [header, legend_area, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(legend as u16),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(frame, header, model, mode, now);
    if legend {
        frame.render_widget(Paragraph::new(legend_text(model)), legend_area);
    }
    let footer_text = match mode {
        Mode::Live { .. } => "q 終了   ←/→ 時間幅   space 一時停止",
        Mode::Report { .. } => "q 終了",
    };
    frame.render_widget(
        Paragraph::new(footer_text).style(Style::default().fg(Color::DarkGray)),
        footer,
    );

    let count = (body.height / MIN_PANEL_HEIGHT).min(PANELS.len() as u16) as usize;
    if count == 0 {
        return;
    }
    let rows = Layout::vertical(vec![Constraint::Ratio(1, count as u32); count]).split(body);
    for i in 0..count {
        draw_panel(frame, rows[i], model, mode, i, i + 1 == count);
    }
}

fn draw_header(
    frame: &mut Frame,
    area: Rect,
    model: &Model,
    mode: &Mode,
    now: DateTime<FixedOffset>,
) {
    let current = model.current_label();
    let mac = model
        .networks
        .iter()
        .find(|n| Some(n.label) == current)
        .and_then(|n| n.mac.as_deref());
    let channel = model
        .channel
        .map_or("ch -".to_string(), |(ch, bw)| format!("ch{ch} {bw}MHz"));
    let span = match mode {
        Mode::Live { span_label, .. } | Mode::Report { span_label } => span_label,
    };
    let left = format!("wifilog ─ ルーター {} ─ {channel} ─ {span}", short_mac(mac));

    let (right, color) = match mode {
        Mode::Live { paused, .. } => {
            let stale = model.last_record.is_none_or(|t| now - t >= STALE);
            if stale {
                ("⚠ 計測が止まっています（wifilog status で確認）".to_string(), Color::Yellow)
            } else if *paused {
                ("⏸ 一時停止".to_string(), Color::Yellow)
            } else {
                (format!("● 計測中 {}", now.format("%H:%M:%S")), Color::Green)
            }
        }
        Mode::Report { .. } => (
            format!(
                "期間 {}〜{}",
                model.window.from.format("%m/%d %H:%M"),
                model.window.to.format("%m/%d %H:%M")
            ),
            Color::Reset,
        ),
    };
    frame.render_widget(Paragraph::new(left), area);
    frame.render_widget(
        Paragraph::new(right).style(Style::default().fg(color)).alignment(Alignment::Right),
        area,
    );
}

fn legend_text(model: &Model) -> String {
    let mut parts: Vec<String> = model
        .networks
        .iter()
        .map(|n| format!("{} {} ({})", n.label, short_mac(n.mac.as_deref()), fmt_secs(n.secs)))
        .collect();
    if !model.switches.is_empty() {
        let switches: Vec<String> = model
            .switches
            .iter()
            .map(|(x, label)| {
                let t = model.window.from + Duration::milliseconds((x * 1000.0) as i64);
                format!("{} →{label}", t.format("%H:%M"))
            })
            .collect();
        parts.push(format!("│ 切替 {}", switches.join(", ")));
    }
    parts.join("  ")
}

/// 見出しの右に出す数字。
fn panel_value(model: &Model, mode: &Mode, metrics: &[Metric]) -> String {
    if model.networks.len() >= 2 {
        let primary = metrics[0];
        let mut nets: Vec<_> = model.networks.iter().collect();
        nets.sort_by_key(|n| std::cmp::Reverse(n.secs));
        let mut parts: Vec<String> = nets
            .iter()
            .take(3)
            .filter_map(|n| {
                let s = model.stats.get(&(primary, n.label))?;
                Some(format!("{} {}{}", n.label, fmt(primary, s.avg), unit(primary).trim()))
            })
            .collect();
        if nets.len() > 3 {
            parts.push(format!("他 {}", nets.len() - 3));
        }
        return format!("{}（平均）", parts.join(" / "));
    }

    let label = model.current_label();
    let mut text = match mode {
        Mode::Live { .. } => metrics
            .iter()
            .filter_map(|&m| Some(format!("{} {}{}", name(m), fmt(m, *model.latest.get(&m)?), unit(m))))
            .collect::<Vec<_>>()
            .join(" / "),
        Mode::Report { .. } => metrics
            .iter()
            .filter_map(|&m| {
                let s = model.stats.get(&(m, label?))?;
                Some(format!(
                    "{} 平均 {}{}（{}〜{}）",
                    name(m),
                    fmt(m, s.avg),
                    unit(m),
                    fmt(m, s.min),
                    fmt(m, s.max)
                ))
            })
            .collect::<Vec<_>>()
            .join("  "),
    };
    if metrics.contains(&Metric::RouterMs) {
        if let Some(label) = label {
            let pct = |t: Target| {
                model.loss.get(&(t, label)).map_or("-".to_string(), |l| format!("{:.0}%", l * 100.0))
            };
            text.push_str(&format!(" ・ ロス {} / {}", pct(Target::Router), pct(Target::Internet)));
        }
    }
    text
}

fn y_bounds(model: &Model, metrics: &[Metric]) -> [f64; 2] {
    let ys = metrics
        .iter()
        .filter_map(|m| model.lines.get(m))
        .flatten()
        .flatten()
        .map(|p| p.1);
    let (mut lo, mut hi) = ys.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), y| {
        (lo.min(y), hi.max(y))
    });
    if !lo.is_finite() {
        return [0.0, 1.0];
    }
    if metrics.iter().all(|m| matches!(m, Metric::Retx | Metric::Fail)) {
        lo = 0.0;
    }
    let pad = if hi > lo { (hi - lo) * 0.1 } else { 1.0 };
    if lo != 0.0 {
        lo -= pad;
    }
    hi += pad;
    [lo, hi]
}

fn draw_panel(
    frame: &mut Frame,
    area: Rect,
    model: &Model,
    mode: &Mode,
    index: usize,
    last: bool,
) {
    let metrics = PANELS[index];
    let [lo, hi] = y_bounds(model, metrics);
    let markers: Vec<[(f64, f64); 2]> =
        model.switches.iter().map(|&(x, _)| [(x, lo), (x, hi)]).collect();

    let mut datasets: Vec<Dataset> = markers
        .iter()
        .map(|m| {
            Dataset::default()
                .marker(Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(Color::DarkGray))
                .data(m)
        })
        .collect();
    for &m in metrics {
        for seg in model.lines.get(&m).into_iter().flatten() {
            datasets.push(
                Dataset::default()
                    .marker(Marker::Braille)
                    .graph_type(GraphType::Line)
                    .style(Style::default().fg(color(m)))
                    .data(seg),
            );
        }
    }

    let title = Line::from(format!(" {} ", PANEL_TITLES[index]));
    let value = Line::from(format!(" {} ", panel_value(model, mode, metrics))).right_aligned();
    let x_labels: Vec<String> = if last {
        vec![
            model.window.from.format("%H:%M").to_string(),
            model.window.to.format("%H:%M").to_string(),
        ]
    } else {
        Vec::new()
    };
    let y_metric = metrics[0];
    let chart = Chart::new(datasets)
        .block(Block::bordered().title(title).title(value))
        .x_axis(Axis::default().bounds([0.0, model.window.secs()]).labels(x_labels))
        .y_axis(
            Axis::default()
                .bounds([lo, hi])
                .labels([fmt(y_metric, lo), fmt(y_metric, hi)]),
        );
    frame.render_widget(chart, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{LqmRecord, LqmValues, PingRecord, Record, Target};
    use crate::series::{Window, build};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use unicode_width::UnicodeWidthStr;

    const A: &str = "aa:aa:aa:aa:aa:aa";
    const B: &str = "bb:bb:bb:bb:bb:bb";

    fn at(secs: i64) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339("2026-11-08T10:00:00+09:00").unwrap()
            + chrono::Duration::seconds(secs)
    }

    fn records(nets: &[(&str, i64, i64)]) -> Vec<Record> {
        let mut out = Vec::new();
        for &(net, from, to) in nets {
            for s in (from..to).step_by(5) {
                out.push(Record::Lqm(LqmRecord {
                    t: at(s),
                    net: Some(net.into()),
                    v: LqmValues {
                        rssi: -40 - (s % 7) as i32,
                        noise: -95,
                        snr: 35,
                        cca: 12.0,
                        tx: 866.7,
                        rx: 866.7,
                        txf: 20,
                        fail: 0,
                        retx: (s % 3) as u32,
                        ch: Some(36),
                        bw: Some(80),
                    },
                }));
                out.push(Record::Ping(PingRecord {
                    t: at(s),
                    net: Some(net.into()),
                    dst: Target::Router,
                    ms: Some(2.8),
                }));
            }
        }
        out
    }

    fn model(nets: &[(&str, i64, i64)]) -> Model {
        build(&records(nets), Window { from: at(0), to: at(900) }, 200)
    }

    /// 画面を行ごとの文字列にする（全角文字の 2 マス目は飛ばす）。
    fn render(m: &Model, mode: &Mode, now: DateTime<FixedOffset>, w: u16, h: u16) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, m, mode, now)).unwrap();
        let buf = term.backend().buffer();
        (0..h)
            .map(|y| {
                let mut line = String::new();
                let mut x = 0;
                while x < w {
                    let sym = buf[(x, y)].symbol();
                    line.push_str(sym);
                    x += sym.width().max(1) as u16;
                }
                line
            })
            .collect()
    }

    fn row_of(lines: &[String], text: &str) -> usize {
        lines
            .iter()
            .position(|l| l.contains(text))
            .unwrap_or_else(|| panic!("{text:?} not found in:\n{}", lines.join("\n")))
    }

    const LIVE: Mode = Mode::Live { paused: false, span_label: "直近15分" };
    const REPORT: Mode = Mode::Report { span_label: "過去15分" };

    #[test]
    fn panels_are_stacked_in_order() {
        let lines = render(&model(&[(A, 0, 900)]), &LIVE, at(900), 120, 40);
        let rows: Vec<usize> = PANEL_TITLES.iter().map(|t| row_of(&lines, t)).collect();
        assert!(rows.windows(2).all(|w| w[0] < w[1]), "rows: {rows:?}");
        assert_eq!(
            PANEL_TITLES,
            ["RSSI / SNR", "遅延（ルーター / インターネット）", "チャンネル混雑度", "送信速度", "再送・失敗"]
        );
    }

    #[test]
    fn report_header_shows_period() {
        let lines = render(&model(&[(A, 0, 900)]), &REPORT, at(900), 120, 40);
        assert!(lines[0].contains("期間 "), "{}", lines[0]);
        assert!(!lines[0].contains("● 計測中"), "{}", lines[0]);
    }

    #[test]
    fn live_shows_warning_when_stale() {
        let m = model(&[(A, 0, 900)]);
        let last = m.last_record.unwrap();
        let lines = render(&m, &LIVE, last + chrono::Duration::seconds(60), 120, 40);
        assert!(lines[0].contains("計測が止まっています"), "{}", lines[0]);
        let fresh = render(&m, &LIVE, last + chrono::Duration::seconds(3), 120, 40);
        assert!(fresh[0].contains("● 計測中"), "{}", fresh[0]);
    }

    #[test]
    fn legend_only_with_multiple_networks() {
        let one = render(&model(&[(A, 0, 900)]), &LIVE, at(900), 120, 40);
        assert!(!one[1].contains("A aa:aa:…:aa"), "{}", one[1]);

        let two = render(&model(&[(A, 0, 450), (B, 450, 900)]), &LIVE, at(900), 120, 40);
        assert!(two[1].contains("A aa:aa:…:aa"), "{}", two[1]);
        assert!(two[1].contains("B bb:bb:…:bb"), "{}", two[1]);
    }

    #[test]
    fn small_terminal_does_not_panic() {
        let lines = render(&model(&[(A, 0, 900)]), &LIVE, at(900), 80, 12);
        row_of(&lines, "RSSI / SNR");
        assert!(!lines.iter().any(|l| l.contains("再送・失敗")));
    }
}
