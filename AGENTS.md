# Repository guidance

## Scope and sources

- Treat `LLFA-Link-Light-Bridge-IMPLEMENTATION-INSTRUCTIONS-v0.3.1.md` as the implementation specification. Follow the user's current instructions when they refine it.
- Read `README.md`, `docs/DECISIONS.md`, and `docs/PHASE-0-1-REPORT.md` before changing architecture or integration boundaries.
- Record design deviations in `docs/DECISIONS.md`. Keep protocol, ABI, integration notes, and validation reports consistent with implemented behavior.
- Inspect actual runtime sources before naming integration hooks. Cite the checkout commit and source locations. Do not infer hardware success from mocks or source inspection.

## Architecture and safety boundaries

- Keep `light-bridge-hmd` in the Monado process and `light-bridge-host` in the WiVRn Server process. Headless tools are test harnesses, not the main runtime.
- Separate session logic, protocol codecs, USB inventory/planning/backend, and FFI. Keep high-frequency pose delivery independent of slow control and USB operations; bound queues and support cancellation/join.
- Export only an explicitly selected, allowlisted Tracker. Always deny arpara HMD `2c30:1050`. Reject interface-only requests instead of expanding them to WholeDevice.
- Track USB resource ownership by session. Require runtime driver release before Host detach, then HMD unexport. Never detach unrelated ports or run arbitrary privileged shell commands from the libraries.
- Preserve pose device/origin/frame/clock identity and timestamp semantics. Keep Tracker reference pose distinct from a calibrated head pose; do not claim IMU/Lighthouse fusion without implementing and verifying it.
- BSB support is outside the initial implementation. Preserve extension types and explicit Unsupported stubs without claiming capture support.

## Licensing

- Project code uses `LGPL-3.0-or-later`. Keep Cargo package metadata consistent by inheriting `workspace.package.license`.
- Keep `LICENSE` (GNU LGPL v3) and `COPYING` (GNU GPL v3, incorporated by LGPL v3) intact. Dependencies retain their own licenses.

## Build and generated files

- Use Rust/Cargo for required build-time generation. Do not introduce a mandatory Python or standalone generator step.
- `crates/light-bridge-ffi/build.rs` invokes the pinned cbindgen build dependency. Normal builds generate `OUT_DIR/light_bridge.h` without modifying sources.
- Never edit `crates/light-bridge-ffi/light_bridge.h` by hand. After intentional ABI changes, refresh its snapshot with:

  ```sh
  LLB_HEADER_DIR="$PWD/crates/light-bridge-ffi" cargo build -p light-bridge-ffi --locked
  ```

- C consumers can request a generated include directory using an absolute `LLB_HEADER_DIR`. `tests/ffi/run.sh` exercises the generated header against shared and static libraries.
- Keep `Cargo.lock` checked in. Preserve the declared MSRV when resolving dependencies, and distinguish declared compatibility from an actual toolchain test.

## Validation and reporting

After code or ABI changes, run checks appropriate to the change:

```sh
cargo build --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo test --workspace --locked --offline
bash tests/ffi/run.sh
cargo run -p network-session --locked --offline -- --processes
```

Dependencies must be cached before offline commands; `cargo fetch --locked` prepares them. An alternate writable `CARGO_HOME` may be used locally when required by environment permissions.

- Include meaningful failure/cleanup coverage for lifecycle, ownership, protocol, or FFI changes. Rust tests check the generated header snapshot; C smoke tests verify the actual ABI.
- Report native versus arm64 validation, mock versus hardware behavior, runtime linking versus source inspection, and remaining blockers separately.
- Do not include build outputs, credentials, or unrelated files in commits. Follow the user's requested commit identity; do not change global Git settings.

## Phase 2 mock transport

- `network-session` is a runtime adapter test harness. Its subprocess roles are not production daemons.
- Keep TCP control and pose on separate bounded lanes. Bind/accept/connect and UDS notifications run on an adapter control worker, never a pose query thread.
- The TCP preface matches host/correlation/lane; it does not authenticate peers. Generic network C APIs select the explicitly configured kernel backend or default Mock. `_mock` APIs reject kernel configuration. Neither API authenticates remote peers; restrict experiments to an access-controlled network.
- Remote Mock USB accepts only the exact synthetic identity after UsbExportReady. Never treat remote mock metadata as a real inventory/allowlist/export.
- Keep UDS directories private and remove only paths created by the owning adapter. After a notification read fails or times out, close the stream; do not restart framing in the middle of a consumed frame.
- Report runtime stand-in tests separately from building/linking upstream Monado and WiVRn.

