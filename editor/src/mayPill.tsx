// What a flow may do, as a short pill: an icon for the kind of thing it is, a second small one
// for what is done to it (written, sent on, run on each), and its name; the whole sentence is the
// pill's title and what a screen reader hears (MayStrip.tsx). Every kind core can say
// (holon-author's describe.rs) has an icon of its own here, and mayPill.test.tsx holds that: a
// kind added there with none here fails it.

import type { ReactNode } from "react";
import { itemParts } from "./holdings";

/** The things a flow reaches, each drawn: 24-unit strokes, as the nav's icons are (ui/Shell.tsx). */
const THING = {
  // A request coming in: an arrow through a door.
  in: <><path d="M3 12h12M11 8l4 4-4 4" /><path d="M15 4h5v16h-5" /></>,
  // A call going out.
  out: <path d="M7 17 17 7M9 7h8v8" />,
  // An open padlock: not encrypted.
  unlocked: <><rect x="5" y="11" width="14" height="9" rx="2" /><path d="M8 11V8a4 4 0 0 1 7.5-2" /></>,
  plane: <path d="M21 3 10 14M21 3l-7 18-4-7-7-4z" />,
  home: <><path d="M3 11 12 4l9 7" /><path d="M6 10v10h12V10" /><path d="M10 20v-5h4v5" /></>,
  clock: <><circle cx="12" cy="12" r="9" /><path d="M12 7v5l3 2" /></>,
  die: <><rect x="4" y="4" width="16" height="16" rx="3" /><path d="M8.5 8.5h.01M15.5 15.5h.01M12 12h.01M15.5 8.5h.01M8.5 15.5h.01" /></>,
  // One value kept: a cell.
  cell: <><rect x="5" y="5" width="14" height="14" rx="3" /><path d="M9 10h6M9 14h6" /></>,
  // Items kept: rows.
  rows: <><rect x="4" y="4" width="16" height="4.5" rx="1" /><rect x="4" y="9.75" width="16" height="4.5" rx="1" /><rect x="4" y="15.5" width="16" height="4.5" rx="1" /></>,
  folder: <path d="M3 7a1 1 0 0 1 1-1h5l2 2h9a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1z" />,
  database: <><ellipse cx="12" cy="6" rx="7" ry="3" /><path d="M5 6v6c0 1.7 3 3 7 3s7-1.3 7-3V6M5 12v6c0 1.7 3 3 7 3s7-1.3 7-3v-6" /></>,
  // A channel: waves from a point.
  waves: <><circle cx="12" cy="12" r="1.5" /><path d="M8.5 8.5a5 5 0 0 0 0 7M15.5 8.5a5 5 0 0 1 0 7M6 6a8.5 8.5 0 0 0 0 12M18 6a8.5 8.5 0 0 1 0 12" /></>,
  mail: <><rect x="3" y="6" width="18" height="12" rx="2" /><path d="m4 8 8 6 8-6" /></>,
  // A queue: messages in a row.
  queue: <><rect x="3" y="9" width="4.5" height="6" rx="1" /><rect x="9.75" y="9" width="4.5" height="6" rx="1" /><rect x="16.5" y="9" width="4.5" height="6" rx="1" /></>,
  feed: <><path d="M5 5a14 14 0 0 1 14 14M5 11a8 8 0 0 1 8 8" /><path d="M6 18h.01" /></>,
  // A connection held open: both ways.
  socket: <path d="M4 9h14M14 5l4 4-4 4M20 15H6M10 11l-4 4 4 4" />,
  key: <><circle cx="8" cy="15" r="4" /><path d="m11 12 9-9M16 7l3 3M14 9l2 2" /></>,
  // Every secret of the server's: a ring of keys.
  keys: <><circle cx="12" cy="6" r="3" /><path d="M10.5 8.5 6.5 20M13.5 8.5l4 11.5M5.5 17l2.4.8M16 16.6l2.4-.8" /></>,
  terminal: <><rect x="3" y="5" width="18" height="14" rx="2" /><path d="m7 10 3 2-3 2M12 15h5" /></>,
  pause: <><circle cx="12" cy="12" r="9" /><path d="M10 9v6M14 9v6" /></>,
  shield: <><path d="M9 12l2 2 4-4" /><path d="M12 3l8 4v5c0 5-3.5 8-8 9-4.5-1-8-4-8-9V7z" /></>,
  play: <><circle cx="12" cy="12" r="9" /><path d="m10 8 6 4-6 4z" /></>,
  agent: <><rect x="5" y="8" width="14" height="11" rx="2" /><path d="M12 8V4.5M9 13h.01M15 13h.01M9.5 16h5" /><circle cx="12" cy="3.5" r="1" /></>,
  // Another flow's action: from one node to the next.
  hop: <><circle cx="6" cy="12" r="2.5" /><circle cx="18" cy="12" r="2.5" /><path d="M8.5 12h7M13 9.5l2.5 2.5-2.5 2.5" /></>,
  eye: <><path d="M3 12s3.5-6 9-6 9 6 9 6-3.5 6-9 6-9-6-9-6z" /><circle cx="12" cy="12" r="2.5" /></>,
  eyeOff: <><path d="M3 12s3.5-6 9-6 9 6 9 6-3.5 6-9 6-9-6-9-6z" /><circle cx="12" cy="12" r="2.5" /><path d="M4 4l16 16" /></>,
  gauge: <><path d="M4 16a8 8 0 1 1 16 0" /><path d="m12 16 4-5" /></>,
  code: <path d="M8 7 3 12l5 5M16 7l5 5-5 5M13.5 5l-3 14" />,
  chip: <><rect x="7" y="7" width="10" height="10" rx="1.5" /><path d="M10 3v4M14 3v4M10 17v4M14 17v4M3 10h4M3 14h4M17 10h4M17 14h4" /></>,
  history: <><path d="M4 12a8 8 0 1 0 2.5-5.8" /><path d="M4 5v4h4M12 8v4l3 2" /></>,
  // A web page: a browser's window.
  window: <><rect x="3" y="5" width="18" height="14" rx="2" /><path d="M3 9h18M6 7h.01M8.5 7h.01" /></>,
  tag: <><path d="M3 5a2 2 0 0 1 2-2h6l10 10-8 8L3 11z" /><path d="M7.5 7.5h.01" /></>,
  // A process started: a window that runs.
  process: <><rect x="3" y="5" width="18" height="14" rx="2" /><path d="m10 9 5 3-5 3z" /></>,
  // A kind this editor doesn't know yet.
  other: <circle cx="12" cy="12" r="3" />,
} satisfies Record<string, ReactNode>;

