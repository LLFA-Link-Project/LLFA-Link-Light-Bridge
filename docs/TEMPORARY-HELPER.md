# 開発用 helper の一時起動

installer は実装しない。Linux の `usbip-helper` CLI が、明示的な管理者承認を受けた
開発ビルドを `/run` にコピーし、systemd transient service で起動する。
`/etc/systemd/system` への unit 登録、enable、sudoers／polkit の変更は行わない。
Monado／WiVRn は一般ユーザーのまま。現時点では開発者が CLI を実行する方式で、
Dashboard の選択 UI／runtime からの自動管理者認証は未実装。

## 前提

- Linux、system systemd、root-owned `/usr/bin/systemd-run`。systemd-run の Type=exec と collect を使用する。
- Pi に usbip_host と標準 usbipd、Host に vhci_hcd。モジュールのロード／daemon 起動は別途準備する。
- 標準 usbip CLI は root-owned `/usr/bin/usbip` または `/usr/sbin/usbip`。PATH の任意 executable は使わない。
- Pi 実 inventory に基づく Export／Import policy。両 policy の device と remote は Pi 側の値。
  各マシンの client_uid/gid は runtime を動かすユーザーの UID／primary GID。
- 未確定 journal／既存 socket／古い staging が無いこと。これらを自動削除して再試行しない。

## ビルドと起動

各 runtime の Bridge submodule 内で、両 library と helper を同じ更新済み revision からビルドする。

```sh
cargo build -p usbip-helper --release --locked
./target/release/usbip-helper --version
```

Pi の Export 例（Host では import-policy.json）：

```sh
LLB_POLICY="$PWD/export-policy.json"
./target/release/usbip-helper check "$LLB_POLICY"
```

未導入なら check は非零で導入選択の診断を返す。導入済み互換 helper なら成功。
不一致／停止中の既存導入／応答不能／不正所有者なら中断して診断・更新する。
未導入の場合にだけ、一時利用を選択して次を実行する。

```sh
sudo ./target/release/usbip-helper temporary-start "$LLB_POLICY" \
    --acknowledge-development-build
./target/release/usbip-helper check "$LLB_POLICY"
```

sudo の認証が今回の開発 binary を root 実行する明示的な承認となる。flag はその意図の確認であり、
署名検証や、信頼できない binary を安全に root 実行する仕組みではない。
root として起動した CLI は `/proc/self/exe` の実 inode と読み取った policy を
root-owned 0700 の `/run/llfa-link-light-bridge/temporary-{export,import}` へ snapshot する。
sudo 呼出元の UID／primary GID と policy が違う場合は拒否。policy は絶対パスの通常ファイルのみ、
4096 bytes 以下、symlink／HMD／interface-only 相当の busid は拒否する。

一時 service 名は `llb-usbip-temporary-export.service`／`llb-usbip-temporary-import.service`。
helper RPC socket は従来と同じ `/run/llfa-link-light-bridge/{export,import}.sock`。
runtime は元の policy JSON を設定に使用できるが、helper は保護された snapshot を読む。
policy を変更したら runtime と helper の一致確認が失敗する。
usbipd の起動や実 USB 操作は temporary-start 自体では行わない。

```sh
systemctl status llb-usbip-temporary-export.service --no-pager
sudo journalctl -u llb-usbip-temporary-export.service -n 100 --no-pager
```

既存 socket があれば、起動せず実 peer を確認する。一致・空 journal なら再利用、不一致／応答不能は拒否。
socket が無い場合も、従来の helper binary、unit、role policy が存在すれば「停止／不完全な導入」とし、
勝手に一時 helper に切り替えない。対応する初期探索箇所は `/usr/local/libexec/llb-usbip-helper`、
`/etc/systemd/system/llb-usbip-helper@.service`、`/usr/lib/systemd/system/llb-usbip-helper@.service`、
`/etc/llfa-link-light-bridge/{role}.json`。installer 配置 metadata の一般化は後続実装。
stage／systemd 起動／準備失敗では不確定な stage を保持し、再起動で上書きしない。

## runtime の事前確認

`llb_configure_kernel_usbip` の成功後、TCP 設定・session 開始・Lighthouse 初期化より前に
adapter の setup/control worker で `llb_check_kernel_usbip(handle)` を呼ぶ。
blocking handshake（connect 最大2秒、RPC最大3秒）。pose query／callback では呼ばない。
この API は helper を起動せず、管理者権限要求も出さない。未設定／接続後は InvalidState。
不一致等は Protocol、残留 ownership は Ownership と診断を返す。

さらに各 USB worker RPC では、同一接続上で Describe → 操作の順に検証する。
RPC v2、package version、専用 compatibility ID、全 policy が一致しない限り操作を送らない。
helper 側も操作要求の version／ID／UID／device を検証してから kernel engine を呼ぶ。
旧 helper RPC v1 とは互換性がない。両 runtime と helper を同じ新版へ更新する。
Bridge TCP v3／C ABI v1／通知 UDS v1 は変更しない。
build revision は任意の compile-time `LLB_BUILD_REVISION`。未指定は `unspecified` と表示し、
Git HEAD を検証済みと偽らない。互換性の判定は compatibility ID と package version の完全一致。
privileged contract の変更時は compatibility ID を必ず変更する。

## 停止

先に通常の runtime 切断を行い、driver release → Host detach → Pi unexport → Idle を確認する。
その後、各マシンの policy で指定した一般ユーザーから：

```sh
./target/release/usbip-helper temporary-stop "$LLB_POLICY" --runtime-stopped
```

停止要求は同一接続上の handshake 後、helper が直列に処理する。
caller は runtime 停止を明示確認し、helper は ownership journal が空の場合だけ終了する。
新しい Acquire と停止要求が競合した場合、所有記録ができた方は停止を拒否する。
正常時は自身の socket inode を照合して削除し、固定 staged binary／policy と directory を削除する。
transient unit は systemd により回収される。永続 journal directory／role lock は保持する。
installed helper の Shutdown RPC は拒否する。一時 helper は複数 session の間も維持できる。

runtime の異常終了や TCP loss に合わせて helper を即 kill しない。journal が残る停止は拒否する。
手動 `systemctl stop`、電源断、管理者 kill は orderly cleanup ではない。stale socket／stage／journal は
[USBIP-BACKEND.md](USBIP-BACKEND.md) に従って照合する。全 port detach／自動 journal 削除はしない。

## 今回の検証範囲

実行結果は [TEMPORARY-HELPER-REPORT.md](TEMPORARY-HELPER-REPORT.md)。

native Rust／C ABI、固定 transient argv、policy 読取、compatibility／policy mismatch、
同一接続上の mismatch 時に操作を送らないこと、空 journal／runtime acknowledgement の停止 gate を試験する。
実 systemd transient 起動、sudo 認証、root staging、実 helper RPC と kernel USB/IP、Orange Pi ARM64、
両 runtime の新 preflight API の組み込みは未検証。既存 runtime の Mock 試験と区別する。

systemd の挙動は [公式 systemd-run 説明](https://github.com/systemd/systemd/blob/main/man/systemd-run.xml) を参照。
