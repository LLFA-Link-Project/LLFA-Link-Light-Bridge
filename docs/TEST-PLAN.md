# テスト範囲

CI は fmt、clippy warnings-as-errors、Rust tests、生成ヘッダ整合性、C ABI smoke、headless Mock を実行する。
native 実行結果は `PHASE-0-1-REPORT.md` と `PHASE-2-REPORT.md` を参照。CI のリモート実行は未実施。

## 実装済み試験

- 状態順序、不正遷移、session mismatch、start/prepare/tracking_ready/stop 重複、handle 再利用。
- Host prepare 先行／endpoint 先行、export/attach/列挙失敗、正常停止後 ledger=0。
- Heartbeat loss／期限到達後にも stop ACK／cleanup を完了できること。
- control duplicate、同一 op 異種 command、別 session control、stop ACK の再送。
- bounded channel backpressure、recv timeout、切断。
- frame roundtrip、すべての切断位置、version mismatch、unknown kind、余剰 byte。
- HMD worker での前 session／stale／tracking_lost／uncertainty 過大／origin 変更／逆行／duplicate pose 拒否。
- mock reference pose 配信、source semantics 保持、無較正 head_pose_valid=false。
- USB HMD default deny、interface granularity 拒否、inventory ambiguity、別 owner port detach 拒否。
- active destroy の両順序、worker cancel/join、FFI pointer ownership、無効 ABI、panic barrier。
- C caller から create/start/prepare/pose/stop/destroy と null owning slot の重複 destroy。

## 未検証／次 Phase

実 TCP/UDS の認証・負荷・悪意ある peer・公開 network、UDP loss、source timestamp と origin reset の実 integration、
clock drift／RTT jitter、USB cable 抜去、OS／process crash の永続 recovery、実 runtime driver release、
無効時 WiVRn 起動回帰、Pi arm64 build、HMD ローカル IMU 維持、実 Lighthouse tracking／融合。
BSB DirectUsb/UsbMon は Unsupported stub のみ。

性能計測では USB/IP URB、Host pose source→publish、network、FFI→sink、uncertainty と age を分け、
p50/p95/p99 は実機導入後に測る。Mock 実行速度を低遅延性能の証拠にしない。

## Phase 2 追加試験

- 実 TCP IPv4/IPv6、fragmented frame、過大 length、異種 lane、相関不一致、片 lane 接続期限。
- 実 UDS の複数 NIC／IPv6 scope、Start/Stop codec、期限／allocation 前 size 拒否。
- runtime stand-in で prepare 先行／通知先行、3セッション再接続、Client/Host stop、timeout 後 cleanup。
- 通信断で Mock Monado reference を消し、両 TCP worker／HMD pose worker を join。
- 別 Host/HMD 子プロセス、pathname UDS、TCP、独立 Mock USB 台帳、各 resources=0。
- TCP C ABI smoke: shared/static libraries、無較正 reference、release 前 detach 拒否。
- read-only sysfs fixture: 欠落 serial、不完全 inventory、HMD deny、vhci ancestry、exact allowlist。
- remote mock offer: ExportReady 前 attach 拒否、実 device identity 拒否、session ownership。
