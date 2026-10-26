// The admin page's "browse…" for a folder to bind (holon-server's pick.rs). A browser can't give
// a page a folder's path, and the folder is on the server's machine, so the server shows the
// choice: when the editor runs on that machine, the OS's own folder dialog opens there; otherwise
// (or "browse here instead") this lists the server's folders to click through.

import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react";
import { browseFolders, pickFolder, type FolderListing } from "./server";

const button: CSSProperties = { font: "inherit", fontSize: 12, padding: "4px 12px", borderRadius: 3, cursor: "pointer" };
const quiet: CSSProperties = { ...button, color: "#9fb4d4", background: "transparent", border: "1px solid #1b2a45" };
const chip: CSSProperties = { ...quiet, fontSize: 11, padding: "2px 8px" };

export function FolderPicker({ start, onChoose, onClose }: { start: string; onChoose: (path: string) => void; onClose: () => void }) {
  const [listing, setListing] = useState<FolderListing | null>(null);
  const [typed, setTyped] = useState("");
  const [error, setError] = useState("");
  // The OS's dialog is open on the server's machine, waiting for an answer.
  const [waiting, setWaiting] = useState(false);

  // Only the latest listing asked for is shown, and it fills the path field only if nobody has
  // typed there since: the first one (the home folder) can arrive after someone started typing.
  const asked = useRef(0);
  const typedSince = useRef(false);

  const go = useCallback(async (path?: string) => {
    const mine = ++asked.current;
    typedSince.current = false;
    try {
      const l = await browseFolders(path);
      if (mine !== asked.current) return null;
      setListing(l);
      if (!typedSince.current) setTyped(l.path);
      setError("");
      return l;
    } catch (e) {
      if (mine === asked.current) setError(String(e instanceof Error ? e.message : e));
      return null;
    }
  }, []);

  useEffect(() => {
    let gone = false;
    (async () => {
      // Where the path field points, else the home folder; unless someone went elsewhere meanwhile.
      const from = start.trim();
      let l = from ? await go(from) : null;
      if (!l && asked.current === (from ? 1 : 0)) l = await go();
      if (gone || !l?.native) return;
      setWaiting(true);
      try {
        const { path } = await pickFolder(start.trim() || l.path);
        if (gone) return;
        if (path) onChoose(path);
        else onClose();
      } catch (e) {
        // The dialog couldn't be shown, or nobody answered it: browse here.
        if (!gone) setError(String(e instanceof Error ? e.message : e));
      } finally {
        if (!gone) setWaiting(false);
      }
    })();
    return () => {
      gone = true;
    };
    // Once, as it opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="choose a folder"
      onMouseDown={(e) => e.target === e.currentTarget && onClose()}
      style={{ position: "fixed", inset: 0, background: "rgba(5, 10, 20, 0.6)", display: "flex", alignItems: "center", justifyContent: "center", zIndex: 1000 }}
    >
      <div
        style={{
          background: "#0e1a2f",
          border: "1px solid #1b2a45",
          borderRadius: 6,
          padding: 16,
          width: 560,
          maxWidth: "calc(100vw - 32px)",
          color: "#e6eefb",
          fontSize: 13,
          display: "flex",
          flexDirection: "column",
          gap: 10,
        }}
      >
        <div style={{ color: "#cfe3ff" }}>Choose a folder on the server&apos;s machine</div>
        {waiting ? (
          <>
            <div style={{ color: "#9fb4d4", lineHeight: 1.45 }}>A folder dialog opened on this machine: choose the folder there.</div>
            <div style={{ display: "flex", justifyContent: "flex-end" }}>
              <button type="button" style={quiet} onClick={() => setWaiting(false)}>
                browse here instead
              </button>
            </div>
          </>
        ) : (
          <>
            <form
              style={{ display: "flex", gap: 6 }}
              onSubmit={(e) => {
                e.preventDefault();
                e.stopPropagation();
                go(typed);
              }}
            >
              <input
                aria-label="folder to show"
                value={typed}
                onChange={(e) => {
                  typedSince.current = true;
                  setTyped(e.target.value);
                }}
                style={{ flex: 1, font: "inherit", fontSize: 12, color: "#e6eefb", background: "#0b1527", border: "1px solid #1b2a45", borderRadius: 3, padding: "4px 6px" }}
              />
              <button type="submit" style={chip}>
                go
              </button>
              <button type="button" style={chip} disabled={!listing?.parent} title="the folder above" onClick={() => listing?.parent && go(listing.parent)}>
                up
              </button>
            </form>
            {listing && (
              <div style={{ display: "flex", flexWrap: "wrap", gap: 4 }}>
                {listing.roots.map((r) => (
                  <button type="button" key={r.path} style={chip} onClick={() => go(r.path)}>
                    {r.name}
                  </button>
                ))}
              </div>
            )}
            <div aria-label="folders" style={{ maxHeight: "45vh", minHeight: 120, overflowY: "auto", border: "1px solid #1b2a45", borderRadius: 3, background: "#0b1527" }}>
              {listing?.folders.length === 0 && <div style={{ color: "#7e8ea8", fontSize: 12, padding: "6px 8px" }}>no folders in this one</div>}
              {listing?.folders.map((f) => (
                <button
                  type="button"
                  key={f.path}
                  title={f.path}
                  onClick={() => go(f.path)}
                  style={{ display: "block", width: "100%", textAlign: "left", font: "inherit", fontSize: 12, color: "#e6eefb", background: "transparent", border: "none", padding: "4px 8px", cursor: "pointer" }}
                >
                  ▸ {f.name}
                </button>
              ))}
              {listing?.more && <div style={{ color: "#7e8ea8", fontSize: 11, padding: "4px 8px" }}>more folders than shown: type the path</div>}
            </div>
            {error && <div style={{ color: "#f08a8a", fontSize: 12, overflowWrap: "anywhere" }}>{error}</div>}
            <div style={{ display: "flex", justifyContent: "flex-end", gap: 8 }}>
              <button type="button" style={quiet} onClick={onClose}>
                Cancel
              </button>
              <button type="button" style={{ ...button, color: "#0e1a2f", background: "#6fd3e0", border: "none" }} disabled={!listing} onClick={() => listing && onChoose(listing.path)}>
                Choose this folder
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
