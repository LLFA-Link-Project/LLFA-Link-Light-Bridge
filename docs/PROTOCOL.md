# Bridge Mock protocol v1

USB/IP や WiVRn の既存プロトコルとは独立。in-memory と TCP の双方で同じ frame を使用。
フレームは big endian。Rust／C のメモリをそのままネットワークへ送らない。

| Offset | Size | 意味 |
|---|---|---|
| 0 | 4 | payload length (prefix 自体を含まない) |
| 4 | 4 | magic `LLFB` |
| 8 | 4 | version = 1 |
| 12 | 4 | kind |
| 16 | 8 | session ID (nonzero) |
| 24 | 8 | operation ID / pose sequence (nonzero) |
| 32 | variable | pose payload または空 |

制御 frame は32 bytes、pose frame は140 bytes。最大 frame は512 bytes。
不足／余剰 byte、不一致 length/magic/version、未知 kind を reject。未知 field は余剰 byte として reject。
将来 version は negotiation を追加するまで fail closed。

kind: Hello=1, Prepare=2, UsbExportReady=3, UsbAttached=4, TrackingReady=5,
Stop=6, Stopped=7, Heartbeat=8, StopRequest=9, Pose=10。
StopRequest は HMD から Host runtime へ release を要求する追加 message。
phase1 は選択対象が固定 Mock device のため、SessionOffer／UsbDeviceOffer／ClockSync／Error payload は未実装。
endpoint は下記 UDS 通知に実装。実 device identity／capabilities は Phase 3 前に追加が必要。

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
Stop ACK の再送は cleanup 後も受理。再接続で HMD handle／worker を作り直さない。

control の認証、暗号化、datagram loss、実ネットワーク rate limiting、
複数 peer／replay protection は未実装。Mock frame codec をそのまま公開 network listener として使用しない。

## Phase 2 TCP / UDS

TCP listener は単一 Bridge port。接続順序は control、pose の2 socket。
各 socket の preface は `LLBN` (4) + version (u32) + host_id (u64) + correlation_id (u64)
+ lane (u8: control=0, pose=1)、計25 bytes、整数は big endian。
Host は期待した2 lane の preface を確認してから各 socket に ACK byte=1 を返す。
Host/correlation/lane 不一致は接続設定失敗。相関 ID は認証トークンではない。
1回の connect/accept に絶対 deadline、partial read/write 対応。複数 NIC 候補は同一 deadline 内で順に試す。
設定成功後は lane ごとの非ブロッキング worker が bounded queue と socket を橋渡しする。
送信未完了 frame／受信未完了 frame の期限は2秒。片 lane の切断／不正 frame で両 worker を終了。
control に Pose、pose に control を受信すると拒否。受信 pose queue Full は新着を drop、control Full は接続終了。
worker 終了後 caller は Disconnected を観測する（worker 内部の詳細エラー分類は API 未提供）。

UDS notification は length (u32) + `LLBI` + version (u32) + kind (u32: Start=1, Stop=2)
+ host_id (u64) + correlation_id (u64) + address count (u32)、prefix 込み36 bytes。
Start は続けて各 address の UTF-8 byte length (u32) と SocketAddr 文字列（IPv6 scope ID 対応）を格納。
最大8 addresses、全体≤512 bytes、Start の port は非零、Stop の address count は0。
未知 version/kind、余剰 byte、過大 count/length、不正 address を拒否。pose/IMU は格納しない。
通知送受信は絶対 deadline、エラー後は stream を閉じる。partial frame を消費した stream で再開しない。
Rust `ipc::send/receive` は渡された stream を nonblocking に設定する。
本番 adapter は UDS pathname／private directory と peer credential 検証の責任を持つ。

今の TCP/UDS は固定 synthetic tracker・Mock backend の試験用。WiVRn の認証／既存 port を
流用せず、実 WiVRn の Bridge endpoint 広告契約はまだ変更していない。
