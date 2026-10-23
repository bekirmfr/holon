// Open flows, one tab each. The canvas shows the active tab; every other tab is kept whole (its
// canvas, undo history, deploy settings, mode and the run it was looking at) and put back when
// picked. A tab is in edit mode (the flow as edited: draft or saved) or view mode (what is
// deployed, read only, with its runs); view needs a deployment.

import type { Edge } from "@xyflow/react";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import type { FlowDocument, LogEntry } from "./server";
import type { FlowSettings } from "./DeployBar";
import { DEFAULT_SETTINGS } from "./DeployBar";
import type { History } from "./history";
import { newHistory, snapshotOf, type Snapshot } from "./useCanvasHistory";

type Viewport = { x: number; y: number; zoom: number };

/** The flow as edited, set aside while its deployment is viewed. */
export type EditCopy = { nodes: PrimitiveNodeType[]; edges: Edge[]; viewport: Viewport; history: History<Snapshot>; dirty: boolean };

/** What a tab holds about its flow besides the canvas: the app keeps it as one state, so a field
 *  added here is captured and restored with the tab, with nothing else to touch. */
export type FlowMeta = {
  name: string | null;
  /** Whether it exists as a saved flow (opened from one, or saved since). */
  saved: boolean;
  settings: FlowSettings;
  /** What it was last saved or opened as (App's savedKeyOf); "" for one never saved. */
  savedKey: string;
  saveMsg: string;
  /** The deployment shown, in view mode; null in edit mode. */
  viewing: string | null;
  /** The version (unit, hex) whose canvas view mode shows: the deployment's current one, or the
   *  earlier one the selected run ran on. */
  viewUnit: string | null;
  viewTrace: Record<string, string[]> | undefined;
  selectedRun: LogEntry | null;
  step: number | null;
  inspectNode: string | null;
};

/** Everything one tab holds: its flow (`meta`), and the canvas plumbing kept outside React state.
 *  In view mode `nodes`/`edges` are the deployed flow's, and the flow as edited waits in
 *  `beforeView` (null for a tab opened to view: edit loads it then). */
export type TabState = {
  meta: FlowMeta;
  nodes: PrimitiveNodeType[];
  edges: Edge[];
  viewport: Viewport;
  history: History<Snapshot>;
  /** The next node id's counter: past every id the canvas holds. */
  idCounter: number;
  beforeView: EditCopy | null;
  viewDoc: FlowDocument | null;
  /** Unsaved changes, as of when it was set aside. */
  dirty: boolean;
};

export type Mode = "edit" | "view";

/** A fresh flow's meta, in edit mode. */
export function blankMeta(name: string | null): FlowMeta {
  return {
    name,
    saved: false,
    settings: DEFAULT_SETTINGS,
    savedKey: "",
    saveMsg: "",
    viewing: null,
    viewUnit: null,
    viewTrace: undefined,
    selectedRun: null,
    step: null,
    inspectNode: null,
  };
}

/** A fresh, empty flow in edit mode. */
export function blankTab(name: string | null): TabState {
  return {
    meta: blankMeta(name),
    nodes: [],
    edges: [],
    viewport: { x: 0, y: 0, zoom: 1 },
    history: newHistory(snapshotOf([], [])),
    idCounter: 0,
    beforeView: null,
    viewDoc: null,
    dirty: false,
  };
}

/** One field of `FlowMeta` as a setter, value or updater, like useState's. */
export function metaSetter<K extends keyof FlowMeta>(
  setMeta: (update: (m: FlowMeta) => FlowMeta) => void,
  key: K,
): (value: FlowMeta[K] | ((prev: FlowMeta[K]) => FlowMeta[K])) => void {
  return (value) =>
    setMeta((m) => ({ ...m, [key]: typeof value === "function" ? (value as (p: FlowMeta[K]) => FlowMeta[K])(m[key]) : value }));
}

/** What a tab is, as its label says: deployed (a deployment has its name), saved, or a draft
 *  never saved. */
export function tabKind(t: { name: string | null; saved: boolean }, deployed: (name: string) => boolean): "deployed" | "saved" | "draft" {
  if (t.name !== null && deployed(t.name)) return "deployed";
  return t.saved ? "saved" : "draft";
}

/**
 * What opening a saved flow from the flows page does, given the open tabs: go to its tab when it
 * is open (`"switch"`: its unsaved changes are never thrown away by a plain open; "reload saved"
 * is offered beside it for that), or open it in a new tab (`"load"`).
 */
export function openChoice(target: string, tabs: { name: string | null }[]): "switch" | "load" {
  return tabs.some((t) => t.name === target) ? "switch" : "load";
}

/** Which tab to show when `closing` (the active one) closes: the one before it, else after. */
export function nextAfterClose(ids: number[], closing: number): number | null {
  const at = ids.indexOf(closing);
  const rest = ids.filter((id) => id !== closing);
  if (rest.length === 0) return null;
  return rest[Math.max(0, at - 1)];
}
