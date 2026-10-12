# Monado 担当 Codex への LLFA Bridge 組み込み指示書

## /run noexec 修正の引き継ぎ（2026-10-12）

Light-Bridge 修正 commit は **da75130**（またはこれを含む後続公開 commit）。
[配置・安全な復旧手順](TEMPORARY-HELPER-NOEXEC.md) と [検証報告](TEMPORARY-HELPER-NOEXEC-REPORT.md) を参照。
実機ログ35〜37の /run noexec、root所有0700でも203/EXECの停止に対応した。
実行 binary は /var/lib、authorization/socket は /run。mount/fstab の変更は不要。
C ABI、LLBA、helper compatibility ID `llfa-usbip-helper-3-auto-20261012` は変更していない。

今回再確認した baseline は Monado **45866c990a14ae9067b983f61ada2a05cd5474ad**、Bridge fee4eca。
`src/xrt/targets/common/target_llfa_bootstrap.cpp:101` は helper status、`:421` は committed apply。
既存 auto adapter は統合済みで再作成不要。本文の fee4eca 時点の「次の差分作業」は当時の記録で、
今回の作業は Bridge submodule pin 更新、helper の再ビルド、診断文書の更新に限定する。
主 library は Monado 内、UDS／runtime opt-in／IMU／release順序を維持する。

担当者：Bridge commit の remote 公開後、submodule pin を更新して Monado側でコミット・公開する。
WiVRn と同じ Bridge修正版を使用する。sourceを独自forkや未公開patchで置き換えない。

```sh
cd ~/monado
git -C src/external/llfa-link-light-bridge fetch origin
git -C src/external/llfa-link-light-bridge checkout da75130
cmake -S . -B build -DXRT_FEATURE_LLFA_BRIDGE=ON
cmake --build build -j2
cd src/external/llfa-link-light-bridge
LLB_BUILD_REVISION=da75130 cargo build -p usbip-helper --release --locked
./target/release/usbip-helper --version
```

上記の submodule pin は担当が確認後に記録する。Pi側利用者は **担当公開済み Monado remote** に追従し、
`git pull --ff-only`、`git submodule update --init --recursive` で pin を取得して同じビルドを行う。
既存 build設定と描画／arpara option は維持する。
先に runtimeを停止し、修正版 helper の `temporary-recover-auto export --runtime-stopped` で
旧 /run staging を所有権／service／journal 確認後に回収する。
その後 `temporary-start-auto export --acknowledge-development-build` と `check-auto export` を実行する。
正常稼働 helper を recovery で停止・削除しない。journal/socket存在時は復旧手順の照合へ戻る。

WiVRn Clientが自身のCMake出力helperを管理者認証で起動する場合も、その binary が修正済みか確認する。
Monado側 Cargo helperだけ更新して古いClient内蔵helperを再起動しない。
/var/lib ExecStart、ActiveState、MainPID、check-auto、Bridge接続、USB転送を別々に採取して実証する。
このBridge作業ではMonado再ビルド／root実行／Orange Piは未検証。


## 自動準備 SDK の実装完了・次の差分作業

Light-Bridge側の自動準備本体は本版で実装済み。
[実 API](AUTOMATIC-BOOTSTRAP.md) と [検証報告](AUTOMATIC-BOOTSTRAP-REPORT.md) を読む。
Monado 06dd3c29f の既存 adapter を再利用し、auto branch の接続部分を追加する。root／USB実機は未検証。

1. auto mode は policy/IDs 未決定でも arpara HEAD と UDS listener を生成。manual-policy mode は明示診断用に残す。
2. runtime handle を llb_bootstrap_require で kernel-only にする。timing/kernel は別乱数・env値で先に設定しない。
3. Clientから LLBA Prepare と TCP FDを受け、llb_auto_helper_statusで導入状態を取得。Missingだけ Client/launcherに一時認証を依頼。
4. llb_auto_worker で Export Prepare。単一候補は自動選択、複数は stamp付き Candidates/Select をClientと交換。
5. helper PreparedをOfferにしてClient経由でHostへ返す。双方PreparedのCommitを受けてhelperへ適用。
6. helper Committedを llb_bootstrap_apply で設定。instance/session/timing/serialはplan値。
   従来の kernel preflight→TCP→Start/Prepare→ClockSync/pose に合流する。
