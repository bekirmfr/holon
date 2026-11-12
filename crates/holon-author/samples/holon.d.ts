// holon.d.ts: the types of what a flow imports, for TypeScript's own tools
// (an editor's completion, hover and checks). `holon types` wrote it from its catalog, and
// writes it again after an upgrade: don't edit it by hand. Holon's compiler decides what a
// flow means; `holon check` says where the two disagree.

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
     *  `authorization`, `proxy-authorization`, `cookie` and `set-cookie` always are. */
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

declare module "holon" {
  /** The deploy settings' limits a flow may set: a whole number in its unit, a `_ms` or `_seconds` one a duration too. */
  export interface Limits {
    connect_timeout_ms?: number | Duration
    request_timeout_ms?: number | Duration
    claim_wait_ms?: number | Duration
    commit_wait_ms?: number | Duration
    run_budget_ms?: number | Duration
    clock_run_budget_ms?: number | Duration
    max_requests?: number
    per_host_concurrency?: number
    max_request_bytes?: number
    max_response_bytes?: number
    max_header_bytes?: number
    max_redirects?: number
    max_await_seconds?: number | Duration
    max_waiting_runs?: number
    max_call_depth?: number
    inline_file_bytes?: number
    max_file_read_bytes?: number
    max_file_write_bytes?: number
    max_files_per_run?: number
    max_served_bytes?: number
    max_db_rows?: number
    max_db_read_bytes?: number
    db_statement_ms?: number | Duration
    max_db_statements?: number
    max_pack_bytes?: number
    max_packs_per_run?: number
    channel_keep?: number
    channel_buffer?: number
    max_subscribers?: number
    max_websockets?: number
    max_message_bytes?: number
    messages_per_second?: number
  }

  /**
   * `add(a: Any, b: Any) -> out: Any`
   *
   * Adds its numbers, as JavaScript's + does; with a text among them, joins them, writing a number or a boolean as text (1 + 2 + "a" is "3a"). Takes more inputs with +.
   */
  export function add(...args: any[]): any

  /**
   * `multiply(a: Number, b: Number) -> out: Number`
   *
   * Multiplies its numbers. Takes more inputs with +.
   */
  export function multiply(...args: any[]): any

  /**
   * `subtract(a: Number, b: Number) -> out: Number`
   *
   * Subtracts b from a: a − b.
   */
  export function subtract(...args: any[]): any

  /**
   * `divide(a: Number, b: Number) -> out: Number`
   *
   * Divides a by b: a ÷ b. Dividing by zero produces nothing.
   */
  export function divide(...args: any[]): any

  /**
   * `compare(a: Any, b: Any) -> out: Boolean  {op: =}`
   *
   * Compares two numbers, or two texts code unit by code unit as JavaScript does, with the chosen operator (=, !=, <, <=, >, >=): true or false.
   */
  export function compare(...args: any[]): any

  /**
   * `and(a: Boolean, b: Boolean) -> out: Boolean`
   *
   * True when every input is true. Read in order, as JavaScript's && is: the first false decides, and the inputs after it aren't run. Takes more inputs with +.
   */
  export function and(...args: any[]): any

  /**
   * `or(a: Boolean, b: Boolean) -> out: Boolean`
   *
   * True when any input is true. Read in order, as JavaScript's || is: the first true decides, and the inputs after it aren't run. Takes more inputs with +.
   */
  export function or(...args: any[]): any

  /**
   * `not(v: Boolean) -> out: Boolean`
   *
   * The opposite of a true or false.
   */
  export function not(...args: any[]): any

  /**
   * `select(cond: Boolean, if_true: Any, if_false: Any) -> out: Any`
   *
   * Passes on if_true when cond is true, if_false when it's false.
   */
  export function select(...args: any[]): any

  /**
   * `cons(item: Any, list: List<Any>) -> out: List<Any>`
   *
   * A list with item put first, before the items of list.
   */
  export function cons(...args: any[]): any

  /**
   * `head(list: List<Any>) -> out: Any`
   *
   * A list's first item; nothing for an empty list.
   */
  export function head(...args: any[]): any

  /**
   * `tail(list: List<Any>) -> out: List<Any>`
   *
   * A list without its first item; nothing for an empty list.
   */
  export function tail(...args: any[]): any

  /**
   * `is_empty(list: List<Any>) -> out: Boolean`
   *
   * Whether a list has no items.
   */
  export function is_empty(...args: any[]): any

  /**
   * `fold(init: Any, list: List<Any>) -> out: Any  {fn}`
   *
   * Runs its function over a list, carrying a result along: starting from init, each item and the result so far give the next result.
   */
  export function fold(...args: any[]): any

  /**
   * `map(list: List<Any>) -> out: List<Any>  {fn}`
   *
   * Runs its function on each item of a list: the list of results.
   */
  export function map(...args: any[]): any

  /**
   * `math(x: Number) -> out: Number  {fn: abs}`
   *
   * One of JavaScript's Math functions of a number, chosen by fn: abs, sign, floor, ceil, trunc, round, sqrt, cbrt, exp, expm1, log, log2, log10, log1p, sin, cos, tan, their inverses and hyperbolic forms, fround, f16round, clz32. Produces nothing where JavaScript gives NaN or Infinity.
   */
  export function math(...args: any[]): any

  /**
   * `max(a: Number, b: Number) -> out: Number`
   *
   * The largest of its numbers (0 over -0). Takes more inputs with +.
   */
  export function max(...args: any[]): any

  /**
   * `min(a: Number, b: Number) -> out: Number`
   *
   * The smallest of its numbers (-0 under 0). Takes more inputs with +.
   */
  export function min(...args: any[]): any

  /**
   * `aggregate(xs: List<Number>) -> out: Number  {fn: max}`
   *
   * One number from a list of numbers, chosen by fn: its max, its min, hypot (the square root of the sum of the squares) or sum_precise (the exact sum, rounded once). The max or min of an empty list produces nothing.
   */
  export function aggregate(...args: any[]): any

  /**
   * `atan2(y: Number, x: Number) -> out: Number`
   *
   * The angle from the x axis to the point (x, y), in radians: Math.atan2(y, x).
   */
  export function atan2(...args: any[]): any

  /**
   * `power(a: Number, b: Number) -> out: Number`
   *
   * a to the power b, as Math.pow(a, b). Produces nothing where JavaScript gives NaN or Infinity (0 to a negative power, a negative number to a fraction).
   */
  export function power(...args: any[]): any

  /**
   * `remainder(a: Number, b: Number) -> out: Number`
   *
   * What is left of a after dividing by b, with a's sign: a % b. Dividing by zero produces nothing.
   */
  export function remainder(...args: any[]): any

  /**
   * `bitwise(a: Number, b: Number) -> out: Number  {op: and}`
   *
   * A 32-bit integer operation on two numbers, chosen by op: and, or, xor, shl, shr, ushr (JavaScript's &, |, ^, <<, >>, >>>) or imul (Math.imul).
   */
  export function bitwise(...args: any[]): any

