// After a reload the address can name a tab the page had open: the editor opens it again as what the
// server holds under that name now (App.tsx's reopen). Asked of the server when the tab reopens,
// not read from the lists the page keeps: a reload reopens before those have come, and the effect
// that reopens holds the ones of the render it began in. Read so, a deployed flow whose tab was
// never saved was said not to exist (the e2e's slow runner, home.e2e.ts's "panes" test).

import { getFlow, listDeployments, type FlowRecord } from "./server";

/** What a tab reopens as: the flow saved under its name, else that deployment's view, else
 *  nothing (a tab never saved, of a flow not deployed, closed with the page). */
export type Reopening = { kind: "saved"; flow: FlowRecord } | { kind: "deployed" } | { kind: "none" };

/** What the tab named `slug` reopens as, as the server says now. */
export async function reopening(slug: string): Promise<Reopening> {
  const flow = await getFlow(slug).catch(() => null);
  if (flow) return { kind: "saved", flow };
  const deployments = await listDeployments().catch(() => []);
  return deployments.some((d) => d.slug === slug) ? { kind: "deployed" } : { kind: "none" };
}
