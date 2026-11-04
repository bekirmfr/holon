# holon-wasm

The bridge the editor runs: core, the authoring crate and the module engine compiled to
wasm32 and exposed to JavaScript, so the browser hashes, evaluates, checks and lowers a flow with
the very code the server and the command line use. The evidence is in the
[JOURNAL](../../JOURNAL.md) (P4.2a–c, 2026-06-29; the catalog bridge, 2026-06-30),
[primitive-catalog-bridge-L4](../../L4s/primitive-catalog-bridge-L4.md) and
[wasm-module-L4 §5](../../L4s/wasm-module-L4.md).

## 1. What it is

`crates/holon-wasm/src/lib.rs`, one file, a `cdylib` (and an `rlib`, for its native tests). It
depends on [holon-core](holon-core.md), [holon-author](holon-author.md) and
[holon-module](holon-module.md), and never on `holon-host`: the host's file store and IO have no
place in a browser, and the missing dependency keeps them out. `serde`, `serde-wasm-bindgen` and
`serde_json` live here, not in core. The generated package is `editor/wasm/` (`holon_wasm.js`,
`holon_wasm_bg.wasm`, their `.d.ts`), ignored by git and rebuilt from the crate; the editor reaches
it through `editor/src/holon.ts`, which gives types to what the generated `.d.ts` leaves as
`any`.

## 2. How it works

### Building it

CI and releases build it with wasm-pack 0.15.0:

```
wasm-pack build crates/holon-wasm --target web --out-dir ../../editor/wasm --no-opt
```

(`.github/workflows/ci.yml`, jobs `editor` and `e2e`; `.github/build-setup.yml` for releases,
which build the editor into the server).
`--no-opt` skips `wasm-opt`, which fails to parse the core on the macOS arm64 runner; the wasm
tested is the wasm shipped. Without wasm-pack, the same package is two steps:

```
cargo build -p holon-wasm --target wasm32-unknown-unknown --release
wasm-bindgen --target web --out-dir editor/wasm target/wasm32-unknown-unknown/release/holon_wasm.wasm
```

The `wasm-bindgen` command must be the version `Cargo.lock` pins for the crate (0.2.126 now);
the toolchain is pinned with its wasm32 target in `rust-toolchain.toml`. `npm run e2e` rebuilds the
package when it is older than `Cargo.lock` or any of the four crates (`editor/e2e/server.mjs`,
`5e6030c`), with wasm-pack.

### What crosses

- **Units** cross as `WUnit` (holon-author's `wire.rs`, the form flow documents hold), or as
  canonical preimage bytes. `build_preimage` encodes a `WUnit` through core's one encoder,
  `load_unit` decodes, `hash_unit_hex` and `canonicalize_preimage` hash and re-encode. Hashes cross
  as lowercase hex, usable as JS map keys.
- **Values** cross as `WValue`, adjacently tagged (`{ type, value }`), by `serde-wasm-bindgen`, not
  JSON: a run can make `NaN` or `Infinity`, which a JS number holds and JSON doesn't. A `Record`
  crosses as `[key, value]` pairs in key order, since serde-wasm-bindgen would make a map a JS
  `Map`.
- **Errors** cross as `JsError` with a message; validation and encapsulation errors cross
  structured (`WValidationError`, `WEncapError`), each with core's own `Display` as its `message`
  and the child and edge indices the editor needs to mark a node.

### The `Evaluator`

A stateful workspace: a `MapResolver` the editor fills with `insert` (a preimage) or
`insert_module` (the js engine under a budget, rebuilt from the bytes the bundle carries rather
than a preimage), then asks of by hash. `eval` evaluates a preimage against it through
`holon_module::WithEngine`, so a module on the canvas runs for real under its budget, and answers
every declared output. `capabilities`, `capability_names`, `deployed_capability_names` and
`effects_of` are core's closure walks. The canvas operations (`group_of`, `project`,
`propose_encapsulation`, `freeze_encapsulation`, `canonical_child_order`, `expand_composite`,
`inner_canvas`, `freeze_inner`, `flow_as_node`) and the flow's derivation (`derive_server`,
`secret_marks`, `flow_unit`, `print_holon`) are holon-author's, their units stored in the same
workspace. The editor makes one `Evaluator` for the page (`App.tsx`).

### The free functions

