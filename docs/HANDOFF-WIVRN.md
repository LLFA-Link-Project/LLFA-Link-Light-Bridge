# WiVRn 担当 Codex への LLFA Bridge 組み込み指示書

## 引き継ぎの基準：既存統合への差分実装

確認済み baseline は **WiVRn b89ffb72、Bridge submodule d765f16**。
新規 adapter を一から作る指示ではない。実 checkout と自プロジェクトの
`docs/light-bridge.md` を正として、以下の差分を適用する。
ここでの確認はソース確認であり、Orange Pi／実 USB/IP の成功を意味しない。

| 既存の実装 | 自動準備に必要な変更 |
| --- | --- |
| Client の getpeername による Host IP 取得、endpoint packet、UDS sender | 再利用し、双方 local/peer 取得と双方向 bootstrap／inventory/endpoint 交換を追加 |
| target_instance_wivrn の policy読込→Host生成→広告→wait_usb | 先に BootstrapRequest を送る。Offer で session policy 確定後に Host 設定／preflight／広告 |
| light_bridge_host の互換性 preflight、専用 worker、prepare/Ready 管理 | 再利用。helper の environment preparation と双方 Prepared/Commit を前置 |
| wivrn_session の最新 Lighthouse history publisher と終了時 join | 再利用。選択 serial と timing IDs を自動準備結果から取得 |
| driver release→detach と fresh Server child 再接続 | 再利用。追加の daemon／network lease を USB 解放後に回収 |
| opt-in protocol negotiation／Bridge OFF の従来接続 | 再利用し、bootstrap の新 capability と版拒否・無効時回帰を追加 |

SDK の次版 bootstrap／helper API に依存する部分を区別し、候補関数名を存在する API と扱わない。
既存の manual-policy／Mock／Lighthouse publisher 試験を維持して自動準備試験を追加する。
下記 Phase 4／Phase 3 部分は既存契約と検証条件であり、再実装の作業一覧ではない。

## WiVRn 接続連動の自動準備（最新要求、設計・未実装）

[AUTOMATIC-BOOTSTRAP-DESIGN.md](AUTOMATIC-BOOTSTRAP-DESIGN.md) を次の実装契約にする。
WiVRn 接続確立後、listener の固定 Import policy preflight／Lighthouse 初期化より前に BootstrapRequest を送る。
Client は映像待機前から Monado と双方向 UDS Prepare/Offer/Commit を処理する。
各 TCP socket の local/peer address を取得して交換し、Pi の verified USB/IP endpoint と exact identity を Host へ返す。
Host helper の準備完了後に Commit と Bridge endpoint を送る。手動 IP／policy JSON／serial／timing ID を通常利用で要求しない。
Dashboard／Client UI は候補が複数の場合の選択と、一時 helper の管理者承認を扱う。USB 固有操作は helper／Bridge に保持する。
extension packet／UDS codec／次版 helper bootstrap SDK は新規実装が必要。既存通知 packet に無言で fields を追加しない。
現在 WiVRn b89ffb72 は d765f16 と新 preflight を実装済みだが、Pi USB/IP endpoint／inventory の自動交換は未実装。
以下の手動 policy 指示は現行診断経路の説明。auto mode の必須設定として引き継がない。

## Helper 配備の追加要求（2026-10-12）

[Helper 利用契約](HELPER-DEPLOYMENT-CONTRACT.md) を次の導入設計に適用する。
導入済み互換 helper を利用、不一致なら開始中断と更新要求、未導入だけ installer／一時管理者利用の選択。
Dashboard／launcher が選択と診断を扱い、Server の pose 経路で対話や管理者コマンドを実行しない。
SDK／CLI の handshake と一時起動は本版で追加。installer と選択 UI は未実装。
[一時利用手順](TEMPORARY-HELPER.md) に従って、まず開発者が CLI で起動する。
WiVRn b89ffb72 は `server/light_bridge_host.cpp` に listener 作成前の新 preflight を実装済み。
失敗時に広告／tracking 初期化を中断して診断する動作を再利用する。
SDK／helper／WiVRn は同じ版を使用し、root RPC v1 と混在させない。
Dashboard の管理者認証は後続。CLI 停止は通常 cleanup 完了後、runtime user が明示要求する。

## 目的と前提

`LLFA-Link-Light-Bridge-IMPLEMENTATION-INSTRUCTIONS-v0.3.1.md` に沿って、
**WiVRn Server プロセス内**の Bridge Host library と HMD 側 WiVRn Client の通知 adapter を維持して拡張する。
Tracker は Pi の USB/IP export を Host の vhci_hcd に import し、Lighthouse driver 初期列挙より前に準備を終える。
Bridge を別の主動作 daemon に移さない。特権 helper／標準 usbipd は別プロセスでよい。
BSB は追加しない。

