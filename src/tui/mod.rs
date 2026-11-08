//! 画面の描画。`view` と `show-report` で共通。
//!
//! グラフを縦に 5 段積み、すべての段で時刻の軸（x 軸）をそろえる。

use chrono::{DateTime, Duration, FixedOffset};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
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

/// y 軸の目盛りの幅。全段で同じにして、グラフの左端（時刻の軸）をそろえる。
const Y_LABEL_WIDTH: usize = 6;

struct Panel {
    /// グラフに描く指標（同じ単位のものだけ）
    chart: &'static [Metric],
    /// 見出しの右に数字で出す指標
    values: &'static [Metric],
}

const PANELS: [Panel; 5] = [
    // RSSI とノイズを描く。2 本の線の間隔が SNR になる
    Panel { chart: &[Metric::Rssi, Metric::Noise], values: &[Metric::Rssi, Metric::Snr] },
    Panel {
        chart: &[Metric::RouterMs, Metric::InternetMs],
        values: &[Metric::RouterMs, Metric::InternetMs],
    },
    Panel { chart: &[Metric::Cca], values: &[Metric::Cca] },
    Panel { chart: &[Metric::Tx], values: &[Metric::Tx] },
    Panel { chart: &[Metric::Retx, Metric::Fail], values: &[Metric::Retx, Metric::Fail] },
];

fn name(m: Metric) -> &'static str {
    match m {
        Metric::Rssi => "RSSI",
        Metric::Noise => "ノイズ",
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
        Metric::Rssi | Metric::Noise => " dBm",
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
        Metric::Noise => Color::Gray,
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
            let stale = |t: Option<DateTime<FixedOffset>>| t.is_none_or(|t| now - t >= STALE);
            // 今いるネットワークが分かっているのに電波の値だけが来ない（ログの書式の変化など）
            let radio_missing = current.is_some_and(|l| l != '?') && stale(model.last_lqm);
            if *paused {
                ("⏸ 一時停止".to_string(), Color::Yellow)
            } else if stale(model.last_record) {
                ("⚠ 計測が止まっています（wifilog status で確認）".to_string(), Color::Yellow)
            } else if radio_missing {
                ("⚠ 電波の値が取れていません（wifilog.log を確認）".to_string(), Color::Yellow)
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

/// 24 時間以上の期間では、同じ時刻が 2 回出てくるので日付も付ける。
fn time_format(model: &Model) -> &'static str {
    if model.window.secs() >= 24.0 * 3600.0 { "%m/%d %H:%M" } else { "%H:%M" }
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
                format!("{} →{label}", t.format(time_format(model)))
            })
            .collect();
        parts.push(format!("│ 切替 {}", switches.join(", ")));
    }
    parts.join("  ")
}

/// ネットワーク `label` の「ルーター / インターネット」のロス率。
fn loss_text(model: &Model, label: char) -> String {
    let pct = |t: Target| {
        model.loss.get(&(t, label)).map_or("-".to_string(), |l| format!("{:.0}%", l * 100.0))
    };
    format!("{} / {}", pct(Target::Router), pct(Target::Internet))
}

