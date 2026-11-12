// The vocabulary and the import sources, by hand; `types()` (types.rs) adds the leaves from the
// catalog after it. Values are `any`: absence goes through plain `.` in Holon, and an import
// carries no type for what it holds (typescript-syntax L4 §13).

/** A deep copy of a value: a flow's values are copied anyway, so it is the value itself. */
declare function structuredClone<T>(value: T): T

// A flow's values are `any`, and so are the parts Object's functions take from them: without
// these, TypeScript infers `unknown` items from an `any` record.
interface ObjectConstructor {
  values(o: any): any[]
  entries(o: any): [string, any][]
  groupBy(items: any, key: (item: any, index: number) => any): Record<string, any[]>
}

/** What a flow writes a line to its log with, as the standard library has it: `console.log("paid", order.id)`. The host keeps the lines beside the runs; a flow can't read them back. */
interface Console {
  log(...data: any[]): void
  info(...data: any[]): void
  debug(...data: any[]): void
  warn(...data: any[]): void
  error(...data: any[]): void
}
declare var console: Console

declare module "holon" {
  // Only what is marked export leaves the module.
  export {}
  const answer: unique symbol

  /** A length of time, a number and its unit: `"10s"`, `"5m"`, `"2h"`, `"1d"`. */
  export type Duration = `${number}${"s" | "m" | "h" | "d"}`
  /** How a handler's runs are recorded: strict when left out, `"lazy"` or `"unrecorded"`. */
  export type Log = "lazy" | "unrecorded"
  /** A route's path: `/` and segments, `:name` for a part `req.params.name` reads, and a last
   *  `*name` for the rest of the path, one segment or more (`/files/*path`). */
  export type Path = `/${string}`

  /** A flow's settings, the deploy settings. */
  export interface Settings {
    /** The port its server answers on, for a flow with routes. */
    port?: number
    /** How many runs the deployment's log keeps, and for how many days. */
    keep?: { runs?: number; days?: number }
    /** Any other limit of the deploy settings; more than the host's default asks for approval. */
    limits?: Limits
    /** The web origins allowed to call the flow from a browser (CORS), or `"*"`. */
    browsers?: string | string[]
    /** Keys whose values the host never keeps of any run, wherever the run has them: a request,
     *  a message, an input, a call's request or answer, its own answer, what it read or wrote.
     *  `authorization`, `proxy-authorization`, `cookie` and `set-cookie` always are. Each once;
     *  a version that drops one asks a person when an agent deploys it. */
    taint?: string[]
    /** The flow's tags: lowercase letters, digits and `-`, up to 40 of them each. Whoever is bound
     *  on a tag acts on every flow deployed with it; a version that adds one asks for approval. */
    tags?: string[]
  }
  /** What the flow is: its name, which it deploys as, and its settings. Once, after the imports. */
  export function flow(name: string, settings?: Settings): void

  /** A route's request. */
  export interface Request {
    /** The request as one record. */
    request: any
    /** The path's `:name` parts, by name. */
    params: Record<string, string>
    query: Record<string, string>
    headers: Record<string, string>
    /** The body as text. */
    body: string
    /** The body parsed as JSON. */
    json: any
    path: string
    method: string
  }
  /** A route's answer: what `status(…)`, `respond(…)` and `subscribe()` give, returned. */
  export interface Answer {
    readonly [answer]: true
  }
  export interface RouteOptions {
    /** Deliver the event a parked run waits for, by the token the request carries. */
    resumes?: string
    log?: Log
  }
  /** A route's handler: it answers with what it returns. */
  export type RouteHandler = (req: Request) => any
  export interface Routes {
    get(path: Path, handler: RouteHandler, options?: RouteOptions): void
    post(path: Path, handler: RouteHandler, options?: RouteOptions): void
    put(path: Path, handler: RouteHandler, options?: RouteOptions): void
    patch(path: Path, handler: RouteHandler, options?: RouteOptions): void
    delete(path: Path, handler: RouteHandler, options?: RouteOptions): void
    /** WebSocket connections on the path, a run for each block's event: one block at least. */
    websocket(path: Path, blocks: WebSocketBlocks, options?: WebSocketOptions): void
  }
  /** A route: `route.post("/orders", (req) => …)`. The most specific path wins. */
  export const route: Routes

