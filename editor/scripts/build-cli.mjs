// Builds the `holon` command line (cli/holon.ts) into one plain JavaScript file,
// dist-cli/holon.mjs, with the core's WebAssembly beside it: it then needs only Node, no
// TypeScript runner and no dev dependencies. `npm run build:cli`; then `npm install -g .`
// puts `holon` on the PATH (package.json's "bin").

import { build } from "esbuild";
import { copyFileSync, existsSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const wasm = join(root, "wasm", "holon_wasm_bg.wasm");
if (!existsSync(wasm)) {
  console.error("the core isn't built: wasm-pack build crates/holon-wasm --target web --out-dir ../../editor/wasm (from the repository root)");
  process.exit(1);
}
const out = join(root, "dist-cli");
mkdirSync(out, { recursive: true });
await build({
  entryPoints: [join(root, "cli", "holon.ts")],
  outfile: join(out, "holon.mjs"),
  bundle: true,
  platform: "node",
  format: "esm",
  target: "node18",
  banner: { js: "#!/usr/bin/env node" },
  // Vite's build-time settings, which the shared modules read; none apply here.
  define: { "import.meta.env": "{}" },
  logLevel: "warning",
});
copyFileSync(wasm, join(out, "holon_wasm_bg.wasm"));
console.log("built dist-cli/holon.mjs");
