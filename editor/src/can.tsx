// What whoever is signed in may do, as the server's `can` answers it (`GET /auth/me/can`), for the
// editor's buttons and pages to follow (web-console-L4.md §3, principle 6; the access plan's step
// 7c): a button the server would refuse is disabled, its refusal its title; a page shown only to
// whoever may view it is left out of the nav for the others. The server still decides: this says
// beforehand what it would say.
//
// Each button names what it sends (BUTTONS: its route, and the action that route asks, as can.rs's
// route table has it; can.test.tsx holds the two together). Every action is asked at once, once
// per flow (and once with none: the host's actions, and the others on any flow, as the lists ask
// them), however many buttons follow it, and kept until it may have changed:
// - the access document's version moved (`/auth/me`'s `access_version`, read again now and then
//   while anything follows it, and on coming back to the window): a binding made or taken away;
// - a request was refused (what was kept is out of date), or something was changed from here (a
//   binding, a deploy that tags a flow): server.ts's `mayHaveChanged` says which;
// - a minute went by: what the version doesn't move with, a flow tagged by someone else's deploy;
// - someone else signed in: then none of it is theirs.
// While it is read again, what was kept stays shown. Before it is first read, every button it gates
// is disabled, saying it is asking (ASKING): it fails closed. It used to fail open (nothing
// disabled until the server answered, so a button didn't flicker for those who may), and on a
// slow machine a viewer was shown Save enabled for the 15 s the e2e waited (accounts.e2e.ts): a
// button said she may do what she may not. Now those who may see it enabled a moment later.
// A read that gets no answer is asked again shortly, not a round later: a minute of buttons
// waiting on an answer is too long.

import { useEffect, useMemo, useState, useSyncExternalStore, type ReactNode } from "react";
import { listOwned, mayI, whoAmI, type Kind } from "./server";

/** The catalogue (can.rs's `Action::ALL`), in its order; can.test.tsx holds it to the server's. */
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
  "run.steps",
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
/** What `can` answers for an action: true, or the words it refuses in. */
export type Answer = true | string;
/** For each action, its answer. */
export type Answers = Partial<Record<Action, Answer>>;

/** Each button that changes something, and each page whoever may view it is shown: the route it
 *  sends (or the routes, one for each kind it is shown for) and the action that route asks (can.rs's
 *  ROUTES: can.test.tsx holds each to it), and `also`, the second a route asking both asks.
 *  `inside`: what the route's handler asks beyond the route's own action. */
