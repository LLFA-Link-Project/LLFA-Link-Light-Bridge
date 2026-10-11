# Phase 4 時刻・参照姿勢の補足仕様

実装指示書 v0.3.1 の §5.3／Phase 4 を具体化する。2026-10-11 のユーザー指示により、
**Lighthouse ドライバが報告した姿勢時刻を主軸**にする。追加のセンサー生時刻取得は着手条件にしない。
本書は Bridge 側実装の契約。実 USB/IP／実姿勢／Orange Pi／LLFA 融合の合格証拠とは別。

## source 調査と意味

WiVRn `4cf2c7819e3e418f8ec29b38ea741f2ac7c6c1bb`、
`build-dashboard/_deps/monado-src/src/xrt/drivers/steamvr_lh/device.cpp` を確認。

- `:223` の `chrono_timestamp_ns()` は C++ `steady_clock` の ns。
- `:1279`–`:1286` は `steady_clock now + poseTimeOffset * 1e9` を history に push。
- `:824` の `get_latest_pose()` は history の timestamp と relation を対で取得。
- `:831` の `get_pose(query_time)` は予測・外挿を含む。raw driver reference の publisher は latest API を使う。
- `:1256` 以降の relation には driver-from-head／world／chaperone transform が適用済み。
  timestamp を保持することと、pose が素の Tracker 原点を表すことは別。frame と mount の二重適用を避ける。

Monado base `ad7610a5561325a9b368cda5d45fe195d67f4c3b` に未コミット adapter あり。
`src/xrt/auxiliary/os/os_time.h:319` の Monado clock は Linux CLOCK_MONOTONIC。
native Arch の libstdc++ steady_clock と CLOCK_MONOTONIC の10,000 bracket checks は成功。
再現用sourceは `tests/ffi/clock-domain.cpp`。例:

```sh
c++ -std=c++17 -Wall -Wextra -Werror tests/ffi/clock-domain.cpp -o /tmp/llb-clock-domain
/tmp/llb-clock-domain
```

この確認を Orange Pi／別 C++ runtime／time namespace に一般化しない。両担当が対象環境で確認する。

OpenVR の driver contract では poseTimeOffset は PoseUpdated 呼び出しに相対する秒単位の姿勢時刻。
Bridge は raw history timestamp を変更しない。初期 semantics=1 `estimated`（driver-reported time）。
これは sensor の光学観測時刻を取得したという主張ではない。query API を使う場合は semantics=2。
追加センサー時刻を得られなくても driver-reported reference の配信は進めてよい。
source clock uncertainty と、姿勢が物理的な観測時刻をどれだけ正確に表すかは区別する。
Host CLOCK_MONOTONIC ns と確認した integer history timestamp をそのままコピーする契約では、
source clock uncertainty=0 を「追加のsource時計変換誤差なし」として設定できる。
これはセンサー精度／poseTimeOffsetの物理的な正確さが0誤差という主張ではない。
semantics=estimatedを維持し、driver-reported timeの配信・時間整合を進める。
時計ドメインや付加変換の誤差が未確定ならu64::MAXを使い、検証を迂回しない。

## 時計ドメインと設定

Linux 初期実装は `clock_gettime(CLOCK_MONOTONIC)`。wall clock、UTC、CLOCK_MONOTONIC_RAW、
Rust Instant のプロセス起点は使わない。OS clock を調整する機能ではない。

両 adapter が start 前に `llb_configure_timing` を呼ぶ。`LlbTimingConfig` は ABI v1 の prefix と、
tracked_device_id、source_clock_id（Host）、local_clock_id（Monado）、鮮度／未来／uncertainty 制限、
sync interval、request timeout、mapping validity、relative drift bound を持つ。
clock IDs は設定した domain の session 内識別子。timestamp 単位／epoch の自動推測や UTC 変換はしない。
同じ source/local IDs と selected Tracker ID を両担当で共有し、origin/frame ID を実 source の契約に結び付ける。
別マシンの CLOCK_MONOTONIC は別 epoch。ID の一致は数値が同じであることを意味しない。

設定値には実験用既定値を提供するが、性能保証値ではない。実機計測で調整する。
未設定なら従来 Mock 時計モードを維持。live 設定後の `llb_mock_clock` は拒否する。

## ClockSync wire と採時

Bridge frame／TCP preface を **v3** に更新。旧 v2 は拒否、両 submodule を同じ Bridge commit に更新する。
C ABI layout と通知 UDS version は1を維持する。
ClockRequest=11、ClockReply=12、制御 lane、frame 全体72 bytes。
固定 payload は source_clock_id／local_clock_id (u64)、t1/t2/t3 (i64 ns)、すべて big endian。
request の t2/t3 は0。reply は t1 を echo。session と独立した非零 request counter を照合。

