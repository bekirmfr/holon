// The palette: every genesis primitive, in catalog (§13a) order, grouped by category.
// Three top-level sections: FLOWS, MARKETPLACE, PALETTE.  Each section is collapsible.
// PALETTE contains sub-groups (http templates first, then every catalog category); each
// sub-group is also collapsible.  The section and sub-group open state is persisted per
// browser.  When the whole panel is closed it collapses to a 28 px strip (like the
// Inspector) — the parent passes `open`/`onOpen`.

import { useState } from "react";
import type { WUnit } from "./holon";
import { outputColor } from "./theme";
import { InfoIcon } from "./NodeDocs";
import { templateDoc } from "./templates";
import { MARKET_PREFIX, type Listing } from "./marketplace";

/** The MIME key the palette writes and the canvas reads on a drag-and-drop. */
export const DND_MIME = "application/holon-primitive";

/** A dragged saved flow's payload: `flow:<name>` (no primitive or template name has a colon). */
export const FLOW_PREFIX = "flow:";

// ── Catalog grouping ──────────────────────────────────────────────────────────
// Names come from GENESIS in catalog.rs (§13a).  Groups follow the same comment
// sections; some adjacent small sections are merged for a manageable count.

const CATALOG_GROUPS: readonly { name: string; items: ReadonlySet<string> }[] = [
  {
    name: "net",
    items: new Set(["http_request", "http_upload", "http_stream", "flow_call"]),
  },
  {
    name: "core",
    items: new Set([
      "const", "add", "multiply", "subtract", "divide",
      "compare", "and", "or", "not", "select",
      "cons", "head", "tail", "is_empty", "fold", "map",
    ]),
  },
  {
    name: "text",
    items: new Set(["concat", "uppercase", "lowercase", "text_length", "slice", "index_of", "split"]),
  },
  {
    name: "list",
    items: new Set(["list", "get", "append", "extend", "length", "sum", "join", "range", "pair", "some", "none"]),
  },
  {
    name: "record",
    items: new Set(["record", "field", "with_field", "without_field", "keys", "lookup", "insert", "remove", "find"]),
  },
  {
    name: "encode",
    items: new Set(["json_parse", "json_stringify", "sha256", "hmac_sha256", "hex", "base64", "secure_equals"]),
  },
  {
    name: "util",
    items: new Set(["to_text", "as_text", "is_truthy", "guard", "default", "coalesce", "equals"]),
  },
  {
    name: "drivers",
    items: new Set(["clock", "random", "ingress", "secret", "file_arrival", "await", "resume_token"]),
  },
  {
    name: "state",
    items: new Set(["store_read", "store_write",
      "collection_get", "collection_list", "collection_put", "collection_delete"]),
  },
  {
    name: "files",
    items: new Set([
      "file_read", "file_list", "file_write", "file_delete",
      "file_handle", "file_lines", "file_slice", "file_copy",
      "file_create", "file_create_copy",
    ]),
  },
  {
    name: "database",
    items: new Set([
      "db_query", "db_one", "db_exec",
      "db_find", "db_find_one", "db_insert", "db_update", "db_delete",
    ]),
  },
  {
    name: "channel",
    items: new Set(["channel_send", "channel_subscribe", "egress"]),
  },
];

function loadOpen(): Set<string> {
  try {
    const raw = localStorage.getItem("holon.palette.sections");
    if (raw) return new Set(JSON.parse(raw) as string[]);
  } catch { /* ignore */ }
  // Default: top-level sections open, sub-groups all collapsed except http.
  return new Set(["flows", "marketplace", "palette", "http"]);
}

function saveOpen(s: Set<string>) {
  try { localStorage.setItem("holon.palette.sections", JSON.stringify([...s])); } catch { /* ignore */ }
}

function nameOf(u: WUnit): string {
  return u.body.kind === "Primitive" ? u.body.name : "?";
}

// ── Small helpers ─────────────────────────────────────────────────────────────

function chevron(open: boolean) {
  return (
    <span style={{ fontSize: 9, opacity: 0.5, marginLeft: "auto", paddingLeft: 4, flexShrink: 0 }}>
      {open ? "▾" : "▸"}
    </span>
  );
}

