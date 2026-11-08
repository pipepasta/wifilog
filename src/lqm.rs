//! airportd が約 5 秒ごとに出す LQM (Link Quality Metric) のログメッセージを解析する。
//!
//! `[corewifi] LQM: rssi=-40dBm … noise=-95dBm snr=35 cca=11.0% … txRate=866.7Mbps txFrames=29
//!  txFail=0 txRetrans=0 … rxRate=866.7Mbps … channel = 36 BW = 80 band = 2`

use std::collections::HashMap;
use std::str::FromStr;

use crate::record::LqmValues;

/// LQM の行なら値を返す。そうでなければ `None`。
pub fn parse(message: &str) -> Option<LqmValues> {
    let body = message.split_once("LQM: ")?.1;
    let f = fields(body);
    Some(LqmValues {
        rssi: num(&f, "rssi")?,
        noise: num(&f, "noise")?,
        snr: num(&f, "snr")?,
        cca: num(&f, "cca")?,
        tx: num(&f, "txRate")?,
        rx: num(&f, "rxRate")?,
        txf: num(&f, "txFrames")?,
        fail: num(&f, "txFail")?,
        retx: num(&f, "txRetrans")?,
        ch: num(&f, "channel"),
        bw: num(&f, "BW"),
    })
}

/// `key=value` と `key = value`（channel や BW）の両方の書き方を集める。
fn fields(body: &str) -> HashMap<&str, &str> {
    let tokens: Vec<&str> = body.split_whitespace().collect();
    let mut map = HashMap::new();
    let mut i = 0;
    while i < tokens.len() {
        if i + 2 < tokens.len() && tokens[i + 1] == "=" {
            map.insert(tokens[i], tokens[i + 2]);
            i += 3;
            continue;
        }
        if let Some((key, value)) = tokens[i].split_once('=') {
            map.insert(key, value);
        }
        i += 1;
    }
    map
}

/// 単位（dBm、%、Mbps）を外して数値として読む。
fn num<T: FromStr>(f: &HashMap<&str, &str>, key: &str) -> Option<T> {
    let raw = f.get(key)?;
    let end = raw
        .find(|c: char| !(c.is_ascii_digit() || c == '-' || c == '.'))
        .unwrap_or(raw.len());
    raw[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // macOS 26.5 の MacBook Pro (M4) で実際に出ていた行
    const REAL: &str = "[corewifi] LQM: rssi=-40dBm per_ant_rssi=(-39dBm, -40dBm) noise=-95dBm snr=35 cca=11.0% ccaSelfTotal=0 ccaOtherTotal=5 interferenceTotal=5 txRate=866.7Mbps txFrames=29 txFail=0 txRetrans=0 txFallbackRate=520.0Mbps rxRate=866.7Mbps rxFrames=34 rxRetryFrames=0 rxToss=0 beaconRecv=49 beaconSched=49 txFwFrames=21 txFwFail=0 txFwRetrans=0 network = <redacted> bssid = <redacted> channel = 36 BW = 80 band = 2";

    #[test]
    fn parses_real_line() {
        let v = parse(REAL).expect("LQM line should parse");
        assert_eq!((v.rssi, v.noise, v.snr), (-40, -95, 35));
        assert_eq!((v.cca, v.tx, v.rx), (11.0, 866.7, 866.7));
        assert_eq!((v.txf, v.fail, v.retx), (29, 0, 0));
        assert_eq!((v.ch, v.bw), (Some(36), Some(80)));
    }

    #[test]
    fn ignores_other_lines() {
        assert!(parse("[corewifi] LQM-WiFi: (HighBand) rxCrsGlitch=105").is_none());
    }
}
