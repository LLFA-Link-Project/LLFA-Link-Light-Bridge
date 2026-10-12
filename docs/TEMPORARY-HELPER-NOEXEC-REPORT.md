# noexec 一時 helper 修正の検証報告

2026-10-12。実装修正 commit **da75130**。
実機受領ログは ~/ArparaOpiLog/35〜37。fee4eca の /run binary が203/EXECで停止、/runはnoexec、fileはroot所有0700。
本作業は提供ログの読み取りとnative実装／試験。Orange Piへ接続・root操作していない。

- auto/manual両roleの実行snapshotを /var/lib に移し、config/socketを /run に維持。
- mount flag確認とstaged --versionの実行確認後に固定systemd-runで直接起動。OS mount変更や永続登録はない。
- temporary-recover-auto は停止/不存在unit、role flock、空journal/socket、root所有の既知stageを再確認後に回収。
- 不確かなresources、unknown entry、symlink、hardlink、不正mode/owner、query failureを拒否。journal/socketを削除せずserviceを停止しない。
- 正常停止ではbinaryを先に削除し、一般runtime userから見える/run completion markerを最後に削除する。

| Native検証 | 結果 |
| --- | --- |
| cargo build --workspace --locked | 成功 |
| cargo fmt --all -- --check | 成功 |
| cargo clippy --workspace --all-targets --locked --offline -- -D warnings | 成功 |
| cargo test --workspace --locked --offline | 89 passed / 0 failed |
| bash tests/ffi/run.sh | shared/static C ABI 8実行成功 |
| cargo run -p network-session --locked --offline -- --processes | Mock配信／停止成功、両resources=0 |
| cargo build -p usbip-helper --release --locked --offline | 成功 |

追加試験はfixed stage path／ST_NOEXEC rejection、unit statusとPID条件、stage全体のknown-file／owner／mode／link検査、監査中の変更、dangling journal/socket、明示root CLI gate。
service statusはfixture、所有権試験はnative filesystemでcurrent UIDをテスト用expected UIDとして使用した。実root所有fileの削除やsystemdの実起動を試験したものではない。
ABIは変更せず既存generated-header整合性も維持。MSRV宣言1.80、当該toolchainの実行は未検証。

## 統合確認と未検証

WiVRn ff2fb9d6871b26d0a3a0a028eec40daf12ab8921／Monado 45866c990a14ae9067b983f61ada2a05cd5474adをread-onlyで確認。
両submoduleはfee4eca、auto adapterは統合済み。今回はupstream変更・ビルドしていない。
[WiVRn引き継ぎ](HANDOFF-WIVRN.md)／[Monado引き継ぎ](HANDOFF-MONADO.md) に修正commit、既存hook、remote追従と再ビルド手順を追記した。
WiVRn利用者は担当公開remoteに追従し、未公開独自SDKを使用しない。

root systemd起動／failed-stage recovery削除実行／PKEXEC／Orange Pi arm64／実USB/IP・姿勢は未検証。
/var/lib自体がnoexecなら明示停止する。実機は[手順](TEMPORARY-HELPER-NOEXEC.md)でmount、ownership、service、journalを照合してから復旧し、ExecStartが/var/libになったこととhelper active／check-autoを記録する。
このnative結果は実機起動成功やUSB転送成功を示さない。remoteへのpushは本作業では実行していない。
