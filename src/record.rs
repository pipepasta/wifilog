//! Types of the stored records. Written to daily files, one JSON object per line.
//!
//! Example:
//! `{"t":"…","k":"lqm","net":"aa:bb:cc:dd:ee:12","rssi":-41,…}`
//! `{"t":"…","k":"ping","net":"…","dst":"router","ms":2.8}` (`ms: null` means loss)
//! `{"t":"…","k":"net","net":"…","router_ip":"192.168.11.1"}`

use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

/// A ping target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    Router,
    Internet,
}

/// Recorded when the network (router) changes. `net` is the router's MAC address.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetRecord {
    pub t: DateTime<FixedOffset>,
    pub net: Option<String>,
    pub router_ip: Option<String>,
}

/// Radio values taken from airportd's LQM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LqmValues {
    /// Received signal strength (dBm)
    pub rssi: i32,
    /// Noise floor (dBm)
    pub noise: i32,
    /// Signal minus noise (dB)
    pub snr: i32,
    /// Channel utilization (%)
    pub cca: f64,
    /// TX link rate (Mbps)
    pub tx: f64,
    /// RX link rate (Mbps)
    pub rx: f64,
    /// Frames sent
    pub txf: u32,
    /// Frames that failed to send
    pub fail: u32,
    /// Frames retransmitted
    pub retx: u32,
    /// Channel
    pub ch: Option<u32>,
    /// Bandwidth (MHz)
    pub bw: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LqmRecord {
    pub t: DateTime<FixedOffset>,
    pub net: Option<String>,
    #[serde(flatten)]
    pub v: LqmValues,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PingRecord {
    pub t: DateTime<FixedOffset>,
    pub net: Option<String>,
    pub dst: Target,
    /// Round-trip time (ms). `None` is a timeout (loss)
    pub ms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "lowercase")]
pub enum Record {
    Net(NetRecord),
    Lqm(LqmRecord),
    Ping(PingRecord),
}

impl Record {
    pub fn t(&self) -> DateTime<FixedOffset> {
        match self {
            Record::Net(r) => r.t,
            Record::Lqm(r) => r.t,
            Record::Ping(r) => r.t,
        }
    }

    pub fn net(&self) -> Option<&str> {
        match self {
            Record::Net(r) => r.net.as_deref(),
            Record::Lqm(r) => r.net.as_deref(),
            Record::Ping(r) => r.net.as_deref(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lqm_record_json_shape() {
        let json = r#"{"t":"2026-11-08T14:23:08.883+09:00","k":"lqm","net":"aa:bb:cc:dd:ee:12","rssi":-41,"noise":-95,"snr":34,"cca":18.0,"tx":866.7,"rx":866.7,"txf":29,"fail":0,"retx":0,"ch":36,"bw":80}"#;
        let r: Record = serde_json::from_str(json).unwrap();
        let Record::Lqm(l) = &r else {
            panic!("expected lqm")
        };
        assert_eq!(l.v.rssi, -41);
        assert_eq!(l.v.bw, Some(80));
        assert_eq!(r.net(), Some("aa:bb:cc:dd:ee:12"));
        // Round-trips to the same value (key order doesn't matter)
        let back: serde_json::Value = serde_json::to_value(&r).unwrap();
        assert_eq!(
            back,
            serde_json::from_str::<serde_json::Value>(json).unwrap()
        );
    }

    #[test]
    fn ping_timeout_is_null_ms() {
        let json = r#"{"t":"2026-11-08T14:23:15.013+09:00","k":"ping","net":"aa:bb:cc:dd:ee:12","dst":"internet","ms":null}"#;
        let r: Record = serde_json::from_str(json).unwrap();
        assert!(matches!(
            r,
            Record::Ping(PingRecord {
                dst: Target::Internet,
                ms: None,
                ..
            })
        ));
    }

    #[test]
    fn net_record_with_unknown_network() {
        let json = r#"{"t":"2026-11-08T14:23:00+09:00","k":"net","net":null,"router_ip":null}"#;
        let r: Record = serde_json::from_str(json).unwrap();
        assert_eq!(r.net(), None);
    }
}