  /** A connection's opening request, with the connection's id. */
  export interface SocketOpen extends Request {
    /** The connection's id, 16 hex digits: the same on its messages and its close. */
    conn: string
  }
  /** A text frame the connection sent. */
  export interface SocketMessage {
    conn: string
    /** The frame's text. */
    text: string
    /** The text parsed as JSON. */
    json: any
    path: string
    /** The opening request's path parts and query. */
    params: Record<string, string>
    query: Record<string, string>
  }
  /** How the connection ended: the browser's code and reason, the host's, or 1006 when it dropped. */
  export interface SocketClose {
    conn: string
    code: number
    reason: string
    path: string
    params: Record<string, string>
    query: Record<string, string>
  }
  /** A websocket's blocks. `open` answers as a route does (a subscription, a first message, or a
   *  status of 300 or more to refuse it); `message` with the value sent back, if any; `close`
   *  nobody. */
  export interface WebSocketBlocks {
    open?: (req: SocketOpen) => any
    message?: (m: SocketMessage) => any
    close?: (c: SocketClose) => void
  }
  export interface WebSocketOptions {
    /** One level for every block, or each block's own. */
    log?: Log | { open?: Log; message?: Log; close?: Log }
  }

  /** Runs every `period`; `now` is the tick's time, in seconds since the epoch. */
  export function every(period: Duration, handler: (now: number) => any, options?: { log?: Log }): void

  /** A file by handle: what it is, never its bytes. */
  export interface FileHandle {
    folder: string
    name: string
    size: number
    hash: string
    modified: number
  }
  /** The file that started a file handler's run. */
  export interface FileEvent extends FileHandle {
    event: string
  }
  export interface FileOptions {
    /** The names it runs for: `*` and `?` within a folder, `**\/` any number of folders. */
    matching?: string
    log?: Log
  }
  export type FileHandler = (file: FileEvent) => any
  /** A message that came to a mail's mailbox, read. */
  export interface MailMessage {
    /** The sender's address, as From writes it: proves nothing alone. Absent unless one. */
    from?: string
    /** The sender's display name, "" when none. */
    name: string
    to: string[]
    cc: string[]
    subject: string
    /** Its Date, in seconds since the epoch, when it is one. */
    date?: number
    /** Its Message-ID without the brackets, or `sha256:` and its hash when it has none. */
    message_id: string
    /** Its plain text, "" when none. */
    text: string
    html?: string
    /** Each attachment's file name to its text (a text, JSON, XML or CSV type) or bytes. */
    attachments: Record<string, string | Uint8Array>
    /** The mailbox's own server's word that the domain in From sent it (DMARC passed). */
    authenticated: boolean
    /** What that server said: pass, fail, none... */
    verdict: { spf: string; dkim: string; dmarc: string }
  }
  /** An event a feed sent. */
  export interface FeedEvent {
    /** Its data: a value when it is JSON, else its text; a binary WebSocket message's bytes. */
    data: any
    /** Its id, when the feed gives one (a WebSocket's: the fields its binding names, joined with /). */
    id?: string
    /** Its name: "message" unless the feed names it; a Postgres channel's, the channel. */
    event: string
    /** How many events its full buffer dropped before it (`--on-full skip`): 0 unless some were. */
    missed: number
    /** How many waiting events of its key it replaced, theirs counted (`--on-full latest`): 0 unless it did. */
    replaced: number
    /** How many numbers of its key the feed skipped before it, theirs counted (`--sequence`): what came while the host wasn't reading. 0 unless some were. */
    gap: number
  }
  /** A message from a queue. */
  export interface QueueMessage {
    /** Its body: a value when it is JSON, else its text. */
    data: any
    /** The queue's id for it. */
    id: string
    /** Its message attributes whose type is String or Number, as text. */
    attributes: Record<string, string>
    /** When it was sent, in seconds since 1970, as `now` is. */
    sent: number
    /** How many times the queue has handed it out: more than 1 once a run of it failed or was cut off. */
    count: number
    /** A FIFO queue's message group. */
    group?: string
  }
  export const on: {
    file: {
      /** Runs for each file new or changed in the folder, once it stops changing. */
      created(folder: Folder, handler: FileHandler, options?: FileOptions): void
      /** Runs for each file removed from the folder. */
      deleted(folder: Folder, handler: FileHandler, options?: FileOptions): void
    }
    /** Runs for each message that comes to the mail's mailbox, moved once its run is over. */
    mail(mail: Mail, handler: (message: MailMessage) => any, options?: { log?: Log }): void
    /** Runs for each event the feed sends, one at a time, in the order they came. */
    feed(feed: Feed, handler: (event: FeedEvent) => any, options?: { log?: Log }): void
    /** Runs for each message from the queue, one at a time, deleted once its run is over. */
    queue(queue: Queue, handler: (message: QueueMessage) => any, options?: { log?: Log }): void
  }

  /** A start's answer: `{run, waiting}` once the run waits, `{answer, run}` if it finished first. */
  export interface Started {
    run?: string
    /** The token for each point the run waits at, by its name. */
    waiting?: Record<string, string>
    answer?: any
    error?: string
    message?: string
  }
  /**
   * An action, exported: run by a person, an agent a person lets, or another flow, with a JSON
   * value. Called without `await` in a route, it is started.
   */
  export function action<I = any>(handler: (input: I) => any, options?: { log?: Log }): (input?: I) => Started

