#!/usr/bin/env bash
# Builds Holon's JavaScript engine module and puts it beside this script as js-engine.wasm, which
# is committed so a checkout needs no toolchain to run a `js` node (docs/L4s/js-node-L4.md §10).
#
# Needs the wasm32 target: `rustup target add wasm32-unknown-unknown`. The build is reproducible
# given the same toolchain and lockfile (Cargo.lock is committed). No CI job rebuilds it: a change
# to the engine is built here and the new js-engine.wasm committed with it.
set -euo pipefail
cd "$(dirname "$0")"
cargo build --release
cp target/wasm32-unknown-unknown/release/holon_js_engine.wasm js-engine.wasm
echo "js-engine.wasm: $(wc -c < js-engine.wasm) bytes, sha256 $(sha256sum js-engine.wasm | cut -d' ' -f1)"
