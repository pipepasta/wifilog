//! The collector process itself (`wifilog start --foreground`).
//!
//! Spawns a thread per data source, each sending `Event`s to the main thread over a channel.
//! The main thread tags them with the current network via `Stamper`, then writes them with `Appender`.

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

/// Narrows the stream down to airportd's LQM lines.
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

/// Keeps collecting. Never returns (child processes are restarted when they exit).
pub fn run_foreground(dir: &Path, internet: &str) -> Result<()> {
    log(format_args!(
        "Started collecting (data: {}, internet target: {internet})",
        dir.display()
    ));
    let (tx, rx) = mpsc::channel();
    let mut stamper = Stamper::new();
    let mut appender = store::Appender::new(dir.to_path_buf());

    // Wait for the first router check before starting the other sources.
    // Otherwise records from before the network is known (net: null) would end up at the top.
    {
        let tx = tx.clone();
        thread::spawn(move || router_loop(tx));
    }
    let first = rx.recv().context("could not check the router")?;
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
            && let Err(e) = appender.append(&rec)
        {
            log(format_args!("write failed: {e:#}"));
        }
    }
    Ok(())
}

/// The fields we use from one `log stream --style ndjson` line.
#[derive(Deserialize)]
struct LogEvent {
    /// e.g. "2026-11-08 14:23:08.883962+0900"
    timestamp: String,
    #[serde(rename = "eventMessage")]
    event_message: String,
}

fn lqm_loop(tx: Sender<Event>) {
    loop {
        if let Err(e) = lqm_session(&tx) {
            log(format_args!("log stream error: {e:#}"));
        }
        log("log stream exited; restarting in 5 seconds");
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
        .context("cannot start log stream")?;
    let stdout = child.stdout.take().expect("stdout is piped");
    for line in BufReader::new(stdout).lines() {
        let line = line?;
        // Skip non-JSON lines such as the leading "Filtering the log data using ..."
        let Ok(ev) = serde_json::from_str::<LogEvent>(&line) else {
            continue;
        };
        let Ok(t) = DateTime::parse_from_str(&ev.timestamp, "%Y-%m-%d %H:%M:%S%.f%z") else {
            continue;
        };
        if let Some(v) = lqm::parse(&ev.event_message)
            && tx.send(Event::Lqm { t, v }).is_err()
        {
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

/// Starts `ping -i 5 <host>` and a thread that reads its output. Kill the returned `Child` to stop it.
fn start_ping(host: &str, dst: Target, tx: Sender<Event>) -> Result<Child> {
    let mut child = Command::new("ping")
        .args(["-i", PING_INTERVAL_SECS, host])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("cannot start ping {host}"))?;
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
                log(format_args!("ping {host} exited; restarting in 5 seconds"));
            }
            Err(e) => log(format_args!("{e:#}")),
        }
        thread::sleep(RESTART_DELAY);
    }
}

/// Checks the Wi-Fi router every 30 seconds and restarts the router ping when it changes.
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

/// What each thread sends to the main thread.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Lqm {
        t: DateTime<FixedOffset>,
        v: LqmValues,
    },
    Ping {
        t: DateTime<FixedOffset>,
        dst: Target,
        ms: Option<f64>,
    },
    /// The Wi-Fi router. `None` when Wi-Fi is not connected.
    Router {
        t: DateTime<FixedOffset>,
        router: Option<Router>,
    },
}

/// Remembers the current network and tags each record with it.
pub struct Stamper {
    /// `None` until the router has been checked once.
    router: Option<Option<Router>>,
    /// When the network last changed, and the network before that
    changed: Option<(DateTime<FixedOffset>, Option<String>)>,
}

impl Stamper {
    pub fn new() -> Stamper {
        Stamper {
            router: None,
            changed: None,
        }
    }

    fn net(&self) -> Option<String> {
        self.router.as_ref()?.as_ref()?.mac.clone()
    }

    /// The network a record at time `t` belongs to. LQM carries the log's timestamp, so it can arrive
    /// after the router check. Times before the switch count as the previous network.
    fn net_at(&self, t: DateTime<FixedOffset>) -> Option<String> {
        match &self.changed {
            Some((at, prev)) if t < *at => prev.clone(),
            _ => self.net(),
        }
    }

    /// Same IP with only the MAC missing (a transient arp failure) does not count as a change.
    fn is_same_network(&self, router: &Option<Router>) -> bool {
        let Some(current) = &self.router else {
            return false;
        };
        match (current, router) {
            (Some(cur), Some(new)) => cur.ip == new.ip && (new.mac.is_none() || cur.mac == new.mac),
            (None, None) => true,
            _ => false,
        }
    }

