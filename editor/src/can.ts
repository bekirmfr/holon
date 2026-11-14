// What whoever is signed in may do, as the server's `can` answers it (`GET /auth/me/can`), for the
// buttons to follow (the access plan's step 7c): a button the server would refuse is disabled, its
// refusal its title. The server still decides; this says beforehand what it would say.
//
// Every action is asked at once, once per flow (and once with none: the host's actions, and the
// others on any flow, as the lists ask), and kept until it may have changed: the access document's
// version moved (`/auth/me`'s `access_version`, read again now and then while a button follows
// it), a request was refused (what was kept is out of date), something was changed from here (a
// binding, a deploy that tags a flow), or someone else signed in. While it is read again, what was
// kept stays shown; before it is first read, nothing is disabled.

import { useEffect, useSyncExternalStore } from "react";
import { mayI, whoAmI } from "./server";

/** The catalogue (can.rs's `Action::ALL`), in its order; can.test.ts holds it to the server's. */
export const ACTIONS = [
  "flow.view",
  "flow.edit",
  "flow.propose",
  "flow.deploy",
  "flow.approve-deploy",
  "flow.restore",
  "flow.restore-dry-run",
  "flow.lifecycle",
  "flow.delete",
  "run.view",
  "run.call",
  "run.decide",
  "run.export",
  "state.view",
  "state.write",
  "secret.list",
  "secret.set",
  "secret.roll-back",
  "resource.view",
  "resource.bind",
  "resource.unbind",
  "people.view",
  "people.manage",
  "people.assign",
  "role.view",
  "role.define",
  "policy.view",
  "policy.edit",
  "host.view",
  "host.check",
  "host.operate",
] as const;
export type Action = (typeof ACTIONS)[number];
/** For each action, true, or the words the server would refuse it in. */
export type Can = Partial<Record<Action, true | string>>;

/** Each button that changes something (or page an action opens), the route it sends and the
 *  action that route asks (can.rs's ROUTES: can.test.ts holds each to it). `inside`: what the
 *  route's handler asks beyond the route's own action. */
export const BUTTONS = {
  /** Deploy (or Update) a flow: as it is now and as the version would make it (deploy.rs). */
  deploy: { route: "POST /deploy", action: "flow.propose" },
  /** Approve what a version asks for, and deploy it: deploy.rs's `may_approve`; without it the
   *  version waits as a proposal. */
  approve: { route: "POST /deploy", action: "flow.approve-deploy", inside: true },
  /** Run, resume, Run from it. */
  run: { route: "POST /deployments/{slug}/run", action: "flow.lifecycle" },
  pause: { route: "POST /deployments/{slug}/pause", action: "flow.lifecycle" },
  stop: { route: "POST /deployments/{slug}/stop", action: "flow.lifecycle" },
  /** Remove a deployment, every session and its state. */
  remove: { route: "DELETE /deployments/{slug}", action: "flow.delete" },
  /** Delete a saved flow: the handler asks of the flow once it knows whose it is (flows.rs). */
  deleteSaved: { route: "DELETE /flows/{slug}", action: "flow.delete" },
  decline: { route: "POST /proposals/{id}/decline", action: "flow.approve-deploy" },
  /** Revoke, grant back, lower a limit, keep running. */
  grant: { route: "POST /deployments/{slug}/grant", action: "flow.approve-deploy" },
  /** Whether agents may propose its versions. */
  agentsPropose: { route: "POST /deployments/{slug}/settings", action: "flow.deploy" },
  setSecret: { route: "POST /deployments/{slug}/secrets/{name}", action: "secret.set" },
  removeSecret: { route: "DELETE /deployments/{slug}/secrets/{name}", action: "secret.set" },
  rollBackSecret: { route: "POST /deployments/{slug}/secrets/{name}/rollback", action: "secret.roll-back" },
  /** The server's own secrets (Resources). */
  setServerSecret: { route: "POST /admin/secrets/{name}", action: "resource.bind" },
  removeServerSecret: { route: "DELETE /admin/secrets/{name}", action: "resource.unbind" },
  rollBackServerSecret: { route: "POST /admin/secrets/{name}/rollback", action: "resource.bind" },
  capServerSecret: { route: "POST /admin/secrets/{name}/cap", action: "resource.bind" },
  /** Read holon.toml again (Policy). */
  reloadPolicy: { route: "POST /permissions/reload", action: "policy.edit" },
  /** Run a deployment's action by hand. */
  runAction: { route: "POST /trigger/{slug}/{name}", action: "run.call" },
  /** Bind a role (People and access's form). */
  bind: { route: "POST /bindings", action: "people.assign" },
  unbind: { route: "DELETE /bindings/{id}", action: "people.assign" },
  /** Approve, or decline, an access change that waits for people (Approvals' Access changes). */
  approveAccess: { route: "POST /access/proposals/{id}/approve", action: "people.assign" },
  declineAccess: { route: "POST /access/proposals/{id}/decline", action: "people.assign" },
  /** The host's pages. */
  people: { route: "GET /people", action: "people.view" },
  resources: { route: "GET /admin/databases", action: "resource.view" },
  policy: { route: "GET /permissions", action: "policy.view" },
} as const satisfies Record<string, { route: string; action: Action; inside?: true }>;
export type Button = keyof typeof BUTTONS;

