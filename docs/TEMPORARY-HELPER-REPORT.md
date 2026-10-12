# 開発用一時 helper 実装・検証報告

2026-10-12。ユーザー指示により installer を後回しにし、開発用一時起動を実装した。

## 実装

- usbip-helper CLI: check、temporary-start（明示 sudo＋開発 build 承認）、temporary-stop（runtime 停止確認）、--version。
- 保護された /run snapshot、固定 systemd transient service、永続 unit 登録なし、二重起動／既存導入／stale stage 拒否。
- helper RPC v2: 非 mutation Describe、compatibility ID／package version／全 policy、同一接続上の操作前照合。
  各操作要求も helper 側で検証。旧 RPC v1 を拒否。Bridge TCP v3／UDS v1／C ABI v1 は維持。
- 一時 helper の Shutdown は runtime acknowledgement と空 ownership journal が必要。
  installed helper の Shutdown は拒否。正常時のみ所有 socket と固定 stage を削除。
- C API llb_check_kernel_usbip: configure 後、TCP 前の setup worker で事前確認。自動昇格／helper 起動なし。
- Monado／WiVRn handoff、利用契約、ABI／protocol／テスト計画／AGENTS を更新。

## 実行した検証

native Linux x86_64、現在の Rust/C toolchain：

- cargo build --workspace --locked: 成功。
- cargo build -p usbip-helper --release --locked: 成功。
- cargo fmt --all -- --check: 成功。
- cargo clippy --workspace --all-targets --locked --offline -- -D warnings: 成功。
- cargo test --workspace --locked --offline: **69 passed、0 failed**。
- bash tests/ffi/run.sh: **6 shared/static 実行成功**。生成 header snapshot と新 symbol の検証を含む。
- network-session --processes: 成功、Mock Host/HMD resources とも0。
- release CLI --version: compatibility=llfa-usbip-helper-2-20261012、RPC=2、build=unspecified。

追加試験は compatibility mismatch／旧 version／全 policy 差異、UnixStream pair の同一接続 handshake、
不一致時に mutation を送らないこと、Mock kernel engine の OS calls=0／shutdown barrier／journal 保持、
固定 transient argv／任意 path 拒否、policy size／symlink／HMD 拒否、CLI 引数と非 root 起動拒否。

記録: [Rust tests](evidence/temporary-helper-cargo-test.txt)、
[C ABI](evidence/temporary-helper-c-ffi.txt)、[別プロセス Mock](evidence/temporary-helper-network.txt)。

## 未検証・後続

- 実 sudo 認証、root staging と SO_PEERCRED handshake の実サービス往復、system systemd transient 起動／正常 stage 削除。
- 実 kernel USB/IP／usbipd、Pi inventory、通常／異常 cleanup、ARM64／Orange Pi、宣言 MSRV の実 toolchain 試験。
- installer、socket activation、Dashboard の選択 UI／自動管理者認証。
- Monado 978e79022／WiVRn 16bd3529 は read-only source inspection のみ。新 preflight API 呼出の追加／submodule 更新／再リンクは両担当へ引き継いだ。

実 root service、systemd 登録、module load、USB bind／attach／detach／unbind はこの検証では行っていない。
テストは syscall/privileged deployment の実機成功を意味しない。
開発者の明示 sudo は当該 binary を信頼する承認であり、署名付き installer の代替ではない。
失敗時に stage／socket／journal を自動消去する機能は追加していない。