    pub fn handle(&mut self, ev: Event) -> Option<Record> {
        match ev {
            Event::Lqm { t, v } => Some(Record::Lqm(LqmRecord {
                t,
                net: self.net_at(t),
                v,
            })),
            Event::Ping { t, dst, ms } => Some(Record::Ping(PingRecord {
                t,
                net: self.net_at(t),
                dst,
                ms,
            })),
            Event::Router { t, router } => {
                if self.is_same_network(&router) {
                    return None;
                }
                let router_ip = router.as_ref().map(|r| r.ip.clone());
                if self.router.is_some() {
                    self.changed = Some((t, self.net()));
                }
                self.router = Some(router);
                Some(Record::Net(NetRecord {
                    t,
                    net: self.net(),
                    router_ip,
                }))
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
        Some(Router {
            ip: "192.168.11.1".into(),
            mac: Some(mac.into()),
        })
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
            s.handle(Event::Router {
                t: ts(0),
                router: router("aa:aa:aa:aa:aa:aa")
            }),
            Some(Record::Net(NetRecord {
                t: ts(0),
                net: a.clone(),
                router_ip: Some("192.168.11.1".into())
            }))
        );
        let Some(Record::Lqm(l)) = s.handle(Event::Lqm {
            t: ts(5),
            v: values(),
        }) else {
            panic!("lqm expected")
        };
        assert_eq!(l.net, a);
        assert_eq!(
            s.handle(Event::Router {
                t: ts(30),
                router: router("aa:aa:aa:aa:aa:aa")
            }),
            None
        );

        let Some(Record::Net(n)) = s.handle(Event::Router {
            t: ts(31),
            router: router("bb:bb:bb:bb:bb:bb"),
        }) else {
            panic!("net expected")
        };
        assert_eq!(n.net, b);
        assert_eq!(
            s.handle(Event::Ping {
                t: ts(35),
                dst: Target::Router,
                ms: Some(2.8)
            }),
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
        s.handle(Event::Router {
            t: ts(0),
            router: router("aa:aa:aa:aa:aa:aa"),
        });

        assert_eq!(
            s.handle(Event::Router {
                t: ts(30),
                router: None
            }),
            Some(Record::Net(NetRecord {
                t: ts(30),
                net: None,
                router_ip: None
            }))
        );
        let Some(Record::Lqm(l)) = s.handle(Event::Lqm {
            t: ts(35),
            v: values(),
        }) else {
            panic!("lqm expected")
        };
        assert_eq!(l.net, None);
    }

    #[test]
    fn late_lqm_keeps_previous_network() {
        // Log timestamps predate arrival at the main thread, so an old LQM arriving right after a switch belongs to the previous network
        let mut s = Stamper::new();
        s.handle(Event::Router {
            t: ts(0),
            router: router("aa:aa:aa:aa:aa:aa"),
        });
        s.handle(Event::Router {
            t: ts(30),
            router: router("bb:bb:bb:bb:bb:bb"),
        });
        let Some(Record::Lqm(old)) = s.handle(Event::Lqm {
            t: ts(28),
            v: values(),
        }) else {
            panic!("lqm expected")
        };
        assert_eq!(old.net.as_deref(), Some("aa:aa:aa:aa:aa:aa"));
        let Some(Record::Lqm(new)) = s.handle(Event::Lqm {
            t: ts(33),
            v: values(),
        }) else {
            panic!("lqm expected")
        };
        assert_eq!(new.net.as_deref(), Some("bb:bb:bb:bb:bb:bb"));
    }

    #[test]
    fn arp_failure_with_same_ip_keeps_network() {
        let mut s = Stamper::new();
        s.handle(Event::Router {
            t: ts(0),
            router: router("aa:aa:aa:aa:aa:aa"),
        });
        let no_mac = Some(Router {
            ip: "192.168.11.1".into(),
            mac: None,
        });
        assert_eq!(
            s.handle(Event::Router {
                t: ts(30),
                router: no_mac
            }),
            None
        );
        let Some(Record::Lqm(l)) = s.handle(Event::Lqm {
            t: ts(35),
            v: values(),
        }) else {
            panic!("lqm expected")
        };
        assert_eq!(l.net.as_deref(), Some("aa:aa:aa:aa:aa:aa"));
    }
}
