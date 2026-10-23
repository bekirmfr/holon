// Placed nodes as core units: a node's own hash, and staging a unit's closure to holon-server.

import { buildPreimage, Evaluator, type WUnit } from "./holon";
import { stageUnit } from "./server";
import type { PrimitiveNodeType } from "./PrimitiveNode";

export function primitiveName(u: WUnit): string | null {
  return u.body.kind === "Primitive" ? u.body.name : null;
}

// A placed node's own hash, for use as a child in project/group: a primitive rebuilt from its
// name and ports (the same primitive always yields the same hash), a template from its params, a
// grouped composite read off its body. holon-author decides it (flow.rs `unit_for`), through the
// Evaluator, which stores it.
export function unitFor(n: PrimitiveNodeType, ev: Evaluator): string {
  return ev.unit_for(n);
}

// Recursively stage a unit's whole closure to holon-server — not just the root. A composite's
// own preimage references its children BY HASH; it never carries their bytes. The local
// Evaluator already has every one of them (that's how the composite got built in the first
// place), so this walks the SAME closure `capabilities()`/`materialize_closure` walk on the
// server side — children, and any `fn` param (a `map`'s higher-order reference; core's own
// `child_refs` counts it as part of the closure too, so skipping it here would under-stage
// exactly the case that exists to not miss) — and pushes whatever the server doesn't have yet.
// `staged` is the caller's own de-dup set: the same child reachable from two different parents
// (an ordinary DAG, not a tree) only needs sending once. Order between siblings doesn't matter —
// `POST /unit` stores whatever bytes it's given with no closure-completeness check of its own
// (that's `/deploy`'s job, downstream) — only "every hash gets there before `/deploy` is called"
// matters, not the order they arrive in.
export async function stageClosure(ev: Evaluator, hash: string, staged: Set<string>): Promise<void> {
  if (staged.has(hash)) return;
  staged.add(hash); // mark before awaiting anything, so a re-entrant call can't double-queue it
  const unit = ev.get_unit(hash) as WUnit;
  if (unit.body.kind === "Composite") {
    for (const child of unit.body.children) {
      await stageClosure(ev, child, staged);
    }
  }
  const fnParam = (unit.params ?? []).find((p) => p.kind === "Frozen" && p.name === "fn");
  if (fnParam && fnParam.kind === "Frozen") {
    await stageClosure(ev, fnParam.value, staged);
  }
  await stageUnit(buildPreimage(unit));
}
