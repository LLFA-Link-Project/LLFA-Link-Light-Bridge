# Bridge protocol v3

USB/IP や WiVRn の既存プロトコルとは独立。in-memory と TCP の双方で同じ frame を使用。
フレームは big endian。Rust／C のメモリをそのままネットワークへ送らない。

| Offset | Size | 意味 |
|---|---|---|
| 0 | 4 | payload length (prefix 自体を含まない) |
| 4 | 4 | magic `LLFB` |
| 8 | 4 | version = 3 |
| 12 | 4 | kind |
| 16 | 8 | session ID (nonzero) |
| 24 | 8 | operation ID / pose sequence (nonzero) |
| 32 | variable | pose payload／device identity／空 |

identity を持たない制御 frame は32 bytes、pose frame は140 bytes。最大 frame は1024 bytes。
UsbExportReady は VID/PID、serial、canonical sysfs_path、topology、busid の
UsbDeviceIdentity JSON を payload に必須とする。未知フィールドを拒否し、他 kind の identity を拒否する。
不足／余剰 byte、不一致 length/magic/version、未知 kind を reject。未知 field は余剰 byte として reject。
将来 version は negotiation を追加するまで fail closed。

kind: Hello=1, Prepare=2, UsbExportReady=3, UsbAttached=4, TrackingReady=5,
Stop=6, Stopped=7, Heartbeat=8, StopRequest=9, Pose=10, ClockRequest=11, ClockReply=12。
StopRequest は HMD から Host runtime へ release を要求する追加 message。
UsbExportReady の identity は Host の exact import policy と照合してから attach する。
汎用 SessionOffer／capabilities negotiation／Error payload は未実装。
ClockRequest/Reply は下記 Phase 4 の固定 payload。
endpoint は下記 UDS 通知に実装。旧 Bridge v1/v2 は拒否する。

Pose payload 順序: tracked_device_id, tracking_origin_id, frame_id, source_clock_id,
sample_timestamp_ns, clock_uncertainty_ns (各8 bytes)、timestamp_semantics と flags (各4 bytes)、
position[3], quaternion XYZW[4], linear_velocity[3], angular_velocity[3] (IEEE f32、各4 bytes)。
ABI の size／abi_version は wire layout に含まず、復号時にローカルの正しいサイズを設定。
flags: 1 position valid、2 orientation valid、4 tracked、8 linear velocity valid、16 angular velocity valid。
semantics: 0 measured、1 estimated、2 query-time。

control／pose 各方向の容量16 frames。送信は try_send で Full を Backpressure として返す。
Host publish は送信成功時にだけ last sequence を進める。caller が sequence の再試行／新しいサンプルへの置換を判断する。
HMD は pose worker 内で検証し、最新 valid pose の1 slotを更新。無制限蓄積しない。

caller は deadline より十分短い間隔で poll／両方向 heartbeat を実行する。
期限は create の timeout。Host timeout は runtime release request を記録して配信を禁止、
HMD timeout は pose を無効にして StopRequest。停止処理中は timeout を再発させず cleanup ACK を処理する。
無応答 peer への無期限待機はしない。通信切断では Failed を記録し、明示 stop/cleanup を要求する。

状態:
Idle → Connecting → Negotiated → PreparingUsb → UsbAttached → TrackingInitializing → Active
→ Stopping → Cleanup → Idle。通信失敗は Failed。
Host runtime の driver release が確認されるまで detach しない。
Stop ACK の再送は cleanup 後も受理。正常 stop 後の同一 TCP 接続では HMD handle／worker を再利用する。
TCP 切断後の transport 差し替え API は未実装で、cleanup と handle 再作成が必要。

control の認証、暗号化、datagram loss、実ネットワーク rate limiting、
複数 peer／replay protection は未実装。検証ネットワークへのアクセスを制限し、公開 network listener として使用しない。

## Phase 2 TCP / UDS