  export interface JsOptions {
    /** Instructions per call (200M by default). */
    fuel?: number
    /** Memory per call, in bytes (16 MiB by default). */
    memory?: number
    /** The fields of the record it returns, an output each. */
    outputs?: string[]
  }
  /** Runs the function as JavaScript, as written, in a sandbox: no clock, network or storage. */
  export function js<F extends (...args: any[]) => any>(fn: F, options?: JsOptions): F

  export interface RespondOptions {
    status?: number
    headers?: Record<string, string>
    /** Answer with a call's body as it arrives. */
    stream?: boolean
  }
  /** A route's answer with its headers: `respond(html, { headers: { "content-type": "text/html" } })`. */
  export function respond(body: any, options?: RespondOptions): Answer
  /** A route's answer with its status: `status(201, order)`, `status(204)`. */
  export function status(code: number, body?: any): Answer

  export const stripe: {
    /** A verified Stripe webhook; `.event` is the event. */
    verify(req: Request, secret: string, now: number, options?: { tolerance?: number }): any
  }

  /** Parks the run until the event arrives, or its deadline: in an action, not a route. */
  export function event(name: string, options?: { expires?: Duration }): Promise<any>

  /** Parks the run until people approve it in Holon, or one declines, or its deadline: in an
   *  action, not a route. `people: 2` is two different people; `steps`, each step in turn (in any
   *  order with `order: "any"`), taken by principals holding its role. */
  export function approval(name: string, options?: ApprovalOptions): Promise<Decision>
  /** An approval's options: how many people decide it, or its steps (up to 5). A list in
   *  `steps` is a stage, its steps taken in any order. */
  export type ApprovalOptions =
    | { expires?: Duration; people?: 1 | 2 | 3 | 4 | 5 }
    | { expires?: Duration; steps: (Step | Step[])[]; order?: "sequence" | "any" }
  /** One step of an approval. */
  export interface Step {
    /** The role its principals hold where the approval is; left out, anyone holding run.decide
     *  there, a person. An agent's step names one. */
    role?: string
    /** `"agent"` when an agent takes it; `"person"` when left out. A service never does. */
    kind?: "person" | "agent"
    /** How many different principals take it. */
    people?: 1 | 2 | 3 | 4 | 5
  }
  /** What people decided on an approval. */
  export interface Decision {
    approved: boolean
    /** Who approved, by account, in the order they did. */
    by: string[]
    /** Every decision taken on it, a decline included, in the order they came. */
    decisions: Decided[]
    declined_by?: string
    reason?: string
    expired?: true
  }
  /** One decision on an approval: which step it took, who took it, and what they decided. */
  export interface Decided {
    /** The step's number, from 0 as the steps are written: 0 for `people`. */
    slot: number
    /** Who took it, by account. */
    who: string
    kind: "person" | "agent"
    /** The step's role: none for `people`. */
    role?: string
    /** When, seconds since the epoch. */
    at: number
    approved: boolean
    reason?: string
    /** What they noted with it, any JSON (an agent's label and confidence, say). */
    note?: unknown
    /** What let them: the binding they took it through, by its id (`holon access show` lists it). */
    binding?: number
    /** The version of the access document that binding was read in (`holon access show N`). */
    access?: number
  }

  /** A host's answer. */
  export interface Response {
    status: number
    headers: Record<string, string>
    body: string
    /** Why the call wasn't made, when it wasn't: `refused`, `changed`, … */
    error?: string
    message?: string
  }
  export interface RequestOptions {
    headers?: Record<string, string>
    /** Text as it is, anything else as JSON. */
    body?: any
    /** A file sent as the body, by handle: the server streams its bytes. */
    file?: FileHandle
    stream?: boolean
  }
  /** A host the flow calls over HTTPS, its import the permission. */
  export interface Host {
    get(path: string, options?: RequestOptions): Promise<Response>
    post(path: string, options?: RequestOptions): Promise<Response>
    put(path: string, options?: RequestOptions): Promise<Response>
    patch(path: string, options?: RequestOptions): Promise<Response>
    delete(path: string, options?: RequestOptions): Promise<Response>
    head(path: string, options?: RequestOptions): Promise<Response>
  }

