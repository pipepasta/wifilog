mod collector;
mod lqm;
mod net;
mod ping;
mod record;
mod store;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

/// Mac の Wi-Fi の品質を記録して、ターミナルで推移を見る
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 計測を始める（裏で動き続ける）
    Start {
        /// 裏に回らず、その場で計測し続ける（launchd から使う）
        #[arg(long)]
        foreground: bool,
        /// インターネット側の ping の宛先（既定: 環境変数 WIFILOG_INTERNET、なければ 1.1.1.1）
        #[arg(long, value_name = "HOST")]
        internet: Option<String>,
    },
}

const DEFAULT_INTERNET: &str = "1.1.1.1";

/// インターネット側の宛先。フラグ > 環境変数 `WIFILOG_INTERNET` > 既定値 の順。
fn internet_target(flag: Option<String>, env: Option<String>) -> String {
    flag.or(env).unwrap_or_else(|| DEFAULT_INTERNET.to_string())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let dir = store::data_dir()?;
    match cli.command {
        Cmd::Start { foreground, internet } => {
            let internet = internet_target(internet, std::env::var("WIFILOG_INTERNET").ok());
            if !foreground {
                bail!("裏での起動はまだ実装していません（--foreground を付けてください）");
            }
            collector::run_foreground(&dir, &internet)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internet_target_precedence() {
        assert_eq!(internet_target(Some("8.8.8.8".into()), Some("9.9.9.9".into())), "8.8.8.8");
        assert_eq!(internet_target(None, Some("9.9.9.9".into())), "9.9.9.9");
        assert_eq!(internet_target(None, None), "1.1.1.1");
    }
}
