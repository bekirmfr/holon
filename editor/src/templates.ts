// HTTP node templates: `request`, `respond` and `stripe_verify`. Each is a small composite of
// genesis leaves, rebuilt from the node's params whenever it is hashed. What they are, their ports,
// their default params and the units they build are holon-author's (crates/holon-author/src/
// templates.rs), read through the wasm bridge: the same code the command line runs.

import { template_defaults, template_doc, template_interface, template_names } from "../wasm/holon_wasm.js";
import type { WParam, WPort } from "./holon";

/** The templates, in palette order. */
export function templateNames(): string[] {
  return template_names();
}

export function isTemplate(name: string): boolean {
  return template_doc(name) !== undefined;
}

/** What a template does, for the person placing it (core describes the primitives). */
export function templateDoc(name: string): string | null {
  return template_doc(name) ?? null;
}

/** A template node's interface for a given set of params (a respond node's inputs depend on its
 *  status). `hiddenInputs`: the inputs that are fed by the endpoint, not drawn as handles. */
export function templateInterface(name: string, params: WParam[]): { inputs: WPort[]; outputs: WPort[]; hiddenInputs: boolean } {
  return template_interface(name, params);
}

/** The params a freshly dropped template node starts with. */
export function templateDefaults(name: string): WParam[] {
  return template_defaults(name);
}