  /**
   * `number_test(x: Any) -> out: Boolean  {test: finite, coerce: none}`
   *
   * Whether a value is a finite number, an integer, a safe integer or NaN, chosen by test. With coerce "none" only a number can pass (Number.isFinite); with "number" the value is read as a number first (isFinite, isNaN).
   */
  export function number_test(...args: any[]): any

  /**
   * `to_number(x: Any, radix: Number) -> out: Number  {grammar: number}`
   *
   * A value read as a number, by grammar: number (Number(x)), float (parseFloat) or int (parseInt, in radix when it is wired). Produces nothing when it doesn't read as a finite number.
   */
  export function to_number(...args: any[]): any

  /**
   * `number_format(x: Number, digits: Number) -> out: Text  {style: string}`
   *
   * A number written as text, by style: string (toString, in radix digits when wired), fixed (toFixed), precision (toPrecision) or exponential (toExponential), digits as each takes them. Digits out of range produce nothing.
   */
  export function number_format(...args: any[]): any

  /**
   * `concat(a: Text, b: Text) -> out: Text`
   *
   * Joins texts end to end. Takes more inputs with +.
   */
  export function concat(...args: any[]): any

  /**
   * `uppercase(s: Text) -> out: Text`
   *
   * The text in upper case.
   */
  export function uppercase(...args: any[]): any

  /**
   * `lowercase(s: Text) -> out: Text`
   *
   * The text in lower case.
   */
  export function lowercase(...args: any[]): any

  /**
   * `text_length(s: Text) -> out: Number`
   *
   * How long a text is in UTF-16 code units, as JavaScript's s.length counts: a character past U+FFFF, an emoji say, is two.
   */
  export function text_length(...args: any[]): any

  /**
   * `slice(value: Any, start: Number, end: Number) -> out: Any  {mode: length}`
   *
   * Part of a text or a list, by mode, a text in UTF-16 code units. length: end units or items from start (counting from 0). end: from start up to end, each counting from the end when negative, end the length when unwired (s.slice(start, end)). substring: a text's, from start up to end, each clamped to the text, the two swapped when end comes first. Nothing when a piece would cut an emoji in half.
   */
  export function slice(...args: any[]): any

  /**
   * `index_of(haystack: Any, needle: Any, from: Number) -> out: Number`
   *
   * Where needle first appears at or after from (0 when unwired): in a list, the first item that equals it, from counting from the end when negative; in a text, in UTF-16 code units. −1 when it doesn't.
   */
  export function index_of(...args: any[]): any

  /**
   * `split(text: Text, sep: Text, limit: Number) -> out: List<Text>`
   *
   * Cuts a text at each sep into a list of pieces; join's inverse. An empty sep cuts every UTF-16 code unit (nothing for a text with an emoji); limit, when wired, keeps that many pieces.
   */
  export function split(...args: any[]): any

  /**
   * `text_at(text: Text, index: Number) -> out: Text`
   *
   * The UTF-16 code unit of a text at index, as a text: s.charAt(i). The empty text past either end; nothing for half of an emoji.
   */
  export function text_at(...args: any[]): any

  /**
   * `code_at(text: Text, index: Number) -> out: Number  {mode: unit}`
   *
   * The number of a text's character at index: unit, the UTF-16 code unit (s.charCodeAt(i)); point, the code point an emoji begins there (s.codePointAt(i)). Nothing past either end.
   */
  export function code_at(...args: any[]): any

  /**
   * `from_codes(a: Number) -> out: Text  {mode: unit}`
   *
   * A text made of character numbers: unit, UTF-16 code units (String.fromCharCode); point, code points (String.fromCodePoint). Takes more inputs with +. Nothing when a half of an emoji is left alone, or a number isn't a code point.
   */
  export function from_codes(...args: any[]): any

  /**
   * `text_contains(text: Text, part: Text, position: Number) -> out: Boolean  {at: start}`
   *
   * Whether part begins a text at position (start, s.startsWith(t, p)), or ends it there (end, s.endsWith(t, p)). Unwired, position is the start or the end.
   */
  export function text_contains(...args: any[]): any

  /**
   * `trim(text: Text) -> out: Text  {side: both}`
   *
   * A text without JavaScript's white space and line ends on one side or both: s.trim(), s.trimStart(), s.trimEnd().
   */
  export function trim(...args: any[]): any

  /**
   * `pad(text: Text, length: Number, fill: Text) -> out: Text  {side: start}`
   *
   * A text filled to length UTF-16 code units at its start or end with fill, a space when unwired: s.padStart(n, f), s.padEnd(n, f).
   */
  export function pad(...args: any[]): any

  /**
   * `repeat(text: Text, count: Number) -> out: Text`
   *
   * A text count times over: s.repeat(k). Nothing for a negative count.
   */
  export function repeat(...args: any[]): any

  /**
   * `replace(text: Text, pattern: Text, replacement: Text) -> out: Text  {count: first}`
   *
   * A text with the first match of pattern, or every match, replaced: s.replace(a, b), s.replaceAll(a, b). In the replacement, $& is the match, $` what comes before it, $' what comes after and $$ a $.
   */
  export function replace(...args: any[]): any

  /**
   * `normalize(text: Text) -> out: Text  {form: NFC}`
   *
   * A text in a Unicode normalization form, NFC, NFD, NFKC or NFKD: s.normalize(form).
   */
  export function normalize(...args: any[]): any

  /**
   * `uri_encode(text: Text) -> out: Text  {set: component}`
   *
   * A text escaped for a URL: component (encodeURIComponent) escapes all but letters, digits and -_.!~*'(); uri (encodeURI) keeps ;/?:@&=+$,# too.
   */
  export function uri_encode(...args: any[]): any

  /**
   * `uri_decode(text: Text) -> out: Text  {set: component}`
   *
   * A text with its %XX escapes read back as UTF-8: decodeURIComponent, decodeURI (uri, which leaves ;/?:@&=+$,# escaped). Nothing for an escape that isn't valid.
   */
  export function uri_decode(...args: any[]): any

  /**
   * `is_well_formed(value: Any) -> out: Boolean`
   *
   * True for a text, which is always well formed in Holon: s.isWellFormed(). Nothing for anything else.
   */
  export function is_well_formed(...args: any[]): any

  /**
   * `list(a: Any, b: Any) -> out: List<Any>`
   *
   * A list of its inputs, in order. Takes more inputs with +.
   */
  export function list(...args: any[]): any

  /**
   * `get(list: List<Any>, index: Number) -> out: Any`
   *
   * The item of a list at index (counting from 0); nothing past its end.
   */
  export function get(...args: any[]): any

  /**
   * `append(list: List<Any>, item: Any) -> out: List<Any>`
   *
   * A list with item added at the end.
   */
  export function append(...args: any[]): any

  /**
   * `extend(list: List<Any>, items: List<Any>) -> out: List<Any>`
   *
   * A list with every item of items added at the end.
   */
  export function extend(...args: any[]): any

  /**
   * `length(list: List<Any>) -> out: Number`
   *
   * How many items a list has.
   */
  export function length(...args: any[]): any

  /**
   * `sum(list: List<Number>) -> out: Number`
   *
   * The total of a list of numbers.
   */
  export function sum(...args: any[]): any

