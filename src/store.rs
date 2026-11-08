//! 記録の保存。日ごとの JSON Lines ファイル（`samples-YYYY-MM-DD.jsonl`）に追記する。
//!
//! 日付は各レコードの時刻（記録したときのローカル時刻のオフセット付き）で決める。

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Days, FixedOffset, Local, NaiveDate};

use crate::record::{NetRecord, Record};

/// 記録を残す日数。
pub const KEEP_DAYS: i64 = 30;

const PREFIX: &str = "samples-";
const SUFFIX: &str = ".jsonl";

/// データディレクトリ。環境変数 `WIFILOG_DIR` があればそれを使う。
pub fn data_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("WIFILOG_DIR") {
        return Ok(PathBuf::from(dir));
    }
    Ok(dirs::data_local_dir()
        .context("データの保存先ディレクトリが見つかりません")?
        .join("wifilog"))
}

pub fn day_file(dir: &Path, date: NaiveDate) -> PathBuf {
    dir.join(format!("{PREFIX}{}{SUFFIX}", date.format("%Y-%m-%d")))
}

fn date_of_file(path: &Path) -> Option<NaiveDate> {
    let name = path.file_name()?.to_str()?;
    name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?.parse().ok()
}

/// 日ごとのファイルへの追記。日付が変わると次のファイルに切り替える。
pub struct Appender {
    dir: PathBuf,
    date: Option<NaiveDate>,
    file: Option<File>,
    last_net: Option<NetRecord>,
}

impl Appender {
    pub fn new(dir: PathBuf) -> Appender {
        Appender { dir, date: None, file: None, last_net: None }
    }

    pub fn append(&mut self, rec: &Record) -> Result<()> {
        let date = rec.t().date_naive();
        if self.date != Some(date) {
            self.open(date, rec)?;
        }
        if let Record::Net(n) = rec {
            self.last_net = Some(n.clone());
        }
        self.write(rec)
    }

    /// その日のファイルを開く。新しいファイルなら、先頭に今のネットワークを書いておく
    /// （そのファイルだけを読んでも、どのネットワークの記録かわかるように）。
    fn open(&mut self, date: NaiveDate, rec: &Record) -> Result<()> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("{} を作れません", self.dir.display()))?;
        let path = day_file(&self.dir, date);
        let is_new = fs::metadata(&path).map(|m| m.len() == 0).unwrap_or(true);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("{} を開けません", path.display()))?;
        self.file = Some(file);
        self.date = Some(date);

        if let Err(e) = prune(&self.dir, date, KEEP_DAYS) {
            eprintln!("古い記録を消せませんでした: {e:#}");
        }
        if is_new && !matches!(rec, Record::Net(_))
            && let Some(n) = &self.last_net {
                let header = Record::Net(NetRecord { t: rec.t(), ..n.clone() });
                self.write(&header)?;
            }
        Ok(())
    }

    /// 1 行を 1 回の書き込みで書く（途中で止まっても壊れるのは最後の 1 行だけ）。
    fn write(&mut self, rec: &Record) -> Result<()> {
        let mut line = serde_json::to_string(rec)?;
        line.push('\n');
        let file = self.file.as_mut().expect("file is opened before writing");
        file.write_all(line.as_bytes())?;
        Ok(())
    }
}

/// `today - keep_days` より前の日付のファイルを消し、消したパスを返す。
pub fn prune(dir: &Path, today: NaiveDate, keep_days: i64) -> Result<Vec<PathBuf>> {
    let limit = today - chrono::Duration::days(keep_days);
    let mut removed = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if date_of_file(&path).is_some_and(|d| d < limit) {
            fs::remove_file(&path)?;
            removed.push(path);
        }
    }
    removed.sort();
    Ok(removed)
}

/// `from <= t <= to` のレコードを時刻順に返す。読めない行は飛ばす。
pub fn load_range(
    dir: &Path,
    from: DateTime<FixedOffset>,
    to: DateTime<FixedOffset>,
) -> Result<Vec<Record>> {
    let mut out = Vec::new();
    let mut date = from.date_naive();
    while date <= to.date_naive() {
        let path = day_file(dir, date);
        match fs::read_to_string(&path) {
            Ok(text) => out.extend(parse_lines(&text).filter(|r| from <= r.t() && r.t() <= to)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("{} を読めません", path.display())),
        }
        date = date + Days::new(1);
    }
    out.sort_by_key(|r| r.t());
    Ok(out)
}

fn parse_lines(text: &str) -> impl Iterator<Item = Record> + '_ {
    text.lines().filter_map(|l| serde_json::from_str(l).ok())
}

/// 今日のファイルの追記分を追いかける。作った時点のファイルの末尾から読み始める。
pub struct Tail {
    dir: PathBuf,
    date: NaiveDate,
    pos: u64,
    partial: String,
}

impl Tail {
    pub fn new(dir: PathBuf) -> Tail {
        let date = Local::now().date_naive();
        let pos = fs::metadata(day_file(&dir, date)).map(|m| m.len()).unwrap_or(0);
        Tail { dir, date, pos, partial: String::new() }
    }

    /// 前回から追記された、改行で終わった行を返す。途中の行は次回に持ち越す。
    pub fn poll(&mut self) -> Result<Vec<Record>> {
        let mut out = self.read_new()?;
        let today = Local::now().date_naive();
        if today != self.date {
            self.date = today;
            self.pos = 0;
            self.partial.clear();
            out.extend(self.read_new()?);
        }
        Ok(out)
    }