What the editor shows and checks, read from core rather than copied into TypeScript:
`primitive_catalog` (every genesis interface, one call), `primitive_docs`, `variadic_primitives`,
`passes_extra_primitives`, `compatible` (the canvas's connection check), `choice_next` (the
replay walks a run by core's own choice rule), `manifest_items`, `manifest_trigger`,
`permission_kind`, `reads_from_outside`, `describe_item*`. The language: `compile_holon`,
`holon_mistakes`, `holon_hover`, `holon_complete`, `holon_format`, `holon_highlight`,
`holon_runnable_by_hand`, with positions converted to UTF-16 offsets (`Offsets`), as the editor
counts them. The samples: `samples`, `build_sample`. Templates and routes: `template_*`,
`route_path`, `canvas_components`, `find_routes`.

## 3. Why it is this way

- **One evaluator, one encoder.** A TypeScript encoder or evaluator would be a second one, free to
  drift; a hash the editor computes would then not be the server's (JOURNAL P4.2c). So the editor
  builds bytes through `build_preimage` and the palette from `primitive_catalog`
  (primitive-catalog-bridge-L4: one whole-catalog call, reusing `WUnit::from_core`).
- **serde stays out of core.** Core keeps its four dependencies and its purity argument; the mirror
  types (`WValue`, `WCapability`, `WKind`, the error mirrors) live here.
- **A stateful workspace**, so a live editor needn't send the whole closure on every keystroke.
- **The real engine in the browser** (wasm-module-L4 §5): design mode runs a module under the
  budget it will run under on the server, so a check in the editor is a check. `wasmi` was chosen
  because it builds for wasm32; `wasmtime` would have meant a second engine here.
- **Typed in one place.** The generated `.d.ts` types `JsValue` crossings as `any`; `holon.ts`
  mirrors the serde types as discriminated unions, so `tsc` checks them.

## 4. Reach and limits

- **Everything is in one bundle.** The module engine and the js engine's bytes
  (`holon_module::JS_ENGINE`, 3.2 MB, [js-node-L4 §4](../../L4s/js-node-L4.md)) are linked in
  whether a flow places a js node or not: `holon-author` uses them to lower text, with no feature
  to leave them out. holon-module's own header says the bundle carries an engine only where it
  places a module; that is not what the build does.
- **Unoptimised.** The package is built `--no-opt`. Its size is not measured or tracked anywhere.
- **The workspace only grows.** The `Evaluator` has no way to remove a unit; units inserted over a
  long session stay until the page is reloaded. Not measured.
- **Shapes aren't kept.** `MapResolver` keeps no `Shapes`, so each `eval` works a composite's shape
  out again (IP step 1b reaches the server, not the editor;
  [interpreter-path-L4 §5](../../L4s/interpreter-path-L4.md)). Every `eval` also decodes its
  preimage and wraps the resolver afresh.
- **Messages are Rust's.** Most errors are `format!("decode: {e:?}")` of a core error, so a person
  can meet a Rust enum's `Debug` form.
- **A `WType` is untrusted input** (`compatible`, `build_preimage` take one from JS), so decoding it
  is bounded by `MAX_TYPE_DEPTH`; past it, an error, not a stack overflow.
- **No wasm-pack, no automatic rebuild.** The e2e script's rebuild calls wasm-pack; on a machine
  without it, build by hand (above) before `npm run e2e`.
- The crate header still says it depends on core only; it depends on holon-author and holon-module
  too (`Cargo.toml`).

## 5. What the tests hold

The checks are written once as `check_*` functions and registered twice: `native_tests` (15,
`cargo test -p holon-wasm`) and `wasm_tests` (23, `wasm-pack test --node crates/holon-wasm`, CI job
`wasm-parity`).

- `check_hash_parity`: `hash_unit_hex` over a preimage equals `hash_unit`, and
  `canonicalize_preimage` returns the bytes it was given.
- `check_eval_parity`: `add`, `multiply`, `inf - inf` is `NaN`, a composite through the resolver.
- `check_fraction_parity`: 2,000 rounds of `add`, `subtract`, `multiply`, `divide`, `sum` and
  `compare` on seeded operands, each result bit for bit what Rust computes, and the hash of all of
  them equal to a pinned digest, the same under both targets.
- `check_signature_parity`: `hmac_sha256`, `sha256` and `base64` against RFC test vectors.
- `check_wvalue_roundtrip`, `check_wunit_roundtrip`, `check_wtype_roundtrip`; `check_wtype_depth_bound`
  (wasm only: its failure path builds a `JsError`, which has no native implementation).
- `check_primitive_catalog`, `check_variadic_primitives`, `check_compatible`, `check_project`,
  `check_encap_error_structure`, `check_validation_error_structure`.
- Under wasm only, across the real JS boundary: `project_crosses_js_boundary` (the bridged composite
  encodes to the bytes the JS-free path gives, and evaluates), `deployed_capabilities_cross_js_boundary`,
  `get_unit_crosses_js_boundary`, and the rest of `wasm_tests`.
- Native only: `text_offsets_are_utf16`, `node_spans_are_utf16_ranges_that_nest`.
- Above it, `editor/src/*.test.ts` (vitest) run against a freshly built package: `samples.test.ts`
  builds every sample through it and derives its routes through the real core; `npm run e2e`
  deploys each through the editor and server.

**Gaps.** `check_hash_parity` compares two hashes computed in the same target, so it holds the
bridge to core, not wasm to native: no unit hash is pinned as a literal in either suite. That a
wasm-built unit hashes as the server's is held in practice by the end-to-end tests, which deploy
units the editor built, not by a parity test. The fraction and signature checks are the ones that
pin values across targets. `Evaluator::eval` with a module (the js engine in the browser) is
covered by the editor's tests and e2e, not here.

## 6. History

The git history holds little of this crate: it starts at `e49e371` (2026-10-30), after the bridge
was built. From the [JOURNAL](../../JOURNAL.md): hashing bridged and checked against native (P4.2a),
evaluation and `WValue` (P4.2b), construction and `WUnit` (P4.2c), all 2026-06-29;
`primitive_catalog` (2026-06-30); `eval` widened to every output (P4.3); keyed state capabilities
across (2026-07-02); the module engine wired in (2026-10-25). Then, by commit:

- `2c15b96`, `14c7163`, `75ea56d`: where a secret goes, said through the bridge
  (`describe_item_sending`, `secret_marks`).
- `5e6030c`: the e2e run rebuilds a stale package.
- `f06e7a1`: handler log levels through the bridge; checked against a package built with
  wasm-bindgen directly.
- `e40e7b5`: unrecorded handlers among the manifest items the editor shows (`manifest_items`).
