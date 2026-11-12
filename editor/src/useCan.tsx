// What whoever is signed in may do, as the server's `can` answers it (`GET /auth/me/can`), for a
// page's buttons (web-console-L4.md §3, principle 6): a page asks once for every action its buttons'
// routes need (can.rs's route table names them), and a button the caller may not use is disabled,
// with the server's refusal as its title. The answers are kept a little while, so pages and tabs
// asking the same don't ask again, and forgotten when someone else signs in.
//
// Which of the host's pages the nav shows follows the same answers (hostPagesOf).

import { useEffect, useMemo, useState, type ReactNode } from "react";
import { listOwned, mayI, type Kind } from "./server";

/** What `can` answers for an action: true, or the words it refuses in. */
export type Answer = true | string;

/** How long an answer is kept before it is asked again. */
const KEPT_MS = 10_000;
/** How often a page that stays open asks again: a role changed meanwhile shows. */
const AGAIN_MS = 60_000;

/** The answers asked for, by where and which actions; each with when it was asked. */
const kept = new Map<string, { at: number; answers: Promise<Record<string, Answer>> }>();
const forget = () => kept.clear();
if (typeof window !== "undefined") window.addEventListener("holon:session", forget);

/** Where an action is asked about: on a flow (its slug), or, without one, as the host's lists and
 *  pages ask it (bindings.rs `me_can`). */
export type CanAt = { flow?: string | null };

const keyOf = (actions: readonly string[], at: CanAt) => `${at.flow ?? ""}?${[...actions].sort().join(",")}`;

/** The server's answers for `actions` at `at`: one request, kept a while; one that fails isn't. */
export function askCan(actions: readonly string[], at: CanAt = {}): Promise<Record<string, Answer>> {
  const key = keyOf(actions, at);
  const now = Date.now();
  const hit = kept.get(key);
  if (hit && now - hit.at < KEPT_MS) return hit.answers;
  const answers = mayI([...actions], at.flow ? { flow: at.flow } : {});
  kept.set(key, { at: now, answers });
  answers.catch(() => kept.delete(key));
  return answers;
}

/** A control's props for an answer: disabled, with the refusal as its title, when refused; else
 *  as `own` says (its own reasons to be disabled, its own title). */
export function gateOf(answer: Answer, own: { disabled?: boolean; title?: string } = {}): { disabled: boolean; title?: string } {
  return answer === true ? { disabled: own.disabled ?? false, title: own.title } : { disabled: true, title: answer };
}

/** What a page may do. `may` is true until the server has answered (or when it couldn't): the
 *  server still refuses what it refuses, and a button doesn't flicker for those who may.
 *  `holds` is true only once the server said so, for what is shown only to those who hold it. */
export type Can = {
  may: (action: string) => Answer;
  holds: (action: string) => boolean;
  /** `gateOf` for `action`. */
  gate: (action: string, own?: { disabled?: boolean; title?: string }) => { disabled: boolean; title?: string };
  /** Whether the server has answered. */
  known: boolean;
};

/** A `Can` from the server's answers (null while it hasn't answered). */
export function canOf(answers: Record<string, Answer> | null): Can {
  const may = (action: string): Answer => answers?.[action] ?? true;
  return { may, holds: (action) => answers?.[action] === true, gate: (action, own) => gateOf(may(action), own), known: answers !== null };
}

/** Whether whoever is signed in may do each of `actions` at `at`, as `can` answers it: asked once
 *  for them all when the page opens, when someone else signs in, and each minute it stays open. */
export function useCan(actions: readonly string[], at: CanAt = {}): Can {
  const key = keyOf(actions, at);
  const [answers, setAnswers] = useState<{ key: string; answers: Record<string, Answer> } | null>(null);
  useEffect(() => {
    let live = true;
    const [flow, list] = key.split("?");
    const ask = () =>
      askCan(list.split(",").filter(Boolean), { flow: flow || null }).then(
        (a) => live && setAnswers({ key, answers: a }),
        () => undefined,
      );
    void ask();
    const timer = setInterval(ask, AGAIN_MS);
    window.addEventListener("holon:session", ask);
    return () => {
      live = false;
      clearInterval(timer);
      window.removeEventListener("holon:session", ask);
    };
  }, [key]);
  // Another flow's answers are not this one's.
  const now = answers?.key === key ? answers.answers : null;
  return useMemo(() => canOf(now), [now]);
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

/** The host's pages whoever is signed in is shown (the nav's host group, the palette, the pages
 *  themselves): Resources with `resource.view`, Policy with `policy.view`, People and access with
 *  `people.view`, everyone's; or, without it, theirs, to someone who owns an agent or a service. */
export type HostPages = { resources: boolean; people: "everyone" | "owned" | null; policy: boolean; host: boolean };

/** The host's actions the nav asks about. */
export const HOST_VIEWS = ["resource.view", "people.view", "policy.view", "host.view"] as const;

export function hostPagesOf(can: Pick<Can, "holds">, owns: boolean): HostPages {
  return {
    resources: can.holds("resource.view"),
    people: can.holds("people.view") ? "everyone" : owns ? "owned" : null,
    policy: can.holds("policy.view"),
    host: can.holds("host.view"),
  };
}

/** The host's pages `me` is shown, and the `Can` they follow (for why a page isn't). */
export function useHostPages(me: { name: string; kind: Kind }): HostPages & { can: Can } {
  const can = useCan(HOST_VIEWS);
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
