# Phase 3 Linux USB/IP Backend と helper

この文書はコードの使用・試験手順。実 USB/IP 転送が検証済みという報告ではない。
Orange Pi に Tracker、Host に vhci_hcd を配置し、Monado/WiVRn は一般ユーザーで動かす。
低遅延の pose／IMU 経路は両 runtime 内、特権 helper と標準 usbipd は別プロセス。

## 実装

- `light-bridge-usbip::helper::KernelUsbIp`: 一般ユーザープロセスの backend proxy。
- `manager::UsbManager`: bounded command/reply queue 各4、cancel/join、USB 専用 worker。
- `kernel::Engine`: exact policy、所有者 `(instance, session)`、boot ID、操作前 journal、cleanup barrier。
- `kernel::LinuxOps`: sysfs inventory、vhci status、usbip port record、標準 CLI 操作。
- `usbip-helper serve POLICY`: Linux root helper。root shell／任意プログラム／任意引数の RPC はない。
- `HmdClient::with_usb_backend`／`HostServer::with_usb_backend`: worker と selected identity を接続する。
- `llb_configure_kernel_usbip` + `llb_tcp_listen/accept/connect`: C から明示的に backend を選ぶ。

helper は固定候補 `/usr/bin/usbip`／`/usr/sbin/usbip` の root-owned executable を使う。
親 directory も group/world writable でないことを確認。引数は enum から配列生成し shell を起動しない。
環境をクリアして LC_ALL/PATH を固定、stdout/stderr 合計32KiB、CLI 期限5秒。
期限／cancel 時に CLI process group を kill。通常は reap、kernel の uninterruptible wait で
短時間に reap できない場合は別 reaper に渡す。これは kernel 操作の取り消し保証ではない。
CLI failure で lease を凍結し、pending journal を残す。

## Policy と所有権

`examples/usbip/*-policy.template.json` は **未設定テンプレート**。
`192.0.2.1` は文書用アドレスであり接続先の推測値ではない。
本番の値は Orange Pi に接続した Tracker を `usbip-probe` で確認して記入する。
Host Import の device identity も **Pi 側**の VID/PID、serial、busid、canonical sysfs path、topology をコピーする。
Host 上の imported device の busid/path は別物なので、この policy に上書きしない。
両 role の remote は **Pi の usbipd endpoint**。Bridge Host port ではない。
client_uid/gid は respective runtime user の実際の UID/GID。

helper 側 policy は root-owned、非 group/world writable の絶対 path と親 directory に置く。
JSON の未知フィールド、HMD `2c30:1050`、interface busid、空 serial、不正 busid を拒否する。
runtime は同じ expected device/role/remote を含む設定コピーを C ABI に渡す。
これを変えても helper の root policy／認証 UID を変更できない。

RPC: 4-byte big-endian JSON length + ≤4096 bytes、version=1。
固定 Action は Acquire／Release／Inspect。SO_PEERCRED の UID と root policy を照合。
proxy は相手の UID=0 と socket owner=root を照合する。
helper socket は `/run/llfa-link-light-bridge/{export,import}.sock`、root:client_gid、0660。
親 directory は root-owned 0711、state directory は root-owned 0700。
クライアントが socket を置換できる write permission は与えない。

Export は exact inventory と WholeDevice planner を再確認し、hub/vhci ancestor／既存 usbip-host を拒否。
usbip_host／usbip executable／usbipd の到達性を前提条件として検査してから journal intent を保存。
`usbip bind --busid BUSID` 成功と available 状態を確認して ExportReady を送る。

Import は vhci_hcd を確認、attach 前後の全 controller status を比較する。
CLI は `usbip --tcp-port PORT attach --remote IP --busid BUSID`。
新規／変更ポートが1個だけ、元の port は空(status=4)、使用状態(status=6)、
vhci 配下の local device が VID/PID/serial に一致し interfaces が列挙済み、
`/run/vhci_hcd/portN` の remote IP/port/busid が policy と一致するまで、最大3秒待つ。
実測 port/devid/sockfd/local_busid と boot ID を保存し、その後だけ UsbAttached とする。
複数 port の変更、異なる identity、列挙期限切れは成功扱いせず pending journal を残す。