/** What is done to the thing, when it is more than read: drawn small, after it. */
const DOES = {
  // Written: a pencil.
  write: <path d="M4 20l1-5L16 4l4 4L9 19z" />,
  // Read and written: one arrow each way.
  both: <path d="M7 20V4M3 8l4-4 4 4M17 4v16M13 16l4 4 4-4" />,
  // Sent on, or answered with: an arrow out.
  send: <path d="M7 17 17 7M9 7h8v8" />,
  // A run on each one that comes: a bolt.
  on: <path d="M13 3 5 13h6l-1 8 8-10h-6z" />,
} satisfies Record<string, ReactNode>;

export type Thing = keyof typeof THING;
export type Does = keyof typeof DOES;

/** A pill: the thing's icon, what is done to it (none when it is only read or had), and its short
 *  name. */
export type Pill = { thing: Thing; does?: Does; label: string };

/** How each kind with an argument is drawn, and named from its argument. */
const KINDS: Record<string, { thing: Thing; does?: Does; label?: (arg: string) => string }> = {
  HttpRequest: { thing: "out" },
  PlainHttp: { thing: "unlocked", label: (a) => `http://${a}` },
  StateRead: { thing: "cell" },
  StateWrite: { thing: "cell", does: "write" },
  CollectionRead: { thing: "rows" },
  CollectionWrite: { thing: "rows", does: "write" },
  FileRead: { thing: "folder" },
  FileWrite: { thing: "folder", does: "write" },
  FileArrival: { thing: "folder", does: "on" },
  FileServe: { thing: "folder", does: "send" },
  DbRead: { thing: "database", label: (a) => a.replace("/", ".") },
  DbWrite: { thing: "database", does: "write", label: (a) => a.replace("/", ".") },
  ChannelRead: { thing: "waves" },
  ChannelSend: { thing: "waves", does: "send" },
  MailSend: { thing: "mail", does: "send", label: (a) => (a.includes("/") ? `${a.split("/")[0]} → ${a.slice(a.indexOf("/") + 1) === "*" ? "any address" : a.slice(a.indexOf("/") + 1)}` : a) },
  MailArrival: { thing: "mail", does: "on" },
  QueueSend: { thing: "queue", does: "send" },
  QueueMessage: { thing: "queue", does: "on" },
  FeedEvent: { thing: "feed", does: "on" },
  WebSocket: { thing: "socket" },
  SecretRead: { thing: "key" },
  SecretAnswer: { thing: "key", does: "send", label: (a) => `${a} to callers` },
  Log: { thing: "terminal" },
  Await: { thing: "pause" },
  Approval: { thing: "shield", label: (a) => a.split("/")[0] },
  Action: { thing: "play" },
  AgentRuns: { thing: "agent", does: "on" },
  RunsAction: { thing: "hop", label: (a) => a.replace("/", ".") },
  Unrecorded: { thing: "eyeOff" },
  Untaint: { thing: "eye" },
  Browser: { thing: "window", label: (a) => (a === "*" ? "any site" : a.replace(/^https?:\/\//, "")) },
  Tag: { thing: "tag" },
};

/** The kinds with no argument. */
const BARE: Record<string, Pill> = {
  NetworkIngress: { thing: "in", label: "requests" },
  NetworkEgress: { thing: "plane", label: "outbound" },
  Clock: { thing: "clock", label: "clock" },
  Random: { thing: "die", label: "random" },
  PrivateAddresses: { thing: "home", label: "private network" },
  AgentReadsLogs: { thing: "agent", label: "reads its logs" },
  SecretAccess: { thing: "keys", label: "the server's secrets" },
  Spawn: { thing: "process", label: "processes" },
};

/** A limit, by what it bounds: the code a flow carries, how long its log keeps runs, or a count. */
function limitPill(setting: string): Pill {
  const [name, value = ""] = setting.split("=");
  if (name === "module_fuel") return { thing: "code", label: `js ${Number(value).toLocaleString("en-US")} steps` };
  if (name === "module_memory") return { thing: "chip", label: `js ${Math.round(Number(value) / (1024 * 1024))} MiB` };
  if (name === "keep_runs") return { thing: "history", label: `keeps ${value} runs` };
  if (name === "keep_run_days") return { thing: "history", label: `keeps ${value} days` };
  return { thing: "gauge", label: `${name.replace(/_/g, " ")} ${value}`.trim() };
}

/** A manifest item as a pill; one of a kind this editor doesn't know is the dot and the item as
 *  it is written. `leaves` is whether a secret it reads goes anywhere (it is sent, or something
 *  made with it or answered to it is): the pill then says so with the arrow, and its title where. */
export function pillOf(item: string, leaves = false): Pill {
  const { kind, arg } = itemParts(item);
  if (kind === "Limit") return limitPill(/^Limit\((.*)\)$/.exec(item)?.[1] ?? "");
  if (arg === null) return BARE[item] ?? { thing: "other", label: item };
  const drawn = KINDS[kind];
  if (!drawn) return { thing: "other", label: item };
  const does = kind === "SecretRead" && leaves ? "send" : drawn.does;
  return { thing: drawn.thing, ...(does ? { does } : {}), label: drawn.label ? drawn.label(arg) : arg };
}

/** The kinds that write what another reads: `StateWrite("x")` beside `StateRead("x")` is one
 *  pill, read and written ([`MayStrip`] joins them when both are there and changed alike). */
export const WRITES: Record<string, string> = { StateWrite: "StateRead", CollectionWrite: "CollectionRead", FileWrite: "FileRead", DbWrite: "DbRead" };

/** Every kind drawn here, with or without an argument: what mayPill.test.tsx holds to core's. */
export const DRAWN_KINDS: string[] = [...Object.keys(KINDS), ...Object.keys(BARE), "Limit"];

/** A pill's icons: the thing, then what is done to it. */
export function PillIcons({ pill }: { pill: Pill }) {
  return (
    <>
      <svg className="hl-may__icon" viewBox="0 0 24 24" aria-hidden="true" data-thing={pill.thing}>
        {THING[pill.thing]}
      </svg>
      {pill.does && (
        <svg className="hl-may__icon hl-may__icon--does" viewBox="0 0 24 24" aria-hidden="true" data-does={pill.does}>
          {DOES[pill.does]}
        </svg>
      )}
    </>
  );
}
