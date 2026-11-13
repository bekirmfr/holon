import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
// The editor's faces, served with it rather than by a font service: nothing loads from elsewhere,
// so the page works offline, tells no one who opened it, and holds to its policy (editor.rs).
// Each is under the SIL Open Font License 1.1 (its package's LICENSE; the files name it too):
// Inter, JetBrains Mono, and IBM Plex Mono (the Terminal look's), in the weights styles.css uses.
import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "@fontsource/inter/700.css";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/500.css";
import "@fontsource/jetbrains-mono/600.css";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import "@fontsource/ibm-plex-mono/600.css";
import "./styles.css";
import { applyTheme, applyUi, storedTheme, storedUi } from "./look";

// The look this browser last had, before anything paints; the account's, once it is read (App).
applyTheme(storedTheme());
applyUi(storedUi());

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
