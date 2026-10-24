// Sample flows: one per capability, opened from the flows page as an unsaved copy. They are
// holon-author's (crates/holon-author/src/samples.rs), built from the live catalog through the
// flow builder, never from hand-copied node shapes, so a sample can't drift from core:
// samples.test.ts builds every one and derives it through the real core. Every new feature ships
// with a sample that shows it (CLAUDE.md).

import { build_sample, samples as sampleList } from "../wasm/holon_wasm.js";
import type { Evaluator } from "./holon";
import type { FlowDocument } from "./server";
import { fromHex } from "./flowDoc";

export type Sample = {
  /** The flow's name when opened (and its deployment's slug once deployed). */
  name: string;
  title: string;
  /** One line: what it demonstrates. */
  description: string;
  /** The features it shows, as the palette and docs name them. */
  shows: string[];
  /** Deploy settings: a server's port, or a clock flow's interval in seconds. */
  port?: string;
  interval?: string;
  /** Secrets to set under ⚙ before it works, what each is, and where to keep it when not a
   *  value sealed by the server (`env:HOLON_SECRET_…`, `vault:…`). */
  secrets?: { name: string; what: string; from?: string }[];
  /** Permissions it can run without (deploy settings' `optional`); the rest are required. */
  optional?: string[];
  /** Web pages from other sites it answers (deploy settings' `browsers`: `*`, or origins). */
  browsers?: string;
  /** How to try it once deployed. */
  tryIt: string;
  /** The flow in the language, as its file in crates/holon-author/samples/ is written. */
  text: string;
};

let list: Sample[] | null = null;

/** Every sample, in the order the flows page lists them. The wasm module must be loaded. */
export function samples(): Sample[] {
  return (list ??= sampleList() as Sample[]);
}

/** Lay out `sample` on a canvas: the flow document opening it loads. Its holons' units are put
 *  into `ev`, and travel in the document (the whole closure), as a saved flow's do. */
export function buildSample(sample: Sample, ev: Evaluator): FlowDocument {
  const doc = build_sample(sample.name) as FlowDocument;
  for (const hex of doc.units ?? []) ev.insert(fromHex(hex));
  return doc;
}
