/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Where holon-server's control plane is (default http://127.0.0.1:7420). */
  readonly VITE_HOLON_SERVER?: string;
}
