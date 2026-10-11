# Phase 4 時計同期・参照姿勢 API の実装報告

検証日: 2026-10-12（着手・時刻契約の合意は2026-10-11）。
ユーザー指示どおり **ドライバが報告した姿勢時刻を主軸**に採用。
追加センサー時刻の取得は前提にしない。

## 実装済み

- Linux CLOCK_MONOTONIC の本番時計と explicit device/clock/quality policy。
- control lane のClockRequest/Reply、4 timestamp／request相関／deadline／重複拒否。
- 最大8点 window のoffset/RTT/drift、anchor affine mapping、uncertaintyの時間成長。
- 同期成立・期限切れ・domain／時刻不連続・session/Stopの無効化。
- raw Poseのtimestamp／semanticsを保持したまま、mapped sample time／receive time／qualityを別DTOで取得。
- C ABI configure/status/reference timing/diagnostics、Mock時計APIとの排他。
- 既存pose validatorのstale/future/sequence/origin/frame/flagsを本番policyに接続。
- 新session直後のstopと遅延Helloの競合を修正。
- Bridge wire／preface v3。C ABI既存layout／UDS通知v1は維持。
- 補足仕様とMonado／WiVRn別々の新API引き継ぎ指示書。

## source 時刻の確認

WiVRn HEAD `4cf2c7819e3e418f8ec29b38ea741f2ac7c6c1bb` の実ビルドsourceを調査。
steamvr_lh historyは `steady_clock now + poseTimeOffset seconds * 1e9`。
latest APIでrelationとtimestampを取得でき、query APIの予測時刻とは区別できる。
Bridgeはraw値を保存してestimated semanticsで運ぶ。driver-reported timeの利用をsensor生時刻の取得と混同しない。
source relationには既存transformが適用済みなので、origin/frameとmountの二重適用はruntime担当が確認する。
詳細と一次資料は [時刻補足仕様](PHASE-4-TIMING-SPEC.md)。

Monado base `ad7610a5` に未コミットadapterを確認。
Monado os_monotonic_get_nsはCLOCK_MONOTONIC。native Archのlibstdc++ steady_clockとの
10,000 bracket checksは成功（[記録](evidence/phase-4-clock-domain.txt)）。Pi/別stdlibには一般化しない。

## native 検証

Linux x86_64、rustc/cargo 1.99.0、cache `CARGO_HOME=/tmp/light-bridge-cargo`。

| 検証 | 結果 |
|---|---|
| workspace build --locked --offline | 成功 |
| fmt --check | 成功 |
| clippy --workspace --all-targets --locked --offline -D warnings | 成功 |
| cargo test --workspace --locked --offline | 63 passed、0 failed |
| C ABI smoke | in-memory／TCP／live timing × shared／static、6実行に成功 |
| separate-process network-session | 成功、Mock USB/runtime資源=0 |
| diff/header snapshot | 成功 |

[Rust log](evidence/phase-4-cargo-test.txt)、[C ABI log](evidence/phase-4-c-ffi.txt)、
[別プロセス log](evidence/phase-4-network-session.txt)。
新規試験はknown offset、正負drift、varying delay／非対称性／high RTT、replay/domain fault/reset/expiry、
actual TCP／monotonic clockのreference配信、raw↔mapped metadata、stale拒否、新session resetを対象にする。
実TCPでもpose sourceとUSBはMock。native時刻の一致をクロスマシン同期精度の証拠にしない。

## 未完了・未検証

**Phase 4の実機合格条件は未達。** このrepositoryで時計同期基盤を実装したが、
WiVRnの実Lighthouse publisher／Monado sinkへの新API接続は各担当の次作業。
既存runtimeのビルド／テスト成功はユーザー報告であり、今回のBridge v3に対する再検証ではない。
両submoduleを同じ新Bridge commitに更新してbuild/testする必要がある。

- 実driver poseTimeOffset／source uncertainty／origin/frameの実機ログ。
- Orange Pi arm64／clock domain、実USBIP、Wi-Fiの非対称性／jitter／mapping boundの妥当性。
- 実referenceのHost→HMD→Monado配信と変換時刻の確認。
- mount/origin較正、ローカルIMUとの時間整合、LLFA融合。HEADは既存arpara IMUを維持。
- 宣言MSRV 1.80／remote CI／異なるC++時計実装。

3点の同期成立とdrift推定は実験用アルゴリズム。uncertaintyはconfigured frequency boundと
非負通信遅延の仮定に依存し、hardware精度を保証しない。remote認証・暗号化は引き続き未実装。
既存USB resource ownership／release→detach→unexport、UDS契約、BSB範囲は維持。

[Monado指示書](HANDOFF-MONADO.md)／[WiVRn指示書](HANDOFF-WIVRN.md) へPhase 4項目を追記した。