本書を含む Bridge commit を記録し、Monado 担当と同じ commit を使う。
native runtime 統合、timed Lighthouse publisher、ClockSync、helper preflight は実装済み。
実転送・Orange Pi・別マシン間の同期精度・物理環境の補正効果は未検証。
実機成功や Phase 3 合格は、後述の観測が揃うまで主張しない。
最初に両 repository の AGENTS.md、Bridge README、`docs/USBIP-BACKEND.md`、`docs/FFI-ABI.md`、`docs/PROTOCOL.md` を読む。

## Phase 4 の実装済み契約・回帰確認

両担当の Phase 3 組み込み報告を受け、Bridge に live timing API を追加した。
**両 submodule を本書を含む同じ新 commit に更新する。wire v3 と旧 v2 は接続互換がない。**
既存 C ABI layout／UDS はv1を維持。既存build/runtime opt-in、IPCパス、停止・新子プロセス契約は維持する。
`docs/PHASE-4-TIMING-SPEC.md` と生成ヘッダを読み、追加sensor timestampを前提にしない。

両側で start 前に `llb_configure_timing` を成功させる。tracked_device_id／source_clock_id／local_clock_id と
quality limits を共有し、CLOCK_MONOTONIC ns の domain を実環境で確認する。
数値IDはsession内の契約。Mockの固定device=1/clock=1への合わせ込みで実poseを通さない。
Host/Pi の時計 epoch は別。poll/heartbeat を継続し、USB準備中もClockSyncに応答する。

WiVRn 16bd3529 で実 Lighthouse source の publisher が接続済み。
現在の `server/driver/wivrn_session.cpp` は選択デバイスの `steamvr_lh_get_latest_pose` を worker で読み、
元の history timestamp を publish し、device 解放前に join する。この経路を再利用する。
ドライバ変更は既存 `patches/monado` に反映し、build 生成 tree の直接変更だけで済ませない。

1. Host factoryで live TimingConfig を設定する。pollがClockRequestに応答する。
2. 選択serialの `steamvr_lh_get_latest_pose` からrelationとtimestampを対で取得し、
   **raw history timestampをそのまま sample_timestamp_ns** に入れる。新しいhistory timestampだけをpublishする。
   `get_tracked_pose(query_time)`の予測値をraw driver sampleと混同しない。
3. semantics=1 estimated（driver-reported）、source clock ID／device ID／origin/frame を契約どおり設定。
   source clock uncertaintyの根拠を記録。確認済みHost monotonic nsのinteger原値をコピーする場合は、
   追加clock変換誤差なしとして0を使える（センサーの物理誤差0の主張ではない）。
   clock domain／変換誤差が未確定なら最大値。timestampを送信時刻に置換しない。
4. relation flags／velocity validityをDTOへ写し、tracking lossもHMDがreferenceを無効化できるよう配信する。
   world／chaperone／driver-from-head適用済みのframeをMonado担当へ通知し、追加mountの二重適用を避ける。
5. source publisherのworker／借用deviceをdriver解放前に停止・joinする。既存release→detachの順序は維持する。

提出証拠にraw driver history timestamp、poseTimeOffsetの取り扱い、clock-domain bracket確認、
raw→publish→mapped→sinkのsession/sequenceログを追加。USB転送／姿勢受信／融合を別々に報告する。

## 合意済みの IPC・有効化・再接続契約（2026-10-11 追記）

- Start/Stop 通知 UDS の既定パスは `/tmp/llfa-llb.sock`。
  環境変数 `WIVRN_LIGHT_BRIDGE_IPC` で変更可能にし、WiVRn Client と Monado adapter の両方が
  同じ変数名・同じ既定値を使う。変更時は両プロセスに同じパスを設定する。
  未設定時は既定値、空文字や不正なパスは設定エラーとして扱う。
  この設定は通知 UDS 用であり、root USB/IP helper の socket パスとは別。
- Bridge はビルド・実行時とも明示的に有効化する。環境変数で UDS パスを指定しただけでは有効化しない。
  無効時は従来の接続経路と tracking 初期化を維持する。
- 停止順序は Lighthouse ドライバの解放 → USB/IP detach 許可 → Host detach 完了 → HMD unexport。
  ユーザーから WiVRn 側でこの順序を実装したとの報告を受けた。組み込み先の実コードとログで確認し、
  本 repository の Mock 成功と実 runtime の検証結果を区別する。
- Bridge 使用時の切断後の再接続は **新しい WiVRn Server 子プロセス**で行う。
  古い tracking device／USB handle／Bridge handle を再利用しない。
  旧子プロセスの driver release／detach と journal の状態を確認してから新規準備を行う。
  不確定 cleanup は既存の手動照合手順に従い、子プロセスの終了だけで資源解放済みと判断しない。
  新子プロセスは新しい非零 instance/session/correlation ID を使い、Rust worker は fork 後に開始する。
  Monado プロセス自体は維持し、古い reference を無効化して接続 handle を cleanup 後に再作成する。