  /**
   * `join(list: List<Text>, sep: Text) -> out: Text`
   *
   * Joins a list of texts into one, with sep between each.
   */
  export function join(...args: any[]): any

  /**
   * `at(value: Any, index: Number) -> out: Any`
   *
   * The item of a list, or the UTF-16 code unit of a text, at index, counting from the end when it is negative (xs.at(-1) is the last); nothing out of range or for half of an emoji.
   */
  export function at(...args: any[]): any

  /**
   * `indices(list: List<Any>) -> out: List<Number>`
   *
   * A list's positions: 0 to its length − 1 (xs.keys()).
   */
  export function indices(...args: any[]): any

  /**
   * `entries(value: Any) -> out: List<Any>`
   *
   * A list's [index, item] pairs, or a record's [key, value] pairs in key order.
   */
  export function entries(...args: any[]): any

  /**
   * `any(list: List<Any>) -> out: Boolean  {fn}`
   *
   * Whether its function answers true for any item of a list, stopping at the first (xs.some). Its answer is read as JavaScript reads a condition: false, 0 and "" are false, anything else true.
   */
  export function any(...args: any[]): any

  /**
   * `all(list: List<Any>) -> out: Boolean  {fn}`
   *
   * Whether its function answers true for every item of a list, stopping at the first false (xs.every).
   */
  export function all(...args: any[]): any

  /**
   * `filter(list: List<Any>) -> out: List<Any>  {fn}`
   *
   * The items of a list its function answers true for, in order.
   */
  export function filter(...args: any[]): any

  /**
   * `find(list: List<Any>) -> out: Any  {fn}`
   *
   * The first item of a list its function answers true for, stopping there; nothing when none does.
   */
  export function find(...args: any[]): any

  /**
   * `find_index(list: List<Any>) -> out: Number  {fn}`
   *
   * The position of the first item its function answers true for, stopping there; −1 when none does.
   */
  export function find_index(...args: any[]): any

  /**
   * `find_last(list: List<Any>) -> out: Any  {fn}`
   *
   * The last item of a list its function answers true for, looking from the end; nothing when none does.
   */
  export function find_last(...args: any[]): any

  /**
   * `find_last_index(list: List<Any>) -> out: Number  {fn}`
   *
   * The position of the last item its function answers true for, looking from the end; −1 when none does.
   */
  export function find_last_index(...args: any[]): any

  /**
   * `sort_by(list: List<Any>) -> out: List<Any>  {fn}`
   *
   * The items of a list in the order its function puts them: given two items a and b, it answers a number, below 0 for a first, above 0 for b first, 0 to keep their order. Stable, and the same order every time. An answer that isn't a number produces nothing.
   */
  export function sort_by(...args: any[]): any

  /**
   * `includes(haystack: Any, needle: Any, from: Number) -> out: Boolean`
   *
   * Whether an item of a list at or after from equals needle (lists and records by what they hold), or a text holds needle at or after from; from 0 when unwired.
   */
  export function includes(...args: any[]): any

  /**
   * `last_index_of(haystack: Any, needle: Any, from: Number) -> out: Number`
   *
   * The position of the last item of a list that equals needle, or of needle in a text, at or before from (the end when unwired); −1 when there is none.
   */
  export function last_index_of(...args: any[]): any

  /**
   * `reverse(list: List<Any>) -> out: List<Any>`
   *
   * The items of a list in reverse order.
   */
  export function reverse(...args: any[]): any

  /**
   * `to_spliced(list: List<Any>, start: Number, count: Number, items: List<Any>) -> out: List<Any>`
   *
   * A list with count items from start replaced by the items of items: start counts from the end when negative; count unwired is the rest of the list, items unwired is none.
   */
  export function to_spliced(...args: any[]): any

  /**
   * `with_item(list: List<Any>, index: Number, value: Any) -> out: List<Any>`
   *
   * A list with the item at index replaced by value, index counting from the end when negative; nothing out of range.
   */
  export function with_item(...args: any[]): any

  /**
   * `fill(list: List<Any>, value: Any, start: Number, end: Number) -> out: List<Any>`
   *
   * A list with the positions from start up to end set to value, each counting from the end when negative; start unwired is 0, end the length.
   */
  export function fill(...args: any[]): any

  /**
   * `copy_within(list: List<Any>, target: Number, start: Number, end: Number) -> out: List<Any>`
   *
   * A list with the items from start up to end copied over those from target, as they were before; each counts from the end when negative, start unwired is 0, end the length.
   */
  export function copy_within(...args: any[]): any

  /**
   * `sort(list: List<Any>) -> out: List<Any>`
   *
   * The items of a list in JavaScript's default order: each written as text and compared character by character (so 10 comes before 9). Stable.
   */
  export function sort(...args: any[]): any

  /**
   * `flat(list: List<Any>) -> out: List<Any>  {depth: 1}`
   *
   * A list with the items of its lists spread into it, depth levels deep (a whole number, or all).
   */
  export function flat(...args: any[]): any

  /**
   * `is_list(value: Any) -> out: Boolean`
   *
   * Whether a value is a list.
   */
  export function is_list(...args: any[]): any

  /**
   * `unique(list: List<Any>) -> out: List<Any>`
   *
   * Each item of a list once, where it first appears (lists and records by what they hold).
   */
  export function unique(...args: any[]): any

  /**
   * `range(n: Number) -> out: List<Number>`
   *
   * The numbers from 0 to n-1, as a list: map over it to do something n times. n is a whole number up to 10000; any other is nothing.
   */
  export function range(...args: any[]): any

  /**
   * `pair(a: Any, b: Any) -> out: (Any, Any)`
   *
   * Two values held together as one.
   */
  export function pair(...args: any[]): any

  /**
   * `some(value: Any) -> out: Any | Unit`
   *
   * An optional value that is there: holds its input.
   */
  export function some(...args: any[]): any

  /**
   * `none() -> out: Any | Unit`
   *
   * An optional value that isn't there.
   */
  export function none(...args: any[]): any

  /**
   * `record() -> out: Record`
   *
   * An empty record, to add fields to with with_field.
   */
  export function record(...args: any[]): any

  /**
   * `field(record: Record) -> out: Any  {name: id}`
   *
   * One field of a record, by the name set on the node; nothing when the record hasn't it.
   */
  export function field(...args: any[]): any

  /**
   * `with_field(record: Record, value: Any) -> out: Record  {name: id}`
   *
   * The record with a field set to value, by the name set on the node (added, or replaced).
   */
  export function with_field(...args: any[]): any

  /**
   * `without_field(record: Record) -> out: Record  {name: id}`
   *
   * The record without the named field.
   */
  export function without_field(...args: any[]): any

  /**
   * `keys(record: Record) -> out: List<Text>`
   *
   * The names of a record's fields, in order.
   */
  export function keys(...args: any[]): any

  /**
   * `lookup(record: Record, key: Text) -> out: Any`
   *
   * One field of a record, by a key that arrives on a wire (a route's path param, say); nothing when it isn't there.
   */
  export function lookup(...args: any[]): any

