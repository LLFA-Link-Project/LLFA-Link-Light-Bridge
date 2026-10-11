# monado-llfa adapter (Mock)

`mock_adapter.h` は 参照 Tracker pose のコピーと無較正 head_pose_valid=false のテスト用 stand-in。
`tests/ffi/smoke.c` でコンパイル／実行される。実 runtime のソースは変更していない。
実 integration の根拠と次のフック候補は `docs/INTEGRATION-NOTES.md` を参照。
