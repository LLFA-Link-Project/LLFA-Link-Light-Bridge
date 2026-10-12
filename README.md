# LLFA-Link-Light-Bridge

実装指示書 v0.3.1 に基づく Phase 0/1、Phase 2 の実 UDS/TCP・Mock 縦断実装、Phase 3 の Linux USB/IP Backend、Phase 4 の時計同期と参照姿勢 API。
Monado 内の `light-bridge-hmd` と WiVRn Server 内の `light-bridge-host` を主動作核とする。
既存 WiVRn／Monado のツリーは変更していない。USB 操作専用の特権 helper を追加したが、
Bridge の主動作核は両 runtime プロセス内に維持する。

## 実行

通常利用で手動 IP／policy 設定を不要にする次の設計は
[接続連動の自動準備](docs/AUTOMATIC-BOOTSTRAP-DESIGN.md)。設計段階で、現行 code は下記の手動準備方式。

開発用 helper は永続 service 登録なしで一時起動できる。
明示 sudo 承認、互換性確認、正常 cleanup 後の停止手順は
[TEMPORARY-HELPER.md](docs/TEMPORARY-HELPER.md) を参照。installer／Dashboard の選択 UI は未実装。

Rust stable と C compiler が必要。C ヘッダは `build.rs` が Rust の build dependency
`cbindgen` で自動生成する。Python／cbindgen CLI のインストールや手動の前処理は不要。
初回は `cargo build --workspace --locked` で依存を取得する。キャッシュ後は offline ビルドできる。

```sh
cargo build --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo test --workspace --offline
bash tests/ffi/run.sh
cargo run -p mock-session --offline
cargo run -p network-session --locked --offline -- --processes
cargo run -p usbip-probe --locked --offline -- --device 28de:2300
```

`mock-session` は同一テストプロセスで Mock Host／HMD を動かす headless harness。
Hello → Prepare → export → attach/列挙 → TrackingReady → reference pose → Stop → Stopped を実行する。
C smoke は Mock WiVRn Client／Server adapter と Mock Monado reference adapter を経由する。
Tracker の無較正 pose を head pose として有効化しない。

`cargo build` の通常出力は FFI crate の Cargo `OUT_DIR/light_bridge.h`。
C/CMake 向けの配置先を指定する場合は、ビルド時に絶対パスの `LLB_HEADER_DIR` を指定する。
`tests/ffi/run.sh` は `target/ffi-include` へ生成して、そのヘッダで C smoke をビルドする。

```sh
LLB_HEADER_DIR="$PWD/target/ffi-include" cargo build -p light-bridge-ffi --locked
```

## 成果物

| 場所 | 責務 |
|---|---|
| `crates/light-bridge-core` | 状態機械、EndpointInfo、姿勢 DTO／検証、明示 clock mapping、IMU 拡張 stub |
| `crates/light-bridge-protocol` | versioned binary frame、bounded Mock／2-lane TCP transport、UDS 通知 |
| `crates/light-bridge-usbip` | inventory／planner、Mock、Linux USB/IP proxy／所有権 journal／USB worker |
| `crates/light-bridge-hmd` | HMD library、export 制御、専用 pose receiver worker／最新参照姿勢 |
| `crates/light-bridge-host` | Host library、prepare／attach／tracking ready／publish／detach |
| `crates/light-bridge-ffi` | 共通 C ABI、role 別 opaque handle、staticlib／cdylib |
| `adapters/*/mock_adapter.h` | 実 runtime に依存しない C adapter stand-in |
| `tools/mock-session` | in-memory Mock 縦断テスト |
| `tools/network-session` | 実 UDS/TCP・独立 Mock USB・別プロセス runtime stand-in 試験 |
| `tools/usbip-helper` | root policy／peer UID を検証する Linux USB/IP 操作専用 helper |
| `tools/usbip-probe` | sysfs inventory／USB/IP 前提条件の read-only 診断 |

## 現段階の範囲

in-memory bounded transport と、実 TCP（制御／Pose 独立 socket）、実 UDS Start/Stop 通知を実装。
`network-session --processes` は Client→HMD 通知、Host→HMD→Mock Monado reference、stop を
別プロセスで試験する。runtime adapters／clock／pose source は Mock のまま。
Linux kernel backend は標準 usbip CLI と特権 helper を使い、専用 USB worker から操作する。
C ABI は `llb_configure_kernel_usbip` で明示選択し、`llb_tcp_listen/accept/connect` で接続する。
未設定時は Mock。Bridge TCP v3 は exact Tracker identity を伝えるが、remote 認証・暗号化は未実装。
実 USB/IP attach/detach、helper 配備、現行 Phase 4 API の Monado／WiVRn へのリンク、実機での時計推定精度、融合／較正、
Orange Pi arm64 は未検証。現 PC の Tracker inventory の確認を USB/IP 転送成功とは扱わない。
Phase 3 の実機合格条件は未達。実証環境の準備と回収手順は [USB/IP Backend](docs/USBIP-BACKEND.md) を参照。
`llb_mock_link` は同一プロセスの test-only 接続。Mock は OS USB を一切操作しない。
`usbip-probe` は read-only inventory。成功しても USB/IP attach されたことを意味しない。
BSB は None/DirectUsb/UsbMon の拡張型のみ。DirectUsb と UsbMon は Unsupported を返す。

[Monado 担当への引き継ぎ](docs/HANDOFF-MONADO.md)、[WiVRn 担当への引き継ぎ](docs/HANDOFF-WIVRN.md)、
[Phase 3 実装・検証報告](docs/PHASE-3-REPORT.md)、
[Phase 0/1 報告](docs/PHASE-0-1-REPORT.md)、[Phase 2 実装・未完了事項](docs/PHASE-2-REPORT.md)、[統合調査](docs/INTEGRATION-NOTES.md)、
[USB/IP 調査](docs/USBIP-RESEARCH.md)、[仕様との差分](docs/DECISIONS.md)、
[protocol](docs/PROTOCOL.md)、[ABI](docs/FFI-ABI.md)、[時計](docs/CLOCK-SYNC.md)、[Phase 4 補足仕様](docs/PHASE-4-TIMING-SPEC.md)、[Phase 4 報告](docs/PHASE-4-REPORT.md)、
[テスト範囲](docs/TEST-PLAN.md) を参照。

## ライセンス

本プロジェクトは **GNU Lesser General Public License v3.0 またはそれ以降
(SPDX: `LGPL-3.0-or-later`)** で提供します。
本文は [LICENSE](LICENSE)、LGPL v3 が参照する GNU GPL v3 の本文は [COPYING](COPYING) を参照してください。
依存ライブラリにはそれぞれのライセンスが適用されます。
