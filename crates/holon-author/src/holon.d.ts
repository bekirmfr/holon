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

declare module "holon" {
  // Only what is marked export leaves the module.
  export {}
  const answer: unique symbol

  /** A length of time, a number and its unit: `"10s"`, `"5m"`, `"2h"`, `"1d"`. */
  export type Duration = `${number}${"s" | "m" | "h" | "d"}`
  /** How a handler's runs are recorded: strict when left out, `"lazy"` or `"unrecorded"`. */
  export type Log = "lazy" | "unrecorded"
  /** A route's path: `/` and segments, `:name` for a part `req.params.name` reads. */
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
  }
  /** A route: `route.post("/orders", (req) => …)`. The most specific path wins. */
  export const route: Routes

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
  export const on: {
    file: {
      /** Runs for each file new or changed in the folder, once it stops changing. */
      created(folder: Folder, handler: FileHandler, options?: FileOptions): void
      /** Runs for each file removed from the folder. */
      deleted(folder: Folder, handler: FileHandler, options?: FileOptions): void
    }
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
  /** A named folder: `dir[name]` reads a file's text as the run began, `dir[name] = text` writes it. */
  export interface Folder {
    [name: string]: any
    get(name: string): any
    /** The file's bytes, any file: an image, a PDF, an archive. */
    bytes(name: string): Uint8Array
    put(name: string, content: string | FileHandle): any
    delete(name: string): any
    /** A page of files: `{size, hash, modified}` by name. */
    list(page?: Page): any
    /** A handle to a file of any size, no bytes. */
    file(name: string): FileHandle
    /** Lines from line `from` (from the end when negative). */
    lines(name: string, options?: { from?: number; limit?: number }): any
    /** Bytes from `offset`, as text. */
    slice(name: string, options?: { offset?: number; length?: number }): any
    copy(name: string, handle: FileHandle): any
    /** Writes the file only if there is none yet: else the run fails, and nothing it did lands. */
    create(name: string, text: string): any
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
}
/** Another deployed flow's actions, by its slug: `await charge(x)` runs one, `charge(x)` starts it. */
declare module "flow:*"
