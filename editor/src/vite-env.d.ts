/// <reference types="vite/client" />

/** The holon release this editor is built with (vite.config.ts). */
declare const __HOLON_RELEASE__: string;

interface ImportMetaEnv {
  /** Where holon-server's control plane is (default http://127.0.0.1:7420). */
  readonly VITE_HOLON_SERVER?: string;
}