7. USB unexportとIdleの後にExport environment Release。pending helperをruntime終了だけでkillしない。

rootコマンドは temporary-start-auto export --acknowledge-development-build。選択/認証UIは別担当の追加対象。
SDKのpose／yaw correction／IMU／描画経路は再実装しない。helper/SDKを新 compatibility IDへ揃える。
本API接続・WiVRn経由UI・native/ARM64 runtimeビルド結果を分けて提出する。

## 引き継ぎの基準：既存統合への差分実装

確認済み baseline は **Monado 06dd3c29f、Bridge submodule d765f16**。
この adapter を新規作成する指示ではない。実 checkout／自プロジェクトの
`doc/arpara-light-bridge.md` と `doc/arpara-yaw-fusion.md` を正として、以下の差分を適用する。
ここでの確認はソース確認であり、Orange Pi／実 USB/IP の成功を意味しない。

| 既存の実装 | 自動準備に必要な変更 |
| --- | --- |
| target_llfa_bridge.cpp の UDS listener／control worker／Start/Stop | 双方向 bootstrap を追加し、auto mode は policy 未確定でも listener を生成 |
| create_handle の kernel configuration、Start 前の helper preflight | 確定した SessionUsbPlan から設定し、Commit 前に互換性／準備状態を確認 |
| callback の raw pose コピー、poll 後の mapped timing／quality 検査 | 再利用。timing IDs の入力を bootstrap 契約から取得する方式へ変更 |
| stop／Idle／cleanup 確認後の handle 再生成、残留資源の拒否 | 再利用し、追加する environment lease の解放を USB 解放後に接続 |
| arpara IMU／描画、既定 OFF の実験的 yaw correction | 再利用。bootstrap 作業で再実装・較正完了扱いにしない |

本版 SDK の実在 bootstrap codec／helper 操作を既存 adapter へ接続する。
SDK にない関数を既存 API として呼ばない。既存の manual-policy 試験と Bridge OFF 回帰を維持し、
auto mode の新試験を追加する。下記 Phase 4／Phase 3 部分は既存契約と検証条件であり、
すでに実装された機能をもう一度作る作業一覧ではない。

## WiVRn 接続連動の自動準備（設計と統合条件）

[AUTOMATIC-BOOTSTRAP-DESIGN.md](AUTOMATIC-BOOTSTRAP-DESIGN.md) を次の実装契約にする。
通常 auto mode では IP／policy JSON／busid／timing ID の手入力を不要にする。
UDS listener と arpara HEAD は policy 未決定でも生成し、Client の双方向 Prepare → Offer → Commit を worker で処理する。
Pi の単一承認候補を自動選択、複数なら Client UI へ候補を返す。root helper が実 inventory を再検証し exact session plan を返す。
route は WiVRn の実 TCP local/peer と FD から得る。module／usbipd／必要な許可された環境設定は helper が準備する。
Commit 前の USB mutation は禁止。許可 limits と session policy を分離する本版 SDK を使用する。
現在 Monado 06dd3c29f は d765f16 に更新済みで新 preflight を実装しているが、自動 bootstrap は未実装。
以下の手動 policy 指示は現行診断経路の説明。auto mode の必須設定として引き継がない。

## Helper 配備の追加要求（2026-10-12）

