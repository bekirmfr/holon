// The header's "access" button: the tokens this browser sends holon-server when it was started
// with them (crates/holon-server/src/access.rs). Kept in this browser only (server.ts), never
// shown again once saved, and only needed when the server asks for them.

import { useState, type CSSProperties } from "react";
import { createPortal } from "react-dom";
import { getAccessTokens, setAccessTokens } from "./server";

const field: CSSProperties = {
  font: "inherit",
  fontSize: 12,
  padding: "4px 8px",
  color: "#e6eefb",
  background: "#0e1a2f",
  border: "1px solid #1b2a45",
  borderRadius: 3,
  width: 300,
};

export function AccessButton({ style }: { style: CSSProperties }) {
  const [open, setOpen] = useState(false);
  const [admin, setAdmin] = useState("");
  const [secrets, setSecrets] = useState("");
  const [has, setHas] = useState(() => {
    const t = getAccessTokens();
    return { admin: !!t.admin, secrets: !!t.secrets };
  });
  const save = (tokens: { admin: string; secrets: string }) => {
    setAccessTokens(tokens);
    setHas({ admin: !!tokens.admin, secrets: !!tokens.secrets });
    setAdmin("");
    setSecrets("");
    setOpen(false);
  };
  const current = getAccessTokens();
  return (
    <>
      <button style={style} onClick={() => setOpen(true)} title="the tokens this browser sends holon-server, when it was started with them">
        access{has.admin || has.secrets ? " ●" : ""}
      </button>
      {open &&
        createPortal(
          <div
            role="dialog"
            aria-modal="true"
            aria-label="server access"
            onMouseDown={(e) => e.target === e.currentTarget && setOpen(false)}
            onKeyDown={(e) => e.key === "Escape" && setOpen(false)}
            style={{ position: "fixed", inset: 0, background: "rgba(5, 10, 20, 0.6)", display: "flex", alignItems: "center", justifyContent: "center", zIndex: 1000 }}
          >
            <div style={{ background: "#0b1527", border: "1px solid #1b2a45", borderRadius: 6, padding: 16, display: "grid", gap: 10, color: "#9fb4d4", fontSize: 12, maxWidth: "calc(100vw - 32px)" }}>
              <div style={{ color: "#e6eefb", fontSize: 13 }}>server access</div>
              <div style={{ maxWidth: 360, lineHeight: 1.45 }}>
                Only needed when holon-server was started with HOLON_ADMIN_TOKEN (and HOLON_SECRETS_TOKEN). Kept in this browser,
                sent with each request, never shown again.
              </div>
              <label style={{ display: "grid", gap: 4 }}>
                admin token {has.admin && <span style={{ color: "#9ad29f" }}>set</span>}
                <input type="password" autoComplete="off" value={admin} placeholder={has.admin ? "replace…" : "token"} onChange={(e) => setAdmin(e.target.value)} style={field} />
              </label>
              <label style={{ display: "grid", gap: 4 }}>
                secrets token (writing secrets) {has.secrets && <span style={{ color: "#9ad29f" }}>set</span>}
                <input type="password" autoComplete="off" value={secrets} placeholder={has.secrets ? "replace…" : "optional"} onChange={(e) => setSecrets(e.target.value)} style={field} />
              </label>
              <span style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
                <button style={style} onClick={() => save({ admin: "", secrets: "" })} title="forget both tokens in this browser">
                  clear
                </button>
                <button style={style} onClick={() => setOpen(false)}>
                  cancel
                </button>
                <button
                  style={{ ...style, color: "#0e1a2f", background: "#6fd3e0" }}
                  onClick={() => save({ admin: admin.trim() || current.admin, secrets: secrets.trim() || current.secrets })}
                >
                  save
                </button>
              </span>
            </div>
          </div>,
          document.body,
        )}
    </>
  );
}
