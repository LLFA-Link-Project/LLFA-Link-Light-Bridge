# 自動準備 SDK／helper の利用契約

2026-10-12。Light-Bridge の自動準備本体を実装した。
Monado／WiVRn の既存 adapter に本 API を接続する変更と GUI は、別プロジェクト担当の後続作業。
installer は後回し。実 root systemd／package／USB/IP／Orange Pi の成功はまだ未検証。

## 本体と版

- `light-bridge-usbip::bootstrap`: supported-profile selection、WiVRn route に基づく exact session policy、
  timing contract、BothPrepared/Commit gate、immutable peer leases、environment journal／cleanup barrier。
- `fd_channel`: LLBA v1 duplex frame と最大1個の SCM_RIGHTS descriptor。接続済み TCP の IP／port／namespace を検証。
- `bootstrap_rpc`／`auto_worker`: root peer の互換性 handshake、非 mutation discovery、4件の bounded queue、cancel/join。
- `environment::LinuxEnvironment`: 固定 CLI による tools、module、nft ACL、専用 usbipd、到達性と所有 fingerprint の確認。
- `light-bridge-protocol::bootstrap`: WiVRn/UDS 共通の strict JSON envelope。
- `light-bridge-ffi::auto`: 新 C ABI。従来 ABI v1 の struct layout と callback は変更しない。
- `usbip-helper serve-auto`: 動的 session を受け付ける root helper。Commit 後だけ従来の exact kernel socket を公開。

専用 compatibility ID は **llfa-usbip-helper-3-auto-20261012**。
通常 kernel RPC は v2 のまま、auto helper envelope は v1。root helper、SDK、両 runtime を本版から再ビルドする。
旧 compatibility ID の helper は不一致として中断する。
Bridge TCP は v3、従来 LLBI notification は v1。新 bootstrap は別 magic LLBA、schema v1、最大8192 bytes。
bootstrap 用 WiVRn extension/hash の追加は WiVRn 担当の作業。既存の packet tags を変更しない。

noexec `/run` 対応の binary 配置・失敗 staging 復旧は [TEMPORARY-HELPER-NOEXEC.md](TEMPORARY-HELPER-NOEXEC.md)。

## 開発用 helper の開始（policy／IP の指定不要）

各 runtime の Bridge submodule 内で：

```sh
cargo build -p usbip-helper --release --locked
./target/release/usbip-helper --version
```

Pi（Monado／Client のユーザーから）：

```sh
sudo ./target/release/usbip-helper temporary-start-auto export \
    --acknowledge-development-build
./target/release/usbip-helper check-auto export
```

Host（WiVRn Server のユーザーから）：

```sh
sudo ./target/release/usbip-helper temporary-start-auto import \
    --acknowledge-development-build
./target/release/usbip-helper check-auto import
```

不足ツールの固定導入 plan を許可する場合だけ `--allow-package-install` を追加する。
開発用 localhost test では両 helper に `--allow-loopback` を追加する。通常は loopback を拒否する。
sudo は当該開発 binary を root 実行する明示的な承認であり、署名付き installer の代替ではない。
SUDO_UID/GID から runtime account を設定する。既存 helper は一致する authorization の場合だけ再利用。
不一致／停止中の既存導入／stale socket／stage／journal を無条件に置換・削除しない。

起動時は root-owned `/var/lib/llfa-link-light-bridge/temporary-auto-{export,import}` に binary、
`/run/llfa-link-light-bridge/temporary-auto-{export,import}` に authorization を snapshot。unit は `llb-usbip-temporary-auto-{role}.service`、永続登録なし。
role の bootstrap socket は `/run/llfa-link-light-bridge/{role}.auto.sock`、root:runtime_gid 0660。
policy は WiVRn connection の Prepare 時に自動決定する。modules／daemon も Prepare 時に準備する。
SDK は自ら sudo／polkit を起動しない。launcher/UI が read-only status と利用者の選択を扱う。

## Runtime の API 接続順序

全て adapter の setup/control worker で実行する。Host の Rust worker は既存通り fork 後に作る。

1. auto mode の runtime handle を `llb_create` したら **`llb_bootstrap_require`** を呼ぶ。
   policy 未設定の generic TCP API／Mock link は拒否され、失敗時の Mock fallback を防ぐ。