const headStyle: React.CSSProperties = {
  display: "flex", alignItems: "center", width: "100%",
  font: "inherit", background: "transparent", border: "none",
  color: "var(--ink-dim)", cursor: "pointer", textAlign: "left",
  padding: "10px 14px 8px", fontSize: 11, letterSpacing: "0.14em",
  textTransform: "uppercase",
};

const subHeadStyle: React.CSSProperties = {
  display: "flex", alignItems: "center", width: "100%",
  font: "inherit", background: "transparent", border: "none",
  color: "var(--ink-dim)", cursor: "pointer", textAlign: "left",
  padding: "5px 14px 4px 18px", fontSize: 10, letterSpacing: "0.10em",
  textTransform: "uppercase",
};

// ── Draggable item ────────────────────────────────────────────────────────────

function PaletteItem({
  mime, swatch, name, info,
}: { mime: string; swatch: React.CSSProperties; name: string; info: string | null }) {
  return (
    <div
      className="hl-palette__item"
      draggable
      onDragStart={(e) => {
        e.dataTransfer.setData(DND_MIME, mime);
        e.dataTransfer.effectAllowed = "move";
      }}
    >
      <span className="hl-palette__swatch" style={swatch} aria-hidden />
      <span className="hl-palette__name">{name}</span>
      <InfoIcon text={info} />
    </div>
  );
}

// ── Main component ────────────────────────────────────────────────────────────

