// Who holds what, as the People pages show it (bindings.rs): each binding in a few words, as
// `holon people` lists them, a principal's own role on the host, and the roles a kind may hold,
// the ones an admin defined included (`GET /roles`); where a binding holds and until when, as the
// server and `holon bind` say and read them; what a principal's bindings let it do on the host
// and on each flow, worked out as `can` decides it (the plan's D2: `/auth/me/can` answers only
// for whoever asks); what a version of the access document changed; and the catalogue, by what
// each action acts on.

import type { AccessDocument, AccessProposal, BindingEntry, HeldBinding, Kind, Principal, RoleEntry, Scope, Subject } from "./server";

/** Where a binding holds, as the flags write it: nothing for the host, ` on payout`,
 *  ` on tag payments`, ` at payout:ok`. */
export function placeSaid(scope: Scope): string {
  if (scope === "host") return "";
  if ("flows" in scope) return "tag" in scope.flows ? ` on tag ${scope.flows.tag}` : ` on ${scope.flows.slug}`;
  return ` at ${scope.approval.flow.slug}:${scope.approval.name}`;
}

/** A moment, in UTC as the CLI says it: `2026-12-01 09:00 UTC`. */
export const utc = (seconds: number) => `${new Date(seconds * 1000).toISOString().slice(0, 16).replace("T", " ")} UTC`;

/** A binding in a few words: `editor on payout via payments until 2026-12-01 09:00 UTC (expired)`. */
export function bindingSaid(b: HeldBinding): string {
  const via = b.group ? ` via ${b.group}` : "";
  const until = b.expires_at === null ? "" : ` until ${utc(b.expires_at)}`;
  return `${b.role}${placeSaid(b.scope)}${via}${until}${b.expired ? " (expired)" : ""}`;
}

/** Whether `b` is the role bound to its principal on the host itself, not through a group, and
 *  still holding: the one changing its role sets (`PATCH /people`). */
const isOwnHostRole = (b: HeldBinding) => b.scope === "host" && b.group === null && !b.expired;

/** `p`'s own role on the host; "" when none is bound to it there. */
export const hostRoleOf = (p: Principal) => p.bindings.find(isOwnHostRole)?.role ?? "";

/** What else holds for `p`: each binding but its own role on the host. */
export const otherBindings = (p: Principal) => {
  const own = p.bindings.find(isOwnHostRole);
  return p.bindings.filter((b) => b !== own);
};

/** The roles a principal of `kind` may hold, the built-in ones first. */
export const rolesFor = (roles: RoleEntry[], kind: Kind) => roles.filter((r) => r.kinds.includes(kind));

/** What a role is for, as an option's title says it: what an admin wrote, then what it allows. */
export function roleTitle(roles: RoleEntry[], name: string): string {
  const r = roles.find((r) => r.name === name);
  if (!r) return "";
  return r.builtin ? (r.about ?? "") : [r.about, `allows ${r.allows.join(", ")}`].filter(Boolean).join(": ");
}

// ── where, to whom, until when ────────────────────────────────────────────────────────────────

const quoted = (s: string) => JSON.stringify(s);

/** Where a binding holds, in the server's words (bindings.rs `Scope::said`): `on this host`,
 *  `on "payout"`, `on the flows tagged "payments"`, `for "payout"'s approval "ok"`. */
export function scopeSaid(scope: Scope): string {
  if (scope === "host") return "on this host";
  if ("flows" in scope) return "tag" in scope.flows ? `on the flows tagged ${quoted(scope.flows.tag)}` : `on ${quoted(scope.flows.slug)}`;
  return `for ${quoted(scope.approval.flow.slug)}'s approval ${quoted(scope.approval.name)}`;
}

/** Whom a binding is to, in a version's words (`Subject::said`): `bob`, `the group payments`. */
export const subjectSaid = (who: Subject) => ("principal" in who ? who.principal : `the group ${who.group}`);

/** The flow a scope names by its slug: none for the host or a tag. */
export const flowOf = (scope: Scope): string | null => (scope === "host" ? null : "flows" in scope ? ("slug" in scope.flows ? scope.flows.slug : null) : scope.approval.flow.slug);

/** When a binding stops holding, typed as `holon bind --until` reads it (accesscmd.rs `until`): a
 *  while from `now` (`30m`, `12h`, `7d`, `2w`), or a moment in UTC (`2026-12-01`,
 *  `2026-12-01T09:00`, with its seconds too), in seconds. Null when nothing is typed: it holds
 *  until it is taken away. */
