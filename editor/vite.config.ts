import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Minimal config. The wasm package (built into ./wasm via `wasm-pack build --target web`) is
// imported by relative path and loads itself via `init()` + fetch — no wasm plugin needed for dev.
export default defineConfig({
  plugins: [react()],
});
