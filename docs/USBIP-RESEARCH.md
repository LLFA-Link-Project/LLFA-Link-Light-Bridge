# Phase 0: USB/IP と実機調査

2026-10-11、Linux x86_64 `7.2.9-arch1-1` のツール実行環境で read-only 調査。
記録: `evidence/host-lsusb.txt`, `host-lsusb-tree.txt`, `host-usb-devices.txt`。
調査途中にユーザーが arpara Tracker を接続したことを明示した。
初回の未接続結果と、接続後の結果を混同しない。evidence は接続後。

## 実機で確認できた事実

| USB device | busid (この接続時のみ) | Interfaces | driver |
|---|---|---|---|
| arpara Tracker (USB 表示名 Tundra Tracker), `28de:2300` | `1-3.3` | HID 3個 | すべて usbhid |
| Watchman Dongle, `28de:2101` | `1-3.2` | HID 1個 | usbhid |
| Watchman Dongle, `28de:2101` | `1-3.4` | HID 1個 | usbhid |
| USB hub, `1a40:0101` | `1-3` | hub | hub |

Tracker serial は `LHR-CEC1027F`、sysfs realpath は
`/sys/devices/pci0000:00/0000:00:01.2/0000:01:00.0/usb1/1-3/1-3.3`。
Tracker と Dongle は内部 hub 配下の **別 Device**。
Tracker の interface 0/1/2 を単独 export する構成に読み替えない。
user の接続申告と列挙結果により Tracker identity を対応付けた。
これは firmware config の独自検証や Lighthouse の追跡成功を意味しない。

arpara HMD `2c30:1050` は今回接続されていない。
HMD と Tracker の同時接続時の別接続確認・ローカル HMD IMU 維持は未検証。
Bridge allowlist にこれら実機 identity を自動登録していない。Mock は synthetic `ffff:0001`。

## USB/IP の利用可否

- `lsusb`, `lsusb -t`, `usb-devices`, sysfs はサンドボックス内で読めた。
- `usbip`／`usbipd` は PATH に無い。
- `/sys/module/usbip_host`／`vhci_hcd` は無い（ロード済みでない）。
- `/usr/lib/modules/7.2.9-arch1-1/kernel/drivers/usb/usbip/` に
  `usbip-host.ko.zst`, `vhci-hcd.ko.zst`, `usbip-core.ko.zst` は存在する。
  **module file の存在はロード・権限・USB/IP 動作の検証ではない**。
- `/dev/bus/usb` と `/dev/hidraw0` はツール実行環境から見えない。
  承認付き `ls -ld /dev/bus/usb /dev/bus/usb/001/009` でも ENOENT。
  実行環境の device namespace／mount の制限か、ホスト側の公開条件かは未確定。
  サンドボックス内から libusb/hidraw を実際に開けるとは扱わない。
- module load、bind/unbind、attach/detach、HID open、firmware command は実行していない。
- Orange Pi は未接続。Pi 側 kernel／usbipd／権限／到達性は未検証。

Linux の標準 USB/IP は export/import と URB の転送を OS に任せる構成。
参考: [kernel protocol](https://docs.kernel.org/usb/usbip_protocol.html)、
[Linux tools README](https://github.com/torvalds/linux/blob/master/tools/usb/usbip/README)。
Bridge 専用 frame は USB/IP wire protocol を再実装しない。

## 選択・特権境界

`UsbInventory` → `ExportPlanner` → `UsbTransferBackend` を分離。
planner は VID/PID、serial、busid、sysfs path、topology の exact allowlist 照合を要求する。
`2c30:1050` は allowlist に含まれても拒否。
Interfaces 要求は `UnsupportedExportGranularity`。WholeDevice に拡大しない。
Mock も同一 device の別 session への二重 export を拒否し、enumeration ready まで attach 成功にしない。

実 kernel backend／特権 helper は Phase 3。固定操作スキーマ、対象 allowlist、
コマンド引数配列、deadline、stderr/status 検査、所有権記録を実装してから利用する。
一般ユーザープロセスから任意 root shell は実行しない。

## Cleanup と crash recovery

Mock ledger は session に export と port を紐付ける。Host が自身の port を detach してから
HMD が unexport。HMD の request_stop だけでは Host runtime の USB ownership を奪わない。
正常 stop と active destroy の両順序で ledger が空になる試験を実施。
Host destructor の export cleanup は共有 ledger を持つ **Mock 固有の crash 模擬処理**。
実 backend で Host が遠隔 unbind を直接実行する契約ではない。

実 OS 資源は今回作成していない。実 crash recovery は次の設計・検証が必要:

1. helper が session/instance、対象 identity、bind 前 driver、実測 vhci port を永続化する。
2. 起動時に記録と sysfs／usbip port を照合し、自分が作成した対象だけ回収候補にする。
3. runtime release を確認できない port は残留を診断し、他者の port を detach しない。
4. Host detach 確認後に Pi 側 unexport／ローカル driver 復帰を確認する。

## 実機 checklist / blockers

- [ ] Pi と Host の device node 可視性・ユーザー権限、kernel module load を確認。
- [ ] usbip tools／限定 helper を用意し、明示選択した検証 HID で attach/detach。
- [ ] 今回確認した Tracker を Pi に接続して identity/topology を再採取。
- [ ] Host の既存 Lighthouse driver に USB/IP 経由で列挙させる。
- [ ] HMD 同時接続で `2c30:1050` が bind されず IMU が維持されることを確認。
- [ ] Wi-Fi loss／ケーブル抜去／OS crash の限定 cleanup を確認。

実機 Tracker の identity は入手できた。USB/IP 転送・Host 認識・Pi 側検証は依然 blocker。
