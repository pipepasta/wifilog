//! 計測プロセス本体（`wifilog start --foreground`）。
//!
//! データ源ごとにスレッドを立て、`Event` をチャネルでメインスレッドに送る。
//! メインスレッドは `Stamper` で今のネットワークを付けてから `Appender` で書く。

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, FixedOffset, Local};
use serde::Deserialize;

use crate::net::{self, Router};
use crate::ping::{self, PingLine};
use crate::record::{LqmRecord, LqmValues, NetRecord, PingRecord, Record, Target};
use crate::{lqm, store};

/// airportd の LQM の行だけに絞る。
const LQM_PREDICATE: &str = r#"process == "airportd" AND eventMessage CONTAINS "LQM: rssi=""#;
const PING_INTERVAL_SECS: &str = "5";
const ROUTER_CHECK: Duration = Duration::from_secs(30);
const RESTART_DELAY: Duration = Duration::from_secs(5);

fn now() -> DateTime<FixedOffset> {
    Local::now().fixed_offset()
}

fn log(msg: impl std::fmt::Display) {
    eprintln!("{} {msg}", now().format("%Y-%m-%d %H:%M:%S"));
}

/// 計測を続ける。戻らない（子プロセスが終われば起動し直す）。
pub fn run_foreground(dir: &Path, internet: &str) -> Result<()> {
    log(format_args!(
        "計測を開始しました（データ: {}、インターネット側: {internet}）",
        dir.display()
    ));
    let (tx, rx) = mpsc::channel();
    let mut stamper = Stamper::new();
    let mut appender = store::Appender::new(dir.to_path_buf());

    // 最初のルーター確認を待ってから他のデータ源を動かす。
    // そうしないと、ネットワークが分かる前の記録（net: null）が先頭に入ってしまう。
    {
        let tx = tx.clone();
        thread::spawn(move || router_loop(tx));
    }
    let first = rx.recv().context("ルーターの確認ができませんでした")?;
    if let Some(rec) = stamper.handle(first) {
        appender.append(&rec)?;
    }
    {
        let tx = tx.clone();
        thread::spawn(move || lqm_loop(tx));
    }
    {
        let host = internet.to_string();
        thread::spawn(move || internet_ping_loop(host, tx));
    }

    for ev in rx {
        if let Some(rec) = stamper.handle(ev)
            && let Err(e) = appender.append(&rec) {
                log(format_args!("書き込みに失敗しました: {e:#}"));
            }
    }
    Ok(())
}

/// `log stream --style ndjson` の 1 行のうち、使う項目だけ。
#[derive(Deserialize)]
struct LogEvent {
    /// 例: "2026-11-08 14:23:08.883962+0900"
    timestamp: String,
    #[serde(rename = "eventMessage")]
    event_message: String,
}

fn lqm_loop(tx: Sender<Event>) {
    loop {
        if let Err(e) = lqm_session(&tx) {
            log(format_args!("log stream でエラー: {e:#}"));
        }
        log("log stream が終了しました。5 秒後に起動し直します");
        thread::sleep(RESTART_DELAY);
    }
}

fn lqm_session(tx: &Sender<Event>) -> Result<()> {
    let mut child = Command::new("log")
        .args(["stream", "--predicate", LQM_PREDICATE, "--style", "ndjson"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("log stream を起動できません")?;
    let stdout = child.stdout.take().expect("stdout is piped");
    for line in BufReader::new(stdout).lines() {
        let line = line?;
        // 先頭の "Filtering the log data using ..." など、JSON でない行は飛ばす
        let Ok(ev) = serde_json::from_str::<LogEvent>(&line) else { continue };
        let Ok(t) = DateTime::parse_from_str(&ev.timestamp, "%Y-%m-%d %H:%M:%S%.f%z") else {
            continue;
        };
        if let Some(v) = lqm::parse(&ev.event_message)
            && tx.send(Event::Lqm { t, v }).is_err() {
                break;
            }
    }
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

/// `ping -i 5 <host>` を起動し、出力を読むスレッドを立てる。止めるときは返した `Child` を kill する。
fn start_ping(host: &str, dst: Target, tx: Sender<Event>) -> Result<Child> {
    let mut child = Command::new("ping")
        .args(["-i", PING_INTERVAL_SECS, host])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("ping {host} を起動できません"))?;
    let stdout = child.stdout.take().expect("stdout is piped");
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(|l| l.ok()) {
            let ms = match ping::parse_line(&line) {
                PingLine::Reply { ms } => Some(ms),
                PingLine::Timeout => None,
                PingLine::Other => continue,
            };
            if tx.send(Event::Ping { t: now(), dst, ms }).is_err() {
                break;
            }
        }
    });
    Ok(child)
}

fn internet_ping_loop(host: String, tx: Sender<Event>) {
    loop {
        match start_ping(&host, Target::Internet, tx.clone()) {
            Ok(mut child) => {
                let _ = child.wait();
                log(format_args!("ping {host} が終了しました。5 秒後に起動し直します"));
            }
            Err(e) => log(format_args!("{e:#}")),
        }
        thread::sleep(RESTART_DELAY);
    }
}

