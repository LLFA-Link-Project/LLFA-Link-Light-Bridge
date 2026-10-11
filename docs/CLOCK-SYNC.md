# 時計と参照姿勢の現行設計（Phase 4）

**ドライバ報告姿勢時刻を主軸**にし、raw timestamp と変換時刻を別に保持する。
実装と組み込み契約は [Phase 4 時刻補足仕様](PHASE-4-TIMING-SPEC.md)、
検証範囲は [Phase 4 報告](PHASE-4-REPORT.md) を参照。

- `ExternalPose` は source clock、raw sample timestamp、semantics、source uncertainty、origin/frame/device を保持。
- Linux 本番モードは CLOCK_MONOTONIC。`llb_configure_timing` で両側の domain と pose policy を明示する。
- Bridge ClockRequest/Reply は4 timestamp を使い、offset／RTT、window の drift、uncertainty を更新。
- affine mapping の符号は HMD local = Host source + offset に対応する。同期はOS時計を調整しない。
- `llb_get_reference_timing` で Monado clock の sample time、local receive time、total uncertainty を取得。
  callback の raw Pose は書き換えない。poll 復帰後に session/sequence を照合する。
- 同期未成立／期限切れ、stale/future、逆行、tracking lost、過大 uncertainty、origin/frame/session 不整合を拒否。
- mapping 期限切れは3点で再同期。既存 window を壊す時計不連続／domain fault は新 session が必要。

従来 `llb_mock_clock` の fixed offset と synthetic now は試験専用として維持。
未設定は uncertainty=u64::MAX。live 設定後に Mock 時計 API は使用できない。
実験用 policy 既定値は age≤100ms、future≤1ms、uncertainty≤5ms、drift bound 200ppm。
これらは性能保証ではなく、実機でsource品質／ネットワークを測って調整する。

確認済み: known offset／正負 drift／jitter／非対称遅延／reset／expiry の synthetic 試験、
実 TCP と実 monotonic clock の Mock reference、共有／静的 C ABI、native Arch の clock domain 比較。
未検証: 実 Lighthouse publisher 接続、driver uncertainty の実測、Orange Pi domain、Wi-Fi delay、
実 runtime の新 API linking、mount/origin 較正／LLFA 融合。
センサー生 timestamp の取得を前提にせず driver-reported time で配信を進める。
