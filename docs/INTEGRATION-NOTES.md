# Phase 0: 既存ソース監査 (2026-10-11)

## 根拠と参照範囲

Bridge リポジトリは初期状態でコミット無し、実装指示書だけが未追跡で存在した。
以下のローカル checkout を read-only で調査した。両者の `git status --short` は空。
upstream master と同一とは仮定しない。

- WiVRn: `/home/reina/WiVRn`, HEAD `2795a8b63a5b91c3617328b573a460db25bd679d`。
- 適用済み integration: `0e03cec0e90288a7598b597bb9243067bad49fd9`
  (`feat: use Lighthouse HMD tracking with WiVRn server display`)。
- Monado LLFA: `/home/reina/monado`, HEAD `ad7610a5561325a9b368cda5d45fe195d67f4c3b`。

## WiVRn Server

`server/driver/wivrn_session.cpp:141-174` で Lighthouse の利用条件を決定し、
session constructor 内の `steamvr_lh_create_devices(nullptr, this)` (`:174`) が初期列挙を行う。
今回必要な Prepare／USB attach・列挙完了 barrier は **この呼出しより前**。
Host Bridge handle の寿命をこの constructor/destructor 単位に限定すると再接続時に消えるので、
サーバー lifetime の所有場所は次 Phase で別途設計する必要がある。
constructor が例外を投げた際の Lighthouse device rollback (`:230` 付近) と衝突させない。

`server/driver/wivrn_session.cpp:178-202` は serial／auto により source device を選択し、
`hmd.set_external_head(..., steamvr_lh_get_latest_pose, mount)` を呼ぶ。
WiVRn head は表示機能を維持し、Lighthouse origin を採用する。
`server/driver/wivrn_hmd.cpp:140-178`／`external_head.h:66-102` で latest observation と
予測問い合わせを分離し、鮮度・tracked flags を検査する。

`patches/monado/0013-d-steamvr_lh-tracking-only-hmd.patch:14-17,43-48,159-162`
では最新 relation history の時刻を取得する API が追加され、OpenVR poseTimeOffset は秒から ns に変換される。
これは履歴への観測登録時刻の根拠であり、センサーの光学測定時刻を保証するものではない。
Bridge がこの source を採用する際は clock domain と semantics を再確認し、推定／query-time を実測扱いしない。
現段階の Mock は estimated (`timestamp_semantics=1`)。

`WIVRN_EXTERNAL_HMD_POSE_OFFSET` は未指定なら identity。
Bridge ではこれを実較正済み head pose の証拠として利用しない。
二重 mount 補正を避けるため、raw Tracker reference をどこで採取するかを次 Phase で確定する。

## WiVRn Client の endpoint 通知候補

`client/wivrn_client.cpp:74` 付近の `wivrn_session::handshake(T address, ...)` は IPv4/IPv6 の
address を既に持ち、`:127-134`／`:179-186` 付近では stream port に書き換えて connect する。
`:224`／`:237` の constructor も接続先 address を保持している。
**既存の動画 stream port は Bridge port ではない**。

endpoint を知る段階で通知できることは確認したが、Bridge port／host ID／correlation ID を
提示する既存 packet または UDS は見つかっていない。既存暗号 handshake と
Bridge session を関連付ける仕組み、切断時の通知、Monado 側 socket owner は未実装。
Mock adapter は `EndpointInfo`／`LlbEndpoint` を直接通知し、WiVRn wire protocol を変更しない。

## Monado LLFA

`src/xrt/targets/common/target_builder_llfa.c:82-102` は arpara を開いた後、
ローカル `survive_get_tracker_devices()` により Tracker を初期化する。
Bridge export 中にこの経路が同じ Tracker を開くことを避ける必要がある。
`src/xrt/targets/common/target_llfa.c:72-93` の `t_llfa_wrap_head()` は
`multi_create_tracking_override(XRT_TRACKING_OVERRIDE_DIRECT, ..., mount)` を使う。
**既存実装は直接 pose 置換であり、IMU と Lighthouse の融合ではない**。

`doc/arpara-llfa-link.md` に mount の方向は
`world_from_hmd = world_from_tracker * inverse(hmd_from_tracker)` と記載。
`T_HMD_Tracker = hmd_from_tracker` を設定する必要があるが、今回の Mock は適用しない。
既存 driver に Bridge external-reference sink は確認できず、`xrt_imu_sink` の存在だけで
LLFA 専用の pose 受信 API があるとは扱わない。

## 次の統合作業

1. Host handle を WiVRn Server lifetime に所有させ、初回列挙前に期限付き Prepare barrier を置く。
2. endpoint／correlation の通知契約を決める。接続先既知イベントと Bridge negotiation と TrackingReady を分ける。
3. Monado LLFA に reference と clock metadata の同一プロセス受信口を作り、ローカル Tracker probing を抑止する。
4. Host runtime の tracking source、origin reset、driver release の acknowledgement を確定する。
5. 実 runtime をビルド・起動し、Bridge 無効の既存動作を回帰検証する。

今回これらはソース監査のみ。WiVRn／Monado の再ビルド・実リンク・起動回帰は未実施。
