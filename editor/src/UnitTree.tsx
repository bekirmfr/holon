// Transitive inspection (P4.4.1): a read-only descent into a unit's own closure, annotated with
// derived kind + capabilities at every level, to the leaves. Deliberately NOT expand-to-inspect
// (P4.3.1) again — that one mutates the canvas, placing a composite's children back as real,
// editable nodes; this one only ever peeks. Nothing here changes `nodes`/`edges` state at all.
//
// Mechanically this needs nothing new beyond what P4.3.1 and this same inch's bridge work
// already built: get_unit (already used by expand) descends one level at a time; capabilities/
// effects_of already recurse internally over whatever unit they're given, so calling them once
// per hash encountered while walking gives that subtree's own manifest at that level — there is
// no separate "per-level" bridge mode, just repeated use of three calls that already exist.

import { useMemo, useState } from "react";
import { Evaluator, type WKind, type WUnit } from "./holon";

type InspectNode = {
  hash: string;
  name: string;
  isComposite: boolean;
  /** By consent name, from core. */
  capabilities: string[];
  kind: WKind[];
  children: InspectNode[];
};

function buildInspectTree(hash: string, ev: Evaluator): InspectNode {
  const unit = ev.get_unit(hash) as WUnit;
  const capabilities = ev.capability_names(hash) as string[];
  const kind = ev.effects_of(hash) as WKind[];
  if (unit.body.kind === "Primitive") {
    return { hash, name: unit.body.name, isComposite: false, capabilities, kind, children: [] };
  }
  // A module is code with ports and no inside: a leaf of the tree, named by its hash.
  if (unit.body.kind === "Module") {
    return { hash, name: `module ${hash.slice(0, 8)}`, isComposite: false, capabilities, kind, children: [] };
  }
  return {
    hash,
    name: hash.slice(0, 8),
    isComposite: true,
    capabilities,
    kind,
    children: unit.body.children.map((h) => buildInspectTree(h, ev)),
  };
}

// A separate, small color set from theme.ts's own TYPE_COLOR — kind is a different dimension
// (what a unit MAY DO) from wire type (what a value IS), never shown on the same element, so a
// little overlap in hue wouldn't actually be confusing, but these are picked distinct from every
// existing TYPE_COLOR value anyway. Roughly a severity gradient: Pure (nothing to see) is the
// same muted ink-dim already used for de-emphasis; Sink (irreversible) reuses the app's existing
// warning/error orange.
const KIND_COLOR: Record<WKind, string> = {
  Pure: "#7e8ea8",
  Driver: "#6fd3e0",
  State: "#c77dff",
  Sink: "#e0916f",
};

function InspectRow({ node, depth }: { node: InspectNode; depth: number }) {
  const [open, setOpen] = useState(depth < 2);
  const hasChildren = node.children.length > 0;
  const kindLabel = node.kind.length > 0 ? node.kind.join(", ") : "Pure";
  const kindColor = node.kind.length > 0 ? KIND_COLOR[node.kind[0]] : KIND_COLOR.Pure;

  return (
    <div style={{ marginLeft: depth === 0 ? 0 : 14 }}>
      <div
        onClick={() => hasChildren && setOpen((o) => !o)}
        style={{
          display: "flex",
          alignItems: "center",
          gap: 6,
          padding: "2px 0",
          cursor: hasChildren ? "pointer" : "default",
        }}
      >
        <span style={{ width: 10, color: "#7e8ea8", fontSize: 10 }}>
          {hasChildren ? (open ? "▾" : "▸") : ""}
        </span>
        <span style={{ color: node.isComposite ? "#6fd3e0" : "#e8eef7" }}>{node.name}</span>
        <span style={{ color: kindColor, fontSize: 11 }}>{kindLabel}</span>
        {node.capabilities.length > 0 && (
          <span style={{ color: "#9fb4d4", fontSize: 10 }} title="capabilities">
            {node.capabilities.join(", ")}
          </span>
        )}
      </div>
      {open && node.children.map((c, i) => <InspectRow key={i} node={c} depth={depth + 1} />)}
    </div>
  );
}

/**
 * A unit's closure as a tree, read only: the Inspector's "unit" tab. `hash` is the unit inspected
 * (for a placed node, resolved by the caller with `unitFor`), so nothing here needs to know
 * whether the root came from a fresh primitive or an already-frozen composite.
 */
export function UnitTree({ hash, ev }: { hash: string; ev: Evaluator }) {
  // Built once per hash, synchronously: every call here (get_unit, capabilities, effects_of) is
  // a synchronous wasm call against data already resolved in this session.
  const tree = useMemo(() => {
    try {
      return { ok: true as const, node: buildInspectTree(hash, ev) };
    } catch (err) {
      return { ok: false as const, message: err instanceof Error ? err.message : String(err) };
    }
  }, [hash, ev]);
  return tree.ok ? <InspectRow node={tree.node} depth={0} /> : <div style={{ color: "#e0916f" }}>{tree.message}</div>;
}