/** Why `button` would be refused, as the server words it; undefined while it may, or while that
 *  isn't known yet. */
export function refusal(can: Can | null, button: Button): string | undefined {
  const said = can?.[BUTTONS[button].action];
  return typeof said === "string" ? said : undefined;
}

/** A button's `disabled` and `title` as `can` has them: refused, disabled with the refusal; else
 *  `off` and `title` as they were. */
export function gate(can: Can | null, button: Button, off = false, title?: string): { disabled: boolean; title?: string } {
  const no = refusal(can, button);
  return no === undefined ? { disabled: off, title } : { disabled: true, title: no };
}

// ── what is kept ─────────────────────────────────────────────────────────────────────────────
/** Each flow's answer ("" for none), with the round it was read in. */
const kept = new Map<string, { can: Can; round: number }>();
/** The flows being read, by round. */
const asking = new Set<string>();
/** Moves on each time what was kept may be out of date. */
let round = 0;
const listeners = new Set<() => void>();
const tell = () => listeners.forEach((l) => l());

/** What was kept may be out of date: read it again where it is shown, showing it meanwhile.
 *  `wholly`: someone else signed in, so none of it is theirs. */
export function forgetCan(wholly = false): void {
  round += 1;
  if (wholly) {
    kept.clear();
    accessVersion = null;
  }
  tell();
}

function read(flow: string): void {
  const now = kept.get(flow);
  const tag = `${round}:${flow}`;
  if ((now && now.round === round) || asking.has(tag)) return;
  asking.add(tag);
  const at = round;
  mayI([...ACTIONS], flow ? { flow } : {})
    .then(
      (can) => {
        if (at !== round) return;
        kept.set(flow, { can: can as Can, round: at });
        tell();
      },
      // Not read: kept as it was (nothing disabled before a first answer), asked again next round.
      () => undefined,
    )
    .finally(() => asking.delete(tag));
}

// ── the access version, read again while something follows it ───────────────────────────────
/** How often the access version is read while a button follows `can`. */
export const VERSION_EVERY_MS = 15_000;
let accessVersion: number | null = null;
let timer: ReturnType<typeof setInterval> | null = null;

/** Read `/auth/me`'s access version: moved since last read, what was kept is forgotten. */
export async function checkVersion(): Promise<void> {
  try {
    const v = (await whoAmI()).access_version;
    if (v === undefined) return;
    const moved = accessVersion !== null && v !== accessVersion;
    accessVersion = v;
    if (moved) forgetCan();
  } catch {
    // signed out, or the server away: read again next time
  }
}

const onFocus = () => void checkVersion();
const onChanged = () => forgetCan();
const onSession = () => forgetCan(true);

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  if (listeners.size === 1) {
    void checkVersion();
    timer = setInterval(() => void checkVersion(), VERSION_EVERY_MS);
    window.addEventListener("focus", onFocus);
    window.addEventListener("holon:can", onChanged);
    window.addEventListener("holon:session", onSession);
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) {
      if (timer !== null) clearInterval(timer);
      timer = null;
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("holon:can", onChanged);
      window.removeEventListener("holon:session", onSession);
    }
  };
}

/** The round what is kept was read in, followed as `useCan` follows it (the access version read
 *  again now and then and on coming back to the window): it moves when what was kept may be out of
 *  date, for useCan.tsx's answers to be asked again then too. */
export function useCanRound(): number {
  return useSyncExternalStore(subscribe, () => round);
}

/** What whoever is signed in may do on `flow` (with none: the host's actions, and the others on
 *  any flow), as the server answers; null until first read. Every action at once, read once per
 *  flow and kept, however many buttons follow it. */
export function useCan(flow?: string | null): Can | null {
  const key = flow ?? "";
  const at = useSyncExternalStore(subscribe, () => round);
  const can = useSyncExternalStore(subscribe, () => kept.get(key)?.can ?? null);
  useEffect(() => read(key), [key, at]);
  return can;
}

/** What whoever is signed in may do on `flow` now, outside a render (deploying under a name just
 *  typed): the answer kept this round, else read and kept. Null when it can't be read: the server
 *  decides. */
export async function canOn(flow: string): Promise<Can | null> {
  const now = kept.get(flow);
  if (now && now.round === round) return now.can;
  const at = round;
  try {
    const can = (await mayI([...ACTIONS], flow ? { flow } : {})) as Can;
    if (at === round) {
      kept.set(flow, { can, round: at });
      tell();
    }
    return can;
  } catch {
    return null;
  }
}

/** For tests: nothing kept, no round, no version. */
export function resetCan(): void {
  kept.clear();
  asking.clear();
  round = 0;
  accessVersion = null;
}
