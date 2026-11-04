# Holon editor

The web editor: where a person draws or writes a flow, deploys it after seeing what it may do, and
reads its runs back node by node. A Vite, React and TypeScript app that runs the **same** core and
language as the server, compiled to wasm (`crates/holon-wasm`), and talks to a Holon server over its
REST API. The server serves the built editor at `/`; this folder is for working on it.

How it is put together, what each part does and what the tests hold:
[docs/engineering/components/editor.md](../docs/engineering/components/editor.md). Using it:
[docs/manual/guides/editor.md](../docs/manual/guides/editor.md).

## Prerequisites

- Rust with the `wasm32-unknown-unknown` target, and `wasm-pack` (`wasm-pack build` needs it;
  without it, `cargo build -p holon-wasm --target wasm32-unknown-unknown --release` and a
  `wasm-bindgen` of the version in `Cargo.lock` do the same).
- Node.js (a recent LTS) and npm.

## Run

From the repository root:

```sh
# 1. The wasm bridge, built into ./editor/wasm (not in git). Again whenever holon-core,
#    holon-author, holon-module or holon-wasm change.
wasm-pack build crates/holon-wasm --target web --out-dir ../../editor/wasm --no-opt

# 2. A server for it to talk to (prints the setup code for a new one's first admin).
cargo run -p holon -- serve

# 3. The editor, against that server (http://127.0.0.1:7420; VITE_HOLON_SERVER names another).
cd editor
npm install
npm run dev
```

Open the URL Vite prints and sign in.

## Test

In `editor/`:

- `npx tsc --noEmit -p .` and `npm test` (vitest): the editor's own tests, and `samples.test.ts`,
  which builds every sample through the real core.
- `npm run e2e` (Playwright): the real editor in a browser against real servers it starts itself
  (`e2e/server.mjs`), every sample opened and deployed.
- `npm run build`: the production build, which `cargo build -p holon --features embed-editor`
  embeds in the server.