    fn read_new(&mut self) -> Result<Vec<Record>> {
        let mut file = match File::open(day_file(&self.dir, self.date)) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        file.seek(SeekFrom::Start(self.pos))?;
        let mut buf = String::new();
        self.pos += file.read_to_string(&mut buf)? as u64;
        self.partial.push_str(&buf);

        let Some(last_newline) = self.partial.rfind('\n') else {
            return Ok(Vec::new());
        };
        let complete: String = self.partial.drain(..=last_newline).collect();
        Ok(parse_lines(&complete).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{LqmRecord, LqmValues, NetRecord};
    use std::fs;
    use std::io::Write;

    fn ts(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    fn lqm(t: &str) -> Record {
        Record::Lqm(LqmRecord {
            t: ts(t),
            net: Some("aa:bb:cc:dd:ee:01".into()),
            v: LqmValues {
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
            },
        })
    }

    fn net(t: &str) -> Record {
        Record::Net(NetRecord {
            t: ts(t),
            net: Some("aa:bb:cc:dd:ee:01".into()),
            router_ip: Some("192.168.11.1".into()),
        })
    }

    fn date(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn lines(path: &Path) -> Vec<Record> {
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn appender_rotates_and_writes_net_header() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = Appender::new(dir.path().to_path_buf());
        a.append(&net("2026-11-08T23:59:58+09:00")).unwrap();
        a.append(&lqm("2026-11-08T23:59:59+09:00")).unwrap();
        a.append(&lqm("2026-11-09T00:00:03+09:00")).unwrap();

        let next = lines(&day_file(dir.path(), date("2026-11-09")));
        assert_eq!(next.len(), 2);
        let Record::Net(head) = &next[0] else { panic!("first line should be net: {next:?}") };
        assert_eq!(head.net.as_deref(), Some("aa:bb:cc:dd:ee:01"));
        assert_eq!(head.router_ip.as_deref(), Some("192.168.11.1"));
        assert_eq!(head.t, ts("2026-11-09T00:00:03+09:00"));
        assert_eq!(next[1], lqm("2026-11-09T00:00:03+09:00"));
        assert_eq!(lines(&day_file(dir.path(), date("2026-11-08"))).len(), 2);
    }

    #[test]
    fn prune_keeps_last_30_days() {
        let dir = tempfile::tempdir().unwrap();
        for d in ["2026-10-08", "2026-10-09", "2026-11-08"] {
            fs::write(day_file(dir.path(), date(d)), "").unwrap();
        }
        fs::write(dir.path().join("wifilog.log"), "log").unwrap();

        let removed = prune(dir.path(), date("2026-11-08"), 30).unwrap();

        assert_eq!(removed, vec![day_file(dir.path(), date("2026-10-08"))]);
        assert!(!day_file(dir.path(), date("2026-10-08")).exists());
        assert!(day_file(dir.path(), date("2026-10-09")).exists());
        assert!(day_file(dir.path(), date("2026-11-08")).exists());
        assert!(dir.path().join("wifilog.log").exists());
    }

    #[test]
    fn load_range_spans_midnight() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = Appender::new(dir.path().to_path_buf());
        a.append(&lqm("2026-11-08T23:40:00+09:00")).unwrap();
        a.append(&lqm("2026-11-08T23:50:00+09:00")).unwrap();
        a.append(&lqm("2026-11-09T00:10:00+09:00")).unwrap();
        a.append(&lqm("2026-11-09T00:20:00+09:00")).unwrap();

        let got = load_range(
            dir.path(),
            ts("2026-11-08T23:45:00+09:00"),
            ts("2026-11-09T00:15:00+09:00"),
        )
        .unwrap();

        assert_eq!(
            got,
            vec![lqm("2026-11-08T23:50:00+09:00"), lqm("2026-11-09T00:10:00+09:00")]
        );
    }

    #[test]
    fn load_range_skips_broken_lines() {
        let dir = tempfile::tempdir().unwrap();
        let good = serde_json::to_string(&lqm("2026-11-08T10:00:00+09:00")).unwrap();
        let good2 = serde_json::to_string(&lqm("2026-11-08T10:00:05+09:00")).unwrap();
        fs::write(
            day_file(dir.path(), date("2026-11-08")),
            format!("{good}\n{{\"k\":\"lqm\"\n{good2}\n"),
        )
        .unwrap();

        let got = load_range(
            dir.path(),
            ts("2026-11-08T09:00:00+09:00"),
            ts("2026-11-08T11:00:00+09:00"),
        )
        .unwrap();

        assert_eq!(got.len(), 2);
    }

    #[test]
    fn tail_returns_only_complete_new_lines() {
        let dir = tempfile::tempdir().unwrap();
        let today = Local::now().date_naive();
        let path = day_file(dir.path(), today);
        let line = serde_json::to_string(&lqm("2026-11-08T10:00:00+09:00")).unwrap();
        let mut f = OpenOptions::new().create(true).append(true).open(&path).unwrap();
        let mut tail = Tail::new(dir.path().to_path_buf());

        write!(f, "{line}\n{line}\n").unwrap();
        assert_eq!(tail.poll().unwrap().len(), 2);

        let (head, rest) = line.split_at(10);
        write!(f, "{head}").unwrap();
        assert_eq!(tail.poll().unwrap().len(), 0);

        writeln!(f, "{rest}").unwrap();
        assert_eq!(tail.poll().unwrap().len(), 1);
    }
}