[Helper 利用契約](HELPER-DEPLOYMENT-CONTRACT.md) を次の導入設計に適用する。
導入済み互換 helper を利用、不一致なら開始中断と更新要求、未導入だけ installer／一時管理者利用の選択。
UI のない Monado は launcher に構造化した診断を渡す。
SDK／CLI の handshake と一時起動は本版で追加。installer と選択 UI は未実装。
[一時利用手順](TEMPORARY-HELPER.md) に従って、まず開発者が CLI で起動する。
Monado 06dd3c29f では Start worker に `llb_check_kernel_usbip(handle)` が追加済み。
TCP 接続／session 適用前に失敗を診断し、その Start を中断する動作を再利用する。
SDK／helper／Monado は同じ版を使用し、root RPC v1 と混在させない。
USB 操作前の判定と既存 cleanup barrier を維持する。CLI 停止は通常 cleanup 完了後、runtime user が明示要求する。

## 目的と前提

`LLFA-Link-Light-Bridge-IMPLEMENTATION-INSTRUCTIONS-v0.3.1.md` に沿って、
Orange Pi の **Monado プロセス内**に組み込まれた Bridge HMD library を維持して拡張する。
Tracker USB は Host に転送し、arpara HMD USB／ローカル IMU は Monado に残す。
本書を含む Bridge commit を `git rev-parse HEAD` で記録し、WiVRn 担当と同じ commit を使用する。

現在の成果は Phase 3 Backend/helper と Phase 4 ClockSync／時刻 API のコードと Mock／故障試験。
Monado 側の native adapter 統合、live timing 受信、opt-in yaw correction は実装済み。
実 USB/IP、Orange Pi arm64、実機での時刻同期精度、物理環境での yaw 補正効果は未検証。
現 PC にだけ `28de:2300` Tracker があり、本番 Pi はまだ使っていない。
Phase 3 の実機合格条件は後述の証拠が揃うまで未達として扱う。
BSB 対応は追加しない。

最初にこのプロジェクトと Bridge の AGENTS.md、Bridge README、
`docs/USBIP-BACKEND.md`、`docs/FFI-ABI.md`、`docs/PROTOCOL.md` を読む。
既存 adapter／API と新 bootstrap の設計上の候補を区別する。現在の source にある機能は再利用する。

## Phase 4 の実装済み契約・回帰確認

両担当の Phase 3 組み込み報告を受け、Bridge に live timing API を追加した。
**両 submodule を本書を含む同じ新 commit に更新する。wire v3 と旧 v2 は接続互換がない。**
既存 C ABI layout／UDS はv1を維持。既存build/runtime opt-in、IPCパス、停止・新子プロセス契約は維持する。
`docs/PHASE-4-TIMING-SPEC.md` と生成ヘッダを読み、追加sensor timestampを前提にしない。

両側で start 前に `llb_configure_timing` を成功させる。tracked_device_id／source_clock_id／local_clock_id と
quality limits を共有し、CLOCK_MONOTONIC ns の domain を実環境で確認する。
数値IDはsession内の契約。Mockの固定device=1/clock=1への合わせ込みで実poseを通さない。
Host/Pi の時計 epoch は別。poll/heartbeat を継続し、USB準備中もClockSyncに応答する。

`target_llfa_bridge.cpp`／`doc/arpara-light-bridge.md` は 978e79022 で統合され、
06dd3c29f で helper preflight が追加済み。以下はその実装の維持条件。

1. production factory が kernel backend に加えて live timing を設定する。Mock clock はtest factoryだけ。
2. callbackでraw Poseをコピー。poll復帰後に `llb_get_reference_timing` を取得し session/sequence を照合。
   sinkに raw timestamp／semantics、mapped sample timestamp、receive timestamp、uncertainty／generation を保存する。
3. `llb_get_clock_status` と `llb_get_pose_diagnostics` をpollし、同期未成立／期限切れ／clock fault／tracking lostでsinkを無効化する。
   callback内のFFI再入は禁止。mappingのないraw timestampをMonado sample時刻として比較しない。