2. Host の実 WiVRn control TCP FD から `llb_bootstrap_new_request` で Prepare packet を作る。
   IP と非零 session／host／instance／generation／device/source/local clock IDs を生成する。
   optional TimingConfig template は quality limits のみ使い、IDs を生成値に置き換える。
3. Host は `llb_bootstrap_create` で coordinator を作り、Prepare packet を既存の認証済み WiVRn control で Client へ渡す。
   Client は Monado の同 UID UDS へ `llb_bootstrap_ipc_send` と自分の control TCP FD で転送する。
4. Monado の policy-less auto listener は `llb_bootstrap_ipc_receive` で packet と owned route FD を受け取る。
   fd は Root helper の Prepare が submit されるまで保持し、以後自分の duplicate を close できる。
   受信後に元 TCP を read／shutdown／flags 変更しない。UDS framing error 後は元 stream を close する。
5. `llb_auto_helper_status(role)` は Ready／Busy／Missing／InstalledStoppedOrManual／NeedsUpdate／Unknown を返す。
   Missing だけ一時 helper の管理者承認を提案する。その他の失敗を Missing とみなさない。
   status は read-only だが handshake があるので control worker で呼ぶ。
6. `llb_auto_worker_create(role)` で dedicated worker を作る。
   `llb_bootstrap_helper_command` operation=1 で Prepare → Export command に変換し、
   `llb_auto_worker_submit` に route FD と非零 operation ID を渡す。submit は FD を duplicate する。
   `llb_auto_worker_poll` で完了を取る。空の queue は6。JSON buffer は全 API で8192 bytesを確保する。
7. 候補表示が必要なら auto helper Action=Inventory を submit（FDなし）。
   `llb_bootstrap_candidates_packet` で Prepare stamp 付き候補 packet にして Client UI へ転送する。
   単一候補は serial NULL/0 の PrepareExport で自動選択。複数は Select packet の stamp を active request と照合し、
   cached Prepare packet と選択 serial を helper_command に渡す。0台では接続を案内する。
8. 成功した helper Reply=Prepared を `llb_bootstrap_response_packet` で Export Offer にする。
   Client 経由で Host へ戻し、Host が `llb_bootstrap_receive` で受理する。
9. Host は Offer → helper_command operation=2 で Import Prepare を作り、Host の実 control TCP FD を submit。
   helper が Pi endpoint の逆方向到達を確認し exact Import policy を生成する。
   Reply=Prepared を packet にして coordinator に受理させる。
10. **双方 Prepared の後だけ `llb_bootstrap_commit`** が成功する。
    Commit packet を両 helper の operation=3 に渡す。helper Reply=Committed を受け取る。
    coordinator／helper は stamp、exact device、endpoint、immutable peer lease を照合する。
11. `llb_bootstrap_get_info` で LlbBootstrapInfo（prefix size/ABI=1）、serial、timing、session、Host bind address を取得する。
    `llb_bootstrap_apply` で committed reply を unlinked runtime handle に適用する。
    `llb_check_kernel_usbip` で exact root kernel socket を確認する。
12. Host の `host_bind` は port=0 なので従来の `llb_tcp_listen` で実 port を取得して広告する。
    host_id／correlation_id は info の値を使い、既存 constructor の別の乱数で置き換えない。
    Pi の connect address/scope は Client の actual peer に advertised port を組み合わせる。
    HMD の info.host_bind にある Host 側 scope を HMD の interface index として使わない。
13. 以後は既存の Hello → Prepare → export/attach/enumeration → Lighthouse → Ready → ClockSync/pose の実装を再利用する。

helper JSON は `Envelope {version:1, compatibility_id, role:"Export"|"Import", action, body?}`。
Inventory／Describe／Shutdown は bodyなし、FDなし。PrepareExport／PrepareImport は FDが必須。
Reply は `Response {version, compatibility_id, package_version, authorization, pending, result}`。
**poll return=0 は RPC完了であり、result.Err を成功と扱わない。** transport failure は return=8 と transport_error JSON。
operation ID は pending 間で再利用せず、packet の generation/stamp も照合する。
destroy は cancel/join のみで、OS cleanup を成功扱いしない。失敗時は status／journals を確認する。

## 初期準備の実処理と対応範囲