通常 cleanup は **runtime driver release → Host detach 完了 → HMD unbind/rebind**。
Host detach 前に保存 fingerprint と現在の port／device を再照合。release acknowledgement 無しでは拒否。
HMD は Host の Stop（detach 完了の契約）を受けた後にだけ unexport。
kernel available=1 が必要で、busy=2 は拒否。元の device driver／interface drivers の復帰を確認する。
成功確認後だけ journal を削除する。library Drop は実 USB を強制 detach/unbind しない。

CLI と外部管理者が同じ vhci ports を同時操作する構成は未検証。初期実証では helper を唯一の USB/IP 操作主体にする。
root が同一 port/device を外部で付け直す ABA を sysfs fingerprint だけで完全に排除できるとは主張しない。
USB/IP と Bridge TCP の remote 認証／暗号化は未実装。初期実証はアクセス制限した検証ネットワークで行う。

## フル環境での準備（現在の PC では未実行）

1. 両マシンの distro/kernel に対応する usbip／usbipd を導入し、Pi で usbip_host、Host で vhci_hcd をロード。
2. Pi の usbipd を開始し、Host から Pi の configured TCP port に到達することを確認。
3. Tracker と arpara HMD を Pi に別 USB 接続し、`usbip-probe`／`lsusb -t` を採取。
4. exact policy を作成。helper executable と policy を管理者が root-owned の保護された場所へ配置。
5. `examples/systemd/llb-usbip-helper@.service` は未実機検証の配備テンプレート。
   Pi は export、Host は import の1 instance を使う。runtime と usbipd を root にする構成ではない。
6. Rust workspace build と shared/static C smoke を通し、次に runtime adapter を enable して準備する。
7. Host の `lsusb` と `usbip port`、driver 検出、port／session journal を採取する。
8. runtime 解放と正常停止後、Host imported port が空、Pi の Tracker が元の driver に復帰、両 journal が消えたことを確認する。

Rust build は `cargo build --workspace --locked`、配備用 helper は `cargo build -p usbip-helper --release --locked`。
ABI ヘッダ生成は build.rs に含まれ、Python 前処理は不要。

## 残留資源の回収

永続 journal は `/var/lib/llfa-link-light-bridge/{export,import}.json`。
version、role、device、remote、instance/session、boot ID、stage、port fingerprint、前 driver を保存する。
helper 起動時に読み込むが **既存資源は自動 adopt／detach しない**。
既存 socket pathname も自動 unlink しない。正常終了・SIGTERM 後でも残るため再起動前に所有確認が必要。

1. runtime／helper を停止し、対象 tracking driver の handle が解放済みか確認。
2. 管理者が journal、現在 boot ID、sysfs identity、vhci status、usbip port record を照合。
3. 正確に一致した自分の resource のみ回収候補にする。不一致／pending attach で port 不明なら推測で detach しない。
4. Host の対象 port detach と空状態を確認してから Pi の対象 busid unbind／driver 復帰を確認。
5. OS 状態が正常になったことを確認して初めて、対応する journal／`.new`／stale socket を管理者が回収する。
   バス ID 変更やケーブル抜去の場合は、残った match_busid も照合する。全 port 一括 detach はしない。
6. fresh session/instance ID で helper/runtime を再開。

Intent と post-operation 保存の間の crash、CLI error、enumeration timeout は不確定状態として停止する。
これは安全側に倒す手順であり、OS crash／Wi-Fi 切断の自動復旧を検証したものではない。

参考（設計確認、実装へソースをコピーしていない）:
[Linux USB/IP protocol](https://docs.kernel.org/usb/usbip_protocol.html)、
[CLI bind](https://github.com/torvalds/linux/blob/master/tools/usb/usbip/src/usbip_bind.c)、
[CLI unbind/rebind](https://github.com/torvalds/linux/blob/master/tools/usb/usbip/src/usbip_unbind.c)、
[vhci sysfs](https://github.com/torvalds/linux/blob/master/drivers/usb/usbip/vhci_sysfs.c)。
