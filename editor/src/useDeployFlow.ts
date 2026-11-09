// Deploying the open flow: one action for the whole flow, named after it (DeployBar in the
// header). It deploys as the server its handlers derive (routes, clocks, file handlers, actions);
// a flow with none starts nothing, and doesn't deploy. Before anything is sent, the person sees
// exactly what it may touch (the server's consent gate checks the same list), in the dialog.
// And running, pausing or stopping it once deployed.

import { useCallback, useMemo, useState, type MutableRefObject } from "react";
import type { Edge, Viewport } from "@xyflow/react";
import type { DerivedServer } from "./canvasOps";
import { documentUnits } from "./docUnits";
import { bindingsOf } from "./holonText";
import { isWEncapError } from "./diagnostics";
import { describeItemSending, type Evaluator } from "./holon";
import { stageClosure } from "./canvasUnits";
import { changeDeployment, deployUnit, previewDeploy, type DeploymentEntry, type FlowDocument } from "./server";
import { browsersOf, type FlowSettings } from "./DeployBar";
import { touchLines, touchText } from "./touches";
import type { PrimitiveNodeType } from "./PrimitiveNode";

type Dialogs = { confirm: (message: string, action?: string) => Promise<boolean>; prompt: (message: string, initial?: string) => Promise<string | null> };