  /**
   * `insert(record: Record, key: Text, value: Any) -> out: Record`
   *
   * The record with key set to value, the key arriving on a wire.
   */
  export function insert(...args: any[]): any

  /**
   * `remove(record: Record, key: Text) -> out: Record`
   *
   * The record without key, the key arriving on a wire.
   */
  export function remove(...args: any[]): any

  /**
   * `values(record: Record) -> out: List<Any>`
   *
   * A record's values, in key order: Object.values(r).
   */
  export function values(...args: any[]): any

  /**
   * `from_entries(entries: List<Any>) -> out: Record`
   *
   * A record made of [key, value] pairs, a later key replacing an earlier: Object.fromEntries(es). Nothing when a pair isn't a list of a text key and a value.
   */
  export function from_entries(...args: any[]): any

  /**
   * `merge(a: Record, b: Record) -> out: Record`
   *
   * Every field of its records, a later record's where two have one: { ...a, ...b }. Shallow: a record inside is replaced, not merged. Takes more records with +.
   */
  export function merge(...args: any[]): any

  /**
   * `has(record: Record, key: Text) -> out: Boolean`
   *
   * Whether a record has the key, with any value, null included: k in r, Object.hasOwn(r, k).
   */
  export function has(...args: any[]): any

  /**
   * `group_by(list: List<Any>) -> out: Record  {fn}`
   *
   * The items of a list in lists by the key its function answers for each, a text: Object.groupBy(xs, f). Each list keeps the items' order; the keys come sorted. A key that isn't a text produces nothing.
   */
  export function group_by(...args: any[]): any

  /**
   * `same_value(a: Any, b: Any) -> out: Boolean`
   *
   * Whether two values are the same as JavaScript's Object.is says: -0 isn't 0. Lists and records compare by what they hold.
   */
  export function same_value(...args: any[]): any

  /**
   * `type_of(value: Any) -> out: Text`
   *
   * What JavaScript's typeof says of a value: "number", "boolean", "string" or "object" (a list, a record or null).
   */
  export function type_of(...args: any[]): any

  /**
   * `json_parse(text: Text) -> out: Any  {fn}`
   *
   * Reads JSON text into a value (records, lists, numbers, text…); nothing when it isn't valid JSON. With fn, a function of each key and value, applied from the inside out, its answer standing in for the value (JSON.parse(t, reviver)); one it answers nothing for is left out.
   */
  export function json_parse(...args: any[]): any

  /**
   * `json_stringify(value: Any) -> out: Text  {indent, keys, fn}`
   *
   * Writes a value out as JSON text, as JavaScript's JSON.stringify: indent lays it out a member a line, keys (a JSON list) keeps only those keys of every record, in that order, and fn, a function of each key and value applied from the outside in, writes its answer instead (the replacer); one it answers nothing for is left out.
   */
  export function json_stringify(...args: any[]): any

  /**
   * `date_part(time: Any, offset: Any) -> out: Number  {part: time}`
   *
   * A date's time value, or one of its parts in UTC, chosen by part: time (new Date(x): a number of milliseconds since 1970, or a text as date_parse reads it), year, month (0 to 11), date, day (0 is Sunday), hours, minutes, seconds, milliseconds (getUTCFullYear() and the rest). Given an offset (+01:00, or minutes east of UTC), the part there: its hours, its date. Nothing for a date out of range.
   */
  export function date_part(...args: any[]): any

  /**
   * `date_utc(year: Number) -> out: Number`
   *
   * The time value of a date and time in UTC: Date.UTC(year, month, date, hours, minutes, seconds, ms), month counting from 0, the parts after the year optional (month 0, date 1, the rest 0); a year 0 to 99 is 1900 plus it. Takes the parts after the year with +.
   */
  export function date_utc(...args: any[]): any

  /**
   * `date_set(time: Any, value: Number) -> out: Number  {part: year}`
   *
   * A date with parts replaced, from part on, the rest its own: new Date(t).setUTCHours(h, m, s, ms) and the other setUTC… methods, as the time value they give. The date is a time value or a text, as for new Date(t). A part past its end carries (hour 25 is 1 the next day).
   */
  export function date_set(...args: any[]): any

  /**
   * `date_text(time: Any, offset: Any) -> out: Text  {format: iso}`
   *
   * A date as text, by format: iso (new Date(t).toISOString(): 2000-02-29T12:00:00.000Z), date (2000-02-29) or utc (toUTCString(): Tue, 29 Feb 2000 12:00:00 GMT). Given an offset (+01:00, or minutes east of UTC), iso and date are the time there, iso written with it: 2000-02-29T13:00:00.000+01:00. The date is a time value or a text, as for new Date(t).
   */
  export function date_text(...args: any[]): any

  /**
   * `date_parse(text: Text) -> out: Number`
   *
   * The time value a date's text gives: Date.parse(t), in JavaScript's date format (2000-02-29, 2000-02-29T12:00:00Z, …). A time without Z or an offset produces nothing: Holon has no local time.
   */
  export function date_parse(...args: any[]): any

  /**
   * `date_add(time: Any, amount: Number, offset: Any) -> out: Number  {unit: days}`
   *
   * A date amount days, months or years later (earlier when negative), by unit, as a time value. A month keeps the day of the month, or is the month's last day when it has fewer: January 31 and a month is February 28. Given an offset, its calendar's months.
   */
  export function date_add(...args: any[]): any

  /**
   * `business_day_add(date: Any, days: Number, holidays: List<Any>) -> out: Text  {weekend: sat_sun}`
   *
   * The date days business days after date (before it when negative), as YYYY-MM-DD; with 0, date itself when it is a business day, else the next. A business day is in neither the weekend (sat_sun by default, fri_sat or none) nor holidays, a list of dates such as 2026-12-25.
   */
  export function business_day_add(...args: any[]): any

  /**
   * `business_days_between(from: Any, to: Any, holidays: List<Any>) -> out: Number  {weekend: sat_sun}`
   *
   * How many business days come after from up to to, to included; negative when to is before from. Dates are YYYY-MM-DD texts or time values; holidays, a list of dates, and the weekend as for business_day_add.
   */
  export function business_days_between(...args: any[]): any

  /**
   * `is_business_day(date: Any, holidays: List<Any>) -> out: Boolean  {weekend: sat_sun}`
   *
   * Whether a date (YYYY-MM-DD, or a time value's date in UTC) is in neither the weekend (sat_sun by default, fri_sat or none) nor holidays, a list of dates.
   */
  export function is_business_day(...args: any[]): any

  /**
   * `sha256(text: Any) -> out: Text  {encoding: hex}`
   *
   * The SHA-256 digest of a text (its UTF-8) or of bytes, as hex or base64.
   */
  export function sha256(...args: any[]): any

  /**
   * `hmac_sha256(key: Any, message: Any) -> out: Text  {encoding: hex}`
   *
   * The HMAC-SHA256 signature of message under key, each a text or bytes, as hex or base64: how webhooks are signed.
   */
  export function hmac_sha256(...args: any[]): any

  /**
   * `hex(text: Text) -> out: Text  {mode: encode}`
   *
   * Encodes text as hex, or decodes hex back to text.
   */
  export function hex(...args: any[]): any

