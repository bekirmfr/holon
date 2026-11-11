# fixtures

- `modules/js-engine/`: the JavaScript engine a `js` node runs, its source and `build.sh`, and the
  `js-engine.wasm` that script builds. The wasm is committed, and `holon-module` compiles it in
  (`JS_ENGINE`, `crates/holon-module/src/lib.rs`), so every build carries it; no CI job rebuilds
  it, so a change to the engine is built with `build.sh` and committed with its wasm.
- The rest (`*.bin`, `*.json`) are hand-test artifacts from building the server, kept for
  reference: unit preimages written by `holon serve --emit-test-*` (`crates/holon/src/fixtures.rs`;
  only `--emit-test-unit` is left) and request bodies used with curl before the server had
  integration tests, some for routes since removed (`/registry`, `/cache`, `/fire`). Nothing reads
  them automatically.
