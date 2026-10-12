# テスト範囲

CI は fmt、clippy warnings-as-errors、Rust tests、生成ヘッダ整合性、C ABI smoke、headless Mock を実行する。
native 実行結果は `PHASE-0-1-REPORT.md` 、`PHASE-2-REPORT.md` と `PHASE-3-REPORT.md` を参照。CI のリモート実行は未実施。

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

## Phase 3 追加試験

- Fake KernelOps／永続 journal: runtime release barrier、busy export 拒否、実測 port fingerprint、別 owner、再起動時 adopt 拒否、journal failure 前の OS mutation 禁止。
- ambiguous attach／CLI failure の pending 保持、実ファイル atomic 保存／crash `.new`、HMD deny／interface busid／不正 vhci status／未許可 UID。
- CLI 固定 argv／IPv6 scope、期限 kill/reap、失敗 exit status、helper packet size／cancel／peer UID、一般ユーザーの偽 helper 拒否。
- actual USB worker + slow Mock: control poll 非ブロッキング、export/attach 中の Stop、cancel/join、identity mismatch 拒否、Prepare 前の Stop。
- protocol v2 identity roundtrip／旧 version 拒否、kernel FFI opt-in と Mock API 拒否。
- shared/static C smoke: UDS Start/Stop codec、notification role/session/peer guard、既存 lifecycle/TCP。

これらは LinuxOps の実 kernel 操作を実行しない。フル環境では
[Backend の実機手順](USBIP-BACKEND.md) と両 handoff の acceptance checklist を実施する。
実 attach/detach、ケーブル／Wi-Fi 切断、連続再接続、実 tracking driver 検出、HMD IMU 維持、
runtime 配備、helper service、arm64/MSRV は未検証のまま分けて記録する。

## Phase 4 追加試験

- known offset、正負 drift、varying delay／非対称性を含む synthetic mapping と error budget、high RTT/domain mismatch/backward/reset/expiry。
- ClockSync codec の全切断位置、旧 wire v2／過大／余剰 payload 拒否。
- HMD request ID／t1 echo／replay、clock domain fault の session 内無効化と新 session reset。
- actual TCP＋CLOCK_MONOTONIC＋Mock USB による同期成立／raw reference保持／mapped timing／stale拒否／同期失効／session変更。
- shared/static C timing API smoke。既存の4 ABI smoke も継続。
- native libstdc++ steady_clock と CLOCK_MONOTONIC の bracket比較。

実結果は [Phase 4 報告](PHASE-4-REPORT.md)。実ドライバ・Pi・融合の試験とは区別する。

## 開発用一時 helper

- 同一 package version の異なる compatibility ID／旧 RPC v1／異なる全 policy は拒否する。
- UnixStream pair の handshake 不一致で Acquire を送らないこと、一致時に同じ stream 上で操作すること。
- Mock kernel engine の Describe は非 mutation。不一致要求で OS calls=0、空 journal／release ack の shutdown gate、所有記録保持。
- 固定 transient service argv、任意 role／program／policy path 拒否。policy の絶対 path／size／symlink／HMD 拒否。
- C shared/static smoke の新 API symbol と null handle 拒否、未設定 handle の InvalidState。
- 実 systemd／sudo／root staging／正常 stage cleanup／異常 journal recovery／ARM64 は未検証。
  実試験時は導入済み一致／不一致／停止中、二重起動、認証拒否、起動失敗、通常・異常停止を記録する。

## 接続連動の自動準備

- 手動 IP／policy JSON／busid／timing ID なしの接続から module/daemon/USB 準備、Ready、ClockSync/reference、解放。
- TCP local/peer と route FD の検証、別 connection/replay/IPv4-mapped/IPv6 scope／DHCP／複数NIC／逆方向到達失敗。
- 0/単一/複数 candidate、HMDのみ、重複serial、vhci/hub、不完全 inventory、選択後の抜き差し。
- 既存 helper の不一致／停止中、管理者認証拒否、不足 tools/modules、daemon/port/firewall 競合と両側 Prepared/Commit。
- Commit 前の mutation=0、片側失敗の準備 lease 回収、USB 開始後の release barrier、他所有 module/daemon/rule 保持。
- 新 WiVRn extension／双方向 UDS／次版 helper RPC の版拒否と Bridge OFF 回帰、取消と有界 deadline。
- root systemd／実USB/IP／Orange Pi と native Mock を分離。SDK／FakeOps／FD／C ABI の検証結果は [AUTOMATIC-BOOTSTRAP-REPORT.md](AUTOMATIC-BOOTSTRAP-REPORT.md)。実 root と統合 runtime の auto-setup 試験は未実施。