export const BUTTONS = {
  // ── a flow, in the editor ──
  /** Save it. */
  save: { route: "POST /flows", action: "flow.edit" },
  /** Deploy (or Update) it, as it is now and as the version would make it (deploy.rs). */
  deploy: { route: "POST /deploy", action: "flow.propose" },
  /** Deploy a version that asks for nothing new, without a proposal (deploy.rs). */
  deployNow: { route: "POST /deploy", action: "flow.deploy", inside: true },
  /** Approve what a version asks for, and deploy it: deploy.rs's `may_approve`; without it the
   *  version waits as a proposal. */
  approve: { route: "POST /deploy", action: "flow.approve-deploy", inside: true },
  decline: { route: "POST /proposals/{id}/decline", action: "flow.approve-deploy" },
  /** Send a request to the deployed server (Try). */
  request: { route: "POST /deployments/{slug}/request", action: "run.call" },
  /** Run a deployment's action by hand. */
  runAction: { route: "POST /trigger/{slug}/{name}", action: "run.call" },
  /** Tick one of its clocks now. */
  tick: { route: "POST /deployments/{slug}/tick", action: "flow.lifecycle" },
  // ── a deployment ──
  /** Run, resume, run from a session. */
  run: { route: "POST /deployments/{slug}/run", action: "flow.lifecycle" },
  pause: { route: "POST /deployments/{slug}/pause", action: "flow.lifecycle" },
  stop: { route: "POST /deployments/{slug}/stop", action: "flow.lifecycle" },
  /** Remove a deployment, every session and its state. */
  remove: { route: "DELETE /deployments/{slug}", action: "flow.delete" },
  /** Revoke, or grant back, what its version was granted; keep it running. */
  grant: { route: "POST /deployments/{slug}/grant", action: "flow.approve-deploy" },
  /** Its settings: a limit lowered, whether agents may propose its versions. */
  settings: { route: "POST /deployments/{slug}/settings", action: "flow.deploy" },
  /** Go back to a point in its history: the route asks for a dry run, its handler for going back. */
  restore: { route: "POST /deployments/{slug}/restore", action: "flow.restore", inside: true },
  /** Save its runs and the units they ran, for `holon verify`: their steps and values, and the
   *  evidence made of them (R55). */
  export: { route: "GET /deployments/{slug}/export", action: "run.steps", also: "run.export" },
  setSecret: { route: "POST /deployments/{slug}/secrets/{name}", action: "secret.set" },
  removeSecret: { route: "DELETE /deployments/{slug}/secrets/{name}", action: "secret.set" },
  rollBackSecret: { route: "POST /deployments/{slug}/secrets/{name}/rollback", action: "secret.roll-back" },
  // ── saved flows ──
  /** Delete a saved flow: the handler asks of the flow once it knows whose it is (flows.rs). */
  deleteSaved: { route: "DELETE /flows/{slug}", action: "flow.delete" },
  // ── the host ──
  /** Bind a database, a folder, mail, a feed or a queue; and pick a folder to (Resources). */
  bindResource: {
    route: ["POST /admin/databases", "POST /admin/folders", "GET /admin/folders/browse", "POST /admin/mails", "POST /admin/feeds", "POST /admin/queues"],
    action: "resource.bind",
  },
  unbindResource: {
    route: ["DELETE /admin/databases/{name}", "DELETE /admin/folders/{name}", "DELETE /admin/mails/{name}", "DELETE /admin/feeds/{name}", "DELETE /admin/queues/{name}"],
    action: "resource.unbind",
  },
  /** The server's own secrets (Resources). */
  setServerSecret: { route: "POST /admin/secrets/{name}", action: "resource.bind" },
  removeServerSecret: { route: "DELETE /admin/secrets/{name}", action: "resource.unbind" },
  rollBackServerSecret: { route: "POST /admin/secrets/{name}/rollback", action: "resource.bind" },
  capServerSecret: { route: "POST /admin/secrets/{name}/cap", action: "resource.bind" },
  /** The host's storage settings, and its sweeps. */
  operate: { route: ["POST /admin/storage", "POST /admin/store/sweep", "POST /admin/folders/sweep"], action: "host.operate" },
  /** Check every run log again (the overview). */
  check: { route: "POST /summary/check", action: "host.check" },
  /** A removed flow's keys forgotten with it (Remove's checkbox): its kept records unreadable. */
  forgetKeys: { route: "POST /admin/keys/forget", action: "host.operate" },
  /** Read holon.toml again (Policy). */
  reloadPolicy: { route: "POST /permissions/reload", action: "policy.edit" },
  // ── people (People and access) ──
  /** Add a person (an invite) or an agent or a service. */
  addPrincipal: { route: "POST /people", action: "people.manage" },
  /** Give a role on the host, adding someone or changing theirs: binding it asks people.assign
   *  in the handler (auth.rs's `may_give`). */
  giveRole: { route: ["POST /people", "PATCH /people/{name}"], action: "people.assign", inside: true },
  /** Change someone: their email, disabled, an agent's owner (a role, `giveRole` too). */
  changePrincipal: { route: "PATCH /people/{name}", action: "people.manage" },
  resetPerson: { route: "POST /people/{name}/reset", action: "people.manage" },
  removePrincipal: { route: "DELETE /people/{name}", action: "people.manage" },
  /** Make or revoke an agent's or a service's tokens: its owner may too (can.rs's `owns`), which
   *  the host's answer can't say (`mayTokens` in PeoplePages). */
  tokens: { route: ["POST /people/{name}/tokens", "DELETE /people/{name}/tokens/{id}"], action: "people.manage" },
  /** The identity provider people sign in through. */
  oidc: { route: "POST /admin/oidc", action: "people.manage" },
  /** Make, change or remove a group (its members too). */
  groups: { route: ["PUT /groups/{name}", "DELETE /groups/{name}"], action: "people.assign" },
  /** Define, change or remove a role. */
  roles: { route: ["PUT /roles/{name}", "DELETE /roles/{name}"], action: "role.define" },
  /** Bind a role (People and access's form). */
  bind: { route: "POST /bindings", action: "people.assign" },
  unbind: { route: "DELETE /bindings/{id}", action: "people.assign" },
  /** Approve, or decline, an access change that waits for people (Approvals' Access changes). */
  approveAccess: { route: "POST /access/proposals/{id}/approve", action: "people.assign" },
  declineAccess: { route: "POST /access/proposals/{id}/decline", action: "people.assign" },
  // ── the host's pages ──
  /** Resources, and the names of the server's env variables a secret may come from. */
  resources: { route: ["GET /admin/databases", "GET /admin/secrets/env"], action: "resource.view" },
  people: { route: "GET /people", action: "people.view" },
  policy: { route: "GET /permissions", action: "policy.view" },
  /** The host's own state: the overview's admin parts. */
  host: { route: "GET /admin/host", action: "host.view" },
} as const satisfies Record<string, { route: string | readonly string[]; action: Action; also?: Action; inside?: true }>;
export type Button = keyof typeof BUTTONS;

