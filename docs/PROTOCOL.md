# Bridge Mock protocol v1

USB/IP や WiVRn の既存プロトコルとは独立。現段階は in-memory frame transport。
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
endpoint／identity／capabilities の wire schema は実通信導入前に追加する。

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

control の認証、暗号化、TCP partial read/write、datagram loss、実ネットワーク rate limiting、
複数 peer／replay protection は未実装。Mock frame codec をそのまま公開 network listener として使用しない。
