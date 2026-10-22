// A projection's promoted inputs: their starting values and the field that edits each one.

import type { CSSProperties } from "react";
import type { WPort, WType, WValue } from "./holon";
import { typeColor, typeLabel } from "./theme";

// A promoted input's starting value: the type's zero value for the three scalar forms that have
// one (Number→0, Boolean→false, Text→"") — the SAME convention `const`'s own default and core's
// `sourced_mock` already use, reused rather than invented fresh. Everything else (List/Product/Sum/
// Effect/Any) has no literal form — same boundary `const`'s CONST_TYPES already draws — so it starts
// `null` ("wire in": honestly unsupplied, not a guessed default).
//
// A `null` slot does NOT mean "eval blocks" — core's `eval` drops absent slots before `compute` ever
// runs (`inputs.iter().flatten()`; "compute's arity check is the firing rule" per eval.rs's own
// comment), uniformly for every primitive, no special-casing here or there. Whether that reads as
// "not ready" depends entirely on the target primitive: `add` destructures exactly 2 values, so a
// dropped slot fails its own arity match ⇒ not ready. `list` has no arity requirement at all, so it
// happily builds from however many values are actually present — including zero, `[]`. Both are
// correct; neither needs different handling in this file.
export function defaultBoundaryValue(ty: WType): WValue | null {
  if (ty === "Number") return { type: "Number", value: 0 };
  if (ty === "Boolean") return { type: "Boolean", value: false };
  if (ty === "Text") return { type: "Text", value: "" };
  return null;
}

const boundaryFieldStyle: CSSProperties = {
  font: "inherit",
  fontSize: 11,
  color: "#cfe3ff",
  background: "#0e1a2f",
  border: "1px solid #1b2a45",
  borderRadius: 3,
  padding: "1px 4px",
  width: 64,
};

// One promoted input's field. A scalar gets a real, type-native control — a number input, a
// checkbox, a text input — not a generic text box needing string parsing: unlike `const`, a
// boundary value is never hashed (it's a pure eval-time argument), so there's no reason to route
// it through core's own string parser the way a frozen param has to. A non-scalar honestly has no
// literal form (the same boundary CONST_TYPES already draws) and shows "wire in", not a guess.
// Colour comes from theme.ts — the same "type is colour" legend the canvas nodes already use.
export function BoundaryField({
  port,
  label: displayLabel,
  value,
  onChange,
}: {
  port: WPort;
  label: string;
  value: WValue | null;
  onChange: (v: WValue | null) => void;
}) {
  const row: CSSProperties = { display: "flex", alignItems: "center", justifyContent: "flex-end", gap: 6 };
  const label = (
    <span style={{ color: typeColor(port.ty), fontSize: 11 }} title={typeLabel(port.ty)}>
      {displayLabel}
    </span>
  );

  if (port.ty === "Number") {
    const raw = value?.type === "Number" ? String(value.value) : "";
    return (
      <div style={row}>
        {label}
        <input
          type="number"
          style={boundaryFieldStyle}
          value={raw}
          onChange={(e) => {
            const v = e.target.value;
            onChange(v === "" ? null : { type: "Number", value: Number(v) });
          }}
        />
      </div>
    );
  }
  if (port.ty === "Boolean") {
    const checked = value?.type === "Boolean" ? value.value : false;
    return (
      <div style={row}>
        {label}
        <input
          type="checkbox"
          checked={checked}
          onChange={(e) => onChange({ type: "Boolean", value: e.target.checked })}
        />
      </div>
    );
  }
  if (port.ty === "Text") {
    const raw = value?.type === "Text" ? value.value : "";
    return (
      <div style={row}>
        {label}
        <input
          type="text"
          style={{ ...boundaryFieldStyle, width: 84 }}
          value={raw}
          onChange={(e) => onChange({ type: "Text", value: e.target.value })}
        />
      </div>
    );
  }
  // List / Product / Sum / Effect / Any — no literal form. Honestly unsupplied, not defaulted.
  return (
    <div style={row}>
      {label}
      <span style={{ ...boundaryFieldStyle, color: "#5a6d8c", textAlign: "center" }}>wire in</span>
    </div>
  );
}