1. HMD control poll が t1 を採り、request を bounded queue へ送る。
2. Host control poll が request を処理する際に t2 を採る。
3. Host が reply を queue に入れる直前に t3 を採る。
4. HMD control poll が対応する reply を処理する際に t4 を採る。

採時は socket arrival の hardware timestamp ではない。queue／thread scheduling の遅れを RTT に含める。
同時 outstanding request は1、deadline 超過で破棄。request ID／t1 の不一致、重複応答は推定へ使わない。
Host は重複 request を無視。ClockSync は lifecycle の operation counter／heartbeat deadline を進めない。
USB worker／pose query に ClockSync 待機を入れない。

## 推定・変換・品質

Host 中点 H=(t2+t3)/2、HMD 中点 L=(t1+t4)/2。
RTT=(t4-t1)-(t3-t2)。固定 offset の符号は L-H。
これは RFC 5905 の4 timestamp と同じ符号関係を HMD ← Host に適用したもの。
実装は整数 i128 で差分を計算し、丸め・overflow を検査する。

最大8点の window、3 valid points 以上で同期成立。100ms 以上の source span で端点差分から drift を推定し、
設定した ±max_drift_ppb に制限。短い span は drift=0、周波数の未確定性は error budget に含める。
変換は anchor を用いて:

```text
local_sample = local_anchor + (raw_host_sample - source_anchor)
             + (raw_host_sample - source_anchor) * drift_ppb / 1e9
```

uncertainty は各点の RTT/2、整数丸め、exchange 中の relative frequency bound、
window の fit residual、anchor から sample／現在時刻までの drift bound による成長を含む。
これに pose の source uncertainty を加算する。uncertainty は設定した drift bound と非負通信遅延の仮定に依存する。
4 timestamp だけでネットワーク非対称性や sensor 精度を実測・除去したとは主張しない。

高 RTT／過大 uncertainty の点は拒否。mapping が期限切れなら姿勢無効、poll 時に window を破棄し3点で再同期。
時計逆行／domain 不一致／許容 bound を超える不連続で既存 window が破棄された場合は、
HMD の live pose をその session 中は停止。新 session の start で解除する。
session 変更、Stop、通信失敗は mapping／latest を破棄。origin/frame 変更は新 session が必要。
通常の drift 更新で pose sequence／origin 履歴をリセットしない。

## Monado への受け渡しと API

- `llb_monotonic_time_ns`: clock domain 確認・adapter 用の現在時刻。
- `llb_get_clock_status`: synchronized、window 点数、generation、anchor、drift、RTT、uncertainty、validity。
- event=2 の `LlbPose`: **元の driver timestamp／semantics を保持**。
- `llb_get_reference_timing`: 最後に届けた pose の変換時刻、local receive 時刻、total uncertainty、generation。
  callback 内で再入せず、poll 復帰後に取得。コピーした pose と session/sequence を一致させる。
- `llb_get_pose_diagnostics`: cumulative rejected count と last reason。
  reason=0 accepted、1 clock/inactive、2 pose policy、3 malformed。詳細なセンサー品質診断ではない。

受信・取り出しの両方で鮮度／uncertainty を確認。最新1 slot だけ保持し無制限に蓄積しない。
Monado sink は原値と変換値を両方保存する。mapped time は Monado monotonic ns として履歴へ渡す。
受信時刻を sample time として使わず、同期未成立・期限切れ・tracking lost の reference を融合へ送らない。
時刻変換の成立は head pose の較正・融合成立ではない。既存 arpara IMU の HEAD 経路を維持する。

## 検証・未検証

synthetic offset／正負 drift／delay jitter／非対称 delay／高 RTT／domain mismatch／逆行／期限を core で検証。
actual TCP＋実 CLOCK_MONOTONIC＋Mock USB＋synthetic driver reference を縦断し、raw metadata／mapped timing を確認。
共有／静的 C ABI から configure/sync/publish/timing/stop を検査。
実 Lighthouse publisher の接続、Orange Pi 時計確認、実 USB/IP、Wi-Fi jitter、clock bound の妥当性、
mount/origin 較正と LLFA 融合は未検証。両 runtime 担当が API を組み込み実測する。

参考一次資料（独自実装、ソース転載なし）:
[OpenVR DriverPose_t](https://github.com/ValveSoftware/openvr/blob/master/headers/openvr_driver.h)、
[RFC 5905 §8](https://www.rfc-editor.org/rfc/rfc5905.html#section-8)、
[libstdc++ steady_clock](https://github.com/gcc-mirror/gcc/blob/master/libstdc%2B%2B-v3/src/c%2B%2B11/chrono.cc)。