export function untilOf(text: string, now: number): { at: number } | { error: string } | null {
  const t = text.trim();
  if (t === "") return null;
  const wrong = { error: `until is a date in UTC (2026-12-01, 2026-12-01T09:00) or a while (30m, 12h, 7d, 2w), not ${quoted(t)}` };
  const unit = /^(\d+)([a-zA-Z])$/.exec(t);
  if (unit) {
    const seconds = ({ m: 60, h: 3_600, d: 86_400, w: 7 * 86_400 } as Record<string, number>)[unit[2]];
    return seconds === undefined ? wrong : { at: now + Number(unit[1]) * seconds };
  }
  const m = /^(\d+)-(\d+)-(\d+)(?:T(\d+):(\d+)(?::(\d+))?)?$/.exec(t);
  if (!m) return wrong;
  const [y, mo, d, h, min, s] = [m[1], m[2], m[3], m[4] ?? "0", m[5] ?? "0", m[6] ?? "0"].map(Number);
  if (mo < 1 || mo > 12 || d < 1 || d > 31 || h > 23 || min > 59 || s > 59) return wrong;
  // Days since 1970, as the CLI counts them (Hinnant's days_from_civil).
  const [yy, mm] = mo <= 2 ? [y - 1, mo + 9] : [y, mo - 3];
  const era = Math.floor(yy / 400);
  const yoe = yy - era * 400;
  const doy = Math.floor((153 * mm + 2) / 5) + d - 1;
  const days = era * 146_097 + yoe * 365 + Math.floor(yoe / 4) - Math.floor(yoe / 100) + doy - 719_468;
  const at = days * 86_400 + h * 3_600 + min * 60 + s;
  return at < 0 ? wrong : { at };
}

// ── what bindings give ────────────────────────────────────────────────────────────────────────

/** A flow as access sees it: its slug, and the tags its deployed version has. */
export type Tagged = { slug: string; tags: string[] };

/** What a principal may do, from what holds for it, as `can` decides (can.rs). */
export type Gives = {
  /** The host's own actions (its people, roles, resources, policy and itself): bound on the host. */
  host: string[];
  /** A flow's actions on every flow, those deployed later too: bound on the host. */
  everyFlow: string[];
  /** What it holds only on the flows it saved itself: the agent role's `flow.edit`. */
  own: string[];
  /** Each tag it is bound on, and what that gives on every flow tagged so, later ones too. */
  tags: { tag: string; actions: string[] }[];
  /** Each flow it holds something on, deployed or named by a binding: everything it may do there
   *  (`everyFlow` included), and the approval points it decides there by a binding at one. */
  flows: { slug: string; actions: string[]; decides: string[] }[];
};

const resourceOf = (action: string) => action.split(".")[0];
/** What a binding on the host holds the host for, and what one on a flow or a tag holds it for
 *  (can.rs `Resource::takes`). */
const OF_HOST = ["resource", "people", "role", "policy", "host"];
const OF_FLOW = ["flow", "run", "state", "secret"];
/** What is a person's, whatever role an agent holds (can.rs `a_persons`). */
const A_PERSONS = ["flow.approve-deploy", "flow.lifecycle", "flow.delete", "flow.restore", "secret.set", "secret.roll-back", "host.operate"];
/** The host's fixed rules, past what roles hold (can.rs `fixed`): an agent does nothing that is a
 *  person's, and a service decides no approval. */
export const fixedRefuses = (kind: Kind, action: string) =>
  (kind === "agent" && (A_PERSONS.includes(action) || ["people", "role", "resource"].includes(resourceOf(action)))) || (kind === "service" && action === "run.decide");
/** What a built-in role holds only on its holder's own flows (can.rs `Own`): the agent role saves
 *  only the flows it saved. */
const OWN_ONLY: Record<string, string[]> = { agent: ["flow.edit"] };

/** Whether a scope holds on the flow `slug`, deployed with `tags`, for a flow's actions: the
 *  host, the flow by its slug, or a tag it has. An approval point's holds only for deciding it. */
export const coversFlow = (scope: Scope, slug: string, tags: string[]) =>
  scope === "host" || ("flows" in scope && ("slug" in scope.flows ? scope.flows.slug === slug : tags.includes(scope.flows.tag)));

/** What a principal of `kind` may do with `bindings` (its own and its groups', `GET /people`), as
 *  `can` decides it: a binding that has expired, or of a role its kind may not hold, gives
 *  nothing; one on the host holds the host and every flow; on a flow, that flow; on a tag, every
 *  flow deployed with it; at an approval point, deciding it; and the host's fixed rules hold over
 *  any role. Each list in the catalogue's order. */
