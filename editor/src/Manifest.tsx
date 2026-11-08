// A permission's badge: whether the flow needs it to work or runs without it (core's rule,
// holon.ts's permissionKind), with why on hover. Settings' permissions show one beside each.

import { permissionKind } from "./holon";

/** "required" or "optional", with why on hover. */
export function KindBadge({ item, optional, trigger }: { item: string; optional?: string[]; trigger?: string | null }) {
  const { kind, why } = permissionKind(item, optional, trigger);
  return (
    <span title={why} style={{ color: kind === "required" ? "var(--warn)" : "var(--faint)", border: "1px solid var(--border)", borderRadius: 3, padding: "0 4px" }}>
      {kind}
    </span>
  );
}
