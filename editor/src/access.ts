// Who holds what, as the People pages show it (bindings.rs): each binding in a few words, as
// `holon people` lists them, a principal's own role on the host, and the roles a kind may hold,
// the ones an admin defined included (`GET /roles`).

import type { HeldBinding, Kind, Principal, RoleEntry, Scope } from "./server";

/** Where a binding holds, as the flags write it: nothing for the host, ` on payout`,
 *  ` on tag payments`, ` at payout:ok`. */
export function placeSaid(scope: Scope): string {
  if (scope === "host") return "";
  if ("flows" in scope) return "tag" in scope.flows ? ` on tag ${scope.flows.tag}` : ` on ${scope.flows.slug}`;
  return ` at ${scope.approval.flow.slug}:${scope.approval.name}`;
}

/** A moment, in UTC as the CLI says it: `2026-12-01 09:00 UTC`. */
const utc = (seconds: number) => `${new Date(seconds * 1000).toISOString().slice(0, 16).replace("T", " ")} UTC`;

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