export function givesOf(kind: Kind, bindings: HeldBinding[], roles: RoleEntry[], flows: Tagged[]): Gives {
  const live = bindings.filter((b) => !b.expired && (roles.find((r) => r.name === b.role)?.kinds ?? []).includes(kind));
  const order = catalogue(roles);
  const rank = (a: string) => (order.includes(a) ? order.indexOf(a) : order.length);
  const sorted = (actions: string[]) => [...new Set(actions)].sort((a, b) => rank(a) - rank(b));
  const held = (b: HeldBinding, of: string[]) => b.allows.filter((a) => of.includes(resourceOf(a)) && !fixedRefuses(kind, a) && !(OWN_ONLY[b.role] ?? []).includes(a));
  const onHost = live.filter((b) => b.scope === "host");
  const everyFlow = sorted(onHost.flatMap((b) => held(b, OF_FLOW)));
  const tagOf = (s: Scope) => (s !== "host" && "flows" in s && "tag" in s.flows ? s.flows.tag : null);
  const tags = [...new Set(live.flatMap((b) => tagOf(b.scope) ?? []))]
    .sort()
    .map((tag) => ({ tag, actions: sorted(live.filter((b) => tagOf(b.scope) === tag).flatMap((b) => held(b, OF_FLOW))) }))
    .filter((t) => t.actions.length > 0);
  const slugs = [...new Set([...flows.map((f) => f.slug), ...live.flatMap((b) => flowOf(b.scope) ?? [])])].sort();
  const each = slugs.map((slug) => {
    const tagged = flows.find((f) => f.slug === slug)?.tags ?? [];
    const actions = sorted(live.filter((b) => coversFlow(b.scope, slug, tagged)).flatMap((b) => held(b, OF_FLOW)));
    const at = (s: Scope) => (s !== "host" && "approval" in s && s.approval.flow.slug === slug ? s.approval.name : null);
    const decides = fixedRefuses(kind, "run.decide") ? [] : [...new Set(live.filter((b) => b.allows.includes("run.decide")).flatMap((b) => at(b.scope) ?? []))].sort();
    return { slug, actions, decides };
  });
  return {
    host: sorted(onHost.flatMap((b) => held(b, OF_HOST))),
    everyFlow,
    own: sorted(live.flatMap((b) => (OWN_ONLY[b.role] ?? []).filter((a) => b.allows.includes(a) && !fixedRefuses(kind, a)))),
    tags,
    flows: each.filter((f) => f.actions.length > 0 || f.decides.length > 0),
  };
}

/** Whether `who` may view the flow `slug`, so that the editor lists it to them: a principal by
 *  what holds for it; a group by the bindings to it, as `holon bind` asks before it says an
 *  approval point's holder can't see the flow. True when it can't tell (someone not listed). */
export function seesFlow(who: Subject, slug: string, principals: Principal[], bindings: BindingEntry[], roles: RoleEntry[], flows: Tagged[]): boolean {
  const tags = flows.find((f) => f.slug === slug)?.tags ?? [];
  if ("principal" in who) {
    const p = principals.find((p) => p.name === who.principal);
    return !p || (givesOf(p.kind, p.bindings, roles, flows).flows.find((f) => f.slug === slug)?.actions.includes("flow.view") ?? false);
  }
  const allows = (role: string) => roles.find((r) => r.name === role)?.allows ?? [];
  return bindings.some((b) => "group" in b.who && b.who.group === who.group && !b.expired && coversFlow(b.scope, slug, tags) && allows(b.role).includes("flow.view"));
}

