import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
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
