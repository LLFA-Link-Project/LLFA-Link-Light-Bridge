# Phase 0 / Phase 1 実施報告

2026-10-11。実装仕様: `LLFA-Link-Light-Bridge-IMPLEMENTATION-INSTRUCTIONS-v0.3.1.md`。
Phase 0 のソース監査と接続された Tracker の read-only inventory を実施し、
Phase 1 のビルド可能な Mock skeleton を実装・検証した。
Phase 0 の USB/IP 実転送・Host tracking driver 認識は未検証。

## Phase 0

1. **変更／成果物**
   - `INTEGRATION-NOTES.md`: WiVRn integration と Monado LLFA の監査、実在するフック候補と不足 API。
   - `USBIP-RESEARCH.md`, `evidence/host-*.txt`: 接続後の USB inventory、制約、cleanup 設計、実機 checklist。
2. **既存ソースから確認した事実**
   - WiVRn HEAD `2795a8b63a5b91c3617328b573a460db25bd679d`、integration commit `0e03cec0e90288a7598b597bb9243067bad49fd9`。
   - `server/driver/wivrn_session.cpp:174` は constructor 内で `steamvr_lh_create_devices()`。
     Bridge Prepare はこの初期列挙前に必要。`:197` で existing external head source を設定する。
   - `patches/monado/0013-d-steamvr_lh-tracking-only-hmd.patch` は latest history timestamp を返す API と秒→ns 変換を持つ。
   - WiVRn Client `client/wivrn_client.cpp:74,224,237` に IPv4/IPv6 接続先がある。
     Bridge port/correlation を通知する既存契約は未確認。
   - Monado HEAD `ad7610a5561325a9b368cda5d45fe195d67f4c3b`。
     `src/xrt/targets/common/target_llfa.c:87` は DIRECT override。
     ローカル Tracker を開く既存 builder を Bridge export 時に抑止する必要がある。
   - `src/xrt/drivers/arpara/arpara_hmd.cpp:95-118,197-202,289` は独立した HID reader と IMU 3DoF 処理、join 後 HID 解放。
     LLFA の head override と、IMU/Lighthouse 融合の完成を混同しない。
3. **設計差分**
   - ソースは入手できたため「存在不明」ではなく具体的な統合候補を記録。
     実 runtime adapter の適用・build は今回の skeleton には含めない。
4. **実行した調査と結果**
   - `git status/log/show/rev-parse`, `rg`, source reads: integration／LLFA 実装を確認。
   - `lsusb`, `lsusb -t`, `usb-devices`, sysfs reads: ユーザーが接続した arpara Tracker は
     `28de:2300`、表示名 Tundra Tracker、serial `LHR-CEC1027F`、3 HID interfaces。
     hub 配下の Watchman Dongle 2個とは別 Device。
   - `command -v usbip usbipd`: ツール無し。module file は存在するが未ロード。
   - `/dev/bus/usb` と hidraw node が見えず、承認付き node 存在確認も ENOENT。
5. **動作すること／Mock／未検証**
   - 実機 identity/topology の読み取りのみ確認。HID open／USB/IP／tracking は未検証。
   - arpara HMD は未接続。HMD＋Tracker 同時接続、ローカル IMU 維持は未検証。
6. **OS 資源／cleanup**
   - bind／attach／module load／driver 切替は一切実行していない。今回作成した OS USB/IP 資源はない。
7. **次作業／blocker**
   - device namespace／権限、usbip tools、限定 helper、Orange Pi の kernel／到達性を確認する。
   - Tracker の Pi 側 inventory と Host driver の実 USB/IP 認識を採取する。
   - Host handle のサーバー lifetime owner、driver release acknowledgement、Monado reference sink を確定する。

## Phase 1

1. **実装した変更**
   - `Cargo.toml` / `Cargo.lock`: Rust workspace、6 crates + headless tool。runtime 外部依存無し。
     初期提出後、ヘッダ生成を build.rs + Rust build dependency cbindgen に移行。
   - `light-bridge-hmd`: create 相当 constructor、endpoint/start、control poll、export、
     pose worker、latest reference slot、stop request、cancel/join。
   - `light-bridge-host`: start、prepare intent、attach/enumeration barrier、tracking_ready、
     pose publish、runtime release の確認、所有 port の detach。
   - `core`: state machine、session ID、EndpointInfo、ExternalPose、clock mapping、拒否 policy、IMU extension stub。
   - `protocol`: length/version/session/op 付き binary codec、制御と pose を分離した容量16の Mock transport。
   - `usbip`: inventory / planner / backend trait、default deny／WholeDevice、Mock ledger／故障注入。
   - `ffi`: role 別 opaque handle、create/start/prepare/tracking_ready/publish/poll/stop/destroy、
     state/release query、heartbeat、C callback、panic barrier、staticlib/cdylib。
   - `build.rs` / `cbindgen.toml` と生成 `light_bridge.h`、C smoke、3 Mock adapters、CI workflow。