/** The approval points a deployed version has, by name: its manifest's `Approval("payout/…")`. */
export const approvalsOf = (items: string[]) => [...new Set(items.flatMap((i) => /^Approval\("([^/"]+)\//.exec(i)?.[1] ?? []))];

// ── the catalogue and roles ───────────────────────────────────────────────────────────────────

/** Every action, in the catalogue's order: the admin role allows each one. */
const catalogue = (roles: RoleEntry[]) => roles.find((r) => r.name === "admin" && r.builtin)?.allows ?? [];

/** `actions` by what each acts on, in the order given: `[{of: "flow", actions: ["flow.view"]}]`. */
function byResource(actions: string[]): { of: string; actions: string[] }[] {
  const out: { of: string; actions: string[] }[] = [];
  for (const a of actions) {
    const last = out[out.length - 1];
    if (last?.of === resourceOf(a)) last.actions.push(a);
    else out.push({ of: resourceOf(a), actions: [a] });
  }
  return out;
}

/** The catalogue by what each action acts on, as a role's form lists it, a checkbox each. */
export const catalogueOf = (roles: RoleEntry[]) => byResource(catalogue(roles));

/** What `allows` holds, by what it acts on: `flow view, edit · run decide`; `everything` for the
 *  whole catalogue. */
export function allowsSaid(allows: string[], roles: RoleEntry[]): string {
  const all = catalogue(roles);
  if (all.length > 0 && all.every((a) => allows.includes(a))) return "everything";
  const rank = (a: string) => (all.includes(a) ? all.indexOf(a) : all.length);
  return byResource([...allows].sort((a, b) => rank(a) - rank(b)))
    .map((r) => `${r.of} ${r.actions.map((a) => a.slice(r.of.length + 1)).join(", ")}`)
    .join(" · ");
}

/** Who holds `role`, and where, by every binding of it still holding: `bob, the group ops on payout`. */
export const heldBy = (bindings: BindingEntry[], role: string) =>
  bindings
    .filter((b) => b.role === role && !b.expired)
    .map((b) => `${subjectSaid(b.who)}${placeSaid(b.scope)}`)
    .join(", ");

// ── a version's changes ───────────────────────────────────────────────────────────────────────

/** One change a version of the access document made, said: a binding, a role or a group added,
 *  removed or changed, or how approvals are decided, or whom the run file signs in as. */
export type AccessChange = { of: "binding" | "role" | "group" | "approvals" | "run file"; change: "added" | "removed" | "changed"; said: string };

/** What a list became: `+ bob, − cy`. */
const plusMinus = (before: readonly string[], after: readonly string[]) =>
  [...after.filter((x) => !before.includes(x)).map((x) => `+ ${x}`), ...before.filter((x) => !after.includes(x)).map((x) => `− ${x}`)].join(", ");

/** What `after` changed from `before`, the version before it (none for the first): bindings by
 *  their ids, which are never reused, roles and groups by their names, then the settings. */
export function changesOf(before: AccessDocument | null, after: AccessDocument): AccessChange[] {
  const out: AccessChange[] = [];
  type Bound = AccessDocument["bindings"][number];
  const boundSaid = (b: Bound) => `${b.role} to ${subjectSaid(b.who)} ${scopeSaid(b.scope)}${b.expires_at ? `, until ${utc(b.expires_at)}` : ""}`;
  const was = before?.bindings ?? [];
  for (const b of was) if (!after.bindings.some((a) => a.id === b.id)) out.push({ of: "binding", change: "removed", said: boundSaid(b) });
  for (const a of after.bindings) {
    const b = was.find((x) => x.id === a.id);
    if (!b) out.push({ of: "binding", change: "added", said: boundSaid(a) });
    else if (boundSaid(b) !== boundSaid(a)) out.push({ of: "binding", change: "changed", said: `${boundSaid(b)} → ${boundSaid(a)}` });
  }
  const roles = before?.roles ?? [];
  for (const r of roles) if (!after.roles.some((x) => x.name === r.name)) out.push({ of: "role", change: "removed", said: r.name });
  for (const r of after.roles) {
    const then = roles.find((x) => x.name === r.name);
    if (!then) {
      out.push({ of: "role", change: "added", said: `${r.name}: allows ${r.allows.join(", ")}; held by ${r.kinds.join(" or ")}` });
      continue;
    }
    const said = [
      plusMinus(then.allows, r.allows) && `allows ${plusMinus(then.allows, r.allows)}`,
      plusMinus(then.kinds, r.kinds) && `held by ${plusMinus(then.kinds, r.kinds)}`,
      (then.about ?? "") !== (r.about ?? "") && "about changed",
    ].filter(Boolean);
    if (said.length > 0) out.push({ of: "role", change: "changed", said: `${r.name}: ${said.join("; ")}` });
  }
  const groups = before?.groups ?? [];
  for (const g of groups) if (!after.groups.some((x) => x.name === g.name)) out.push({ of: "group", change: "removed", said: g.name });
  for (const g of after.groups) {
    const then = groups.find((x) => x.name === g.name);
    if (!then) {
      out.push({ of: "group", change: "added", said: `${g.name}: ${g.members.length > 0 ? g.members.join(", ") : "no one"}` });
      continue;
    }
    const said = [plusMinus(then.members, g.members), (then.about ?? "") !== (g.about ?? "") && "about changed"].filter(Boolean);
    if (said.length > 0) out.push({ of: "group", change: "changed", said: `${g.name}: ${said.join("; ")}` });
  }
  if (before) {
    for (const key of ["exclude_deployer", "agents", "starter"] as const) {
      if (before.approvals[key] !== after.approvals[key]) out.push({ of: "approvals", change: "changed", said: `${key} ${after.approvals[key] ? "on" : "off"}` });
    }
    const [changes, was] = [after.approvals.changes ?? null, before.approvals.changes ?? null];
    if (changes !== was) out.push({ of: "approvals", change: "changed", said: `changes ${changes ?? "off"}` });
  }
  const [local, then] = [after.local ?? null, before?.local ?? null];
  if (local !== then) {
    const said = `the run file signs in as ${local ?? "no one"}${then ? ` (was ${then})` : ""}`;
    out.push({ of: "run file", change: then === null ? "added" : local === null ? "removed" : "changed", said });
  }
  return out;
}

// ── access changes that wait for people ──────────────────────────────────────────────────────

/** "1 person", "2 people". */
const people = (n: number) => (n === 1 ? "1 person" : `${n} people`);

/** A change the server kept as an access change that waits, as `holon bind` says it
 *  (accesscmd.rs `waiting`): "bound approver to dee on this host: waits for 2 people other than
 *  you to approve it (access change 3)". */
export const waitsSaid = (p: AccessProposal) => `${p.what}: waits for ${people(p.needs)} other than you to approve it (access change ${p.id})`;

/** What a change's answer says, if the server kept it as an access change that waits: else null. */
export const waitingOf = (answer: unknown): string | null =>
  typeof answer === "object" && answer !== null && "proposal" in answer ? waitsSaid((answer as { proposal: AccessProposal }).proposal) : null;

/** Where an access change stands, as `holon access proposals` lists it (accesscmd.rs
 *  `proposal_state`): "waits, 1 of 2 approved (bob)", "made as version 12 (approved by bob)",
 *  "declined by bob: not this week", "withdrawn by ada: the wrong flow", "stale: …". */
export function changeState(p: AccessProposal): string {
  const approved = p.approvals.map((a) => a.who);
  const who = (s: string) => (approved.length > 0 ? ` (${s}${approved.join(", ")})` : "");
  switch (p.state) {
    case "waiting":
      return `waits, ${approved.length} of ${p.needs} approved${who("")}`;
    case "applied":
      return `made as version ${p.version}${who("approved by ")}`;
    case "declined":
      return p.declined_by === p.author ? `withdrawn by ${p.author}: ${p.reason ?? ""}` : `declined by ${p.declined_by}: ${p.reason ?? ""}`;
    case "stale":
      return `stale: ${p.stale ?? ""}`;
  }
}

/** What deciding an access change did, as `holon access approve` says it (accesscmd.rs
 *  `decided_line`). */
export function decidedChange(p: AccessProposal): string {
  switch (p.state) {
    case "waiting": {
      const more = Math.max(0, p.needs - p.approvals.length);
      return `Approved access change ${p.id}: it waits for ${more} more ${more === 1 ? "person" : "people"} (${p.what}).`;
    }
    case "applied":
      return `Approved access change ${p.id}, the last approval it waited for: made, as access version ${p.version}: ${p.what}.`;
    case "declined":
      return p.declined_by === p.author ? `Withdrew access change ${p.id}: ${p.what}.` : `Declined access change ${p.id}: ${p.what}.`;
    case "stale":
      return `Access change ${p.id} no longer applies: ${p.stale ?? ""}.`;
  }
}

/** Why `me` doesn't approve the waiting access change `p`, as the server would refuse it, when it
 *  is plain from `p` (access_proposals.rs `approve`): they proposed it, approved it already, or
 *  could act as whom it gives more (a way in that names them first). Null if none of those: the
 *  server still decides (what it gives, which they must hold). */
export function notYours(p: AccessProposal, me: string): string | null {
  if (p.author === me) return "you proposed it: people other than you approve it";
  if (p.approvals.some((a) => a.who === me)) return "you approved it";
  const way = (p.ways_in ?? []).find((w) => w.startsWith(`${me} `));
  return way ? `it gives more to someone you could act as (${way}): it is approved by someone who couldn't` : null;
}

/** How many access changes wait for `me` to decide: waiting, and not plainly not theirs. */
export const changesToDecide = (changes: AccessProposal[], me: string) => changes.filter((p) => p.state === "waiting" && notYours(p, me) === null).length;
