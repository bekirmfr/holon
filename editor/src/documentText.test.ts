// The text the editor shows for a document is the code the document deploys as.
//
// Guards: wherever the editor shows a saved or proposed document's text (the review of a
// proposal, the diff beside a deploy, the text pane of a tab opened), what it shows compiles to
// the unit the document's canvas deploys as, whatever text was saved beside the canvas.
//
// Why: a document carries its code twice, as a canvas (what deploys) and as a text (`source`,
// what a person reads), and whoever saved or proposed it wrote both: the editor, the agent tools,
// or anything that can post JSON with an agent's token. The editor showed `source` as it came.
//
// If it fails: a person approving an agent's proposal reads one program and deploys another,
// within the permissions the approval lists. The review is the one place a person is asked to
// read the code, so it is the one place that must not show something else.
//
// Every sample is a document; beside each canvas is put its own text, another sample's, its own
// with a route added or its port changed, and one that isn't a flow. The same rule is held for
// the command line and the agent tools by holon-author's
// `the_text_a_document_shows_is_the_code_it_deploys`, on the function this one reaches through
// the bridge.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import type { Edge } from "@xyflow/react";
import { initSync, Evaluator } from "../wasm/holon_wasm.js";
import { flowUnitHash } from "./canvasOps";
import { restoreUnits } from "./docUnits";
import { compileHolon, documentText, sourceOf } from "./holonText";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import { samples } from "./samples";
import type { FlowDocument } from "./server";
import { settingsOf, tabFromDocument } from "./tabs";

initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });

/** The unit `doc`'s canvas deploys as: what a review's approve deploys (App.tsx `prepareSaved`). */
function deploysAs(doc: FlowDocument): string {
  // In a fresh workspace, from what the document carries: as opening it does.
  const ev = new Evaluator();
  restoreUnits(doc, ev);
  return flowUnitHash(doc.nodes as PrimitiveNodeType[], doc.edges as Edge[], ev, settingsOf(doc.server).port, settingsOf(doc.server).tags ?? []);
}

/** The unit `text` compiles to, or null when it doesn't compile. */
function says(text: string): string | null {
  try {
    const ev = new Evaluator();
    const { doc } = compileHolon(text, ev);
    return flowUnitHash(doc.nodes as PrimitiveNodeType[], doc.edges as Edge[], ev, settingsOf(doc.server).port, settingsOf(doc.server).tags ?? []);
  } catch {
    return null;
  }
}

// Each sample compiled several times over: up to seconds alone (chess), more beside a whole run's
// other files, past vitest's five.
const COMPILES = 30_000;

describe("the text a document shows is the code it deploys", () => {
  const all = samples();
  let forged = 0;
  for (const [i, sample] of all.entries()) {
    it(sample.name, { timeout: COMPILES }, () => {
      const doc = compileHolon(sample.text, new Evaluator()).doc;
      const unit = deploysAs(doc);
      expect(says(sample.text)).toBe(unit);

      // Its own text, as saved, and in the pane of its tab.
      const honest = { ...doc, source: sample.text };
      expect(documentText(sample.name, honest)).toBe(sample.text);
      expect(sourceOf(sample.name, honest)).toBe(sample.text);
      expect(tabFromDocument(sample.name, honest, true, new Evaluator()).meta.text.value).toBe(sample.text);
      // With no text saved, its canvas printed: the same code.
      expect(says(documentText(sample.name, doc))).toBe(unit);

      // Texts that say something else.
      const other = all[(i + 1) % all.length];
      const others = [other.text, `${sample.text}\nroute.get("/one-more", () => "added")\n`, "not a flow at all"];
      if (sample.port) others.push(sample.text.replace(`port: ${sample.port}`, `port: ${Number(sample.port) + 1}`));
      for (const said of others) {
        const shown = documentText(sample.name, { ...doc, source: said });
        expect(says(shown), `beside a text that says ${said.slice(0, 60)}…`).toBe(unit);
        if (says(said) === unit) continue;
        forged += 1;
        expect(shown).not.toBe(said);
        expect(sourceOf(sample.name, { ...doc, source: said })).toBeNull();
        // Opened, its pane holds no typed text: it prints the canvas.
        expect(tabFromDocument(sample.name, { ...doc, source: said }, true, new Evaluator()).meta.text.value).toBeNull();
      }
    });
  }
  it("puts texts that aren't the canvas's beside most canvases", () => {
    expect(forged).toBeGreaterThan(all.length * 2);
  });
});