export function Palette({
  catalog,
  templates,
  flows,
  market,
  docs,
  open = true,
  onOpen,
}: {
  catalog: WUnit[];
  templates: readonly string[];
  flows: string[];
  market: { url: string; error: string; entries: { listing: Listing; adoption: number; versions: number }[] };
  docs: Map<string, string>;
  open?: boolean;
  onOpen?: (open: boolean) => void;
}) {
  const [sections, setSections] = useState<Set<string>>(loadOpen);

  const toggle = (key: string) => {
    setSections((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key); else next.add(key);
      saveOpen(next);
      return next;
    });
  };

  // All palette sub-groups (http + catalog groups)
  const paletteSubKeys = ["http", ...CATALOG_GROUPS.map((g) => g.name)];
  const allPaletteOpen = paletteSubKeys.every((k) => sections.has(k));

  const toggleAllPalette = () => {
    setSections((prev) => {
      const next = new Set(prev);
      if (allPaletteOpen) {
        paletteSubKeys.forEach((k) => next.delete(k));
      } else {
        paletteSubKeys.forEach((k) => next.add(k));
      }
      saveOpen(next);
      return next;
    });
  };

  // When closed, render a 28 px fold strip identical to the inspector's.
  if (!open) {
    return (
      <aside className="hl-palette hl-palette--closed">
        <button
          className="hl-palette__fold"
          onClick={() => onOpen?.(true)}
          title="open the palette"
          aria-label="open the palette"
        >
          palette
        </button>
      </aside>
    );
  }

  return (
    <aside className="hl-palette">
      {/* ── FLOWS ─────────────────────────────────────────────────────── */}
      <button
        style={headStyle}
        title="saved flows, each placed as one node: the flow as saved now"
        onClick={() => toggle("flows")}
      >
        flows {chevron(sections.has("flows"))}
      </button>
      {sections.has("flows") && (
        <div className="hl-palette__list" style={{ flex: "none", maxHeight: 160 }}>
          {flows.length === 0 && (
            <div style={{ color: "var(--ink-dim)", fontSize: 11, padding: "2px 8px" }}>
              save a flow to place it here
            </div>
          )}
          {flows.map((name) => (
            <PaletteItem
              key={name}
              mime={FLOW_PREFIX + name}
              swatch={{ background: "transparent", border: "1px solid #6fd3e0" }}
              name={name}
              info={`A saved flow, placed as one node (a holon): the flow as saved now. Its unwired inputs and outputs become the node's ports.`}
            />
          ))}
        </div>
      )}

      {/* ── MARKETPLACE ───────────────────────────────────────────────── */}
      <button
        style={headStyle}
        title={market.url ? `holons published to ${market.url}` : "set a marketplace on the flows page"}
        onClick={() => toggle("marketplace")}
      >
        marketplace {chevron(sections.has("marketplace"))}
      </button>
      {sections.has("marketplace") && (
        <div className="hl-palette__list" style={{ flex: "none", maxHeight: 160 }}>
          {!market.url && (
            <div style={{ color: "var(--ink-dim)", fontSize: 11, padding: "2px 8px" }}>
              set one on the flows page
            </div>
          )}
          {market.error && (
            <div style={{ color: "#e0916f", fontSize: 11, padding: "2px 8px" }}>{market.error}</div>
          )}
          {market.url && !market.error && market.entries.length === 0 && (
            <div style={{ color: "var(--ink-dim)", fontSize: 11, padding: "2px 8px" }}>
              nothing published yet
            </div>
          )}
          {market.entries.map(({ listing, adoption, versions }) => (
            <div
              key={listing.id}
              className="hl-palette__item"
              draggable
              title={`place "${listing.name}" (${listing.root.slice(0, 8)}), published by ${listing.publisher}`}
              onDragStart={(e) => {
                e.dataTransfer.setData(DND_MIME, MARKET_PREFIX + listing.id);
                e.dataTransfer.effectAllowed = "move";
              }}
            >
              <span className="hl-palette__swatch" style={{ background: "transparent", border: "1px dashed #6fd3e0" }} aria-hidden />
              <span className="hl-palette__name">
                {listing.name}
                {listing.yanked && <span style={{ color: "#e0916f" }}> yanked</span>}
                {adoption > 0 && (
                  <span style={{ color: "var(--ink-dim)" }} title={`placed in ${adoption} other listing(s)`}>
                    {" "}↑{adoption}
                  </span>
                )}
              </span>
              <InfoIcon
                text={`${listing.description}\n\nPublished by ${listing.publisher}${versions > 1 ? `, ${versions} versions` : ""}. ${
                  listing.manifest.length ? `It may: ${listing.manifest.join(", ")}.` : "It only computes."
                } Placing it checks every unit against its hash.`}
              />
            </div>
          ))}
        </div>
      )}

      {/* ── PALETTE ───────────────────────────────────────────────────── */}
      <div style={{ display: "flex", alignItems: "center" }}>
        <button style={{ ...headStyle, flex: 1 }} onClick={() => toggle("palette")}>
          palette {chevron(sections.has("palette"))}
        </button>
        {sections.has("palette") && (
          <button
            onClick={toggleAllPalette}
            title={allPaletteOpen ? "collapse all groups" : "expand all groups"}
            style={{
              font: "inherit", background: "transparent", border: "none",
              color: "var(--ink-dim)", cursor: "pointer", fontSize: 9,
              padding: "0 10px 0 0", lineHeight: 1, flexShrink: 0,
            }}
          >
            {allPaletteOpen ? "−" : "+"}
          </button>
        )}
        {onOpen && (
          <button
            onClick={() => onOpen(false)}
            title="hide palette"
            style={{
              font: "inherit", background: "transparent", border: "none",
              color: "var(--ink-dim)", cursor: "pointer", fontSize: 13,
              padding: "0 10px 0 0", lineHeight: 1, flexShrink: 0,
            }}
          >
            ‹
          </button>
        )}
      </div>

      {sections.has("palette") && (
        <div style={{ display: "flex", flexDirection: "column" }}>
          {/* http (templates) ──────────────────────────────── */}
          <button style={subHeadStyle} onClick={() => toggle("http")}>
            http {chevron(sections.has("http"))}
          </button>
          {sections.has("http") && (
            <div className="hl-palette__list" style={{ flex: "none" }}>
              {templates.map((name) => (
                <PaletteItem
                  key={name}
                  mime={name}
                  swatch={{ background: outputColor("Record") }}
                  name={name}
                  info={templateDoc(name)}
                />
              ))}
            </div>
          )}

          {/* catalog groups ─────────────────────────────────── */}
          {CATALOG_GROUPS.map((group) => {
            const units = catalog.filter((u) => group.items.has(nameOf(u)));
            if (units.length === 0) return null;
            return (
              <div key={group.name}>
                <button style={subHeadStyle} onClick={() => toggle(group.name)}>
                  {group.name} {chevron(sections.has(group.name))}
                </button>
                {sections.has(group.name) && (
                  <div className="hl-palette__list" style={{ flex: "none" }}>
                    {units.map((u) => {
                      const name = nameOf(u);
                      return (
                        <PaletteItem
                          key={name}
                          mime={name}
                          swatch={{ background: outputColor(u.outputs?.[0]?.ty) }}
                          name={name}
                          info={docs.get(name) ?? null}
                        />
                      );
                    })}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </aside>
  );
}
