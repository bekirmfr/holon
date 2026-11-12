// The Console design's blocks, as the host's pages draw them (HostPages.tsx, ResourceViews.tsx,
// PolicyViews.tsx): a small button, the one to do, a field, a row (a card of its own), what a
// section is about, and a quiet note. A section's heading is styles.css's hl-host-page__head.

import type { CSSProperties } from "react";

export const small: CSSProperties = { font: "inherit", fontSize: "var(--fs-xs)", fontWeight: 500, minHeight: 26, padding: "2px 9px", color: "var(--ink)", background: "var(--surface)", border: "1px solid var(--border-strong)", borderRadius: "var(--r-sm)", cursor: "pointer", whiteSpace: "nowrap" };
export const primary: CSSProperties = { ...small, color: "var(--accent-ink)", background: "var(--accent)", borderColor: "var(--accent)" };
export const input: CSSProperties = { font: "inherit", fontSize: "var(--fs-sm)", minHeight: 30, padding: "4px 10px", color: "var(--ink)", background: "var(--surface)", border: "1px solid var(--border-strong)", borderRadius: "var(--r-sm)" };
export const row: CSSProperties = { display: "flex", alignItems: "center", gap: 10, padding: "10px 14px", marginBottom: 8, background: "var(--surface)", border: "1px solid var(--border)", borderRadius: "var(--r-lg)", boxShadow: "var(--shadow)", fontSize: "var(--fs-sm)" };
export const about: CSSProperties = { color: "var(--muted)", fontSize: "var(--fs-sm)", marginBottom: 12, lineHeight: 1.55, maxWidth: "100ch" };
export const quiet: CSSProperties = { color: "var(--muted)", fontSize: "var(--fs-xs)" };