/** A control's own reasons to be disabled, and its own title. */
type Own = { disabled?: boolean; title?: string };

/** A control's props for an answer: disabled, with the refusal as its title, when refused; else
 *  as `own` says. */
export function gateOf(answer: Answer, own: Own = {}): { disabled: boolean; title?: string } {
  return answer === true ? { disabled: own.disabled ?? false, title: own.title } : { disabled: true, title: answer };
}

/** What whoever is signed in may do, for the buttons to follow. */
export type Can = {
  /** True, or the words the server would refuse `button` in; ASKING until it has answered. */
  may: (button: Button) => Answer;
  /** Whether the server said `button` is theirs: for what is shown only to whoever holds it. */
  holds: (button: Button) => boolean;
  /** `gateOf` for `button`. */
  gate: (button: Button, own?: Own) => { disabled: boolean; title?: string };
  /** Whether the server has answered. */
  known: boolean;
};

/** What a button says while the server hasn't answered what whoever is signed in may do: it is
 *  disabled until it has (fails closed; the file's head says why). */
export const ASKING = "a moment: asking the server what you may do";

/** A `Can` from the server's answers (null while it hasn't answered). */
export function canOf(answers: Answers | null): Can {
  // Each action the button's route asks, the first refused saying why.
  const asked = (button: Button): Action[] => {
    const b: { action: Action; also?: Action } = BUTTONS[button];
    return b.also ? [b.action, b.also] : [b.action];
  };
  const may = (button: Button): Answer =>
    answers === null ? ASKING : (asked(button).map((a) => answers[a] ?? true).find((said) => said !== true) ?? true);
  return { may, holds: (button) => asked(button).every((a) => answers?.[a] === true), gate: (button, own) => gateOf(may(button), own), known: answers !== null };
}

/** A form, or a group of controls, that needs `may`: every control in it disabled, with the
 *  refusal as their title, when refused. It draws no box of its own. */
export function Gated({ may, children }: { may: Answer; children: ReactNode }) {
  const refused = may !== true;
  return (
    <fieldset className="hl-gated" disabled={refused} title={refused ? may : undefined}>
      {children}
    </fieldset>
  );
}

// ── what is kept ─────────────────────────────────────────────────────────────────────────────
/** Each flow's answers ("" for none), with the round they were read in and when they were asked. */
const kept = new Map<string, { answers: Answers; round: number; asked: number }>();
/** The flows being read, by round. */
const asking = new Set<string>();
/** Moves on each time what was kept may be out of date. */
let round = 0;
/** Counts the reads sent, of answers and of the access version: which was asked before which. */
let sent = 0;
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

/** How long after a read that got no answer it is asked again: twice as long each time after,
 *  up to how often the access version is read. */
export const RETRY_MS = 1_000;
/** Each flow's wait before it is read again, while its reads get no answer. */
const retries = new Map<string, number>();

function read(flow: string): void {
  const now = kept.get(flow);
  const tag = `${round}:${flow}`;
  if ((now && now.round === round) || asking.has(tag)) return;
  asking.add(tag);
  const at = round;
  const asked = ++sent;
  mayI([...ACTIONS], flow ? { flow } : {})
    .then(
      (answers) => {
        retries.delete(flow);
        if (at !== round) return;
        kept.set(flow, { answers: answers as Answers, round: at, asked });
        tell();
      },
      // Not read: kept as it was (asking, before a first answer), and asked again shortly while
      // a button follows it and nothing has read it since.
      () => {
        const wait = retries.get(flow) ?? RETRY_MS;
        retries.set(flow, Math.min(wait * 2, VERSION_EVERY_MS));
        setTimeout(() => {
          if (listeners.size > 0 && at === round) read(flow);
        }, wait);
      },
    )
    .finally(() => asking.delete(tag));
}