  /**
   * `base64(text: Text) -> out: Text  {mode: encode, alphabet: standard, bytes: utf8}`
   *
   * Encodes text as base64, or decodes it back; standard or URL-safe alphabet. Its bytes are the text's UTF-8, or with latin1 each character one byte: btoa(s), atob(s).
   */
  export function base64(...args: any[]): any

  /**
   * `secure_equals(a: Any, b: Any) -> out: Boolean`
   *
   * Whether two texts, or two byte strings, are equal, compared in constant time: for checking signatures and tokens without leaking where they differ.
   */
  export function secure_equals(...args: any[]): any

  /**
   * `text_to_bytes(text: Text) -> out: Bytes  {encoding: utf8}`
   *
   * A text's bytes: its UTF-8, or what its hex, base64 or base64url says (nothing if it isn't that). new TextEncoder().encode(s), Uint8Array.fromHex(s), Uint8Array.fromBase64(s).
   */
  export function text_to_bytes(...args: any[]): any

  /**
   * `bytes_to_text(bytes: Bytes) -> out: Text  {encoding: utf8}`
   *
   * Bytes as a text: read as UTF-8 (nothing if they aren't), or written as hex, padded base64 or unpadded base64url. new TextDecoder().decode(b), b.toHex(), b.toBase64().
   */
  export function bytes_to_text(...args: any[]): any

  /**
   * `bytes_length(bytes: Bytes) -> out: Number`
   *
   * How many bytes: b.length.
   */
  export function bytes_length(...args: any[]): any

  /**
   * `bytes_concat(a: Bytes, b: Bytes) -> out: Bytes`
   *
   * Bytes joined, in order.
   */
  export function bytes_concat(...args: any[]): any

  /**
   * `bytes_to_list(bytes: Bytes) -> out: List<Number>`
   *
   * Bytes as a list of numbers from 0 to 255: Array.from(b).
   */
  export function bytes_to_list(...args: any[]): any

  /**
   * `list_to_bytes(list: List<Any>) -> out: Bytes`
   *
   * A list of whole numbers from 0 to 255 as bytes: new Uint8Array(xs). Nothing if an item isn't one, where JavaScript wraps it.
   */
  export function list_to_bytes(...args: any[]): any

  /**
   * `decimal_add(a: Any, b: Any) -> out: Text`
   *
   * The exact sum of decimals written as text (1234.56), or of numbers as their shortest text: 0.1 and 0.2 give 0.3. Its scale is the largest of theirs: 1.10 and 2 give 3.10. Nothing for a text that isn't a decimal.
   */
  export function decimal_add(...args: any[]): any

  /**
   * `decimal_sub(a: Any, b: Any) -> out: Text`
   *
   * a − b, exactly, its scale the larger of theirs.
   */
  export function decimal_sub(...args: any[]): any

  /**
   * `decimal_mul(a: Any, b: Any) -> out: Text`
   *
   * The exact product of decimals, its scale the sum of theirs: 19.99 times 3 is 59.97.
   */
  export function decimal_mul(...args: any[]): any

  /**
   * `decimal_div(a: Any, b: Any, scale: Number) -> out: Text  {rounding: half_even}`
   *
   * a ÷ b to scale places, rounded by rounding (half_even by default). Nothing when b is zero.
   */
  export function decimal_div(...args: any[]): any

  /**
   * `decimal_round(a: Any, scale: Number) -> out: Text  {rounding: half_even}`
   *
   * A decimal to scale places, padded with zeros (5 to 2 places is 5.00), rounded by rounding: half_even (a bank's, the default), half_up, half_down, up (away from zero), down (toward zero), ceiling or floor.
   */
  export function decimal_round(...args: any[]): any

  /**
   * `decimal_sum(list: List<Any>) -> out: Text`
   *
   * The exact sum of a list's decimals: a control sum. 0 for an empty list.
   */
  export function decimal_sum(...args: any[]): any

  /**
   * `decimal_compare(a: Any, b: Any) -> out: Number`
   *
   * −1, 0 or 1 as a is less than, equal to or more than b, by value: 1.10 and 1.1 are equal.
   */
  export function decimal_compare(...args: any[]): any

  /**
   * `csv_parse(text: Text) -> out: List<Any>  {header: first_row, separator: ,}`
   *
   * Reads CSV (RFC 4180): with header first_row (the default), a list of records keyed by the first row's names; with none, a list of lists. Every field is a text. Nothing for a malformed file: a quote that never closes, a row with more or fewer fields than the header, a column named twice.
   */
  export function csv_parse(...args: any[]): any

  /**
   * `csv_format(rows: List<Any>, columns: List<Text>) -> out: Text  {header: first_row, separator: ,}`
   *
   * Writes CSV: given columns, each row a record written in that order (a missing field empty), the names first unless header is none; without, each row a list. A field holding the separator, a quote or a line break is quoted, and each line ends with CRLF.
   */
  export function csv_format(...args: any[]): any

  /**
   * `xml_parse(text: Text) -> out: List<Any>`
   *
   * Reads XML 1.0 into its root element, a list as JsonML writes one: the name, then a record of its attributes when it has any, then its children in order, each an element or a text. Names are kept as written, a prefix too; comments, instructions and the white space between elements are dropped. Nothing for text that isn't well-formed, or that has a DOCTYPE: no entity is ever defined or fetched.
   */
  export function xml_parse(...args: any[]): any

  /**
   * `xml_format(element: List<Any>) -> out: Text  {declaration: utf8, layout: compact}`
   *
   * Writes an element, a list as xml_parse gives one, as XML: the declaration first unless declaration is none, on one line or indented two spaces a level (layout). Texts and attributes are escaped, a number or a Boolean written as JavaScript writes it. Nothing for a name that isn't an XML name, a character XML can't hold, or an item that is neither an element, a text, a number nor a Boolean.
   */
  export function xml_format(...args: any[]): any

  /**
   * `xml_find(element: List<Any>, path: Text) -> out: List<Any>`
   *
   * The elements along a path of names separated by /, the first among the element's children, in document order; * is any name. An empty list when none matches.
   */
  export function xml_find(...args: any[]): any

  /**
   * `xml_text(element: List<Any>, path: Text) -> out: Text`
   *
   * An element's text, every text in it at any depth joined, as the DOM's textContent; given a path, the first match's. Nothing when none matches.
   */
  export function xml_text(...args: any[]): any

  /**
   * `xml_attribute(element: List<Any>, name: Text) -> out: Text`
   *
   * An element's attribute, by name; nothing when it has none so named.
   */
  export function xml_attribute(...args: any[]): any

  /**
   * `gzip(data: Any) -> out: Bytes`
   *
   * Gzips a text (its UTF-8) or bytes: deflated at level 6, with no file name and time 0, so the same data gives the same bytes everywhere.
   */
  export function gzip(...args: any[]): any

  /**
   * `gunzip(bytes: Bytes) -> out: Bytes`
   *
   * The bytes inside a gzip, each member's joined. Nothing for a header, checksum or length that is wrong, bytes after the last member that aren't another, or past 64 MiB inflated.
   */
  export function gunzip(...args: any[]): any