/// 見出しの右に出す数字の候補（長い順）。入りきるものを使う。
fn panel_values(model: &Model, mode: &Mode, metrics: &[Metric]) -> Vec<String> {
    let label = model.current_label();
    let has_delay = metrics.contains(&Metric::RouterMs);

    // レポートで期間内に複数のネットワークがあるときは、混ぜずにネットワークごとの平均を並べる
    if matches!(mode, Mode::Report { .. }) && model.networks.len() >= 2 {
        let primary = metrics[0];
        let mut nets: Vec<_> = model.networks.iter().collect();
        nets.sort_by_key(|n| std::cmp::Reverse(n.secs));
        let per_net = |limit: usize, with_loss: bool| {
            let mut parts: Vec<String> = nets
                .iter()
                .take(limit)
                .filter_map(|n| {
                    let s = model.stats.get(&(primary, n.label))?;
                    let loss = if has_delay && with_loss {
                        format!("（ロス {}）", loss_text(model, n.label))
                    } else {
                        String::new()
                    };
                    Some(format!("{} {}{}{loss}", n.label, fmt(primary, s.avg), unit(primary).trim()))
                })
                .collect();
            if nets.len() > limit {
                parts.push(format!("他 {}", nets.len() - limit));
            }
            format!("平均 {}", parts.join(" / "))
        };
        return vec![per_net(3, true), per_net(2, true), per_net(1, true), per_net(1, false)];
    }

    let join = |value: &dyn Fn(Metric) -> Option<f64>, named: bool| -> String {
        metrics
            .iter()
            .filter_map(|&m| {
                let v = fmt(m, value(m)?);
                Some(if named { format!("{} {v}{}", name(m), unit(m)) } else { format!("{v}{}", unit(m)) })
            })
            .collect::<Vec<_>>()
            .join(" / ")
    };
    let loss = match label {
        Some(label) if has_delay => format!(" ・ ロス {}", loss_text(model, label)),
        _ => String::new(),
    };

    match mode {
        // view は常に今いるネットワークの最新値
        Mode::Live { .. } => {
            let latest = |m: Metric| model.latest.get(&m).copied();
            let named = join(&latest, true);
            let plain = join(&latest, false);
            vec![format!("{named}{loss}"), format!("{plain}{loss}"), named, plain]
        }
        Mode::Report { .. } => {
            let stat = |m: Metric| label.and_then(|l| model.stats.get(&(m, l))).copied();
            let avg = join(&|m| stat(m).map(|s| s.avg), false);
            let min = join(&|m| stat(m).map(|s| s.min), false);
            let max = join(&|m| stat(m).map(|s| s.max), false);
            vec![
                format!("平均 {avg} ・ 最小 {min} ・ 最大 {max}{loss}"),
                format!("平均 {avg} ・ 最大 {max}{loss}"),
                format!("平均 {avg}{loss}"),
                format!("平均 {avg}"),
            ]
        }
    }
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
    let panel = &PANELS[index];
    let metrics = panel.chart;
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

    // 見出しの後ろに、線の色と指標名の対応（━RSSI ━ノイズ）を付ける
    let mut spans = vec![Span::raw(format!(" {} ", PANEL_TITLES[index]))];
    for (i, &m) in metrics.iter().enumerate() {
        let sep = if i == 0 { "" } else { " " };
        spans.push(Span::styled(format!("{sep}━{}", name(m)), Style::default().fg(color(m))));
    }
    spans.push(Span::raw(" "));
    let title = Line::from(spans);
    // 枠の角 2 つと、見出しとの間の余白 2 つ分を引いた幅に収まる候補を使う
    let room = (area.width as usize).saturating_sub(title.width() + 4);
    let value = panel_values(model, mode, panel.values)
        .into_iter()
        .map(|v| Line::from(format!(" {v} ")))
        .find(|l| l.width() <= room)
        .unwrap_or_default()
        .right_aligned();
    let x_labels: Vec<String> = if last {
        vec![
            model.window.from.format(time_format(model)).to_string(),
            model.window.to.format(time_format(model)).to_string(),
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
                .labels([lo, hi].map(|v| format!("{:>w$}", fmt(y_metric, v), w = Y_LABEL_WIDTH))),
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
                out.push(Record::Ping(PingRecord {
                    t: at(s),
                    net: Some(net.into()),
                    dst: Target::Internet,
                    ms: Some(10.5),
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

    /// 各段のグラフの左端（y 軸の線）の列が、全段でそろっている。
    #[test]
    fn chart_columns_are_aligned_across_panels() {
        let lines = render(&model(&[(A, 0, 900)]), &LIVE, at(900), 120, 40);
        let axis_cols: Vec<usize> = PANEL_TITLES
            .iter()
            .map(|t| {
                let row = &lines[row_of(&lines, t) + 1];
                let chars: Vec<char> = row.chars().collect();
                // 枠の左の線の次に出てくる縦線が y 軸
                chars.iter().skip(1).position(|&c| c == '│').unwrap() + 1
            })
            .collect();
        assert!(axis_cols.windows(2).all(|w| w[0] == w[1]), "axis columns: {axis_cols:?}");
    }

    /// 1 段目は dBm の値（RSSI とノイズ）だけを描くので、y 軸の範囲は負の値に収まる。
    #[test]
    fn first_panel_plots_only_dbm_values() {
        let [lo, hi] = y_bounds(&model(&[(A, 0, 900)]), PANELS[0].chart);
        assert!(hi < 0.0 && lo < -95.0, "bounds: {lo}..{hi}");
    }

    /// 数字が長くても、見出しは隠れない。
    #[test]
    fn long_values_do_not_hide_titles() {
        let lines = render(&model(&[(A, 0, 900)]), &REPORT, at(900), 90, 40);
        for t in PANEL_TITLES {
            row_of(&lines, t);
        }
    }

    #[test]
    fn paused_never_shows_stale_warning() {
        let m = model(&[(A, 0, 900)]);
        let late = m.last_record.unwrap() + chrono::Duration::seconds(120);
        let paused = Mode::Live { paused: true, span_label: "直近15分" };
        let lines = render(&m, &paused, late, 120, 40);
        assert!(lines[0].contains("一時停止"), "{}", lines[0]);
        assert!(!lines[0].contains("止まっています"), "{}", lines[0]);
    }

    #[test]
    fn live_with_two_networks_shows_current_values_and_loss() {
        let lines = render(&model(&[(A, 0, 450), (B, 450, 900)]), &LIVE, at(900), 140, 40);
        let row = &lines[row_of(&lines, "遅延（ルーター / インターネット）")];
        assert!(row.contains("ネット 10.5 ms"), "{row}");
        assert!(row.contains("ロス"), "{row}");
    }

    #[test]
    fn report_with_two_networks_shows_loss() {
        let lines = render(&model(&[(A, 0, 450), (B, 450, 900)]), &REPORT, at(900), 140, 40);
        let row = &lines[row_of(&lines, "遅延（ルーター / インターネット）")];
        assert!(row.contains("A 2.8"), "{row}");
        assert!(row.contains("ロス"), "{row}");
    }

    #[test]
    fn panel_titles_carry_a_color_key() {
        let lines = render(&model(&[(A, 0, 900)]), &LIVE, at(900), 140, 40);
        assert!(lines[row_of(&lines, "RSSI / SNR")].contains("━RSSI ━ノイズ"));
        assert!(lines[row_of(&lines, "遅延（ルーター / インターネット）")].contains("━ルーター ━ネット"));
        assert!(lines[row_of(&lines, "再送・失敗")].contains("━再送 ━失敗"));
    }

    #[test]
    fn live_warns_when_only_radio_values_stop() {
        // ping は届き続けているが、電波の値（LQM）が 60 秒来ていない
        let mut recs = records(&[(A, 0, 840)]);
        for s in (840..900).step_by(5) {
            recs.push(Record::Ping(PingRecord {
                t: at(s),
                net: Some(A.into()),
                dst: Target::Router,
                ms: Some(2.8),
            }));
        }
        let m = build(&recs, Window { from: at(0), to: at(900) }, 200);
        let lines = render(&m, &LIVE, at(900), 120, 40);
        assert!(lines[0].contains("電波の値が取れていません"), "{}", lines[0]);
    }

    #[test]
    fn long_windows_label_axis_with_dates() {
        let window = Window { from: at(900) - chrono::Duration::hours(24), to: at(900) };
        let m = build(&records(&[(A, 0, 900)]), window, 200);
        let lines = render(&m, &REPORT, at(900), 120, 40);
        let bottom = lines[lines.len() - 3..].join("\n");
        assert!(bottom.contains("11/07 10:15") && bottom.contains("11/08 10:15"), "{bottom}");
    }
}