  export interface Page {
    prefix?: string
    after?: string
    limit?: number
  }
  /** A collection: `notes[key]` reads an item as the run began, `notes[key] = v` puts it. */
  export interface Collection {
    [key: string]: any
    get(key: string): any
    put(key: string, value: any): any
    delete(key: string): any
    /** A page of items, in key order. */
    list(page?: Page): any
  }
  /** A named folder: `dir[name]` reads a file's text as the run began, `dir[name] = text` writes it, or bytes as they are. */
  export interface Folder {
    [name: string]: any
    get(name: string): any
    /** The file's bytes, any file: an image, a PDF, an archive. */
    bytes(name: string): Uint8Array
    put(name: string, content: string | Uint8Array | FileHandle): any
    delete(name: string): any
    /** A page of files: `{size, hash, modified}` by name. */
    list(page?: Page): any
    /** A handle to a file of any size, no bytes. */
    file(name: string): FileHandle
    /** A route's answer that is the file, its bytes sent by the host: `return site.serve("index.html")`. */
    serve(name: string): Answer
    /** Lines from line `from` (from the end when negative). */
    lines(name: string, options?: { from?: number; limit?: number }): any
    /** Bytes from `offset`, as text. */
    slice(name: string, options?: { offset?: number; length?: number }): any
    copy(name: string, handle: FileHandle): any
    /** Writes the file only if there is none yet: else the run fails, and nothing it did lands. */
    create(name: string, content: string | Uint8Array): any
    create_copy(name: string, handle: FileHandle): any
  }
  /** A named database: its SQL written in quotes (the tables are the permission), values after it as `$1`, `$2`, … */
  export interface Database {
    /** The rows, as the run began. */
    query(sql: string, ...values: any[]): any[]
    /** The first row, or no value. */
    one(sql: string, ...values: any[]): any
    /** An insert, update or delete when the run finishes; `{ expect: n }` last, to land only if it changes `n` rows. */
    exec(sql: string, ...values: any[]): any
    find(collection: string, filter: object, options?: { sort?: object; limit?: number }): any[]
    find_one(collection: string, filter: object, options?: { sort?: object }): any
    insert(collection: string, document: object): any
    update(collection: string, filter: object, update: object, options?: { expect?: number }): any
    delete(collection: string, filter: object, options?: { expect?: number }): any
  }
  /** A channel: what is sent reaches whoever subscribed. */
  export interface Channel {
    send(message: any): any
    /** A subscription, a route's answer: `return updates.subscribe()`. */
    subscribe(options?: { from?: string }): Answer
    /** A call's answer, to the channel as it arrives: `updates.pipe(api.post(…))`. */
    pipe(call: Promise<Response>): any
  }
  /** A mail resource: what is sent goes out once the run is over, from the binding's address; a
   *  mailbox it names is read by `on.mail`. */
  export interface Mail {
    send(message: {
      to: string | string[]
      cc?: string | string[]
      subject: string
      text: string
      html?: string
      /** A file name to its text or bytes. */
      attachments?: Record<string, string | Uint8Array>
    }): any
  }
  /** A feed an admin binds on the host: listened to by `on.feed`. */
  export interface Feed {
    readonly kind: "feed"
  }
  /** A queue an admin binds on the host: sent to once the run is over, listened to by `on.queue`. */
  export interface Queue {
    send(message: {
      /** The body: JSON, or text as it is. */
      data: any
      /** A FIFO queue's message group: one group's messages come out in the order sent. */
      group?: string
    }): any
  }
}

/** A secret set for the deployment: someone sets it before the flow works. */
declare module "secret:*" {
  const secret: string
  export default secret
}
/** The flow's state: `state.key` reads it as the run began, `state.key = v` writes it. */
declare module "state" {
  const state: Record<string, any>
  export default state
}
declare module "collection:*" {
  import type { Collection } from "holon"
  const collection: Collection
  export default collection
}
declare module "folder:*" {
  import type { Folder } from "holon"
  const folder: Folder
  export default folder
}
declare module "database:*" {
  import type { Database } from "holon"
  const database: Database
  export default database
}
declare module "channel:*" {
  import type { Channel } from "holon"
  const channel: Channel
  export default channel
}
declare module "mail:*" {
  import type { Mail } from "holon"
  const mail: Mail
  export default mail
}
declare module "feed:*" {
  import type { Feed } from "holon"
  const feed: Feed
  export default feed
}
declare module "queue:*" {
  import type { Queue } from "holon"
  const queue: Queue
  export default queue
}
declare module "host:*" {
  import type { Host } from "holon"
  const host: Host
  export default host
}
declare module "holon:clock" {
  /** The time the run started, in seconds since the epoch; in a clock, the tick's. */
  export const now: number
}
declare module "holon:random" {
  /** One random number for the run, recorded. */
  export const random: number
  /** The run's 32 random bytes, recorded: what `pgp_encrypt` draws a message's randomness from. */
  export const random_bytes: Uint8Array
}
/** Another deployed flow's actions, by its slug: `await charge(x)` runs one, `charge(x)` starts it. */
declare module "flow:*"