  /**
   * `zip(files: Record) -> out: Bytes`
   *
   * Zips a record of files, name to a text (its UTF-8) or bytes: each deflated, in name order, dated 1980-01-01. Nothing for a name that would land outside the folder it is unzipped into (empty, a directory's, starting with /, holding a .. part or a backslash), a value that isn't a text or bytes, or more than 10,000 files.
   */
  export function zip(...args: any[]): any

  /**
   * `unzip(bytes: Bytes) -> out: Record`
   *
   * A zip's files, a record of name to bytes, its directories left out. Nothing for what isn't a zip, an entry encrypted or compressed other than stored or deflated, a checksum or size that doesn't match, two entries of one name, the 64-bit form, more than 10,000 files or past 64 MiB inflated.
   */
  export function unzip(...args: any[]): any

  /**
   * `jwt_verify(token: Text, key: Any, now: Number) -> out: Record  {issuer, audience, leeway: 0}`
   *
   * A JWT's claims, its payload as a record, when its signature verifies with key and they hold at now (seconds, as exp is): exp required and after now, nbf not after it, each widened by leeway; iss equal to issuer and aud holding audience, each when given. The key decides the algorithm, never the token: a shared secret (a text or bytes that isn't a public key, at least the hash's length) allows HS256, HS384 and HS512; an RSA key (2,048 to 4,096 bits) RS256 to PS512; a P-256 key ES256; an Ed25519 key EdDSA. A public key is a PEM (a text or bytes), a DER key (bytes) or a JWK (a record or its JSON), whatever whitespace or byte-order mark comes first, and never a secret; a JWKS gives the key the token's kid names. Nothing for a token that doesn't verify, alg none, or crit.
   */
  export function jwt_verify(...args: any[]): any

  /**
   * `jwt_decode(token: Text) -> out: Record`
   *
   * A JWT's header and payload, {header, payload}, verifying nothing: what it says before it is trusted, to pick the key that checks it. Nothing for what isn't three base64url parts whose header and payload are JSON objects.
   */
  export function jwt_decode(...args: any[]): any

  /**
   * `pgp_encrypt(data: Any, keys: Any, random: Bytes, now: Number) -> out: Bytes  {armor: binary}`
   *
   * An OpenPGP message: data (a text's UTF-8, or bytes) encrypted to each of keys, to its newest encryption key valid at now, as GnuPG reads it: a session key for each, then AES-256 with the modification detection code over the data, dated now. random is the run's random bytes (random_bytes), from which every random byte is drawn, so a rerun writes the same message. A key is a public key, armored or bytes: version 4, RSA (2,048 to 4,096 bits) or Curve25519, valid by its own signatures (not expired, not revoked). armor ascii gives the armored text, as bytes. Nothing for a key without an encryption key valid at now, or fewer than 32 random bytes.
   */
  export function pgp_encrypt(...args: any[]): any

  /**
   * `pgp_verify(data: Any, signature: Any, keys: Any, now: Number) -> out: Record`
   *
   * {signer, key, created} when one of signature's signatures (detached, armored or bytes) over data verifies with one of keys: the primary key's fingerprint, the signing key's, and when it signed. The key must be able to sign and be valid when it signed (not expired, not revoked; a subkey bound by its primary and its own back-signature), the signature made no later than now and not expired at it, over SHA-224 to SHA-512. A text signature is over the data with CR LF line endings. Nothing otherwise, SHA-1 signatures included.
   */
  export function pgp_verify(...args: any[]): any

  /**
   * `pgp_key(key: Any) -> out: Record`
   *
   * What one public key says, as its own signatures bear out: {fingerprint, user_ids, created, expires, revoked}, its fingerprint as GnuPG writes it (40 hex digits, upper case), expires absent when it never does. What a person compares before trusting a key. Nothing for several keys at once, a secret key, or a key no self-signature holds for.
   */
  export function pgp_key(...args: any[]): any

  /**
   * `to_text(value: Any) -> out: Text`
   *
   * Any value written out as text, as a literal: a text comes out in quotes.
   */
  export function to_text(...args: any[]): any

  /**
   * `as_text(value: Any) -> out: Text`
   *
   * Any value as it reads in a message: a text is itself, a number or true/false as written, a record or list as JSON. For putting a value into a text.
   */
  export function as_text(...args: any[]): any

  /**
   * `is_truthy(value: Any) -> out: Boolean`
   *
   * Whether a value counts as true, as JavaScript's Boolean(x) says: false for false, 0, NaN, empty text and none; true for anything else, an empty list or record included. How if (x) and !x read a value that isn't true or false.
   */
  export function is_truthy(...args: any[]): any

  /**
   * `guard(cond: Boolean, value: Any) -> out: Any`
   *
   * Passes value on only when cond is true; otherwise nothing, so what follows doesn't run.
   */
  export function guard(...args: any[]): any

  /**
   * `coalesce(a: Any, b: Any) -> out: Any`
   *
   * Passes a on, or b when a didn't arrive: a fallback that is itself computed, where default's is a fixed value.
   */
  export function coalesce(...args: any[]): any

  /**
   * `equals(a: Any, b: Any) -> out: Boolean`
   *
   * Whether two values are the same.
   */
  export function equals(...args: any[]): any

  /**
   * `fail(message: Text) -> out: Any`
   *
   * Nothing, with message given to the run's log as the reason: JavaScript's throw new Error(message). What reads it has no value, unless a ?? or a catch gives one.
   */
  export function fail(...args: any[]): any

  /**
   * `http_request(path: Text, headers: Record, body: Any, trigger: Any) -> out: Record  {method: GET, host: example.com}`
   *
   * Calls the host set on the node (HTTPS) with the method, path, headers and body, and answers {status, headers, body}. Performed by the server between rounds, within the deployment's limits; the host is a permission it asks for. Holds back when a wired input didn't arrive.
   */
  export function http_request(...args: any[]): any

  /**
   * `http_upload(path: Text, headers: Record, file: Record, trigger: Any) -> out: Record  {method: PUT, host: example.com}`
   *
   * Calls the host set on the node like http_request, with the file a handle names as the body: the server streams its bytes from the folder (one the deployment reads), never through the flow, and fails the call if the file changed since the run read it.
   */
  export function http_upload(...args: any[]): any

  /**
   * `http_stream(path: Text, headers: Record, body: Any, trigger: Any) -> out: Record  {method: POST, host: example.com, pipe}`
   *
   * Calls the host set on the node like http_request, and the server forwards the answer's body to the route's caller as it arrives (a 2xx answer; the reply is 200 with the answer's content type): a model's tokens, say, as they come. With pipe set to a channel's name, the pieces go to that channel's subscribers instead, as they arrive (a shared live transcript). The flow still gets the whole answer, as http_request's.
   */
  export function http_stream(...args: any[]): any

