#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
export LLB_HEADER_DIR="$PWD/target/ffi-include"
cargo build -p light-bridge-ffi --locked --offline
cc -std=c11 -Wall -Wextra -Werror -I "$LLB_HEADER_DIR" tests/ffi/smoke.c \
 -L target/debug -llight_bridge_ffi -Wl,-rpath,"$PWD/target/debug" -o target/debug/ffi-smoke
target/debug/ffi-smoke
cc -std=c11 -Wall -Wextra -Werror -I "$LLB_HEADER_DIR" tests/ffi/smoke.c \
 target/debug/liblight_bridge_ffi.a -lpthread -ldl -lm -o target/debug/ffi-static-smoke
target/debug/ffi-static-smoke
cc -std=c11 -Wall -Wextra -Werror -I "$LLB_HEADER_DIR" tests/ffi/network-smoke.c \
 -L target/debug -llight_bridge_ffi -lpthread -Wl,-rpath,"$PWD/target/debug" -o target/debug/ffi-network-smoke
target/debug/ffi-network-smoke
cc -std=c11 -Wall -Wextra -Werror -I "$LLB_HEADER_DIR" tests/ffi/network-smoke.c \
 target/debug/liblight_bridge_ffi.a -lpthread -ldl -lm -o target/debug/ffi-static-network-smoke
target/debug/ffi-static-network-smoke
cc -std=c11 -Wall -Wextra -Werror -I "$LLB_HEADER_DIR" tests/ffi/clock-smoke.c \
 -L target/debug -llight_bridge_ffi -Wl,-rpath,"$PWD/target/debug" -o target/debug/ffi-clock-smoke
target/debug/ffi-clock-smoke
cc -std=c11 -Wall -Wextra -Werror -I "$LLB_HEADER_DIR" tests/ffi/clock-smoke.c \
 target/debug/liblight_bridge_ffi.a -lpthread -ldl -lm -o target/debug/ffi-static-clock-smoke
target/debug/ffi-static-clock-smoke
cc -std=c11 -Wall -Wextra -Werror -I "$LLB_HEADER_DIR" tests/ffi/auto-smoke.c \
 -L target/debug -llight_bridge_ffi -Wl,-rpath,"$PWD/target/debug" -o target/debug/ffi-auto-smoke
target/debug/ffi-auto-smoke
cc -std=c11 -Wall -Wextra -Werror -I "$LLB_HEADER_DIR" tests/ffi/auto-smoke.c \
 target/debug/liblight_bridge_ffi.a -lpthread -ldl -lm -o target/debug/ffi-static-auto-smoke
target/debug/ffi-static-auto-smoke
