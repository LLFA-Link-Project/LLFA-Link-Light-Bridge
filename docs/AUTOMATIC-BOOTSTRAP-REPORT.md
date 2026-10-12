# 自動準備 SDK／helper 実装・検証報告

2026-10-12。設計の Light-Bridge 本体を実装。Monado プロセス内の HMD library、WiVRn Server プロセス内の Host library という構成を維持する。headless harness は実証用。installer と BSB は対象外。

## 実装結果

- 接続済み TCP descriptor の local/peer IP・port・network namespace を確認し、SCM_RIGHTS で root helper に渡す。IP／busid／policy／timing ID の手入力を不要にする API を追加。
- root inventory から supported Tracker 28de:2300 を選択。単一候補は自動、複数候補は serial 選択、ゼロ候補は停止。HMD 2c30:1050、vhci、重複 serial、不完全 inventory、既存 USB/IP export は拒否。
- root 授権 limits と exact session plan を分離。helper 互換性と全授権を同じ接続で照合。不一致／停止中／残留 journal から一時 helper や Mock に fallback しない。
- 固定コマンドによる prerequisites、module、nft、専用 transient usbipd、逆方向到達確認を実装。BothPrepared／Commit 前は USB bind／attach しない。Commit 後だけ従来 exact-policy kernel socket を公開する。
- 環境 intent を OS mutation 前に永続化。所有 PID／start time／socket、nft handle と snapshot、boot ID、session leases を照合。runtime release と USB journal 空を確認して daemon／firewall を解放。取消／Drop は worker join を行い、OS cleanup 成功を推定しない。
- bounded worker（要求／応答各4件）、strict LLBA v1 bootstrap、C ABI API を追加。ヘッダは build.rs／cbindgen で生成。既存 TCP v3／LLBI v1／C ABI v1 を維持。
- policy／IP 不要の `temporary-start-auto`、`check-auto`、`temporary-stop-auto` を追加。管理者承認は明示 sudo。永続 service 登録や sudoers 変更はしない。

実装契約と起動例は [AUTOMATIC-BOOTSTRAP.md](AUTOMATIC-BOOTSTRAP.md)。特権契約の compatibility ID は `llfa-usbip-helper-3-auto-20261012`。

## Native 検証

以下すべて成功。通常の Cargo cache と locked dependencies を使用した。

| 検証 | 結果 |
| --- | --- |
| `cargo build --workspace --locked` | 成功、生成 C header snapshot 更新 |
| `cargo fmt --all -- --check` | 成功 |
| `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` | 成功 |
| `cargo test --workspace --locked --offline` | 84 passed、0 failed |
| `bash tests/ffi/run.sh` | 8実行成功（shared/static、各 automatic ABI を含む） |
| `cargo run -p network-session --locked --offline -- --processes` | 成功、Mock Pose [1,2,3]、両端 resources=0 |
| `cargo build -p usbip-helper --release --locked --offline` | 成功 |

自動準備の engine は FakeOps による module／firewall／daemon／journal failure、cleanup barrier、再起動 journal 非 adopt、lease 改変、stale session、候補選択を検証。FD 試験では native TCP／UDS、SCM_RIGHTS、CLOEXEC、元 TCP の flags／データ保持、過剰 FD・版・長さ・取消の拒否を確認した。C ABI は実 shared/static library にリンクし、両端 Commit gate と kernel-required による Mock fallback 拒否を確認した。

別プロセス network-session は既存 Mock lifecycle の回帰試験であり、新 WiVRn bootstrap packet の統合試験ではない。

## 実環境／統合の未検証事項

- root systemd／modprobe／package manager／nft／usbipd コマンドはこの作業では実行していない。具体的な Linux backend を実装したが FakeOps 成功を実 root 準備成功とは扱わない。package 操作の timeout や失敗後の OS 変更は残り得るため journal と OS を照合する。
- 実 Tracker の USB/IP 転送、Lighthouse 起動、実 Pose／時計同期、Orange Pi Armbian、arm64 ビルドは未検証。MSRV 1.80 は維持したが当該 toolchain でのビルドは未実施。
- native Linux の private/link-local LAN、port 3240、nft、同一 network namespace に限定。global IPv6、NAT、別 namespace、自動 kernel 更新は未対応。IPv6 link-local scope、複数 NIC、既存 firewall との到達性は実環境で確認が必要。
- Arch package database は refresh せず、許可 package と依存を preview し既存 package 更新を拒否。Ubuntu／Armbian の usbip wrapper と kernel/module package の整合は実機で未検証。不足した kernel の置換は行わず停止する。
- Monado checkout `06dd3c29f`、WiVRn checkout `b89ffb72` を参照した。いずれも既存 adapter／timing／helper preflight は存在し、Bridge submodule は d765f16。今回は両 upstream の source を変更・ビルドしていない。新 SDK の適用だけでは接続連動の自動準備は有効にならない。
- WiVRn connection bootstrap packet、Client→Monado duplex UDS、候補選択／一時管理者認証 UI、両端 Prepared／Commit／取消 orchestration は upstream 担当の追加統合。Bridge の build/runtime opt-in と disabled regression を維持する。
- 既存資源の自動 adopt、uncertain journal の削除、foreign port detach は行わない。root operator が USB/IP 管理を並行変更しない前提で実証する。remote 認証／暗号化、installer、較正・融合 acceptance は本作業で成立していない。

[Monado 引き継ぎ](HANDOFF-MONADO.md)／[WiVRn 引き継ぎ](HANDOFF-WIVRN.md) は既存実装を再利用する差分手順として更新した。
