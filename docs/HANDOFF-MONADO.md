# Monado 担当 Codex への LLFA Bridge 組み込み指示書

## 目的と前提

`LLFA-Link-Light-Bridge-IMPLEMENTATION-INSTRUCTIONS-v0.3.1.md` に沿って、
Orange Pi の **Monado プロセス内**に Bridge HMD library を組み込む。
Tracker USB は Host に転送し、arpara HMD USB／ローカル IMU は Monado に残す。
本書を含む Bridge commit を `git rev-parse HEAD` で記録し、WiVRn 担当と同じ commit を使用する。

現在の成果は Phase 3 Backend/helper のコードと Mock／故障試験。
実 USB/IP、Orange Pi arm64、実 runtime linking、時刻同期、LLFA 融合は未検証。
現 PC にだけ `28de:2300` Tracker があり、本番 Pi はまだ使っていない。
Phase 3 の実機合格条件は後述の証拠が揃うまで未達として扱う。
BSB 対応は追加しない。

最初にこのプロジェクトと Bridge の AGENTS.md、Bridge README、
`docs/USBIP-BACKEND.md`、`docs/FFI-ABI.md`、`docs/PROTOCOL.md` を読む。
ここに挙げる新しい runtime 設定／adapter は実装対象であり、既存関数として扱わない。

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
- `src/xrt/targets/common/target_llfa.h`: 既存 helper API。Bridge external-reference sink はまだない。
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

生成ヘッダ `light_bridge.h` の実シグネチャを正とする。ABI version=1、Bridge wire version=2。

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
現在は `llb_mock_clock` による synthetic mapping しかなく、実時刻同期の実装ではない。
時計・origin・mount が検証されるまでは head pose valid／融合成功に昇格しない。
mount が T_HMD_Tracker のとき world head = world tracker × inverse(T_HMD_Tracker)。
既存較正の方向を実ソースで再確認する。Phase 4 の実時計／融合は別の明示作業として報告する。

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