例（Bridge の明示的有効化設定は各 runtime で別途指定）:

```sh
export WIVRN_LIGHT_BRIDGE_IPC=/tmp/llfa-llb.sock
```

既定パスの親 `/tmp` は共有 directory なので private directory 前提にはしない。
Monado listener は socket を同 UID 向けの `0600` にし、peer credential を照合する。
既存の socket／symlink／他 owner のパスを無条件に unlink しない。
複数 instance を動かす場合は環境変数で別パスを指定し、Client と Monado の組を一致させる。
今回の追記は両 adapter の設定契約であり、Bridge library 自体が環境変数を読んで listener を作る実装ではない。

## 確認済み実ソース

WiVRn checkout `/home/reina/WiVRn`、調査 HEAD `2795a8b63a5b91c3617328b573a460db25bd679d`。
integration commit `0e03cec0e90288a7598b597bb9243067bad49fd9`。
checkout が変わっていれば位置／lifetime を再調査する。

- `server/driver/wivrn_session.cpp:174`: constructor 内 `steamvr_lh_create_devices(nullptr,this)`。
- 同 `:197`: `hmd.set_external_head(...,steamvr_lh_get_latest_pose,mount)`。
- `server/driver/wivrn_hmd.cpp`: external head の latest/predicted source。
- `patches/monado/0013-d-steamvr_lh-tracking-only-hmd.patch`: latest history timestamp API。
- `client/wivrn_client.cpp:74`: `wivrn_session::handshake`、`:224,237`: IPv6/IPv4 constructor。
- `common/wivrn_packets.h:783`: `to_headset::handshake` は現在 stream_port のみ。
- `server/driver/wivrn_connection.cpp`／`server/ipc_server_cb.cpp` と `server/main.cpp` を lifetime/広告経路の追加調査対象にする。
  main は runtime child を fork する構成がある。Rust worker を親で開始してから fork しない。

上記はPhase 3開始時点の調査。現HEAD `4cf2c781` は `to_headset::light_bridge_endpoint` と
早期広告／Client通知を実装済み（`docs/light-bridge.md` と実コードで確認）。
Phase 4では既存の広告互換性を維持し、WiVRn stream_port を Bridge port として流用しない。
既存最新履歴 timestamp を、測定済みの光学サンプル時刻と断定しない。

## Server の作業順序

1. Bridge 無効時の既存 WiVRn／Lighthouse 起動を記録。build/runtime を opt-in とし、無効時は広告／listener／helper も起動しない。
2. runtime **Server lifetime owner** に Bridge handle と control worker を保持する。
   VR session の tracking driver lifetime と区別する。実際に driver を動かす child process 内で library を開始する。
3. CMake から Bridge FFI を build/link。Host は role=`LLB_HOST`、kernel policy role=`Import`。
4. Host Bridge listener を bind して port を取得し、信頼した WiVRn handshake/metadata 経路で
   Client に `{bridge_port, host_id, correlation_id}` を早期提示する契約を実装する。
   既存 protocol compatibility／version と旧 Client の挙動を確認し、無断の wire layout 変更を避ける。
5. Client が TCP 接続先 address と広告を得たら Pi の Monado に UDS Start を送る。
   Lighthouse 初期化完了や映像開始を待たない。WiVRn 接続、Bridge start、driver init を別イベントにする。
6. Host start と prepare intent を記録し、状態 TrackingInitializing=5 が来るまで driver 初期列挙を開始しない。
   待機中は control poll/heartbeat を止めず、render/pose query thread もブロックしない。
7. attach／OS enumeration barrier 後だけ `steamvr_lh_create_devices`。
   許可した Tracker serial を明示選択し、device usable を確認した後に `llb_tracking_ready`。
8. 停止時は pose publish を止め、tracking driver の参照／USB handles を実際に解放してから
   `llb_request_stop(host,1)`。driver を解放していない状態で1を渡さない。
9. Host detach 完了後の Stop、Pi unexport 完了後の Stopped を処理し、Idle=0 と両 journal 解放を確認する。

constructor rollback／exception の経路でも release barrier を守る。
pose worker が終了したことだけを tracking driver の USB release acknowledgement に読み替えない。
real backend の Drop は強制 detach しない。残留は journal と OS 状態で限定回収する。

## C ABI の接続

生成ヘッダ `light_bridge.h` が実シグネチャ。ABI version=1、Bridge wire version=3。

