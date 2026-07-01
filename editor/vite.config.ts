import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Minimal config. The wasm package (built into ./wasm via `wasm-pack build --target web`) is
// imported by relative path and loads itself via `init()` + fetch — no wasm plugin needed for dev.
//
// `defineConfig` is imported from "vitest/config" rather than "vite" — it re-exports Vite's own
// with the `test` field's types included, so this stays ONE config (Vitest reuses it directly),
// not a second, parallel vitest.config.ts that could drift from this one.
//
// `environment: "jsdom"`, not "node": this first test batch is pure logic (no component
// rendering), but the functions under test live in App.tsx, which transitively imports
// `@xyflow/react` and its CSS at module scope — importing any named export from that file still
// evaluates the whole module. "node" would likely be fine (CSS imports are stripped to no-ops by
// Vitest by default; the hooks/components are only ever defined, never invoked, since nothing
// renders `<App />`) but "likely fine" isn't the same as verified, and this couldn't be run here
// to confirm either way. jsdom removes the question entirely for a small, one-time cost.
export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
  },
});