/// 30 秒ごとに Wi-Fi 側のルーターを確かめ、変わったらルーター向けの ping を起動し直す。
fn router_loop(tx: Sender<Event>) {
    let mut ping: Option<(String, Child)> = None;
    loop {
        let router = net::wifi_device().and_then(|d| net::current_router(&d));
        let want_ip = router.as_ref().map(|r| r.ip.clone());

        let exited = ping
            .as_mut()
            .is_some_and(|(_, child)| matches!(child.try_wait(), Ok(Some(_))));
        let changed = ping.as_ref().map(|(ip, _)| ip.clone()) != want_ip;
        if exited || changed {
            if let Some((_, mut child)) = ping.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            if let Some(ip) = want_ip {
                match start_ping(&ip, Target::Router, tx.clone()) {
                    Ok(child) => ping = Some((ip, child)),
                    Err(e) => log(format_args!("{e:#}")),
                }
            }
        }

        if tx.send(Event::Router { t: now(), router }).is_err() {
            break;
        }
        thread::sleep(ROUTER_CHECK);
    }
}

/// 各スレッドからメインスレッドへ送るもの。
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Lqm { t: DateTime<FixedOffset>, v: LqmValues },
    Ping { t: DateTime<FixedOffset>, dst: Target, ms: Option<f64> },
    /// Wi-Fi 側のルーター。Wi-Fi に繋がっていなければ `None`。
    Router { t: DateTime<FixedOffset>, router: Option<Router> },
}

/// 今のネットワークを覚えておき、各レコードに付ける。
pub struct Stamper {
    /// まだ一度もルーターを確認していなければ `None`。
    router: Option<Option<Router>>,
}

impl Stamper {
    pub fn new() -> Stamper {
        Stamper { router: None }
    }

    fn net(&self) -> Option<String> {
        self.router.as_ref()?.as_ref()?.mac.clone()
    }

    pub fn handle(&mut self, ev: Event) -> Option<Record> {
        match ev {
            Event::Lqm { t, v } => Some(Record::Lqm(LqmRecord { t, net: self.net(), v })),
            Event::Ping { t, dst, ms } => {
                Some(Record::Ping(PingRecord { t, net: self.net(), dst, ms }))
            }
            Event::Router { t, router } => {
                if self.router.as_ref() == Some(&router) {
                    return None;
                }
                let router_ip = router.as_ref().map(|r| r.ip.clone());
                self.router = Some(router);
                Some(Record::Net(NetRecord { t, net: self.net(), router_ip }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{NetRecord, PingRecord};

    fn ts(sec: u32) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(&format!("2026-11-08T10:00:{sec:02}+09:00")).unwrap()
    }

    fn router(mac: &str) -> Option<Router> {
        Some(Router { ip: "192.168.11.1".into(), mac: Some(mac.into()) })
    }

    fn values() -> LqmValues {
        LqmValues {
            rssi: -40,
            noise: -95,
            snr: 35,
            cca: 10.0,
            tx: 866.7,
            rx: 866.7,
            txf: 10,
            fail: 0,
            retx: 0,
            ch: Some(36),
            bw: Some(80),
        }
    }

    #[test]
    fn stamps_current_network() {
        let mut s = Stamper::new();
        let a = Some("aa:aa:aa:aa:aa:aa".to_string());
        let b = Some("bb:bb:bb:bb:bb:bb".to_string());

        assert_eq!(
            s.handle(Event::Router { t: ts(0), router: router("aa:aa:aa:aa:aa:aa") }),
            Some(Record::Net(NetRecord {
                t: ts(0),
                net: a.clone(),
                router_ip: Some("192.168.11.1".into())
            }))
        );
        let Some(Record::Lqm(l)) = s.handle(Event::Lqm { t: ts(5), v: values() }) else {
            panic!("lqm expected")
        };
        assert_eq!(l.net, a);
        assert_eq!(
            s.handle(Event::Router { t: ts(30), router: router("aa:aa:aa:aa:aa:aa") }),
            None
        );

        let Some(Record::Net(n)) =
            s.handle(Event::Router { t: ts(31), router: router("bb:bb:bb:bb:bb:bb") })
        else {
            panic!("net expected")
        };
        assert_eq!(n.net, b);
        assert_eq!(
            s.handle(Event::Ping { t: ts(35), dst: Target::Router, ms: Some(2.8) }),
            Some(Record::Ping(PingRecord {
                t: ts(35),
                net: b,
                dst: Target::Router,
                ms: Some(2.8)
            }))
        );
    }

    #[test]
    fn wifi_off_becomes_unknown_network() {
        let mut s = Stamper::new();
        s.handle(Event::Router { t: ts(0), router: router("aa:aa:aa:aa:aa:aa") });

        assert_eq!(
            s.handle(Event::Router { t: ts(30), router: None }),
            Some(Record::Net(NetRecord { t: ts(30), net: None, router_ip: None }))
        );
        let Some(Record::Lqm(l)) = s.handle(Event::Lqm { t: ts(35), v: values() }) else {
            panic!("lqm expected")
        };
        assert_eq!(l.net, None);
    }
}