TCP listener は単一 Bridge port。接続順序は control、pose の2 socket。
各 socket の preface は `LLBN` (4) + version=3 (u32) + host_id (u64) + correlation_id (u64)
+ lane (u8: control=0, pose=1)、計25 bytes、整数は big endian。
Host は期待した2 lane の preface を確認してから各 socket に ACK byte=1 を返す。
Host/correlation/lane 不一致は接続設定失敗。相関 ID は認証トークンではない。
1回の connect/accept に絶対 deadline、partial read/write 対応。複数 NIC 候補は同一 deadline 内で順に試す。
設定成功後は lane ごとの非ブロッキング worker が bounded queue と socket を橋渡しする。
送信未完了 frame／受信未完了 frame の期限は2秒。片 lane の切断／不正 frame で両 worker を終了。
control に Pose、pose に control を受信すると拒否。受信 pose queue Full は新着を drop、control Full は接続終了。
worker 終了後 caller は Disconnected を観測する（worker 内部の詳細エラー分類は API 未提供）。

UDS notification は length (u32) + `LLBI` + version=1 (u32) + kind (u32: Start=1, Stop=2)
+ host_id (u64) + correlation_id (u64) + address count (u32)、prefix 込み36 bytes。
Start は続けて各 address の UTF-8 byte length (u32) と SocketAddr 文字列（IPv6 scope ID 対応）を格納。
最大8 addresses、全体≤1024 bytes、Start の port は非零、Stop の address count は0。
未知 version/kind、余剰 byte、過大 count/length、不正 address を拒否。pose/IMU は格納しない。
通知送受信は絶対 deadline、エラー後は stream を閉じる。partial frame を消費した stream で再開しない。
Rust `ipc::send/receive` は渡された stream を nonblocking に設定する。
本番 adapter は UDS pathname／private directory と peer credential 検証の責任を持つ。

TCP は明示選択した Mock／kernel backend を運ぶ。UDS は endpoint 通知のみ。
WiVRn の認証／既存 port を流用せず、実 WiVRn の Bridge endpoint 広告契約はまだ変更していない。
USB helper の local credential 認証は Bridge remote TCP の認証にはならない。

## Local privileged helper RPC（開発用配備更新）

Bridge frame と別の JSON length-prefix protocol。現在 RPC v2、最大4096 bytes。
Describe／Acquire／Release／Inspect／Shutdown と専用 compatibility ID を使用する。
操作前に同一接続上の Describe で package version／compatibility ID／全 policy を照合し、
helper 側も version／ID／peer UID／device を検証する。旧 helper RPC v1 は拒否する。
Shutdown は一時 helper のみ、runtime acknowledgement と空 ownership journal が必要。
Bridge TCP v3、C ABI v1、通知 UDS v1 は変更しない。
詳細は [TEMPORARY-HELPER.md](TEMPORARY-HELPER.md)。

## Phase 4 ClockSync

制御 lane の Request=11／Reply=12 は session ID と独立 request counter を持つ。
payload は source_clock_id、local_clock_id (u64)、t1/t2/t3 (i64 ns)、計40 bytes、frame全体72 bytes。
request の t2/t3 は0。reply の t1 を outstanding request と照合し、t4 はHMDで採る。
未知 domain、負の時刻、不正順序／余剰 payload は拒否。同期 frame は lifecycle counter と heartbeat を更新しない。
採時位置／推定式／期限／再同期は [PHASE-4-TIMING-SPEC.md](PHASE-4-TIMING-SPEC.md) を参照。

## 接続連動の自動準備（2026-10-12）

[AUTOMATIC-BOOTSTRAP.md](AUTOMATIC-BOOTSTRAP.md) に SDK／helper と新 C API、LLBA v1／SCM_RIGHTS の契約を記載。
Bridge TCP v3、LLBI notification v1、C ABI v1 は維持。manual helper RPC v2、auto helper schema v1、
compatibility ID は `llfa-usbip-helper-3-auto-20261012`。旧 helper の不一致は中断する。
[検証報告](AUTOMATIC-BOOTSTRAP-REPORT.md) は Mock／native FD と実 root・実機未検証を分離している。