// ── the access version, read again while something follows it ───────────────────────────────
/** How often the access version is read while a button follows `can`. */
export const VERSION_EVERY_MS = 15_000;
/** How often what is kept is read again however the version stands. */
export const AGAIN_EVERY_MS = 60_000;
let accessVersion: number | null = null;
/** When the version kept was asked: an answer to a read asked before it is older, and dropped. */
let versionAsked = 0;
let timers: ReturnType<typeof setInterval>[] = [];

/** Read `/auth/me`'s access version: moved since last read, what was kept is forgotten. The
 *  first one answered (the page's first, or one after a sign-in) has nothing to have moved from:
 *  what was kept is forgotten when it was asked before this read was, access may have moved
 *  between the two (the page's first version read held up, a binding made, a later one
 *  answering). Reads asked at once at the start ask the version first, and keep what follows. */
export async function checkVersion(): Promise<void> {
  const asked = ++sent;
  try {
    const v = (await whoAmI()).access_version;
    if (v === undefined || asked < versionAsked) return;
    const moved = accessVersion === null ? [...kept.values()].some((k) => k.asked < asked) : v !== accessVersion;
    accessVersion = v;
    versionAsked = asked;
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
    timers = [setInterval(() => void checkVersion(), VERSION_EVERY_MS), setInterval(onChanged, AGAIN_EVERY_MS)];
    window.addEventListener("focus", onFocus);
    window.addEventListener("holon:can", onChanged);
    window.addEventListener("holon:session", onSession);
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) {
      timers.forEach(clearInterval);
      timers = [];
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("holon:can", onChanged);
      window.removeEventListener("holon:session", onSession);
    }
  };
}

/** What whoever is signed in may do on `flow` (with none: the host's actions, and the others on
 *  any flow), as the server answers. Every action at once, read once per flow and kept, however
 *  many buttons follow it. */
export function useCan(flow?: string | null): Can {
  const key = flow ?? "";
  const at = useSyncExternalStore(subscribe, () => round);
  const answers = useSyncExternalStore(subscribe, () => kept.get(key)?.answers ?? null);
  useEffect(() => read(key), [key, at]);
  return useMemo(() => canOf(answers), [answers]);
}

/** What whoever is signed in may do on `flow` now, outside a render (deploying under a name just
 *  typed, a key pressed): what was kept this round, else read and kept. When it can't be read,
 *  everything may: the server decides. */
export async function canOn(flow: string): Promise<Can> {
  const now = kept.get(flow);
  if (now && now.round === round) return canOf(now.answers);
  const at = round;
  const asked = ++sent;
  try {
    const answers = (await mayI([...ACTIONS], flow ? { flow } : {})) as Answers;
    if (at === round) {
      kept.set(flow, { answers, round: at, asked });
      tell();
    }
    return canOf(answers);
  } catch {
    // A click already made: the server decides, and says why when it refuses.
    return canOf({});
  }
}

/** For tests: nothing kept, no round, no version. */
export function resetCan(): void {
  kept.clear();
  asking.clear();
  retries.clear();
  round = 0;
  sent = 0;
  accessVersion = null;
  versionAsked = 0;
}

// ── the host's pages ─────────────────────────────────────────────────────────────────────────
/** The host's pages whoever is signed in is shown (the nav's host group, the palette, the pages
 *  themselves): Resources with `resource.view`, Policy with `policy.view`, People and access with
 *  `people.view`, everyone's; or, without it, theirs, to someone who owns an agent or a service. */
export type HostPages = { resources: boolean; people: "everyone" | "owned" | null; policy: boolean; host: boolean };

export function hostPagesOf(can: Pick<Can, "holds">, owns: boolean): HostPages {
  return {
    resources: can.holds("resources"),
    people: can.holds("people") ? "everyone" : owns ? "owned" : null,
    policy: can.holds("policy"),
    host: can.holds("host"),
  };
}

/** The host's pages `me` is shown, and the `Can` they follow (for why a page isn't). */
export function useHostPages(me: { name: string; kind: Kind }): HostPages & { can: Can } {
  const can = useCan();
  // Whether they own an agent or a service (`GET /auth/owned`): a person only.
  const [owns, setOwns] = useState(false);
  useEffect(() => {
    let live = true;
    if (me.kind !== "person") {
      setOwns(false);
      return;
    }
    listOwned().then(
      (o) => live && setOwns(o.length > 0),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [me.name, me.kind]);
  return useMemo(() => ({ ...hostPagesOf(can, owns), can }), [can, owns]);
}
