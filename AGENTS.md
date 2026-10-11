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
```

Dependencies must be cached before offline commands; `cargo fetch --locked` prepares them. An alternate writable `CARGO_HOME` may be used locally when required by environment permissions.

- Include meaningful failure/cleanup coverage for lifecycle, ownership, protocol, or FFI changes. Rust tests check the generated header snapshot; C smoke tests verify the actual ABI.
- Report native versus arm64 validation, mock versus hardware behavior, runtime linking versus source inspection, and remaining blockers separately.
- Do not include build outputs, credentials, or unrelated files in commits. Follow the user's requested commit identity; do not change global Git settings.