2. **既存ソースとの関係**
   - Phase 0 の監査結果に基づく独立実装。WiVRn／Monado checkout は未変更。
   - 架空の upstream 関数や sink を追加していない。実 linking／融合成功は主張しない。
3. **仕様との差分・理由**
   - `DECISIONS.md` を参照。in-memory Mock transport、caller が駆動する非ブロッキング control poll、
     専用 pose worker、synthetic clock と参照 Tracker pose のみ。
   - SessionOffer／実 device offer／ClockSync wire exchange／認証は次 Phase。
   - IMU abstraction は core 内、BSB DirectUsb/UsbMon は Unsupported。
4. **ビルド／テスト結果**

   環境: native Linux x86_64、rustc/cargo 1.99.0。宣言 MSRV 1.80 は未実行検証。
   Orange Pi arm64 toolchain は無く、cross/native arm64 build は未実施。

   | コマンド | 結果 |
   |---|---|
   | `cargo fmt --all -- --check` | 成功 |
   | `cargo clippy --workspace --all-targets --offline -- -D warnings` | 成功、警告なし |
   | `cargo test --workspace --offline` | 21 tests passed / 0 failed |
   | `cargo build --workspace --offline` | 成功 |
   | `cargo build -p light-bridge-ffi --release --offline` | 成功、`.so` / `.a` 生成 |
   | `cargo test` の generated_header_matches_checked_in_abi | build.rs 生成ヘッダの整合性を検証 |
   | `bash tests/ffi/run.sh` | debug shared/static C ABI smoke 成功 |
   | 同じ C smoke を `target/release` の shared/static library にリンクして実行 | 成功 |
   | `cargo run -p mock-session --offline` | reference `[1,2,3]` 配信、stop 後 resources=0 |

   Rust test 内訳: core 4、ffi 2、hmd 3、protocol 2、usbip 2、mock-session 8。
   詳細ログ: `evidence/cargo-test.txt`。C smoke は追加の実行試験で、この21件に含めない。
   CI 定義は追加したが remote CI はまだ実行していない。
5. **現在動作すること／Mock のみ／未実装**
   - Mock endpoint 通知→Hello→Prepare→export→attach/列挙→TrackingReady→Pose→Stop/Stopped。
   - 両順序の prepare、重複／誤 session／不正 frame、期限切れ後 cleanup、故障注入、再接続、worker shutdown。
   - HMD worker が古い／逆行／lost／高 uncertainty／origin 変更 pose を拒否する。
   - C から Mock runtime adapters を通して reference を受け取り、無較正 head_pose_valid=false を確認。
   - 実 TCP/UDP/UDS、kernel USB/IP、privileged helper、実 runtime linking、時計推定／較正／融合は未実装。
6. **資源と cleanup**
   - Host stop は runtime_released を要求し、自分の session port だけ detach、次に HMD unexport。
   - 通信期限切れでは pose を止め、runtime 解放と明示 cleanup を要求。停止 ACK は deadline 後も処理できる。
   - HMD destroy は pose を無効にして cancel/join。userdata に worker は触れない。
   - Active destroy の両順序で共有 Mock ledger が空になる。これは実 OS crash recovery の代替ではない。
7. **次 Phase の具体的な作業／blocker**
   - Phase 2: endpoint/correlation の通知契約、実 Bridge transport、Host pre-enumeration barrier、
     Monado external-reference sink を実装する。実 runtime source は入手済み。
   - Phase 3: 実機の device access／usbip tools／Pi、allowlisted helper、独立 Host/HMD ledger を用意する。
   - Phase 4: Tracker source の厳密な timestamp semantics/origin、mount calibration、clock offset/drift を確定する。
   - 実 runtime release 契約と実 USB/IP が不明な間は、対象実機機能を完成扱いしない。

## ヘッダ生成の更新

ユーザーの指示により Python 前処理を廃止。Cargo build が cbindgen を直接呼び、
OUT_DIR にヘッダを生成する。C smoke は LLB_HEADER_DIR を指定し生成物を使用する。
初回の依存取得以外に外部 generator や Python は不要。
更新後の検証: fmt／clippy 成功、Rust 22 tests passed / 0 failed、
生成ヘッダを使う shared/static C ABI smoke の両方が成功。
この実行は依存キャッシュを `/tmp/light-bridge-cargo` に配置して実施した。
通常の利用には Cargo の標準キャッシュを使用できる。
