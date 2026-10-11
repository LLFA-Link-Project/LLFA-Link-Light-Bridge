# wivrn-server adapter (Mock)

`mock_adapter.h` は runtime release 済みの mock stop 呼出し のテスト用 stand-in。
`tests/ffi/smoke.c` でコンパイル／実行される。実 runtime のソースは変更していない。
実 integration の根拠と次のフック候補は `docs/INTEGRATION-NOTES.md` を参照。
