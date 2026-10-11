# 時計と参照姿勢の Phase 1 境界

`ExternalPose` は source clock ID、sample timestamp、semantics、uncertainty、origin/frame/device を保持。
Mock は clock ID=1、estimated=1。受信時刻を測定時刻に上書きしない。
`ClockMapping` の符号は **HMD local time = Host sample time + offset_ns**。
受信側 synthetic now と mapping は Mock adapter が明示注入する。
既定 uncertainty=u64::MAX なので mapping を指定しない pose は拒否される。

PoseValidator は policy の対象 device、非有限値／quaternion、session、sequence、逆行時刻、
stale／future、tracked flags、clock ID、不確実性、origin/frame 変更を拒否する。
受信側 now に対する鮮度を検査し、latest slot を取り出す際も更新済み now で鮮度を再確認する。
clock mapping 変更時は latest と validator を無効化。origin reset は新しい session／明示再構成が必要。

default policy: device=1 (Mock)、age≤100ms、future≤1ms、uncertainty≤5ms。
これらは Mock 用の拒否試験値で、実機の性能目標値ではない。
Rust の PoseValidator policy は設定可能。FFI／HMD library からの policy 設定 API は次 Phase の課題。
velocity flags を保持し、reference Tracker を head pose に変換しない。融合／較正は Monado 側の次作業。

確認済み: synthetic offset=+100ns の時刻変換、未知 clock／逆行／stale／不確実性過大の拒否。
未実装: ClockSync exchange、RTT／offset／drift 推定、jitter、clock reset の完全な履歴管理、
Host runtime の光学計測時刻保証、Monado monotonic clock への実測 mapping。
実 timestamp semantics を決めるまでは measured として融合しない。
