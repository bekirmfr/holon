// Resources' tables (web-console-L4.md §9.1): each folder, database, mail, feed and queue bound is
// a row of the same table (ResourceViews.tsx's ResourceTable), the resource, its details and who
// uses it. What a row says of its binding is read here, one kind at a time: where it is (null when
// the caller isn't shown it: a folder's path and a database's whereabouts go only to whoever may
// bind resources, L4 Q4), and what else the binding says, in a few words each.

import type { DatabaseEntry, FeedEntry, FolderEntry, MailEntry, QueueEntry } from "./server";
import type { Kind } from "./holdings";
import { sizeText } from "./storage";

/** One binding as its row says it: its kind and name, the deployment it is bound for (null: the
 *  server's), where it is (null: not shown to the caller, or not known), and the rest. */
export type ResourceLine = { kind: Kind; name: string; deployment: string | null; where: string | null; notes: string[] };

const access = (a: "read" | "read-write") => (a === "read" ? "read only" : "read and write");
const heldForRuns = (lock: "commit" | "run" | undefined) => (lock === "run" ? ["held for whole runs"] : []);

/** A folder: its path (to whoever may bind), its access, what the log keeps of its reads, its cap. */
export function folderLine(f: FolderEntry, seesWhere: boolean): ResourceLine {
  const keep = f.keep === "hash" ? ["reads kept by hash"] : f.keep.startsWith("bytes for ") ? [`reads kept ${f.keep.slice("bytes for ".length)}, then by hash`] : [];
  return {
    kind: "folder",
    name: f.name,
    deployment: f.deployment,
    where: seesWhere ? (f.path ?? null) : null,
    notes: [access(f.access), ...keep, ...(f.cap != null ? [`capped at ${sizeText(f.cap)}`] : []), ...(f.shared ? ["other servers bind it too"] : []), ...heldForRuns(f.lock)],
  };
}

/** A database: a SQLite file, or where a connection's URL is fetched from and where it points
 *  (to whoever may bind); its engine and access. */
export function databaseLine(d: DatabaseEntry, seesWhere: boolean): ResourceLine {
  const where = !seesWhere ? null : d.connection ? (d.address ? `${d.address} (from ${d.connection})` : `from ${d.connection}`) : (d.path ?? null);
  return { kind: "database", name: d.name, deployment: d.deployment, where, notes: [d.engine, access(d.access), ...heldForRuns(d.lock)] };
}

/** A mail: the server it sends through, as whom and to whom; the mailbox it receives in. */
export function mailLine(m: MailEntry): ResourceLine {
  const parts = [
    ...(m.smtp ? [`${m.address} (from ${m.smtp}) · as ${m.from} · to ${m.to.join(", ")}`] : []),
    ...(m.mailbox
      ? [`receives in ${m.mailbox.address} ${m.mailbox.folder} (from ${m.mailbox.imap}), moved to ${m.mailbox.done} · ${m.mailbox.verdict_from ? `${m.mailbox.verdict_from}'s verdict` : "no verdict believed"}`]
      : []),
  ];
  return { kind: "mail", name: m.name, deployment: m.deployment, where: parts.join(" · ") || null, notes: m.oauth ? [`logs in with OAuth (${m.oauth.address} from ${m.oauth.source})`] : [] };
}

/** A feed: where it is, what each request sends, what a WebSocket is sent, how much may wait. */
export function feedLine(f: FeedEntry): ResourceLine {
  const said = [
    f.database ? `listens on ${f.address} (the database ${f.database})` : `${f.address} (from ${f.url})`,
    ...(Object.keys(f.headers).length > 0 ? [`sends ${Object.entries(f.headers).map(([h, s]) => `${h} from ${s}`).join(", ")}`] : []),
    ...((f.subscribe ?? []).length > 0 ? [`subscribes with ${(f.subscribe ?? []).join(", ")}`] : []),
    ...(f.heartbeat ? [`sends ${f.heartbeat} every ${f.every ?? 30}s`] : []),
    ...(f.only ? [`runs only ${Object.entries(f.only).map(([k, vs]) => `${k} ${vs.join(" or ")}`).join(" and ")}`] : []),
    ...(f.id ? [`its id ${f.id.join("/")}`] : []),
    ...(f.sequence ? [`numbered by ${f.sequence}${f.key ? `, each ${f.key} on its own` : ""}`] : []),
    ...(f.resume?.query ? [`resumes from its position with ${f.resume.query}= in the URL`] : []),
    ...(f.resume?.header ? [`resumes from its position with the header ${f.resume.header}`] : []),
    ...(f.resume?.message ? [`resumes from its position with ${f.resume.message.join(", ")}`] : []),
    `${f.buffer} may wait${f.on_full === "skip" ? ", the rest dropped" : f.on_full === "latest" ? `, one a ${f.key}, the newest` : ""}`,
    `quiet for ${f.idle}s at most`,
  ];
  return { kind: "feed", name: f.name, deployment: f.deployment, where: said.join(" · "), notes: [] };
}

/** A queue: where it is, FIFO or not, its largest message, how long a message is out of sight. */
export function queueLine(q: QueueEntry): ResourceLine {
  const said = [`${q.address} (from ${q.url})`, ...(q.fifo ? ["FIFO"] : []), `${Math.round(q.largest / 1024)} KiB at most`, `out of sight for ${q.visibility}s`, ...(q.keep_failed ? ["a failed run's message is left for the redrive"] : [])];
  return { kind: "queue", name: q.name, deployment: q.deployment, where: said.join(" · "), notes: [] };
}

/** A row's accessible name: its kind and name, and the deployment it is bound for. */
export const lineLabel = (l: Pick<ResourceLine, "kind" | "name" | "deployment">) => `${l.kind} ${l.name}${l.deployment ? ` for ${l.deployment}` : ""}`;
