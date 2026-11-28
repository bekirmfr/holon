// Every canvas the editor shows of a kept flow is drawn from its text.
//
// Guards: wherever the editor shows a flow the server keeps (a saved flow, a proposal under review,
// a deployed version, the version a run ran on), the canvas is the kept text compiled here, with
// its hand-drawn layout put back only on the code it was drawn for, and it deploys as the code
// that runs; one that compiles here to other code is refused, not shown.
//
// Why: a flow travels and is kept as its text (docs/L4s/text-deploy-plan.md); the server keeps no
// canvas. The editor compiles the text with the compiler the server and the command line use
// (holon-author through the bridge). A reading that took a canvas from anywhere else, or put a
// layout on other code, would show a person one program while another runs.
//
// If it fails: a reviewer approves, or a person reads a run on, a canvas that isn't the code.
//
// Every sample, read back through each of the editor's readings of the server, with the server
// answering only what it keeps (text and layout). Held for the command line and the agent tools
// by holon-author's `every_sample_reads_back_as_its_own_code_and_drawing`.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import type { Edge } from "@xyflow/react";
import { initSync, Evaluator } from "../wasm/holon_wasm.js";
import { flowUnitHash } from "./canvasOps";
import { restoreUnits } from "./docUnits";
import { compileHolon } from "./holonText";
import { flowToKeep, keptDocument, type Layout } from "./keptFlow";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import { samples } from "./samples";
import { getDeployedDocument, getFlow, getProposal, setSession, type FlowDocument } from "./server";
import { settingsOf, tabFromDocument } from "./tabs";

initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });

/** The unit `doc`'s canvas deploys as, in a fresh workspace from what it carries (as opening it). */
function deploysAs(doc: FlowDocument): string {
  const ev = new Evaluator();
  restoreUnits(doc, ev);
  return flowUnitHash(doc.nodes as PrimitiveNodeType[], doc.edges as Edge[], ev, settingsOf(doc.server).port, settingsOf(doc.server));
}

const positions = (doc: FlowDocument) => (doc.nodes as { id: string; position: { x: number; y: number } }[]).map((n) => [n.id, n.position.x, n.position.y]);

// Each sample compiled several times over: up to seconds alone (chess), past vitest's five.
const COMPILES = 30_000;

/** The server, answering only what it keeps of `kept`: a saved flow's, a proposal's and a
 *  deployment's text and layout, never a canvas. */
let kept: { text: string; layout: Layout | null; unit: string } = { text: "", layout: null, unit: "" };
beforeAll(() => {
  setSession({ token: "t", name: "ada", kind: "person", role: "editor" } as Parameters<typeof setSession>[0]);
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string) => {
      const path = new URL(url, "http://holon.test").pathname;
      const body = path.startsWith("/flows/")
        ? { name: "x", saved_at_epoch_seconds: 1, text: kept.text, layout: kept.layout }
        : path.startsWith("/proposals/")
          ? { id: 1, slug: "x", unit: kept.unit, why: "", agent: true, at_ms: 0, needs: [], state: "waiting", text: kept.text, layout: kept.layout }
          : path.endsWith("/text")
            ? { unit: kept.unit, version: 1, text: kept.text, layout: kept.layout }
            : null;
      return new Response(JSON.stringify(body ?? { error: "no route" }), { status: body ? 200 : 404, headers: { "Content-Type": "application/json" } });
    }),
  );
});
afterAll(() => {
  vi.unstubAllGlobals();
  setSession(null);
});

describe("every canvas the editor shows of a kept flow is drawn from its text", () => {
  const all = samples();
  for (const [i, sample] of all.entries()) {
    it(sample.name, { timeout: COMPILES }, async () => {
      const drawn = keptDocument(sample.text);
      const unit = deploysAs(drawn);
      expect(drawn.source).toBe(sample.text);
      // Its tab's text pane holds the text as written.
      expect(tabFromDocument(sample.name, drawn, true, new Evaluator()).meta.text.value).toBe(sample.text);

      // Moved by hand: what keeping it sends is its text and where every node is.
      const moved: FlowDocument = { ...drawn, nodes: (drawn.nodes as { position: unknown }[]).map((n, k) => ({ ...n, position: { x: 7000 + k, y: 2 * k + 0.5 } })) };
      const sent = flowToKeep(moved, sample.text, unit);
      expect(sent.text).toBe(sample.text);
      expect(sent.layout?.unit).toBe(unit);

      // Each reading of the server: a saved flow, a proposal, the version deployed, a run's version.
      kept = { text: sample.text, layout: sent.layout, unit };
      const readings = [(await getFlow(sample.name)).document, (await getProposal(1)).document!, await getDeployedDocument(sample.name), await getDeployedDocument(sample.name, unit)];
      for (const shown of readings) {
        expect(deploysAs(shown)).toBe(unit);
        expect(shown.source).toBe(sample.text);
        expect(positions(shown)).toEqual(positions(moved));
      }

      // Held to other code than runs: refused, wherever it is read.
      const other = all[(i + 1) % all.length];
      const otherUnit = deploysAs(keptDocument(other.text));
      if (otherUnit !== unit) {
        kept = { text: sample.text, layout: null, unit: otherUnit };
        await expect(getDeployedDocument(sample.name)).rejects.toThrow(/other code/);
        await expect(getProposal(1)).rejects.toThrow(/other code/);
        // A layout drawn for other code moves nothing.
        const elsewhere = keptDocument(other.text, sent.layout);
        expect(positions(elsewhere)).toEqual(positions(keptDocument(other.text)));
        // A canvas its text doesn't spell isn't kept.
        expect(() => flowToKeep(moved, other.text, unit)).toThrow();
      }
      // The text, compiled as the text pane compiles it, is the same code.
      const ev = new Evaluator();
      const { doc } = compileHolon(sample.text, ev);
      expect(flowUnitHash(doc.nodes as PrimitiveNodeType[], doc.edges as Edge[], ev, settingsOf(doc.server).port, settingsOf(doc.server))).toBe(unit);
    });
  }
});
