# wifilog

A tool that runs in the background on your Mac, records Wi-Fi quality, and lets you watch how it changes in the terminal.

- Records Wi-Fi metrics (RSSI, noise, SNR, channel utilization, TX rate, retries / failures) about every 5 seconds
- Records latency and packet loss to the router and to the internet (`1.1.1.1` by default) every 5 seconds
- See it live with `wifilog view`, or look back at a past period with `wifilog show-report`, both drawn as the same charts
- No sudo or Location Services permission needed. Records stay on your Mac and are never sent anywhere

macOS only (tested on macOS 26.5, MacBook Pro M4).

## Install

### From GitHub Releases (Apple Silicon)

Prebuilt binaries are published on the [Releases](https://github.com/pipepasta/wifilog/releases) page.

```sh
curl -fsSL https://github.com/pipepasta/wifilog/releases/latest/download/wifilog-aarch64-apple-darwin.tar.gz \
  | tar -xzf - wifilog
mkdir -p ~/.local/bin && mv wifilog ~/.local/bin/   # any directory on your PATH
wifilog --version
```

Or with [mise](https://mise.jdx.dev/):

```sh
mise use -g github:pipepasta/wifilog
```

Or with the GitHub CLI:

```sh
gh release download --repo pipepasta/wifilog --pattern 'wifilog-aarch64-apple-darwin.tar.gz*'
shasum -a 256 -c wifilog-aarch64-apple-darwin.tar.gz.sha256   # verify the checksum
tar -xzf wifilog-aarch64-apple-darwin.tar.gz wifilog
```

The binary is not signed or notarized. If you download the archive with a web browser, macOS Gatekeeper will refuse to run it; clear the quarantine flag first:

```sh
xattr -d com.apple.quarantine wifilog
```

(`curl` and `gh` don't set this flag, so the commands above need no extra step.)

### From source

Requires a recent Rust toolchain (the repository pins Rust 1.97.1 in `mise.toml`). This is also the way to install on Intel Macs, for which no prebuilt binary is provided (wifilog has only been tested on Apple Silicon).

```sh
cargo install --git https://github.com/pipepasta/wifilog
```

Or from a local clone:

```sh
cargo install --path .
```

## Usage

```sh
wifilog start                 # start recording in the background
wifilog view                  # watch live (starts recording if stopped; recording continues after you close it)
wifilog show-report --1h      # view a past period (--5m / --1h / --24h / --7d; --1h if omitted)
wifilog status                # whether it's running and when it last recorded
wifilog stop                  # stop recording
```

### Shell completion

`wifilog completions <shell>` prints a completion script for `bash`, `zsh`, `fish`, `elvish` or `powershell`. For zsh, add this to `~/.zshrc`:

```sh
source <(wifilog completions zsh)
```

Then `wifilog s<Tab>` offers `start` / `status` / `stop` / `show-report`, and `wifilog show-report --<Tab>` offers the periods. Typos are caught too: `wifilog veiw` suggests `view`.

### Screen

Five charts are stacked vertically. They all share the same time axis, so you can compare what happened at the same moment by reading straight down.

1. **RSSI / SNR**: two lines, RSSI and noise (the gap between them is the SNR)
2. **Latency**: round-trip time to the router and to the internet, with the packet loss rate in the title
3. **Channel utilization**
4. **TX rate**
5. **Retries / failures**

If the network (router) changes during the period, each network gets a label (A, B, …), and a vertical line is drawn through every chart at the moment of the switch. The numbers in the titles are shown separately for each network.

Keys (`view`): `q` quit, `←` / `→` time range (5 min, 15 min, 1 hour), `space` pause

## Data

- Location: `~/Library/Application Support/wifilog/`
  - `samples-YYYY-MM-DD.jsonl`: the records (one JSON object per line). Files older than 30 days are deleted automatically
  - `wifilog.log`: the collector process's log
  - `wifilog.pid`: the collector process's pid
- Environment variables
  - `WIFILOG_DIR`: change the data location
  - `WIFILOG_INTERNET`: change the internet ping target (can also be set with `wifilog start --internet <HOST>`, which takes precedence)

## What it can't capture

**The SSID and BSSID are not available.** macOS returns them redacted (`<redacted>`) to programs without Location Services permission. Networks are told apart by the **router's MAC address** instead.

Wi-Fi metrics are read from airportd's LQM messages streamed by `log stream`. This log format is undocumented, so a macOS update may break it. If the `Wi-Fi` time in `wifilog status` stops updating (or `view` shows "no Wi-Fi metrics"), suspect this first.

## Start automatically at login (launchd)

Create `~/Library/LaunchAgents/dev.pipepasta.wifilog.plist` (adjust the `wifilog` path and `USER` to your environment).

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>dev.pipepasta.wifilog</string>
  <key>ProgramArguments</key>
  <array>
    <string>/Users/USER/.cargo/bin/wifilog</string>
    <string>start</string>
    <string>--foreground</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardErrorPath</key>
  <string>/Users/USER/Library/Application Support/wifilog/wifilog.log</string>
</dict>
</plist>
```

```sh
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/dev.pipepasta.wifilog.plist   # register and start
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/dev.pipepasta.wifilog.plist     # unregister
```

If you run `wifilog stop` while it runs under launchd, `KeepAlive` restarts it immediately. Use `launchctl bootout` to stop it.

## Development

The toolchain (Rust 1.97.1 with rustfmt/clippy) and [lefthook](https://github.com/evilmartians/lefthook) are pinned in `mise.toml`.

```sh
mise install       # install Rust and lefthook
mise run setup     # install the git hooks
cargo test
cargo clippy --all-targets -- -D warnings
```

The pre-commit hook runs `rustfmt` on the staged `.rs` files and re-stages them, so every commit is formatted.

## License

[MIT](LICENSE)