  /**
   * `flow_call(input: Any, trigger: Any) -> out: Record  {flow: billing, action: charge}`
   *
   * Runs the action set on the node of the deployed flow set on the node, with the input (a value; {} when none is wired), and answers {answer, run} when it ran, or {error, message} when it didn't (not_found, not_running, no_action, waiting, failed, too_deep, refused). Performed by the server between rounds, in-process, as an ordinary action run of the other flow, logged there as run by this flow; the pair is a permission it asks for. Holds back when a wired input didn't arrive.
   */
  export function flow_call(...args: any[]): any

  /**
   * `flow_start(input: Any, trigger: Any) -> out: Record  {flow: billing, action: charge}`
   *
   * Starts the action set on the node of the deployed flow set on the node (this flow's own, or another's), with the input (a value; {} when none is wired), and doesn't wait for it to end: the server runs it in-process until it finishes or first waits, and answers then, {answer, run} when it finished, {run, waiting} when it waits (waiting names each point it waits at, with the token that resumes it), or {error, message} when it didn't run. Never waits itself, so a route hands out the link of a run that waits. The pair is a permission it asks for. Holds back when a wired input didn't arrive.
   */
  export function flow_start(...args: any[]): any

  /**
   * `clock() -> out: Number`
   *
   * The time the run started, in seconds since 1970. The same everywhere in one run, and recorded, so a replay sees it too.
   */
  export function clock(...args: any[]): any

  /**
   * `random() -> out: Number`
   *
   * A random number from 0 up to 1, the same everywhere in one run, and recorded: read from the run's random bytes.
   */
  export function random(...args: any[]): any

  /**
   * `random_bytes() -> out: Bytes`
   *
   * The run's 32 random bytes, the same everywhere in one run, and recorded: what pgp_encrypt draws a message's randomness from.
   */
  export function random_bytes(...args: any[]): any

  /**
   * `secret() -> out: Text  {name: api_key}`
   *
   * A secret's value, by the name set on the node, from the deployment's secrets (set under ⚙). Never logged: it shows as ‹secret:name›. Reading it is a permission.
   */
  export function secret(...args: any[]): any

  /**
   * `file_arrival() -> out: Record  {folder: files, on: new, matching}`
   *
   * The file in a folder that started the run: its handle {folder, name, size, hash, modified, event}, event new or changed; for a file deleted (on: deleted), as last seen, without a hash. A flow with one runs once for each file that arrives there and settles (or, on: deleted, is removed), its name matching the glob matching if set, not on requests.
   */
  export function file_arrival(...args: any[]): any

  /**
   * `mail_arrival() -> out: Record  {mail: ops}`
   *
   * The message that came to the mailbox of the mail resource an admin bound to the name set on the node (mail), which started the run: {from, name, to, cc, subject, date, message_id, text, html, attachments, authenticated, verdict}. authenticated is the mailbox's own server's word that the domain in From sent it (DMARC passed); From alone proves nothing. attachments is a record of each file's name to its text (a text, JSON, XML or CSV type) or bytes. A server's on.mail handler runs once for each message, which is moved to the binding's done folder once the run is over.
   */
  export function mail_arrival(...args: any[]): any

  /**
   * `feed_event() -> out: Record  {feed: prices}`
   *
   * The event the feed an admin bound to the name set on the node (feed) sent, which started the run: {data, id, event}. data is a value when the event's data is JSON, else its text; id is the event's id when the feed gives one; event is its name, message unless the feed names it. A server's on.feed handler runs once for each event, one at a time, in the order they came; the host keeps where the feed is, so a reconnect resumes after the last event run on, and an event run on already isn't run again.
   */
  export function feed_event(...args: any[]): any

  /**
   * `queue_message() -> out: Record  {queue: payouts}`
   *
   * The message that came to the queue an admin bound to the name set on the node (queue), which started the run: {data, id, attributes, sent, count, group}. data is its body, a value when it is JSON, else its text; id is the queue's id for it; attributes its message attributes of type String or Number, as text; sent when it was sent, in seconds since 1970; count how many times the queue has handed it out; group a FIFO queue's message group. A server's on.queue handler runs once for each message, one at a time, in the order they came; the host deletes it once its run is over, and a message run on already isn't run again.
   */
  export function queue_message(...args: any[]): any

  /**
   * `resume_token() -> out: Text  {await: event}`
   *
   * The token that delivers an event to the waiting point (await) named on the node: put it in a link or a callback. Never logged.
   */
  export function resume_token(...args: any[]): any

  /**
   * `store_read() -> out: Any  {key: value}`
   *
   * The deployment's state under the key set on the node, as the run started. Reading it is a permission.
   */
  export function store_read(...args: any[]): any

  /**
   * `store_write(value: Any, trigger: Any) -> out: Effect  {key: value}`
   *
   * Sets the deployment's state under the key set on the node to value, when the run finishes. Writing it is a permission.
   */
  export function store_write(...args: any[]): any

  /**
   * `collection_get(key: Text) -> out: Any  {collection: items}`
   *
   * The item of a collection under key, as the run found the collection; nothing when there is none. Reading it is a permission.
   */
  export function collection_get(...args: any[]): any

  /**
   * `collection_list(prefix: Text, after: Text) -> out: Record  {collection: items, limit: 100}`
   *
   * A record of a collection's items, by key, in key order, up to limit: those whose key starts with prefix, after the key after (both optional, for paging).
   */
  export function collection_list(...args: any[]): any

  /**
   * `collection_put(key: Text, value: Any, trigger: Any) -> out: Effect  {collection: items}`
   *
   * Puts value into a collection under key when the run finishes (added, or replaced). Writing it is a permission.
   */
  export function collection_put(...args: any[]): any

  /**
   * `collection_delete(key: Text, trigger: Any) -> out: Effect  {collection: items}`
   *
   * Deletes a collection's item under key when the run finishes.
   */
  export function collection_delete(...args: any[]): any

  /**
   * `file_read(name: Text) -> out: Text  {folder: files}`
   *
   * The text of the file name in a folder, as the run found it; nothing when there is none. The folder is a name an admin binds to a real folder. Reading it is a permission.
   */
  export function file_read(...args: any[]): any

  /**
   * `file_list(prefix: Text, after: Text) -> out: Record  {folder: files, limit: 100}`
   *
   * A record of a folder's files (name, size, hash, modified) in name order, up to limit: those whose name starts with prefix, after the name after (both optional, for paging).
   */
  export function file_list(...args: any[]): any

  /**
   * `file_write(name: Text, text: Any, trigger: Any) -> out: Effect  {folder: files}`
   *
   * Writes a text, or bytes as they are, to the file name in a folder when the run finishes (created, or replaced whole). Writing it is a permission.
   */
  export function file_write(...args: any[]): any

  /**
   * `file_delete(name: Text, trigger: Any) -> out: Effect  {folder: files}`
   *
   * Deletes the file name in a folder when the run finishes.
   */
  export function file_delete(...args: any[]): any

  /**
   * `file_handle(name: Text) -> out: Record  {folder: files}`
   *
   * A handle to the file name in a folder, of any size, without its bytes: {folder, name, size, hash, modified}; nothing when there is none. Copy it to another folder, or pass it on.
   */
  export function file_handle(...args: any[]): any

