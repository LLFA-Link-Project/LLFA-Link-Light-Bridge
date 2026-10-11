# Phase 1 の実装判断と仕様との差分

1. **6 crate + headless harness**: 必須の HMD／Host library を維持し、IMU stub と clock DTO は core に置いた。
   BSB 専用 crate／capture 実装は作成しない。
2. **外部依存なし**: std の bounded mpsc と pose receiver thread を用いる。
   Phase 1 transport は encode/decode 済み frame を in-memory に送る。実 TCP/UDP/UDS は未実装。
   phase1 の versioned protocol skeleton を検証する選択で、実ネットワーク成功とは報告しない。
3. **USB は明示 Mock のみ**: libraries の constructor は Shared Mock backend を受け取る。
   inventory/planner/backend trait は分離したが、kernel backend を inject する constructor と
   real device offer、Host/Pi 間独立 ownership ledger は Phase 3 で追加が必要。
4. **制御は adapter が poll**: Mock の非ブロッキング制御・USB 操作を caller の control thread で行う。
   pose receiver は別 worker、pose channel は control と別。低速の実 USB 操作をこの poll に追加してはならない。
   実 backend と network 導入時に control／USB worker、cancelable deadline を追加する。
5. **FFI callback は poll/request の caller thread**: worker は userdata にアクセスしない。
   再入不可・handle ごとの直列化を C 側に要求。destroy は callbacks を無効化し、pose worker を cancel/join。
6. **Pose は参照 Tracker のまま**: 較正／融合は未実装。Mock Monado adapter の head_pose_valid は常に false。
   velocity／origin／clock／semantics を含む DTO と拒否 policy は実装したが、head pose に昇格しない。
7. **時計は synthetic**: Mock adapter が受信側 now と ClockMapping を明示注入する。
   NTP 的 exchange／offset drift 推定を実装したとは扱わない。
8. **session/operation**: 相関 ID を session ID として Mock 通知。caller は新しい接続で一意な非零 ID を発行する。
   制御 operation ID は v1 の各ステップ固定。直前の同種再送は no-op、遅延古い op は reject、
   同じ op ID の異種 command は Protocol error。無制限の履歴は保持しない。
   最新 session ID の使い回しは禁止。過去全 session の履歴管理／認証は実 network で決める。
9. **Header 生成 (更新)**: Python generator を廃止し、build.rs から Rust build dependency
   cbindgen 0.29.4 を呼ぶ。ヘッダは Cargo OUT_DIR に自動生成、任意の C include directory へ
   LLB_HEADER_DIR で配置可能。ソース内 snapshot との一致は cargo test、C smoke は生成物を直接使用。
   Rust の構文解析と型解決を既存 generator に任せ、独自の文字列パーサーを維持しない。
   runtime crates の外部依存は無いが、ビルド時には cbindgen と依存の初回取得が必要になる。
   Cargo.lock は宣言 MSRV 1.80 を考慮して解決 (非対象の WASI 依存は除く)。実 MSRV 実行は未検証。


仕様の同一プロセス構成、arpara HMD default deny、WholeDevice 制約、runtime release → detach → unexport は維持。
Phase 2/3/4 相当の実統合、OS USB、時計推定／融合を完成扱いしない。

## Phase 2 の更新（上記 Phase 1 判断を部分的に置き換える）

- 外部 runtime 依存を増やさず std TCP/UDS を追加。制御／pose を独立 socket にし、
  bounded queues、partial I/O、設定 deadline、cancel/join を実装。
- Rust endpoint 通知は複数 NIC／IPv6 scope を維持。C Endpoint ABI は単一候補のまま、既存 layout は変更しない。
- Host/HMD の Mock 台帳を分離。Host は現在 session の UsbExportReady を受信した場合だけ
  固定 MOCK-TRACKER の mock offer を記録する。実 inventory／device offer／USB authorization ではない。
- test subprocess は runtime stand-in であり、本番 daemon に構成変更しない。
- 実ソースは存在するが upstream adapter の組み込み・build はまだ未完了。
  Mock adapter の成功を runtime 統合や無効時 WiVRn 起動回帰の証拠にしない。
- read-only SysfsInventory／usbip-probe は Phase 0 の診断改善。Phase 3 の USB 操作を先行実装したとは扱わない。
- 認証・暗号化・広告の信頼契約は未実装。TCP C API は `_mock` を明示し、本番 listener と扱わない。