4. driver-reported timestampはestimatedとして扱う。追加sensor時刻取得は不要。
   source relationが既にdriver-from-head／world／chaperone変換を含むことをWiVRn担当と確認し、mountを二重適用しない。
5. HEADは既存arpara IMUを維持する。mapped referenceの受信確認、mount/origin較正、LLFA融合の成功は別々に報告する。

提出証拠にClockSync status／RTT／drift／uncertainty、raw↔mapped timestamp、session/sequence、
失効／clock reset／tracking lossの無効化と新session復帰を追加。実時計精度をMockテストから推測しない。

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

Monado checkout: `/home/reina/monado`、調査 HEAD `ad7610a5561325a9b368cda5d45fe195d67f4c3b`。
対象 checkout が変わっていれば再調査する。

- `src/xrt/targets/common/target_builder_llfa.c:82`: arpara HMD 初期化。
- 同 `:102`: `survive_get_tracker_devices` によるローカル Tracker open。
- `src/xrt/targets/common/target_llfa.c:87`: `multi_create_tracking_override` の DIRECT override。
- `src/xrt/targets/common/target_llfa.h`: 既存 LLFA API。統合済み target_llfa_bridge adapter は reference mailbox を持つ。
- `src/xrt/drivers/arpara/arpara_hmd.cpp`: 独立した HID reader／IMU 3DoF／shutdown join。
- mount の契約は既存 `doc/arpara-llfa-link.md` と Bridge `docs/INTEGRATION-NOTES.md` を再確認する。

DIRECT override は IMU/Lighthouse 融合の完成ではない。既存 reader とヘッド姿勢経路を無条件に置き換えない。

## 作業順序

1. **無効時の通常起動を先に記録**する。Bridge を build/runtime とも opt-in にし、無効時は library、UDS、USB helper の開始も Tracker 探索変更も行わない。
2. CMake から Rust FFI library を build/link。`light-bridge-hmd` は同一 Monado プロセスの opaque handle に保持する。
   HMD/system の lifetime owner に adapter を置き、WiVRn Client の切断だけで Monado 全体を終了しない。
3. Orange Pi 上の WiVRn Client から通知を受ける UDS listener を adapter の control worker に置く。
   pathname は上記の既定値／環境変数契約に従い、既存／他 owner の path を勝手に unlink しない。
   Pose／IMU は UDS に流さない。同じ UID の client を初期契約とする。
4. Start の Bridge Host endpoint を受けたら、Lighthouse 初期化を待たず TCP 接続と session start を行う。
5. Bridge enabled 時だけ LLFA の **local Tracker open を抑止**する。arpara HMD の HID open／reader はそのまま。
6. `UsbExportReady`／`UsbAttached`／`TrackingReady` の lifecycle と参照姿勢 callback を記録。
7. Stop／切断／timeout で参照姿勢を即無効化し、Host の runtime release→detach→Stop を待って HMD unexport。
8. 正常停止と異常停止、再接続、無効時 regression を実行して証拠を残す。

## C ABI の具体的な接続

生成ヘッダ `light_bridge.h` の実シグネチャを正とする。ABI version=1、Bridge wire version=3。

- `llb_create`: role=`LLB_HMD`、callback/userdata、timeout_ms（USB 初期化待機でも heartbeat を回せる値）。
- `llb_configure_kernel_usbip`: TCP 接続前に1回。socket UTF-8 byte buffer、runtime policy JSON buffer、各長さ、非零 instance ID。
  role=`Export`、Pi の exact Tracker identity と usbipd endpoint を含む。Root helper 側 policy と一致させる。
- Client から受けた socket fd は `llb_ipc_receive(fd, timeout_ms, &kind, &endpoint)` で復号できる。
  `endpoint.size=sizeof(endpoint)`／abi_version=1 を事前設定。kind=1 Start、kind=2 Stop。
  Start は ABI v1 の選択済み単一 NIC。複数候補は Rust IPC API で扱うか、C 側は Unsupported を処理する。
