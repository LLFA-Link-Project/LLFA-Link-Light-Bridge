# Phase 3 Linux USB/IP Backend 実装・検証報告

検証日: 2026-10-11。仕様: `LLFA-Link-Light-Bridge-IMPLEMENTATION-INSTRUCTIONS-v0.3.1.md`。
**Backend のコードと Mock／故障試験を実装した。Phase 3 の実機合格条件は未達。**
現 PC の Tracker のみで、Orange Pi／arpara HMD／両 runtime の実統合は未使用。

## 成果物

- ビルド可能な Rust workspace、Monado 内 HMD／WiVRn Server 内 Host library の構成を維持。
- standard usbip CLI による bind/attach/detach/unbind と sysfs 列挙待ちを実装。
- 一般ユーザー library の kernel proxy、bounded queue／cancel/join を持つ専用 USB worker。
- root helper: SO_PEERCRED、exact identity policy、固定 executable／argv、期限／出力上限。
- intent を OS mutation 前に永続化。instance/session/boot ID、測定した vhci port fingerprint、前 driver を記録。
- runtime release → Host detach 完了 → HMD unbind/rebind／元 driver 復帰確認。
- Bridge frame/TCP preface v2 の UsbExportReady identity。C ABI layout／UDS version は1を維持。
- C kernel configuration、generic TCP、UDS Start/Stop、HMD notification application。
- helper policy／systemd 配備テンプレート、未確定 journal／stale socket の手動照合手順。
- BSB は既存拡張型／Unsupported stub のまま。今回の実装対象に含めない。

## native 検証結果

Linux x86_64、kernel `7.2.9-arch1-1`、rustc/cargo `1.99.0`。
環境の writable cache `CARGO_HOME=/tmp/light-bridge-cargo` を使用。

| コマンド | 結果 |
|---|---|
| `cargo build --workspace --locked --offline` | 成功 |
| `cargo fmt --all -- --check` | 成功 |
| `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` | 成功 |
| `cargo test --workspace --locked --offline` | 56 passed、0 failed |
| `bash tests/ffi/run.sh` | in-memory／TCP × shared／static の4実行に成功。UDS codec も検査 |
| `cargo run -p network-session --locked --offline -- --processes` | Client UDS → HMD TCP → Host prepare/Mock attach → reference pose → stop 成功。両 Mock ledger=0 |
| `cargo run -p usbip-probe --locked --offline -- --device 28de:2300` | read-only inventory 成功 |
| `git diff --check` | 成功 |

[Rust test log](evidence/phase-3-cargo-test.txt)、[C ABI log](evidence/phase-3-c-ffi.txt)、
[別プロセス Mock log](evidence/phase-3-network-session.txt)、[USB inventory](evidence/phase-3-usbip-probe.txt)。

追加試験は helper ownership/journal/CLI と slow Mock を使った USB worker を検証。
export/attach 中の Stop、poll を低速 USB 操作で停止させないこと、destroy の cancel/join、
別 owner／identity／不正 peer の拒否、不確定 attach の保持、driver release barrier を対象にする。
これらは kernel の実 bind／attach 成功を示す試験ではない。

## この PC の実観測

`lsusb -d 28de:2300`: `Bus 001 Device 013: ID 28de:2300 Valve Software Tundra Tracker`。
ユーザーが接続した arpara Tracker の観測 identity は busid `1-3.3`、serial `LHR-CEC1027F`。
`lsusb -t`／sysfs で3 HID interface と `usbhid` を確認、vhci ancestor はない。
ローカル USB としての列挙であり USB/IP import の証拠ではない。

`usbip`／`usbipd` は PATH に見つからず、usbip_host／vhci_hcd は未ロード。
対応 module file は存在。`/dev/bus/usb` は現在の実行環境では不可視だが、
それだけで sysfs ベースの USB/IP が不可能とは判断しない。
この作業では module load／package install／root helper 起動／USB driver の変更を行っていない。

## 未検証・残る作業

- root helper／usbipd の配備と actual CLI/kernel behavior、実 OS 列挙と tracking driver 検出。
- 実 attach/detach／driver 復帰、Wi-Fi／ケーブル切断、連続再接続、OS/process crash 回収。
- arpara HMD `2c30:1050` とローカル HID/IMU の維持。deny はコード試験のみ。
- Orange Pi arm64、宣言 MSRV 1.80、release 配備、systemd template、リモート CI。
- upstream Monado／WiVRn のリンクと起動回帰、Client endpoint 広告と runtime release 実連携。
- 実 Lighthouse pose、clock offset/drift、timestamp semantics／origin reset、較正／LLFA 融合。
- Bridge TCP／USB/IP の remote 認証・暗号化。現段階はアクセス制限した検証ネットワーク向け。
- TCP 切断時の transport 差し替え API は未実装。正常 stop 後の同一接続は再利用できるが、
  切断後は OS cleanup を確認して接続 handle を作り直す必要がある。

CLI timeout は kernel rollback 保証ではない。不確定 journal を自動 adopt／推測 cleanup しない。
real backend destroy は worker cancel/join のみ。残留 OS 資源は管理者照合が必要。
外部 root 操作による同一 port/device 再利用を fingerprint だけで完全に防ぐとは主張しない。

## 他プロジェクトへの引き継ぎ

- [Monado 担当 Codex 指示書](HANDOFF-MONADO.md): HMD 内 library／通知受信／reference sink と IMU 維持。
- [WiVRn 担当 Codex 指示書](HANDOFF-WIVRN.md): Server 内 library、早期 endpoint 広告、列挙前 prepare、driver release。
- [USB/IP 配備・回収手順](USBIP-BACKEND.md): exact policy、helper、正常停止／不確定状態の扱い。

指示書は調査した実ソースの commit と位置を記録し、新規 adapter を既存 API と区別している。
両担当は同じ Bridge commit を記録して使用し、フル環境が揃った後で Phase 3 の実機合格証拠を採取する。