export function useDeployFlow({
  flowName,
  setFlowName,
  nodes,
  live,
  derived,
  flowCapabilities,
  settings,
  source,
  getViewport,
  ev,
  dialog,
  deployments,
  refreshDeployments,
  setError,
  onDeployed,
}: {
  flowName: string | null;
  setFlowName: (name: string) => void;
  nodes: PrimitiveNodeType[];
  live: MutableRefObject<{ nodes: PrimitiveNodeType[]; edges: Edge[] }>;
  /** The server the canvas declares (canvasOps.deriveServer). */
  derived: DerivedServer;
  /** What the flow would ask for, by consent name. */
  flowCapabilities: string[];
  settings: FlowSettings;
  /** The flow's text as typed, when it stands for the canvas (tabs.ts `sourceToSave`); null when
   *  the text is only the canvas printed. */
  source: string | null;
  getViewport: () => Viewport;
  ev: Evaluator;
  dialog: Dialogs;
  deployments: DeploymentEntry[];
  refreshDeployments: () => void;
  setError: (msg: string) => void;
  onDeployed?: () => void;
}) {
  const [deployBusy, setDeployBusy] = useState(false);
  const deployedEntry = flowName ? deployments.find((d) => d.slug === flowName) : undefined;
  const outOfDate = deployedEntry && derived.serverHash ? deployedEntry.unit !== derived.serverHash : null;

  // The secrets the flow reads, by name: what ⚙ lets you set. From its derived manifest, so a
  // secret inside a placed holon counts; plus its own `secret` nodes, for while it doesn't derive.
  const secretNames = useMemo(
    () =>
      [
        ...new Set([
          ...flowCapabilities.flatMap((c) => {
            const m = /^SecretRead\("(.*)"\)$/.exec(c);
            return m ? [m[1]] : [];
          }),
          ...nodes.flatMap((n) =>
            n.data.name === "secret" && n.data.body.kind === "Primitive"
              ? n.data.params.flatMap((p) => (p.name === "name" && p.kind === "Frozen" ? [p.value] : []))
              : [],
          ),
        ]),
      ].sort(),
    [nodes, flowCapabilities],
  );

  const onDeployFlow = useCallback(async () => {
    setError("");
    let name = flowName;
    if (!name) {
      name = await dialog.prompt("Name this flow first (letters, digits, - and _ only). Its deployment takes the name:");
      if (!name) return;
      // A draft named now (New opens untitled) mustn't quietly replace what runs under that name.
      if (deployments.some((d) => d.slug === name) && !(await dialog.confirm(`"${name}" is deployed already. Update it with this flow?`, "Update"))) return;
      setFlowName(name);
    }
    let hash: string;
    const trace = derived.trace;
    try {
      if (derived.errors.length > 0) throw new Error(derived.errors.join("; "));
      if (derived.endpoints.length + derived.clocks.length + derived.files.length + derived.mails.length + derived.feeds.length + derived.actions.length === 0) {
        throw new Error(live.current.nodes.length === 0 ? "nothing to deploy: the canvas is empty" : "nothing would start a run: add a route, a clock, a file, mail or feed handler or an action");
      }
      if (!derived.serverHash) throw new Error(`"${settings.port}" is not a port (set it under ⚙)`);
      hash = derived.serverHash;
    } catch (err) {
      setError(isWEncapError(err) ? err.message : err instanceof Error ? err.message : String(err));
      return;
    }
    // The host decides what needs approval (its policy, and what this deployment was already
    // granted): ask it, then show asked against granted, and acknowledge exactly what it asks.
    const existing = deployments.some((d) => d.slug === name);
    const requirements = settings.requirements;
    let ask: string[];
    try {
      await stageClosure(ev, hash, new Set());
      const preview = await previewDeploy(name, hash, requirements);
      if (preview.refused) {
        setError(`this host can't grant that: ${preview.refused}`);
        return;
      }
      ask = preview.needs_approval;
      // Each item says whether the flow can run without it (deploy settings' optional ones).
      const optional = requirements?.optional ?? [];
      const said = (i: string) => `• ${describeItemSending(i, preview.sent)}${optional.includes(i) ? " (optional: it runs without it)" : ""}`;
      const lines = [`${existing ? "Update" : "Deploy"} "${name}"?`];
      if (ask.length > 0) lines.push("", "It needs your approval to:", ...ask.map(said));
      // Everything it touches, beside what it touches now, and whether this server is ready for
      // each (a secret not set yet, a folder not bound).
      const touches = touchText(touchLines(preview.touches, preview.settings));
      if (touches.length > 0) lines.push("", "What it touches (+ new, - no longer, ~ changed, ! not ready on this server):", ...touches);
      else if (preview.already_granted.length > 0) lines.push("", "Already granted:", ...preview.already_granted.map(said));
      if (touches.length === 0 && preview.missing_secrets.length > 0) {
        lines.push("", `Not set yet: ${preview.missing_secrets.join(", ")} (set under ⚙ → secrets; until then those nodes stay empty)`);
      }
      const pages = browsersOf(settings);
      if (pages.length > 0) lines.push("", `Web pages from ${pages.includes("*") ? "any site" : pages.join(", ")} may call its routes.`);
      if (existing) lines.push("", "Its state and sessions are kept.");
      const action = ask.length > 0 ? "Approve and deploy" : existing ? "Update" : "Deploy";
      if (!(await dialog.confirm(lines.join("\n"), action))) return;
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      return;
    }
    setDeployBusy(true);
    try {
      const { nodes: ns, edges: es } = live.current;
      const carried = documentUnits(ns.flatMap((n) => (n.data.body.kind === "Composite" ? [n.data.body.hash] : [])), ev);
      // The text and its names ride with the deployment, as they do with a save and with a deploy
      // from text through `holon mcp`: a run then explains as the text reads.
      const bindings = source !== null ? bindingsOf(source, { nodes: ns, edges: es }, ev) : null;
      const document: FlowDocument = {
        nodes: ns,
        edges: es,
        viewport: getViewport(),
        ...carried,
        server: settings,
        trace,
        ...(source !== null ? { source } : {}),
        ...(bindings ? { bindings } : {}),
      };
      await deployUnit(name, hash, ask, document, requirements);
      refreshDeployments();
      onDeployed?.();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setDeployBusy(false);
    }
  }, [flowName, setFlowName, dialog, derived, settings, source, live, ev, deployments, refreshDeployments, getViewport, setError, onDeployed]);

  const onFlowAction = useCallback(
    (action: "run" | "pause" | "stop") => {
      if (!flowName) return;
      setError("");
      changeDeployment(flowName, action)
        .then(() => refreshDeployments())
        .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
    },
    [flowName, refreshDeployments, setError],
  );

  return { deployedEntry, outOfDate, deployBusy, secretNames, onDeployFlow, onFlowAction };
}
