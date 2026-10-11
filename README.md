# LLFA-Link-Light-Bridge

実装指示書 v0.3.1 に基づく Phase 0 調査・Phase 1 Mock skeleton。
Monado 内の `light-bridge-hmd` と WiVRn Server 内の `light-bridge-host` を主動作核とする。
既存 WiVRn／Monado のツリーは変更していない。daemon は作成していない。

## 実行

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
| `crates/light-bridge-protocol` | versioned binary frame と bounded Mock transport |
| `crates/light-bridge-usbip` | inventory／planner／backend trait、Mock ownership ledger／故障注入 |
| `crates/light-bridge-hmd` | HMD library、export 制御、専用 pose receiver worker／最新参照姿勢 |
| `crates/light-bridge-host` | Host library、prepare／attach／tracking ready／publish／detach |
| `crates/light-bridge-ffi` | 共通 C ABI、role 別 opaque handle、staticlib／cdylib |
| `adapters/*/mock_adapter.h` | 実 runtime に依存しない C adapter stand-in |
| `tools/mock-session` | Mock 縦断テスト・headless 実行ツール |

## 現段階の範囲

ネットワークは frame を encode/decode する **in-memory bounded transport**。
TCP/UDP、UDS 通知、認証、実 kernel USB/IP backend、特権 helper、実 runtime へのリンク、
時刻同期の offset/drift 推定、融合／較正適用、Orange Pi arm64 検証は未実装／未検証。
`llb_mock_link` は実配置で使用できる接続方式ではない。Mock は OS USB を一切操作しない。
BSB は None/DirectUsb/UsbMon の拡張型のみ。DirectUsb と UsbMon は Unsupported を返す。

[Phase 0/1 報告](docs/PHASE-0-1-REPORT.md)、[統合調査](docs/INTEGRATION-NOTES.md)、
[USB/IP 調査](docs/USBIP-RESEARCH.md)、[仕様との差分](docs/DECISIONS.md)、
[protocol](docs/PROTOCOL.md)、[ABI](docs/FFI-ABI.md)、[時計](docs/CLOCK-SYNC.md)、
[テスト範囲](docs/TEST-PLAN.md) を参照。

## ライセンス

本プロジェクトは **GNU Lesser General Public License v3.0 またはそれ以降
(SPDX: `LGPL-3.0-or-later`)** で提供します。
本文は [LICENSE](LICENSE)、LGPL v3 が参照する GNU GPL v3 の本文は [COPYING](COPYING) を参照してください。
依存ライブラリにはそれぞれのライセンスが適用されます。