  /**
   * `file_lines(name: Text, from: Number) -> out: List<Text>  {folder: files, limit: 100}`
   *
   * Up to limit lines of the file name in a folder, from line from (0 is the first; -100 is the 100th from the end), without their line endings. Only those lines are read into the flow; the log proves them part of the file.
   */
  export function file_lines(...args: any[]): any

  /**
   * `file_slice(name: Text, offset: Number, length: Number) -> out: Text  {folder: files}`
   *
   * length bytes of the file name in a folder from byte offset (from the end when negative), as text. Only those bytes are read into the flow; the log proves them part of the file.
   */
  export function file_slice(...args: any[]): any

  /**
   * `file_copy(name: Text, from: Record, trigger: Any) -> out: Effect  {folder: files}`
   *
   * Copies the file a handle names into this folder as name when the run finishes, host to host: its bytes never pass through the flow. Fails if the file changed since the run read it.
   */
  export function file_copy(...args: any[]): any

  /**
   * `file_read_bytes(name: Text) -> out: Bytes  {folder: files}`
   *
   * The bytes of the file name in a folder, any file, as the run found it; nothing when there is none. folder.bytes(name). Reading it is a permission.
   */
  export function file_read_bytes(...args: any[]): any

  /**
   * `file_create(name: Text, text: Any, trigger: Any) -> out: Effect  {folder: files}`
   *
   * Writes a text, or bytes as they are, to the file name in a folder when the run finishes, only if no file has that name then: it never replaces one (the run answers 500 if one is there).
   */
  export function file_create(...args: any[]): any

  /**
   * `file_create_copy(name: Text, from: Record, trigger: Any) -> out: Effect  {folder: files}`
   *
   * Copies the file a handle names into this folder as name when the run finishes, only if no file has that name then: it never replaces one.
   */
  export function file_create_copy(...args: any[]): any

  /**
   * `db_query(params: List<Any>) -> out: List<Record>  {database: db, sql: select * from t}`
   *
   * The rows sql selects from a database, as records by column name, as the run found them; params are its $1, $2, …. The database is a name an admin binds. Reading each table it names is a permission.
   */
  export function db_query(...args: any[]): any

  /**
   * `db_one(params: List<Any>) -> out: Record  {database: db, sql: select * from t}`
   *
   * The first row sql selects from a database, as a record by column name; nothing when there is none. params are its $1, $2, ….
   */
  export function db_one(...args: any[]): any

  /**
   * `db_exec(params: List<Any>, trigger: Any) -> out: Effect  {database: db, sql: delete from t where false, expect}`
   *
   * Runs sql (an insert, update or delete) on a database when the run finishes, in one transaction with the run's other writes there; params are its $1, $2, …. With expect, it must change exactly that many rows, or nothing the run did there lands.
   */
  export function db_exec(...args: any[]): any

  /**
   * `db_find(params: List<Any>) -> out: List<Record>  {database: db, collection: items, filter: {}, sort, limit}`
   *
   * The documents filter selects from a collection of a document database (MongoDB), as records, as the run found them; params are its $1, $2, …. sort orders them and limit caps how many. Reading the collection is a permission.
   */
  export function db_find(...args: any[]): any

  /**
   * `db_find_one(params: List<Any>) -> out: Record  {database: db, collection: items, filter: {}, sort}`
   *
   * The first document filter selects from a collection, as a record; nothing when there is none. params are its $1, $2, ….
   */
  export function db_find_one(...args: any[]): any

  /**
   * `db_insert(params: List<Any>, trigger: Any) -> out: Effect  {database: db, collection: items, document: {}}`
   *
   * Adds document to a collection when the run finishes, in one transaction with the run's other writes there; params are its $1, $2, ….
   */
  export function db_insert(...args: any[]): any

  /**
   * `db_update(params: List<Any>, trigger: Any) -> out: Effect  {database: db, collection: items, filter: {}, update: {"$set":{}}, expect}`
   *
   * Applies update (its $set, $inc, … operators) to the documents filter selects, when the run finishes; params are their $1, $2, …. With expect, it must match exactly that many documents, or nothing the run did there lands.
   */
  export function db_update(...args: any[]): any

  /**
   * `db_delete(params: List<Any>, trigger: Any) -> out: Effect  {database: db, collection: items, filter: {}, expect}`
   *
   * Deletes the documents filter selects when the run finishes; params are its $1, $2, …. With expect, it must delete exactly that many, or nothing the run did there lands.
   */
  export function db_delete(...args: any[]): any

  /**
   * `channel_send(pack: Any, trigger: Any) -> out: Effect  {channel: updates}`
   *
   * Sends pack on the channel set on the node once the run is over and its writes have landed: every caller subscribed to the channel gets it as a server-sent event. Sending on it is a permission.
   */
  export function channel_send(...args: any[]): any

  /**
   * `channel_subscribe() -> out: Effect  {channel: updates, from: new}`
   *
   * A subscription to the channel set on the node, for a route to answer with: the caller gets each pack sent on it from then on (from "kept": the packs the channel keeps first), as server-sent events. Letting callers subscribe is a permission.
   */
  export function channel_subscribe(...args: any[]): any

  /**
   * `file_serve(file: Record) -> out: Effect  {folder: site}`
   *
   * A route's answer that is the file a handle of the folder set on the node names: the host sends its bytes as they are, with the type its name says, if the file is still the handle's. Answering with a folder's files is a permission.
   */
  export function file_serve(...args: any[]): any

  /**
   * `mail_send(message: Record, trigger: Any) -> out: Effect  {mail: ops, to: *}`
   *
   * Sends message through the mail resource an admin bound to the name set on the node (mail), once the run is over and its writes have landed: {to, cc, subject, text, html, attachments}, to and cc an address or a list, attachments a record of a file name to its text or bytes. From is the binding's, never the flow's. to set on the node is the recipients the text names, or * when one is computed: a recipient it doesn't name gives nothing, and the host sends only to those the binding allows. At most 50 recipients and 10 MiB. Sending through it, to each recipient, is a permission.
   */
  export function mail_send(...args: any[]): any

  /**
   * `queue_send(message: Record, trigger: Any) -> out: Effect  {queue: payouts}`
   *
   * Sends message to the queue an admin bound to the name set on the node (queue), once the run is over and its writes have landed: {data, group}. data is the body, sent as JSON (text as it is); group, text, is a FIFO queue's message group, whose messages come out in the order sent. Sending to it is a permission.
   */
  export function queue_send(...args: any[]): any

  /**
   * `log(message: Any, trigger: Any) -> out: Effect  {level: log}`
   *
   * Writes message as a line of the deployment's log, at the level set on the node (log, info, debug, warn or error, as the console's methods), once the run is over. The line is kept by the host beside the run, and is no effect the run performed: `holon verify` checks the run without it, and a replay writes none. What the run computed to write it, a template's text say, is in its record as any value is. Text is written as it is, any other value as JSON; a line computed from a secret is withheld, as the record withholds an effect's payload computed from one.
   */
  export function log(...args: any[]): any

  /**
   * `egress(payload: Any, trigger: Any) -> out: Effect`
   *
   * Sends its payload out of the flow as a message, when the run finishes.
   */
  export function egress(...args: any[]): any
}
