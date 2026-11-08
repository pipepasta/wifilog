//! 描画用のデータを組み立てる（ネットワークごとの区切り、線の切れ目、間引き、統計）。
//!
//! x 座標はすべて「表示する期間の始まりからの秒数」。`view` と `show-report` の両方がこれを使う。

use std::collections::HashMap;

use chrono::{DateTime, FixedOffset};

use crate::record::{Record, Target};

/// 同じ指標の点の間隔がこれより空いたら、線を切る（スリープ中など）。
pub const GAP_SECS: i64 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Metric {
    Rssi,
    Snr,
    RouterMs,
    InternetMs,
    Cca,
    Tx,
    Retx,
    Fail,
}

impl Metric {
    /// 間引くときに合計する指標（それ以外は平均）。
    fn is_count(self) -> bool {
        matches!(self, Metric::Retx | Metric::Fail)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    pub from: DateTime<FixedOffset>,
    pub to: DateTime<FixedOffset>,
}

impl Window {
    pub fn secs(&self) -> f64 {
        (self.to - self.from).num_milliseconds() as f64 / 1000.0
    }

    fn x(&self, t: DateTime<FixedOffset>) -> f64 {
        (t - self.from).num_milliseconds() as f64 / 1000.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    pub avg: f64,
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Network {
    /// 出てきた順に 'A', 'B', …。ネットワーク不明は '?'
    pub label: char,
    /// ルーターの MAC アドレス
    pub mac: Option<String>,
    /// 期間の中でそのネットワークにいた秒数
    pub secs: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub window: Window,
    pub networks: Vec<Network>,
    /// ネットワークが切り替わった時刻の x と、切り替わり先の記号
    pub switches: Vec<(f64, char)>,
    /// 指標ごとの線分（連続した区間）のリスト
    pub lines: HashMap<Metric, Vec<Vec<(f64, f64)>>>,
    pub stats: HashMap<(Metric, char), Stats>,
    /// タイムアウト数 ÷ ping の数
    pub loss: HashMap<(Target, char), f64>,
    /// 最後のネットワークでの、各指標の最新値
    pub latest: HashMap<Metric, f64>,
    /// 最後の LQM の (チャンネル, 帯域幅)
    pub channel: Option<(u32, u32)>,
    pub last_record: Option<DateTime<FixedOffset>>,
    /// 最後のネットワークの記号
    current: Option<char>,
}

impl Model {
    /// 最後のネットワーク（今いるネットワーク）の記号。
    pub fn current_label(&self) -> Option<char> {
        self.current
    }
}

/// 間引く前の 1 点。
struct Point {
    t: DateTime<FixedOffset>,
    label: char,
    y: f64,
}

pub fn build(records: &[Record], window: Window, buckets: usize) -> Model {
    let mut recs: Vec<&Record> = records
        .iter()
        .filter(|r| window.from <= r.t() && r.t() <= window.to)
        .collect();
    recs.sort_by_key(|r| r.t());

    let (networks, switches, labels) = label_networks(&recs, &window);
    let current = labels.last().copied();

    let mut points: HashMap<Metric, Vec<Point>> = HashMap::new();
    let mut pings: HashMap<(Target, char), (u32, u32)> = HashMap::new(); // (総数, タイムアウト)
    let mut channel = None;
    for (r, &label) in recs.iter().zip(&labels) {
        let mut push = |m: Metric, y: f64| {
            points.entry(m).or_default().push(Point { t: r.t(), label, y });
        };
        match r {
            Record::Lqm(l) => {
                let v = &l.v;
                push(Metric::Rssi, v.rssi as f64);
                push(Metric::Snr, v.snr as f64);
                push(Metric::Cca, v.cca);
                push(Metric::Tx, v.tx);
                push(Metric::Retx, v.retx as f64);
                push(Metric::Fail, v.fail as f64);
                if let (Some(ch), Some(bw)) = (v.ch, v.bw) {
                    channel = Some((ch, bw));
                }
            }
            Record::Ping(p) => {
                let count = pings.entry((p.dst, label)).or_default();
                count.0 += 1;
                match p.ms {
                    Some(ms) => push(
                        match p.dst {
                            Target::Router => Metric::RouterMs,
                            Target::Internet => Metric::InternetMs,
                        },
                        ms,
                    ),
                    None => count.1 += 1,
                }
            }
            Record::Net(_) => {}
        }
    }

    let width = window.secs() / buckets.max(1) as f64;
    let mut lines = HashMap::new();
    let mut stats = HashMap::new();
    let mut latest = HashMap::new();
    for (&metric, pts) in &points {
        let segments = split_segments(pts);
        lines.insert(
            metric,
            segments
                .iter()
                .map(|seg| downsample(seg, metric, &window, width, buckets))
                .collect(),
        );
        for label in pts.iter().map(|p| p.label).collect::<std::collections::BTreeSet<_>>() {
            let ys: Vec<f64> = pts.iter().filter(|p| p.label == label).map(|p| p.y).collect();
            stats.insert((metric, label), summarize(&ys));
        }
        if let Some(p) = pts.iter().rev().find(|p| Some(p.label) == current) {
            latest.insert(metric, p.y);
        }
    }

    let loss = pings
        .into_iter()
        .map(|(key, (total, timeouts))| (key, timeouts as f64 / total as f64))
        .collect();

    Model {
        window,
        networks,
        switches,
        lines,
        stats,
        loss,
        latest,
        channel,
        last_record: recs.last().map(|r| r.t()),
        current,
    }
}

/// レコードごとのネットワーク記号を決め、切り替わりの位置と各ネットワークにいた時間を求める。
fn label_networks(
    recs: &[&Record],
    window: &Window,
) -> (Vec<Network>, Vec<(f64, char)>, Vec<char>) {
    let mut networks: Vec<Network> = Vec::new();
    let mut switches = Vec::new();
    let mut labels = Vec::with_capacity(recs.len());
    let mut by_mac: HashMap<Option<String>, char> = HashMap::new();
    let mut next = 'A';
    // (今のネットワーク, その記号, 区間の始まり)
    let mut cur: Option<(Option<String>, char, DateTime<FixedOffset>)> = None;

    let add_secs = |networks: &mut Vec<Network>, label: char, secs: i64| {
        if let Some(n) = networks.iter_mut().find(|n| n.label == label) {
            n.secs += secs;
        }
    };

    for r in recs {
        let net = r.net().map(String::from);
        if cur.as_ref().map(|c| &c.0) != Some(&net) {
            let label = *by_mac.entry(net.clone()).or_insert_with(|| match net {
                None => '?',
                Some(_) => {
                    let l = next;
                    next = char::from(next as u8 + 1);
                    l
                }
            });
            if let Some((_, prev, start)) = &cur {
                add_secs(&mut networks, *prev, (r.t() - *start).num_seconds());
                switches.push((window.x(r.t()), label));
            }
            if !networks.iter().any(|n| n.label == label) {
                networks.push(Network { label, mac: net.clone(), secs: 0 });
            }
            cur = Some((net, label, r.t()));
        }
        labels.push(cur.as_ref().expect("set above").1);
    }
    if let (Some((_, label, start)), Some(last)) = (&cur, recs.last()) {
        add_secs(&mut networks, *label, (last.t() - *start).num_seconds());
    }
    (networks, switches, labels)
}

/// ネットワークが変わるか、`GAP_SECS` より空いたところで切る。
fn split_segments(pts: &[Point]) -> Vec<&[Point]> {
    let mut segments = Vec::new();
    let mut start = 0;
    for i in 1..pts.len() {
        let gap = (pts[i].t - pts[i - 1].t).num_seconds() > GAP_SECS;
        if gap || pts[i].label != pts[i - 1].label {
            segments.push(&pts[start..i]);
            start = i;
        }
    }
    if start < pts.len() {
        segments.push(&pts[start..]);
    }
    segments
}

/// 幅 `width` 秒の区間ごとにまとめる。点の x は区間の中央。
fn downsample(
    seg: &[Point],
    metric: Metric,
    window: &Window,
    width: f64,
    buckets: usize,
) -> Vec<(f64, f64)> {
    let bucket = |p: &Point| ((window.x(p.t) / width).floor() as usize).min(buckets.max(1) - 1);
    let mut out: Vec<(f64, f64)> = Vec::new();
    let mut i = 0;
    while i < seg.len() {
        let b = bucket(&seg[i]);
        let mut j = i;
        let mut sum = 0.0;
        while j < seg.len() && bucket(&seg[j]) == b {
            sum += seg[j].y;
            j += 1;
        }
        let y = if metric.is_count() { sum } else { sum / (j - i) as f64 };
        out.push(((b as f64 + 0.5) * width, y));
        i = j;
    }
    out
}

fn summarize(ys: &[f64]) -> Stats {
    let sum: f64 = ys.iter().sum();
    Stats {
        avg: sum / ys.len() as f64,
        min: ys.iter().copied().fold(f64::INFINITY, f64::min),
        max: ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{LqmRecord, LqmValues, PingRecord};

    const A: &str = "aa:aa:aa:aa:aa:aa";
    const B: &str = "bb:bb:bb:bb:bb:bb";

    fn at(secs: i64) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339("2026-11-08T10:00:00+09:00").unwrap()
            + chrono::Duration::seconds(secs)
    }

    fn window(from: i64, to: i64) -> Window {
        Window { from: at(from), to: at(to) }
    }

    fn lqm(secs: i64, net: Option<&str>, rssi: i32, retx: u32) -> Record {
        Record::Lqm(LqmRecord {
            t: at(secs),
            net: net.map(String::from),
            v: LqmValues {
                rssi,
                noise: -95,
                snr: 30,
                cca: 10.0,
                tx: 800.0,
                rx: 800.0,
                txf: 10,
                fail: 0,
                retx,
                ch: Some(36),
                bw: Some(80),
            },
        })
    }

    fn ping(secs: i64, net: &str, dst: Target, ms: Option<f64>) -> Record {
        Record::Ping(PingRecord { t: at(secs), net: Some(net.into()), dst, ms })
    }

    fn every_5s(from: i64, to: i64, net: Option<&str>) -> Vec<Record> {
        (from..to).step_by(5).map(|s| lqm(s, net, -40, 0)).collect()
    }

    #[test]
    fn labels_networks_in_order_and_records_switch() {
        let mut recs = every_5s(0, 60, Some(A));
        recs.extend(every_5s(60, 120, Some(B)));
        let m = build(&recs, window(0, 120), 24);
        let labels: Vec<char> = m.networks.iter().map(|n| n.label).collect();
        assert_eq!(labels, vec!['A', 'B']);
        assert_eq!(m.switches, vec![(60.0, 'B')]);
    }

    #[test]
    fn unknown_network_is_question_mark() {
        let m = build(&every_5s(0, 30, None), window(0, 30), 10);
        assert_eq!(m.networks[0].label, '?');
    }

    #[test]
    fn line_breaks_on_gap_longer_than_15s() {
        let mut recs = every_5s(0, 15, Some(A));
        recs.extend(every_5s(70, 85, Some(A)));
        let m = build(&recs, window(0, 90), 90);
        assert_eq!(m.lines[&Metric::Rssi].len(), 2);
    }

    #[test]
    fn line_breaks_on_network_switch() {
        let mut recs = every_5s(0, 60, Some(A));
        recs.extend(every_5s(60, 120, Some(B)));
        let m = build(&recs, window(0, 120), 120);
        assert_eq!(m.lines[&Metric::Rssi].len(), 2);
    }

    #[test]
    fn buckets_average_and_sum() {
        let recs = vec![
            lqm(20, Some(A), -40, 1),
            lqm(25, Some(A), -50, 2),
            lqm(35, Some(A), -60, 4),
        ];
        let m = build(&recs, window(0, 60), 2);
        assert_eq!(m.lines[&Metric::Rssi], vec![vec![(15.0, -45.0), (45.0, -60.0)]]);
        let retx: Vec<f64> = m.lines[&Metric::Retx][0].iter().map(|p| p.1).collect();
        assert_eq!(retx, vec![3.0, 4.0]);
    }

    #[test]
    fn stats_and_loss_are_per_network() {
        let recs = vec![
            ping(0, A, Target::Router, Some(2.0)),
            ping(5, A, Target::Router, Some(4.0)),
            ping(10, B, Target::Router, Some(20.0)),
            ping(15, B, Target::Router, None),
        ];
        let m = build(&recs, window(0, 20), 20);
        assert_eq!(m.stats[&(Metric::RouterMs, 'A')].avg, 3.0);
        assert_eq!(m.stats[&(Metric::RouterMs, 'B')].avg, 20.0);
        assert_eq!(m.loss[&(Target::Router, 'A')], 0.0);
        assert_eq!(m.loss[&(Target::Router, 'B')], 0.5);
    }

    #[test]
    fn latest_uses_last_network_only() {
        let recs = vec![lqm(0, Some(A), -40, 0), lqm(5, Some(B), -70, 0)];
        let m = build(&recs, window(0, 10), 10);
        assert_eq!(m.latest[&Metric::Rssi], -70.0);
    }
}