- initial profile は **28de:2300**。serial／busid／canonical sysfs／topology は現在の Pi inventory から取得。
  HMD 2c30:1050、vhci、既存 USB/IP exports、不完全 inventory、重複 serial は拒否。
  既存 kernel exports があると標準 usbipd が他デバイスも公開するため、専用 daemon の準備を中断する。
- root helper は接続済み TCP FD の actual local/peer、TCP established state、network namespace cookie を検証する。
  初期対応は IPv4 private/link-local、IPv6 ULA/link-local。public/global IPv6、NAT の逆接続、別 namespace は拒否／到達試験失敗。
- Pi は usbip_host、Host は vhci_hcd を固定 modprobe でロード。driver sysfs を確認し builtin にも対応。
  module を終了時に unload しない。kernel 交換／再起動はしない。
- Pi は port3240 を使用。foreign port/daemon を奪わず、random lease 名の専用 transient usbipd を起動。
  MainPID／process start ticks／exe／実 listen socket inode で確認する。
- nftables の専用 random-name inet table に local/peer/port 条件の accept と、それ以外の当該 port の drop を設定。
  他 rule を flush／無効化しない。既存 firewall の後続 drop はこの accept で回避できないので Host が実到達確認する。
  nftables が使えない場合は失敗する。firewall 無効化による fallback はない。
- package install は root authorization の明示 flag がある時だけ。入力／remote packet は package 名や command を指定できない。
  Debian は usbip/nftables/kmod、Ubuntu は linux-tools-common/linux-tools-generic/nftables/kmod の固定 plan。
  Armbian Ubuntu の wrapper と kernel が不一致なら usbip version で診断し、推測で symlink を作らない。
- Arch は cached sync DB の pacman print と installed versions を検査し、allowlisted な新規 tools/dependencies のみ導入。
  DB refresh／既存 package の upgrade／kernel package は拒否し、OS maintenance を別途案内する。
  他 package manager と同時操作しない。package install は永続 system 変更で、lease cleanup では uninstall しない。

CLI期限／output上限／cancel を維持。package manager failure/timeout は成功扱いせず journal と診断を残す。
この実装を物理 USB/IP、kernel、distro 間の動作保証とみなさない。

## Abort・通常停止・回復

bootstrap 中断時は coordinator の current stamp で Abort を受理する。
helper_command operation=4 は Prepare／Offer／Prepared／Commit から release を生成できるが、runtime_stopped=1 が必要。
helper は authoritative USB journal が空でなければ environment cleanup を拒否する。

正常停止は従来通り **publisher join → Lighthouse release → Host detach → Pi unexport → Idle**。
その後に両 helper の environment Release を呼ぶ。確認済みの daemon を停止し、PID消失と port 解放を確認、
同じ table handle と全 snapshot が一致する firewall だけ削除、absence と journal 削除を確認する。
table改変／PID再利用／pending mutation／recovered journal は自動採用・推測 cleanup しない。
完了した stamp は生存中 helper で再利用しない。新しい接続では fresh Server child／request／handles を使う。

environment journal: `/var/lib/llfa-link-light-bridge/{role}-environment.json`（0600、atomic .new＋fsync）。
USB journal: 従来の `{role}.json`。両方が不確定状態を保持する。
daemon unit: `llb-usbipd-{lease}.service`、nft table: `inet llb_{lease}`。
root CLI と他管理者の ABA／同時変更を完全排除するものではない。初期実験では本 helper を唯一の USB/IP 管理主体にする。
手動回復では boot、stamp、policy、PID/start、unit、listen inode、table snapshot と USB port を照合し、
自分の資源だけを回収する。journal／socket を先に一括削除しない。

一時 helper 自体を止めるのは全 lease が空になってから、runtime user で：

```sh
./target/release/usbip-helper temporary-stop-auto export --runtime-stopped
# Host は import
```

## 検証

[AUTOMATIC-BOOTSTRAP-REPORT.md](AUTOMATIC-BOOTSTRAP-REPORT.md) を参照。
native Mock environment と実 localhost TCP/UDS/SCM_RIGHTS、C ABI は実 root deployment の検証と分ける。
参照: [nftables](https://netfilter.org/projects/nftables/manpage.html)、
[systemd-run](https://github.com/systemd/systemd/blob/main/man/systemd-run.xml)、
[pacman](https://man.archlinux.org/man/pacman.8.en)。
