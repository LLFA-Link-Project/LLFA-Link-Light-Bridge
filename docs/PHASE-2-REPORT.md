# Phase 2 接続先通知・通信の実装報告

2026-10-11、仕様 v0.3.1。**実 UDS/TCP と Mock runtime adapter による縦断試験を実装した。
既存 Monado／WiVRn 本体への組み込みは未完了であり、Phase 2 全体を本番統合済みとは扱わない。**

## 実装と仕様の対応

| 指示書の作業 | 今回の実装・確認 |
|---|---|
| WiVRn Client → HMD IPC | versioned UDS Start/Stop、host/correlation、最大8個の IPv4/IPv6 NIC 候補。別プロセス試験は private directory 内の UDS pathname を使用 |
| Lighthouse 列挙前 PrepareTracking | Server adapter stand-in が prepare intent を記録し、Mock attach/列挙完了後だけ tracking 初期化を許可。通知の先行／後行を試験 |
| ExportReady → Attached → Ready | 実 TCP を通して HMD export 台帳と Host import 台帳を別々に更新。固定 synthetic tracker のみ |
| timestamped pose → Monado Adapter | 専用 TCP lane → HMD worker の検証 → Mock Monado reference slot。無較正 head_pose_valid=false |
| stop／再接続／timeout／競合順序 | 3セッションでライブラリ／worker 再利用、Client stop／Host stop、誤 session Stop 拒否、timeout 後 release→detach→unexport、通信断で参照無効化／join |

Rust の `network::connect/accept` は HMD/Host の既存 constructor に渡せる Transport を返す。
ライブラリを Monado／WiVRn Server プロセス内に置く構成を維持する。
C ABI に `llb_tcp_listen_mock`／`llb_tcp_accept_mock`／`llb_tcp_connect_mock` を追加。
Host bind に port=0 を指定すると割り当てた port を Endpoint に返し、Client 通知前に取得できる。
接続設定は caller control worker で期限付き実行し、start／prepare／tracking init は別イベント。

TCP は制御／Pose の2 socket、各方向 queue=16、frame≤512 bytes。
partial read/write、過大 length、未知 kind/version、別 lane の frame、host/correlation 不一致を拒否する。
送信側 queue Full は Backpressure、受信 pose queue Full は新しい frame を drop、制御 queue Full は接続終了。
両 lane の worker を cancel/join する。clock は synthetic mapping のまま、実時刻同期は Phase 4。

## 再現

```sh
cargo run -p network-session --locked --offline
cargo run -p network-session --locked --offline -- --processes
cargo run -p usbip-probe --locked --offline -- --device 28de:2300
lsusb -d 28de:2300
lsusb -t
```

最初は同一プロセスの実 socket 縦断・3セッション再接続。
`--processes` は test orchestrator 内の Mock WiVRn Client、子プロセスの Mock Host／HMD の3役。
Client は Host のテスト用広告から port を取得し、UDS に通知する。
Host/HMD が実 TCP 経由で通信し、独立 Mock ledger の resources=0 を各プロセスで検証する。
子プロセスは test harness のみ。常駐 daemon／本番の主動作核ではない。
出力例:

```text
POSE reference=[1,2,3] head_valid=false
HMD resources=0
HOST resources=0
Separate processes: WiVRn Client IPC -> HMD TCP -> Host prepare/attach -> Monado reference -> stop: OK (Mock USB/runtime)
```

## 検証結果

native Linux x86_64、Rust/Cargo 1.99.0、依存キャッシュ `/tmp/light-bridge-cargo`。

- workspace build、fmt、clippy warnings-as-errors: 成功。
- workspace Rust tests: 37 passed / 0 failed（header snapshot と別プロセス試験を含む）。
- generated header を使う C ABI smoke: in-memory／TCP × shared／static の4実行成功。
- `network-session` 同一プロセス／別プロセス、既存 `mock-session`: 成功。
- TCP IPv6 loopback、断片化 frame、過大 length、lane 混入、誤 correlation、片 lane 未接続の期限切れを検証。
- sysfs fixture で欠落 serial、inventory 不完全、HMD deny、vhci ancestry、exact identity allowlist を検証。
- 独立台帳の attach 故障→offer 破棄→次セッション再接続を確認。
- remote CI、MSRV 1.80、arm64/Orange Pi は未実行。

詳細ログ: [Rust tests](evidence/phase-2-cargo-test.txt)、[USB 診断](evidence/phase-2-usbip-probe.txt)、[lsusb](evidence/phase-2-lsusb.txt)。

## 接続された Tracker

read-only `usbip-probe` と `lsusb` で `28de:2300`、Tundra Tracker、busid `1-3.3`、
serial `LHR-CEC1027F`、3 HID interfaces と各 `usbhid` driver を確認。
現時点では `Bus 001 Device 013`。device number は再接続で変わるため identity の固定値に使わない。
`vhci_hcd` ancestor はなくローカル USB として列挙されている。
USB/IP ツールは PATH に無く、usbip_host/vhci_hcd は未ロード、module file はある。
`/dev/bus/usb` はこの実行環境から見えない。
診断成功は USB open／転送成功を意味しない。bind/attach/module load は一切実施していない。

## 未完了の実 runtime 統合と次の作業

実ソースの再確認:

- WiVRn HEAD `2795a8b63a5b91c3617328b573a460db25bd679d`。
  `server/driver/wivrn_session.cpp:174` の `steamvr_lh_create_devices` より前に準備 barrier が必要。
  `client/wivrn_client.cpp:224,237` の constructor に Host IPv4/IPv6 アドレスがある。
  handshake の WiVRn stream port を Bridge port として流用していない。
- Monado HEAD `ad7610a5561325a9b368cda5d45fe195d67f4c3b`。
  `src/xrt/targets/common/target_builder_llfa.c:102` の local Tracker open は Bridge export 時に抑止が必要。
  既存 LLFA は DIRECT override。新しい external reference sink と較正／融合は未実装。

既存 checkout は変更していない。次の runtime 組み込みには、WiVRn の Bridge port/host/correlation 広告契約、
Server lifetime owner、driver release acknowledgement、Monado reference sink を追加・build する必要がある。
stand-in の `enumerate_after_attach` 等はこのリポジトリの試験関数であり upstream API ではない。
Bridge 無効時の実 WiVRn 起動回帰は未検証。

TCP preface は相関照合であり認証ではない。接続先広告の信頼／暗号化／replay protection、
本番 UDS の peer credentials／通知権限は未実装。現 API は明示 Mock USB、信頼したテスト環境向け。
再接続試験は正常 stop 後の session 再利用であり、無応答 peer や OS crash の永続回収保証ではない。
実 device offer、実 kernel backend、特権 helper、実 attach/detach は Phase 3。
実 Lighthouse pose、時刻同期、較正／融合、HMD ローカル IMU 維持は未検証。BSB は拡張 stub のみ。