## Phase 3 kernel backend

- Use a dedicated cancellable USB worker; never execute helper RPC or CLI operations in control poll or pose queries.
- Privileged operations go only through the root-owned helper with SO_PEERCRED and exact policy. Keep CLI executable paths and argument forms fixed; never introduce a shell RPC.
- Persist intent before OS mutation and retain uncertain journals. Never automatically adopt stale resources or infer an owned port from a guessed number. Runtime release, verified Host detach, then HMD unbind remain mandatory.
- Real backend destroy cancels/joins workers; it does not blindly clean up OS resources. Document residual-resource inspection and incomplete cleanup.
- Bridge frame/preface version is 3; C ABI and UDS notification version remain 1. Keep identity offer encoding and import policy consistent.
- Keep `docs/HANDOFF-MONADO.md`, `docs/HANDOFF-WIVRN.md`, and `docs/PHASE-3-REPORT.md` current. Mock/helper engine tests do not establish Phase 3 hardware acceptance.

## Runtime adapter contract

- Notification UDS defaults to `/tmp/llfa-llb.sock`; both WiVRn Client and Monado adapter use `WIVRN_LIGHT_BRIDGE_IPC` to override it. Path configuration alone does not enable Bridge; build and runtime both require explicit enablement.
- The default is in shared `/tmp`: use same-UID peer credentials and socket mode 0600, verify existing path ownership, and never blindly unlink. Custom paths must match on both adapters.
- WiVRn reconnect after a Bridge disconnect uses a new Server child process and fresh IDs. Release the Lighthouse driver before permitting USB/IP detach; audit old resources before preparing a new child. Do not reuse old tracking devices or start Rust workers before fork.

## Phase 4 driver timing

- Driver-reported history timestamps are the primary reference; do not require additional raw sensor timestamps. Keep original values/semantics and separate mapped/receive times.
- Read `docs/PHASE-4-TIMING-SPEC.md` before changing clocks. CLOCK_MONOTONIC domain equality must be verified in each runtime environment.
- ClockSync uses bounded control queues and separate request IDs; it must not advance lifecycle counters or keep heartbeat alive. Preserve uncertainty and invalidate unsynchronized, expired, discontinuous or wrong-domain mappings.
- New timing APIs do not establish calibration/fusion or hardware acceptance. Update both runtime handoffs and report real versus synthetic source/clock/network/USB separately.

## Development helper deployment

- Installer work is deferred. Follow docs/TEMPORARY-HELPER.md and docs/HELPER-DEPLOYMENT-CONTRACT.md.
- Helper RPC is v2; Bridge TCP v3, notification v1 and C ABI v1 remain unchanged. Bump the dedicated helper compatibility ID on privileged contract changes.
- Check identity and exact policy before mutation on the same connection. A mismatch or unknown/stopped installation must not silently use a temporary helper or Mock.
- Temporary startup requires explicit administrator approval of the development binary, protected /run staging and fixed transient systemd arguments. Never enable persistent units or add broad sudoers rules.
- Normal temporary shutdown requires runtime acknowledgement and an empty ownership journal. Retain uncertain resources; no automatic journal removal or immediate shutdown on runtime exit.

## Connection-driven automatic setup (design, not implemented)

- Follow docs/AUTOMATIC-BOOTSTRAP-DESIGN.md for the next implementation. Default opt-in auto mode must derive/exchange addresses from the WiVRn connection and resolve exact Tracker identity without manual IP/policy/busid/timing-ID entry.
- Bootstrap precedes Lighthouse and stream readiness. Create the Monado listener before resolving policy; exchange Prepare/Offer/Commit and require both sides prepared before USB mutation.
- Automatically select one supported, authorized candidate; require selection for ambiguity and always deny the HMD. Keep root authorization limits separate from immutable per-session exact policy.
- Helper prepares allowed modules, managed usbipd and scoped network access. Preserve initial administrator authorization, helper compatibility rejection, dedicated workers and resource journals. Installer is deferred.
- Treat the bootstrap protocol, helper operations and UI as new work, not existing hooks. Do not loosen current exact-policy checks to implement dynamic addresses.