- `llb_create`: role Host、callback は必要に応じ state のみ、timeout は USB worker 待機を考慮する。
- `llb_configure_kernel_usbip`: TCP setup 前に1回。root import helper の socket、runtime policy JSON、各 byte length、非零 instance ID。
  device identity は Pi 側の snapshot、remote は Pi の usbipd endpoint。
- `llb_tcp_listen(host,&endpoint)`: port=0 を指定でき、実 port が返る。非零 host/correlation ID を広告前に決める。
- `llb_tcp_accept(host)`: control worker 上で期限付き setup。広告後の HMD connect と並行実行。
- `llb_request_start(host,session,NULL)`、`llb_request_prepare(host)`、`llb_poll`／`llb_heartbeat`。
- `llb_get_state(host,&state,&needs_runtime_release)` の state=5 が USB attach/列挙 barrier。
- driver の準備成功後 `llb_tracking_ready(host)`。state=6 でだけ publish を許可。
- `needs_runtime_release=1` は timeout／Client stop／切断の cleanup 要求。release 後に `llb_request_stop(host,1)`。
- `llb_destroy(&host)` は callback/userdata 生存中、control worker と直列に実行。

USB worker は内部で bounded/cancelable。poll/callback から標準 usbip を直接起動しない。
backend 未設定の generic TCP API は Mock。実機試験では configure 成功を記録する。
kernel 設定済み handle では `_mock` API は拒否。Mock と実験を混同しない。
library は同じ TCP 上の正常停止→新 session の handle 再利用をサポートする。
WiVRn 組み込みでは、切断後は合意どおり新しい Server 子プロセスで再接続する。
transport swap は未実装で、旧子プロセスの cleanup を確認してから新しい handle を作成する。

## WiVRn Client の通知契約

この作業も WiVRn repository 側で担当する。Monado 担当と上記の既定パス／環境変数、権限、広告 schema を揃える。
初期構成は Client と Monado が同じ Pi／同 UID の Linux process。
Linux 以外の Client target なら現 FFI IPC API が Unsupported になるため、環境を先に確認し不足を報告する。

UDS connection fd を使い `llb_ipc_send(fd,1,&endpoint,timeout_ms)` で Start、kind=2 で Stop。
fd は借用で caller が所有／close。送受信は shared socket を nonblocking にする。
Endpoint の host/correlation は Server の広告、address は実接続先の IPv4/IPv6（scope ID を保持）、port は Bridge port。
同じ host/correlation の重複通知は idempotent、新しい接続は新しい非零 session/correlation を使う。
切断／handshake failure でも対応する Stop を送る。高頻度 Pose/IMU を UDS に追加しない。
C ABI v1 は選択済み単一 NIC。Rust EndpointInfo/IPC は複数候補を扱えるが、C receive は複数を Unsupported として拒否する。

Client notification に helper root 権限を与えない。UDS listener は Monado adapter の control worker が所有する。
実 WiVRn 広告経路の信頼、Bridge TCP の認証／replay protection は未完成。初期実証を限定ネットワークで実施し、本番公開済みとは報告しない。

## Pose と検証

Phase 3 の実証はまず USB transfer／Tracker driver 検出／release/cleanup。
`llb_publish_pose` の DTO は session/device/origin/frame/clock/semantics/valid flags を保持する。
実 source timestamp の意味、origin、clock mapping を確定せず、latest history timestamp を measured として送らない。
`llb_mock_clock` は synthetic fixture 専用。現行 Phase 4 の live 時刻 API を組み込み、実同期精度と Monado LLFA 融合を別々に検証する。
Mock timestamped pose による runtime adapter 縦断は継続し、実 pose／融合成功とは分けて報告する。

CMake からの build 例:

```sh
LLB_HEADER_DIR="$PWD/build/bridge-include" cargo build --manifest-path ../LLFA-Link-Light-Bridge/Cargo.toml -p light-bridge-ffi --release --locked
```

生成 header と `.so`／`.a` を runtime target に接続。必須の前処理は build.rs 内、Python 不要。

提出物:

- opt-in CMake、Server owner、Client 通知、広告互換性、prepare/driver-release barrier の差分。
- WiVRn と Bridge の commit、実行環境／有効・無効の起動コマンド、無効時 regression。
- Pi export 後の Host `lsusb -t`／`usbip port`、serial 対応、Lighthouse driver 検出の証拠。
- 同じ session/correlation の Prepare→ExportReady→Attached→Ready→Stop/Stopped のログ。
- driver release→自分の port detach→Pi driver 復帰→両 journal 解放の証拠。
- 接続前 prepare、遅い通知、初期列挙失敗、timeout、Wi-Fi/cable loss、再接続、残留 journal の記録。
- 既定 UDS パス／環境変数による変更、再接続時の Server 子プロセス PID 更新と古い tracking device 非再利用の証拠。
- 実機／権限／不足 API の blocker。Mock で進められる箇所は進め、推測で完了扱いしない。
