# C ABI v1

C ヘッダは `crates/light-bridge-ffi/build.rs` が `cbindgen` を Rust ライブラリとして呼び、
`cargo build` の中で自動生成する。Python と外部 generator executable は不要。
設定は `cbindgen.toml`、通常の出力は Cargo `OUT_DIR/light_bridge.h`。
ソース／設定／core DTO の変更で再生成され、通常ビルドはソースツリーを書き換えない。

C/CMake の include directory に配置する場合は絶対パスで指定する:

```sh
LLB_HEADER_DIR="$PWD/target/ffi-include" cargo build -p light-bridge-ffi --locked
```

`crates/light-bridge-ffi/light_bridge.h` はレビュー／配布用 snapshot。
`cargo test` は自動生成物と snapshot の一致を検証する。ABI を変更したときの snapshot 更新は:

```sh
LLB_HEADER_DIR="$PWD/crates/light-bridge-ffi" cargo build -p light-bridge-ffi --locked
```

build dependency は Cargo.lock で固定。初回取得後は offline で動く。
`cargo build -p light-bridge-ffi` は `liblight_bridge_ffi.so` と `.a` も生成する。
同じ ABI の role=0 は HMD library、role=1 は Host library を保持する opaque handle。

Config／Endpoint／Pose は repr(C)、固定幅整数、size と abi_version を必須にする。
不明 version または不一致 size は InvalidArgument。呼出側は少なくとも prefix 8 bytes を
読み取り可能にし、正しい size の構造体を有効・整列済みのポインタで渡す。
`LlbEndpoint` は IPv4/IPv6、port、scope_id、host ID、correlation ID。
Rust `EndpointInfo` は候補複数 address を保持できるが、ABI v1 は選択済み単一 address。

基本順序: create(role) → mock_link(hmd,host) → request_start → request_prepare → poll
→ tracking_ready → publish_pose／poll → request_stop → poll Stopped → destroy。
Mock link は test-only。同一プロセスで2 role を接続し、別プロセスを結ぶ network API は未実装。
Host prepare と HMD endpoint/start の順序はどちらが先でもよい。
Host request_start の endpoint は unused/null 可。HMD は endpoint 必須、correlation=session。

callback は同期的に poll/request_start/request_stop の caller thread で直列に呼ぶ。
event=1 は状態、event=2 は **reference Tracker pose**。
pose pointer は callback 中だけ借用。保存する場合はコピーする。
全 API は各 handle ごとに外部で直列化する。callback から同じ handle の API や destroy を呼ばない。
同時 destroy／use-after-destroy は caller 契約違反。worker は callback と userdata を使用しない。

Host stop の runtime_released=1 は tracking driver の USB 使用を停止し handle を解放済みという約束。
0 では Ownership error。HMD stop は Host に release を依頼するだけ。
`llb_get_state` の needs_runtime_release=1 は runtime 解放と Host stop を要求する。
`llb_heartbeat` と `llb_poll` の周期管理は adapter の control thread に置く。pose query thread には置かない。
`llb_mock_clock` は synthetic receive now、Host→HMD offset、uncertainty を設定し、clock ID=1 に限定。
実時計同期の API ではない。

request error は return code。すべての export は catch_unwind barrier を通る (panic=255)。
C callback 自体が throw/unwind/longjmp して Rust を越えることは禁止。

`destroy(&handle)` は owning slot を null にし、callback を無効化、pose worker を cancel/join して解放。
同じ null slot の2回目 destroy は成功。コピーした古いポインタの destroy は禁止。
worker は5ms recv timeout と bounded processing で退出する。self-join を生む worker callback はない。
OS scheduling に依存するため hard realtime deadline は保証しない。Mock join の1秒以内終了はテスト済み。
Mock destructor の USB cleanup は実 OS crash recovery の保証ではない。