- Start: control worker で `llb_tcp_connect(handle,&endpoint)`、続けて `llb_apply_notification(handle,1,&endpoint)`。
  TCP connect は期限付き setup。実 USB 操作は library の専用 worker へ送られ、`llb_poll` 内では待たない。
- Stop: `llb_apply_notification(handle,2,&endpoint)`。host ID と correlation/session ID が合わない通知は拒否される。
- control worker で `llb_poll` と `llb_heartbeat` を定期実行。callback から API に再入しない。
- `llb_destroy(&handle)` は userdata 生存中に直列で呼ぶ。callback の pose pointer は呼び出し中だけ借用なのでコピーする。

callback event=1 は state、event=2 は reference Tracker pose。
Idle=0、TrackingInitializing=5、Active=6、Stopping=7、Failed=8。
実 helper を選択した handle に `_mock` API を呼ぶと拒否される。Mock と実験の区別を保つ。
backend 未設定の generic TCP API は Mock を使うため、本番実験では configure 成功を必ず確認する。

正常 stop 後の同じ TCP 接続上の新 session は handle／worker を再利用できる。
TCP 切断後の transport swap API は未実装。現 API では cleanup を確認して接続 handle を再作成する。
Monado の runtime lifetime owner は保持する。WiVRn 側は合意どおり新しい Server 子プロセスを使う。
完全な切断再接続の要件に不足があれば Bridge 側に報告し、架空 API を作らない。

## USB／権限と参照姿勢

Pi で usbip_host／usbipd、root export helper を用意する。Monado 自体を root にしない。
policy、helper 配備、journal と recovery は `docs/USBIP-BACKEND.md` の手順を使う。
PC で観測した busid `1-3.3`／sysfs path を Pi の固定値にコピーしない。Pi で再採取する。
`2c30:1050` は allowlist に載せても拒否。WholeDevice 以外へ対象を読み替えない。

参照 sink を新設し、session/device/origin/frame/clock/timestamp semantics/flags を保存する。
Phase 4 の `llb_configure_timing` で live ClockSync を選択する。`llb_mock_clock` は fixture 専用。
時計・origin・mount が検証されるまでは head pose valid／融合成功に昇格しない。
mount が T_HMD_Tracker のとき world head = world tracker × inverse(T_HMD_Tracker)。
既存較正の方向を実ソースで再確認する。時計同期 API の実 runtime 組み込み・精度確認と較正／融合は別々に報告する。

## ビルドと合格条件

```sh
LLB_HEADER_DIR="$PWD/build/bridge-include" cargo build --manifest-path ../LLFA-Link-Light-Bridge/Cargo.toml -p light-bridge-ffi --release --locked
```

CMake の include/link path は生成 include directory と `liblight_bridge_ffi.so`／`.a` に接続する。
必要な生成は build.rs 内で完了する。Orange Pi arm64 は実際に native/cross build し、未実行ならそう記録する。

提出物:

- opt-in CMake/runtime adapter の差分、Monado の commit と Bridge commit、起動コマンド。
- Bridge 無効時の通常 LLFA/HMD regression と、有効時 Mock lifecycle/reference/stop 試験。
- Pi の Tracker export 前後 `lsusb -t`／sysfs、HMD `2c30:1050` がローカル driver に残る証拠。
- ローカル IMU 更新継続、Host detach 後の Tracker driver 復帰、journal 解放の証拠。
- Host 側の WiVRn 報告と同じ correlation/session ID で時系列を照合。
- timeout、Wi-Fi 切断、ケーブル抜去、再接続、残留資源と必要な手動回収を記録。
- 既定 UDS パス／環境変数による変更、新 Server 子プロセスへの再接続と旧 reference の無効化を確認。

実機・API・権限が不足していても Mock は進める。未検証の USB 転送、較正、融合、driver release を完成扱いしない。
