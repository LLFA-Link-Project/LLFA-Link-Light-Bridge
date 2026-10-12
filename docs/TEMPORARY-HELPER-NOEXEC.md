# 一時 helper の noexec 対応と起動失敗からの復旧

2026-10-12。実機ログ `~/ArparaOpiLog/35/`〜`37/` では、Bridge fee4eca の
`/run/llfa-link-light-bridge/temporary-auto-export/helper` が root:root／0700 でも
`/run` の `rw,nosuid,nodev,noexec` により systemd `203/EXEC: Permission denied` で停止した。
ログだけでは journal／socket の不存在や停止後の現在状態までは確認できないため、復旧時に照合する。

## 配置と起動の修正

| 用途 | 自動 mode | 手動診断 mode |
| --- | --- | --- |
| binary（root snapshot、0700） | `/var/lib/llfa-link-light-bridge/temporary-auto-ROLE/helper` | `/var/lib/llfa-link-light-bridge/temporary-ROLE/helper` |
| config（root snapshot、0600） | `/run/llfa-link-light-bridge/temporary-auto-ROLE/authorization.json` | `/run/llfa-link-light-bridge/temporary-ROLE/policy.json` |
| bootstrap socket | `/run/llfa-link-light-bridge/ROLE.auto.sock` | 使用しない |
| USB 操作 socket | `/run/llfa-link-light-bridge/ROLE.sock` | 同左 |

ROLE は固定 `export`／`import`。binary staging と state 親は root 所有0700、run 親は0711、run staging は0700。
既存 staging を上書きせず、実行中の `/proc/self/exe` をコピーする。設定だけは noexec の `/run` でよい。
`statvfs(ST_NOEXEC)` を binary 配置前と起動前に確認し、保護されたコピーの `--version` を実行して互換性を照合してから、
固定 transient `systemd-run --service-type=exec` で直接 binary を起動する。shell／動的 loader 経由の noexec 回避はしない。
mount／fstab を変更せず、installer／永続 unit 登録も行わない。

`/var/lib` 自体も noexec の場合は、明確な診断で停止する。どの filesystem も実行不可な環境を強制的に迂回する機能ではない。
この環境では実行可能な `/var/lib` の配置を管理者と確認し、勝手な remount や user-writable directory へのコピーをしない。
正常停止は USB／環境 lease を先に解放し、binary を削除した後に `/run` staging を削除する。
`/var/lib` は reboot 後も残るため、そこだけに staging が残った場合も次の起動は拒否する。

## Orange Pi：203/EXEC 後の復旧

Monado／WiVRn Client の Bridge 利用を停止し、Host 側も停止・cleanup を確認する。
稼働中に `systemctl stop` で helper を強制終了する手順ではない。
今回のように EXEC 前に失敗した場合に限り、以下の audit と復旧を行う。
修正版の helper をビルドしてから使う（更新 commit は各 handoff に記載）。

```sh
cd ~/monado/src/external/llfa-link-light-bridge
cargo build -p usbip-helper --release --locked
./target/release/usbip-helper --version
findmnt -T /run
findmnt -T /var/lib/llfa-link-light-bridge
sudo systemctl show llb-usbip-temporary-auto-export.service \
  -p LoadState -p ActiveState -p SubState -p MainPID -p ControlPID -p Result -p ExecMainStatus
sudo systemctl show llb-usbip-temporary-export.service \
  -p LoadState -p ActiveState -p SubState -p MainPID -p ControlPID
sudo journalctl -u llb-usbip-temporary-auto-export.service -b --no-pager
sudo namei -l /run/llfa-link-light-bridge/temporary-auto-export/helper
sudo ls -la /run/llfa-link-light-bridge
sudo ls -la /run/llfa-link-light-bridge/temporary-auto-export
sudo ls -la /var/lib/llfa-link-light-bridge
```

`--collect` により unit がすでに消えている場合もある。service は absent または inactive/failed、MainPID/ControlPID=0 が条件。
`export.json`、`export.json.new`、`export-environment.json`、`export-environment.json.new` と
`export.sock`／`export.auto.sock` が **すべて不存在** であることを確認する。
state directory の単なる `export.lock` は正常な排他用 file で、削除しない。

次の明示コマンドは自身でも所有権・service・role flock・journal を照合し、削除前に再確認する：

```sh
sudo ./target/release/usbip-helper temporary-recover-auto export --runtime-stopped
```

削除対象は root 所有0700の既知 stage 内の、root 所有 regular file だけ。
旧 `/run/.../temporary-auto-export/helper`（0700）、`authorization.json`（0600）、
新 `/var/lib/.../temporary-auto-export/helper`（0700）のみ。部分的な staging も検査する。
symlink／hardlink／未知 entry／不正 mode／変更された file／不正 authorization／別 role／query failure は中断する。
全 stage の検査が終わるまで削除を始めない。unit を stop/kill、socket を unlink、journal を削除、USB を detach/unbind する機能はない。

journal や socket が存在する場合、このコマンドで強行しない。
[USB/IP 所有権の残留検査](USBIP-BACKEND.md) と runtime release／Host detach／Pi unexport の契約に従い、
該当 session の OS 資源を照合する。原因を確認しない `rm -rf` や journal 消去での再試行はしない。
service と file を別の root 操作者が並行変更しない前提で実施する。

復旧後、同じ runtime user から明示 sudo で起動する：

```sh
sudo ./target/release/usbip-helper temporary-start-auto export --acknowledge-development-build
./target/release/usbip-helper check-auto export
sudo systemctl show llb-usbip-temporary-auto-export.service -p ActiveState -p MainPID -p ExecStart
sudo journalctl -u llb-usbip-temporary-auto-export.service -b --no-pager -n 40
```

不足 package の導入を許可する場合だけ `--allow-package-install`、localhost test だけ `--allow-loopback` を追加する。
`ExecStart` は `/var/lib/.../helper serve-auto /run/.../authorization.json`。
`check-auto export` 成功と service active を確認してから runtime を開始する。
Host の同種失敗は ROLE を import に読み替える。稼働中 helper は正常 release 後に `temporary-stop-auto ROLE --runtime-stopped` を使う。

## 版と検証の範囲

配置と CLI の修正であり、C ABI／LLBA／manual RPC／USB ownership contract は変更しない。
compatibility ID は `llfa-usbip-helper-3-auto-20261012` のまま。旧 helper がその ID を表示するだけでは本修正適用済みの証拠にならない。
修正 commit の source から helper を再ビルドし、service ExecStart と `--version` の build revision を記録する。
`LLB_BUILD_REVISION` をビルド時に指定できるが、変更時は Cargo の環境依存 rebuild を確認する。

native の fixed-path／noexec flag／service properties／所有権／symlink／hardlink／未知 file／CLI 権限 gate を試験。
root systemd 起動、root recovery の削除実行、Orange Pi arm64、実 USB/IP はこの修正作業では未実行。
実機で上記ログを追加採取し、helper 起動成功と USB 転送成功を分けて報告する。

参考: [Linux statvfs の ST_NOEXEC](https://man7.org/linux/man-pages/man3/statvfs.3.html)。
