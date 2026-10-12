# Helper の導入・互換性・一時利用契約

ユーザー合意（2026-10-12）。installer は後回しとし、開発用 CLI の一時起動、
RPC v2 の互換性 preflight と C API を先に実装した。
利用手順／実装範囲／未検証事項は [TEMPORARY-HELPER.md](TEMPORARY-HELPER.md) を参照。
installer、socket activation、Dashboard の選択 UI／自動認証は未実装。

最新要求の通常利用は [AUTOMATIC-BOOTSTRAP-DESIGN.md](AUTOMATIC-BOOTSTRAP-DESIGN.md) に従い、
WiVRn 接続から route と単一 Tracker を選び、policy と USB/IP 初期準備を自動化する。
許可 limits と session policy を分離した SDK／helper を実装済み。
[AUTOMATIC-BOOTSTRAP.md](AUTOMATIC-BOOTSTRAP.md) を参照。runtime 接続と GUI は後続作業。
以下の現行 exact-policy／一時 CLI は診断・開発経路として維持し、互換性拒否を緩めない。

## 利用判断

USB mutation、Bridge session の開始、Lighthouse 初期化より前に、両 runtime は
それぞれローカル helper の導入状態と互換性を確認する。

| 状態 | 動作 |
| --- | --- |
| 導入済み・互換性一致 | 既存 helper を利用。通常起動で追加の管理者権限要求は出さない |
| 導入済み・不一致 | Bridge の開始を中断。期待／実際の version と build を表示し、installer 経由の更新を要求 |
| 未導入 | 「installer 経由でインストール」「管理者権限で今回だけ利用」を提示。取消可能 |
| 状態不明・応答期限切れ・権限／所有者不正 | 診断付きで中断。未導入と同一視しない |

不一致の場合、一時 helper や Mock へ自動 fallback しない。helper の停止中と未導入は区別する。
installer の更新はユーザーの明示操作と管理者承認を要し、runtime が勝手に置換しない。
「すべて中断」は当該 Bridge 開始試行を意味し、共有 helper や他 session を勝手に停止しない。
開始前に判定することで新しい USB 資源を作らない。既存／不確定資源がある場合は従来の
release → detach → unexport と journal 照合を維持し、判定失敗を理由に journal を削除しない。
相手側の準備失敗も通知し、片側だけ成功扱いで開始しない。

## 互換性識別

Cargo の package version（現在 0.1.0）や RPC version=1 だけでは build の一致を判断できない。
専用の helper compatibility ID を runtime と helper の build に埋め込み、初期実装では完全一致を要求する。
製品 version、build revision、RPC version、role、必要な capability も診断に含める。
現在の compatibility ID は `llfa-usbip-helper-3-auto-20261012`。privileged contract の変更で更新する。
RPC v2 は ID と全 policy を検証する。Git revision の完全一致ではなく専用 ID と package version の一致。
任意 compile-time build revision は診断専用で、未設定は unspecified とする。

インストール情報だけを信用せず、root 所有／保護された配置と実際に接続した root peer を検証し、
非 mutation の handshake で動作中 helper の識別情報を照合する。Acquire より前に照合を完了し、
その後の RPC 接続でも別 helper への入れ替わりを防ぐ。一致していても exact policy、UID、
device identity、所有権、残留 journal の検証を省略しない。

## installer と一時利用

installer は Monado／WiVRn の配布物に同梱可能。root helper、policy、service の登録を一度承認し、
以後は許可された一般ユーザーが固定 RPC だけを利用する。アプリ全体を root で起動しない。
root が実行する binary、policy、unit とその親 directory は一般ユーザーから変更できない配置とする。

一時利用もユーザーが明示選択してから管理者権限を要求する。既存 helper と衝突させず、
root 保護された一時配置から実行する。ユーザー writable のビルド binary を無確認で root 起動する
方式、任意 shell RPC、包括的な sudoers 許可は使わない。
永続 service 登録を残さない場合でも、USB 所有権 journal は永続領域に保持する。

systemd は起動管理の候補。socket activation は現行 helper の inherited FD 対応が必要で、
現時点では使用できない。認証方法（polkit／管理者コマンド等）と実際の UI は各 runtime 担当が実装する。
UI を持たない Monado は構造化した状態／診断を返し、対話可能な launcher／installer が選択を受ける。
選択待ち・認証・helper 起動は adapter の制御経路で行い、pose query／HID callback をブロックしない。

一時 helper の停止／削除は runtime driver release → Host detach → Pi unexport の完了後。
異常時には journal と必要な診断を残す。runtime の終了に合わせて即 kill しない。

## 必須の検証

導入済み一致／不一致／未導入／停止中／応答なし／偽 socket／不正所有者、管理者承認拒否、
installer 更新失敗、二重起動、相手側準備失敗、同一 package version の異なる互換性 ID を試験する。
中断時に新規 bind／attach がゼロであること、正常終了と異常終了で所有権 barrier と journal が
維持されることを確認する。Mock／unit 試験と実 systemd／管理者認証／USB 試験は分けて報告する。

一時起動の実行 snapshot は `/var/lib`、設定は `/run`。noexec の起動失敗と明示復旧は [TEMPORARY-HELPER-NOEXEC.md](TEMPORARY-HELPER-NOEXEC.md)。
