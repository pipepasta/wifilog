# wifilog

Mac の Wi-Fi の品質を常駐して記録し、ターミナルで推移を見るためのツールです。

- 電波の値（RSSI、ノイズ、SNR、チャンネルの混雑度、送信速度、再送・失敗）を約 5 秒ごとに記録します
- ルーターとインターネット（既定は `1.1.1.1`）までの遅延とパケットロスを 5 秒ごとに記録します
- `wifilog view` でリアルタイムに、`wifilog show-report` で過去の期間を、同じ見た目のグラフで確認できます
- sudo も位置情報の許可も要りません。記録は Mac の中だけに保存し、外部には送りません

macOS 専用です（macOS 26.5、MacBook Pro M4 で動作確認）。

## インストール

```sh
cargo install --path .
```

## 使い方

```sh
wifilog start                 # 計測を裏で始める
wifilog view                  # リアルタイムで見る（止まっていれば計測も始める。閉じても計測は続く）
wifilog show-report --1h      # 過去の期間を見る（--5m / --1h / --24h / --7d。省略時は --1h）
wifilog status                # 動いているか、最後にいつ記録したか
wifilog stop                  # 計測を止める
```

### 画面

グラフを縦に 5 段並べます。全段で時刻の軸がそろっているので、同じ瞬間の変化を縦に見比べられます。

1. **RSSI / SNR**: RSSI とノイズの 2 本の線（2 本の間隔が SNR）
2. **遅延**: ルーターとインターネットまでの往復時間。見出しにパケットロス率
3. **チャンネル混雑度**
4. **送信速度**
5. **再送・失敗**

期間の途中でネットワーク（ルーター）が変わったときは、ネットワークごとに A, B, … と記号を振り、切り替わった時刻にグラフ全体を貫く縦線を引きます。見出しの数字はネットワークごとに分けて出します。

キー操作（`view`）: `q` 終了、`←` / `→` 時間幅（5 分・15 分・1 時間）、`space` 一時停止

## データ

- 場所: `~/Library/Application Support/wifilog/`
  - `samples-YYYY-MM-DD.jsonl`: 記録（1 行 1 件の JSON）。30 日より古いものは自動で消します
  - `wifilog.log`: 計測プロセスのログ
  - `wifilog.pid`: 計測プロセスの pid
- 環境変数
  - `WIFILOG_DIR`: データの場所を変える
  - `WIFILOG_INTERNET`: インターネット側の ping の宛先を変える（`wifilog start --internet <HOST>` でも指定でき、そちらが優先）

## 取れない情報

接続先の **SSID・BSSID は取れません**。macOS が位置情報の許可を持たないプログラムには伏せ字（`<redacted>`）で返すためです。代わりに **ルーターの MAC アドレス** でネットワークを区別しています。

電波の値は、`log stream` で流れてくる airportd の LQM のログから読んでいます。このログの書式は公開されていないので、macOS の更新で読めなくなる可能性があります。`wifilog status` で電波の記録時刻が更新されていなければ、これを疑ってください。

## ログイン時に自動で起動する（launchd）

`~/Library/LaunchAgents/dev.pipepasta.wifilog.plist` を作ります（`wifilog` のパスと `USER` は自分の環境に合わせてください）。

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
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/dev.pipepasta.wifilog.plist   # 登録して起動
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/dev.pipepasta.wifilog.plist     # 解除
```

launchd で動かしている間に `wifilog stop` すると、`KeepAlive` によりすぐに起動し直されます。止めたいときは `launchctl bootout` を使ってください。
