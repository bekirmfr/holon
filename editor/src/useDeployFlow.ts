// Deploying the open flow: one action for the whole flow, named after it (DeployBar in the
// header). A flow with routes deploys as its derived server; any other flow as the whole canvas
// projected into one unit (a clock or ingress flow). Before anything is sent, the person sees
// exactly what it may touch (the server's consent gate checks the same list), in the dialog.
// And running, pausing or stopping it once deployed.

import { useCallback, useMemo, useState, type MutableRefObject } from "react";
import type { Edge, Viewport } from "@xyflow/react";
import { flowUnitHash, traceOfUnit, type DerivedServer } from "./canvasOps";
import { documentUnits } from "./docUnits";
import { bindingsOf } from "./holonText";
import { isWEncapError } from "./diagnostics";
import { describeItem, type Evaluator } from "./holon";
import { stageClosure } from "./canvasUnits";
import { changeDeployment, deployUnit, previewDeploy, type DeploymentEntry, type FlowDocument } from "./server";
import { browsersOf, type FlowSettings } from "./DeployBar";
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
    let isServer = false;
    let trace = derived.trace;
    try {
      if (derived.errors.length > 0) throw new Error(derived.errors.join("; "));
      if (derived.endpoints.length > 0 || derived.clocks.length > 0 || derived.files.length > 0 || derived.actions.length > 0) {
        if (!derived.serverHash) throw new Error(`"${settings.port}" is not a port (set it under ⚙)`);
        hash = derived.serverHash;
        isServer = true;
      } else {
        const { nodes: ns, edges: es } = live.current;
        if (ns.length === 0) throw new Error("nothing to deploy: the canvas is empty");
        hash = flowUnitHash(ns, es, ev, settings.port);
        trace = { "*": traceOfUnit(hash, ns, ev) };
      }
    } catch (err) {
      setError(isWEncapError(err) ? err.message : err instanceof Error ? err.message : String(err));
      return;
    }
    // On a schedule (an interval, and no routes), it runs on the clock whether or not it reads it.
    const scheduled = !isServer && settings.interval.trim() !== "";
    const caps = ev.deployed_capability_names(hash, scheduled) as string[];
    let interval: number | undefined;
    if (!isServer && caps.includes("Clock")) {
      interval = Number(settings.interval);
      if (!Number.isInteger(interval) || interval < 1) {
        setError("this flow reads the clock: set how often it fires under ⚙ (whole seconds, at least 1)");
        return;
      }
    }
    // The host decides what needs approval (its policy, and what this deployment was already
    // granted): ask it, then show asked against granted, and acknowledge exactly what it asks.
    const existing = deployments.some((d) => d.slug === name);
    const requirements = settings.requirements;
    let ask: string[];
    try {
      await stageClosure(ev, hash, new Set());
      const preview = await previewDeploy(name, hash, requirements, interval);
      if (preview.refused) {
        setError(`this host can't grant that: ${preview.refused}`);
        return;
      }
      ask = preview.needs_approval;
      // Each item says whether the flow can run without it (deploy settings' optional ones).
      const optional = requirements?.optional ?? [];
      const said = (i: string) => `• ${describeItem(i)}${optional.includes(i) ? " (optional: it runs without it)" : ""}`;
      const lines = [`${existing ? "Update" : "Deploy"} "${name}"?`];
      if (ask.length > 0) lines.push("", "It needs your approval to:", ...ask.map(said));
      if (preview.already_granted.length > 0) {
        lines.push("", "Already granted:", ...preview.already_granted.map(said));
      }
      if (preview.missing_secrets.length > 0) {
        lines.push("", `Not set yet: ${preview.missing_secrets.join(", ")} (set under ⚙ → secrets; until then those nodes stay empty)`);
      }
      const pages = browsersOf(settings);
      if (isServer && pages.length > 0) lines.push("", `Web pages from ${pages.includes("*") ? "any site" : pages.join(", ")} may call its routes.`);
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
      await deployUnit(name, hash, ask, interval, document, requirements);
      refreshDeployments();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setDeployBusy(false);
    }
  }, [flowName, setFlowName, dialog, derived, settings, source, live, ev, deployments, refreshDeployments, getViewport, setError]);

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
