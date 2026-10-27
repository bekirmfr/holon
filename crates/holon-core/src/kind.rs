//! # Kind — the four-bucket classification of what a unit *is* (P2.1.1)
//!
//! `Kind` answers "does this touch the world, and how?" with the coarsest useful
//! partition: [`Pure`](Kind::Pure), [`Driver`](Kind::Driver),
//! [`State`](Kind::State), [`Sink`](Kind::Sink). It is the entry point of the
//! effect layer (P2) and the seam where the pure core (P1) starts to acknowledge
//! that a *runtime* will eventually run.
//!
//! ## Kind is informative, not identity
//!
//! Kind is a **computed view**, never a stored or hashed field. A primitive's kind
//! follows authoritatively from its name (already in the hash); a composite's kind
//! is *derived* from its children (P2.2.1). There is no `kind` field to set, so
//! laundering an effect into a "pure" label is not *prevented* — it is
//! *inexpressible* (invariant `I8`).
//!
//! ## This table is the trust basis — and it is closed
//!
//! [`kind_of`] is core-resident for **every** leaf, effectful ones included: core
//! *declares* `egress` a [`Sink`](Kind::Sink) without being able to *run* it —
//! execution lives in `holon-host`. That split is sound because the leaf set is
//! **closed and trusted**: the permissionless marketplace ships only *composites*
//! (whose kind derives, no declaration), while new *leaves* (new effects) arrive
//! only as trusted host releases ("syscalls"). So there is no untrusted-author
//! declares-a-leaf's-kind surface to defend.
//!
//! Two boundary invariants keep the core/host split honest. Neither can be
//! exercised until host execution exists (P2.4.1 / P2.6.1), so they are recorded
//! here and tested then:
//!
//! 1. **Disjointness** — a leaf is pure-computable-in-core *xor*
//!    effect-executable-in-host, never both. The names [`crate::primitives`]
//!    computes all classify [`Pure`](Kind::Pure); the names the host executes all
//!    classify non-`Pure`; the two sets do not intersect.
//! 2. **No effect without classification** — every name the host can fire as an
//!    effect classifies non-`Pure` here. An effect the host runs that this table
//!    calls `Pure` *is* the laundering hole; this containment forbids it.

/// What a primitive or composite *is*, with respect to effects.
///
use crate::{Hash, Param, Resolver, Unit, UnitBody};
use std::collections::BTreeSet;

/// The non-`Pure` variants are the three ways purity ends, in increasing
/// commitment: a [`Driver`](Kind::Driver) *reads* the world, a
/// [`State`](Kind::State) cell *remembers* across firings, a [`Sink`](Kind::Sink)
/// *changes* the world irreversibly.
///
/// `PartialOrd`/`Ord` are derived only so [`Kind`] can sit in a `BTreeSet`
/// ([`Effects`]) with deterministic iteration — a content-addressed system must
/// not have order wobble. The resulting order (`Pure < Driver < State < Sink`) is
/// **storage order, not a danger priority**: we deliberately do *not* rank the
/// effect axes against each other.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Kind {
    /// No effect: the output is a pure function of the inputs. The
    /// empty-capability case — touches nothing outside what it is given.
    Pure,
    /// Sources stimulus from outside the graph (a clock tick, an inbound
    /// request). Reads the world; does not change it.
    Driver,
    /// Retains a value across firings — the one fenced place purity ends. The
    /// donor primitive from which durable effect contracts are composed (P2.6).
    State,
    /// Performs an irreversible consequence on the world (send a mail, write a
    /// file). The committing edge of the system.
    Sink,
}

impl Kind {
    /// `true` for anything other than [`Pure`](Kind::Pure) — i.e. it touches the
    /// world in some way. This is the coarse gate; the capability surface (P2.3)
    /// refines *what* it touches.
    pub fn is_effectful(self) -> bool {
        self != Kind::Pure
    }
}

// ── Capabilities: the fine-grained manifest (the *primary* classification) ────
//
// Capabilities are primary; [`Kind`] is their coarse four-bucket *projection*
// (P2.3.1). Capability is finer than kind on purpose: `Clock` and `SecretAccess`
// both project to `Driver`, but a consent surface must tell "reads the clock" from
// "reads your secrets" — the distinction kind discards and capabilities keep. By
// *derivable ⟹ derived*, kind is computed from capabilities, never asserted beside
// them, so the two cannot drift.

/// A category of effect — what a leaf touches outside its inputs. Grouped by the
/// [`Kind`] each projects to. Every variant is atomic (no payload) **except**
/// `StateRead`/`StateWrite`, which carry the state key they name — see the
/// keyed-state-capability L4. That's a deliberate, narrower precision upgrade than
/// the general "resource parameter" idea named early in this project's own history
/// (`NetworkEgress("api.evil.com")` and friends stay atomic; only the two state
/// capabilities are keyed, on the concrete need that actually showed up).
///
/// `PartialOrd`/`Ord` exist for deterministic [`BTreeSet`] storage only — storage
/// order, not a danger ranking (same disclaimer as [`Kind`]). **No longer `Copy`**
/// (a `String` payload can't be): every call site that used to rely on a bare copy
/// now clones instead — real, but mechanical (see `kind`/`is_sourced` below, both
/// widened to take `&self` rather than `self` so the ripple stays contained there).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Capability {
    /// Reads wall-clock / timers.
    Clock,
    /// Receives inbound data / requests.
    NetworkIngress,
    /// Reads files of the named folder: `file_read`, `file_list`, `file_handle`, `file_lines`,
    /// `file_slice`. Answered by the host from the
    /// folder an admin bound to that name (`docs/L4s/named-folders-L4.md`).
    FileRead(String),
    /// Starts a run for each new file in the named folder: `file_arrival`, whose value is the
    /// file's handle. The host watches the folder an admin bound to that name.
    FileArrival(String),
    /// Reads a secret / credential (a security-sensitive read).
    SecretAccess,
    /// Reads the named secret from the deployment's secret store: `secret`. Per name, so
    /// consent names which secret a flow may read.
    SecretRead(String),
    /// Waits for an event at the named waiting point, a run continuing when it arrives (`await`,
    /// and `resume_token`, which hands out the token that delivers it). A durable run.
    Await(String),
    /// Sources entropy / nondeterminism.
    Random,
    /// Reads a value retained across firings, at the named key.
    StateRead(String),
    /// Writes a value retained across firings, at the named key.
    StateWrite(String),
    /// Reads items of the named collection: `collection_get`, `collection_list`. Answered by the
    /// host from the collection as the run found it.
    CollectionRead(String),
    /// Puts or deletes items of the named collection: `collection_put`, `collection_delete`,
    /// committed when the run finishes.
    CollectionWrite(String),
    /// Sends outbound (mail, HTTP write).
    NetworkEgress,
    /// Makes an HTTP request to the named host (`host` or `host:port`) and gets its answer:
    /// `http_request`. Per host, so consent names where a flow may call, not just "the network".
    HttpRequest(String),
    /// Writes, copies into and deletes files of the named folder: `file_write`, `file_copy`,
    /// `file_delete`, `file_create`, `file_create_copy`, committed when
    /// the run finishes.
    FileWrite(String),
    /// Sends packs on the named channel: `channel_send`, sent to its subscribers once the run is
    /// over and its writes have landed (`docs/L4s/channels-L4.md`).
    ChannelSend(String),
    /// Lets callers subscribe to the named channel: `channel_subscribe`, a route answering with
    /// the stream of its packs.
    ChannelRead(String),
    /// Starts a process / job.
    Spawn,
    /// Reads rows of a table of a database the host binds: `"shop/orders"`, the database's name
    /// for the flow, then the table, found in the statement a `db_query`, `db_one` or `db_exec`
    /// sends (sql.rs). Answered as the run found it, and checked again when it commits
    /// (`docs/L4s/databases-L4.md`).
    DbRead(String),
    /// Inserts, updates or deletes rows of a table of a database the host binds (`db_exec`),
    /// in one transaction with the run's other writes there, when it finishes.
    DbWrite(String),
}

/// The sentinel key `saturate_caps` uses for the fail-closed "this closure touches
/// state at some key I can't identify" case — an unresolvable reference or unknown
/// leaf can't name a real key (key-space is unbounded strings, nothing to
/// enumerate), so this stands in for "unspecified, be suspicious." **Never a value
/// `store_read`/`store_write` legitimately construct** — `state_key` (below) never
/// produces it from a real, resolved param. One honest, un-closed gap: nothing
/// currently stops a real unit from choosing this exact string as its own key,
/// which would be indistinguishable from the saturation marker. Low practical risk,
/// cheap to close later (reject this string at deploy time) if it ever matters —
/// not solved now.
const SATURATED_STATE_KEY: &str = "<unknown>";

impl Capability {
    /// The coarse [`Kind`] bucket this capability projects to — *read the world* ⇒
    /// `Driver`, *retain* ⇒ `State`, *commit irreversibly* ⇒ `Sink`. Never
    /// [`Kind::Pure`]: purity is the **empty** capability set, not a projection
    /// target. Takes `&self` (not `self`) — `Capability` is no longer `Copy`, and a
    /// method that doesn't need to consume its receiver shouldn't force one.
    pub fn kind(&self) -> Kind {
        use Capability::*;
        match self {
            Clock | NetworkIngress | FileRead(_) | FileArrival(_) | SecretAccess | SecretRead(_) | Await(_) | Random => Kind::Driver,
            StateRead(_) | StateWrite(_) | CollectionRead(_) | CollectionWrite(_) | DbRead(_) | DbWrite(_) => Kind::State,
            NetworkEgress | HttpRequest(_) | FileWrite(_) | ChannelSend(_) | ChannelRead(_) | Spawn => Kind::Sink,
        }
    }

    /// **Sourced** (input-side) ⇔ the host *supplies* this value to the graph (a clock
    /// tick, an inbound request, a file/secret/random read, **a state read**). The
    /// complement is **performed** (output-side): the host *executes* it (egress, file
    /// write, spawn, **state write**). Design mode mocks the sourced side; deploy
    /// performs the rest. The `State` kind splits here — `StateRead` enters at the top,
    /// `StateWrite` exits at the bottom — which is what "time in, consequences out"
    /// always meant. Takes `&self`, same reason as [`kind`](Capability::kind).
    pub fn is_sourced(&self) -> bool {
        use Capability::*;
        matches!(
            self,
            Clock | NetworkIngress | FileRead(_) | FileArrival(_) | SecretAccess | SecretRead(_) | Await(_) | Random | StateRead(_) | CollectionRead(_) | DbRead(_)
        )
    }
}

/// The capability manifest of a unit — the set it **may** touch. Same shape as
/// [`Effects`]: **∅ ⇒ pure** (a positive "touches nothing outside its inputs"), a
/// may-touch upper bound. The union over a closure (P2.3.2) is the same fold as
/// [`effects_of`], with `Capability` in place of [`Kind`].
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Capabilities(BTreeSet<Capability>);

impl Capabilities {
    /// No capabilities ⇒ pure.
    pub fn is_pure(&self) -> bool {
        self.0.is_empty()
    }
    /// Alias of [`is_pure`](Capabilities::is_pure) (the empty manifest is pure).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Does the manifest include this capability?
    pub fn contains(&self, cap: Capability) -> bool {
        self.0.contains(&cap)
    }
    /// Number of distinct capabilities (`0` ⇒ pure).
    pub fn len(&self) -> usize {
        self.0.len()
    }
    /// Iterate the capabilities in deterministic order. `cloned()`, not `copied()` —
    /// `Capability` is no longer `Copy` (the `StateRead`/`StateWrite` keys carry a
    /// `String`).
    pub fn iter(&self) -> impl Iterator<Item = Capability> + '_ {
        self.0.iter().cloned()
    }
    /// The manifest of a unit as deployed: on a schedule (a clock flow's `every`, with no
    /// routes), what its graph derives and `Clock`, which starts its runs whether or not the flow
    /// reads the time; otherwise what its graph derives. The one place that rule is kept: the
    /// host deploys and grants by it, and `holon check` and the editor show it.
    pub fn deployed(mut self, scheduled: bool) -> Self {
        if scheduled {
            self.0.insert(Capability::Clock);
        }
        self
    }
    /// Replayable / cacheable ⇔ **no `State` capability anywhere in the closure** — the
    /// one place the design forfeits caching and replay (`§7`). This is *orthogonal* to
    /// effect-bearing: a sink-bearing but state-free manifest is effect-bearing **and**
    /// replayable — the combination the memo market (P3.3) keys on. Drivers are inputs,
    /// sinks purely construct requests (Fork B); only retained state breaks determinism
    /// across firings. (The positive form of P2.6.3's "cell-bearing ⇒ non-replayable".)
    pub fn is_replayable(&self) -> bool {
        !self.iter().any(|c| c.kind() == Kind::State)
    }
}

/// Classify a **primitive leaf**'s capability manifest by name (and, for the two
/// state-cell leaves, its own `params`) — the *primary* leaf classification
/// (P2.3.1), from which [`kind_of`] and [`effects_of`] are projected.
///
/// - pure leaf ⇒ `Some(∅)` — the positive "touches nothing outside its inputs";
/// - effectful exemplar ⇒ `Some({…})`;
/// - unknown leaf, **or `store_read`/`store_write` with no resolvable `"key"`
///   param**, ⇒ `None` — **fail closed**: a caller treats it at least as
///   conservatively as the full capability set, never as ∅. A state-cell leaf with
///   no key is not a leaf this engine vouches for, the identical standard an
///   unrecognized name already gets held to.
///
/// Closed core table: the marketplace ships only composites (which *derive*); new
/// effect leaves are trusted host releases that add a row here. Core *declares* the
/// manifest; `holon-host` *honours* it at execution.
pub fn capabilities_of(name: &str, params: &[Param]) -> Option<Capabilities> {
    use Capability::*;
    let caps: Vec<Capability> = match name {
        // Pure leaves: the first-order vocabulary `compute` implements, plus
        // higher-order `map` (effect-free). A positive empty manifest.
        "const"
        | "add" | "multiply" | "subtract" | "divide"
        | "compare" | "and" | "or" | "not" | "select"
        | "concat" | "uppercase" | "lowercase" | "text_length" | "slice" | "index_of" | "split"
        | "list" | "get" | "append" | "extend" | "length" | "sum" | "join" | "range"
        | "cons" | "head" | "tail" | "is_empty"
        | "pair"
        | "some" | "none"
        | "record" | "field" | "with_field" | "without_field" | "keys"
        | "lookup" | "insert" | "remove" | "find"
        | "json_parse" | "json_stringify"
        | "sha256" | "hmac_sha256" | "hex" | "base64" | "secure_equals"
        | "to_text" | "as_text" | "is_truthy" | "guard" | "default" | "coalesce" | "equals"
        | "fold" | "map" => vec![],

        // Effectful exemplars (classification only; execution is host-side). A state
        // cell is two leaves: `store_read` sources its slot (input-side), `store_write`
        // commits to it (output-side) — see the P2.6.1 L4. Both are now **keyed** — see
        // the keyed-state-capability L4 — so each needs its own `"key"` param to
        // classify at all.
        "store_read" => vec![StateRead(state_key(params)?)],
        "store_write" => vec![StateWrite(state_key(params)?)],
        // A collection: keyed items, read by key or in order (answered by the host from the
        // collection as the run found it), put and deleted when the run finishes. The collection
        // is a frozen param, so the capability names it exactly.
        "collection_get" | "collection_list" => vec![CollectionRead(ident(params, "collection")?)],
        "collection_put" | "collection_delete" => vec![CollectionWrite(ident(params, "collection")?)],
        // A named folder: files read as the run found them, written and deleted when it finishes.
        // The folder is a frozen param, so the capability names it exactly; the host binds the
        // name to a real folder.
        "file_read" | "file_list" | "file_handle" | "file_lines" | "file_slice" => vec![FileRead(ident(params, "folder")?)],
        "file_write" | "file_copy" | "file_delete" | "file_create" | "file_create_copy" => vec![FileWrite(ident(params, "folder")?)],
        // A new file in a named folder starts a run: the host watches the folder, and the file's
        // handle is the leaf's value. The folder is a frozen param, so the trigger names it.
        "file_arrival" => vec![FileArrival(ident(params, "folder")?)],
        // A database's tables, from the statement the leaf sends: its SQL is a frozen param, so
        // what it reads and writes is known before it runs (sql.rs). A statement that can't be
        // followed classifies as nothing, and fails closed.
        "db_query" | "db_one" => db_tables(params, false)?,
        "db_exec" => db_tables(params, true)?,
        // A document database's collection, a frozen param as a statement's tables are: a find
        // reads it; an insert writes it; an update and a delete read what their filter selects and
        // write it. Each template is checked for what its part may say (documents.rs), and one that
        // can't be fails closed.
        "db_find" | "db_find_one" | "db_insert" | "db_update" | "db_delete" => db_collection(name, params)?,
        // A named channel: packs sent when the run is over, and a route's answer that subscribes
        // to them. The channel is a frozen param, so the capability names it exactly.
        "channel_send" => vec![ChannelSend(ident(params, "channel")?)],
        "channel_subscribe" => vec![ChannelRead(ident(params, "channel")?)],
        "clock" => vec![Clock],
        // A secret is sourced like a state read: the host supplies the named secret's value. The
        // name is a frozen param, so the capability names it exactly.
        "secret" => vec![SecretRead(secret_name(params)?)],
        // A durable run's waiting point, and the token that resumes it: answered by the host when
        // an event arrives (or the wait expires), never computed.
        "await" => vec![Await(ident(params, "name")?)],
        "resume_token" => vec![Await(ident(params, "await")?)],
        "random" => vec![Random],
        "egress" => vec![NetworkEgress],
        // An outbound HTTP request: performed by the host between rounds, its response sourced
        // back into the next one (the run observer's `answer`). The host is a frozen param, so
        // the capability names it exactly.
        "http_request" => vec![HttpRequest(http_host(params)?)],
        // The same request with a file's bytes as its body, streamed by the host from the folder
        // the handle names (one the deployment reads, checked when it is sent).
        "http_upload" => vec![HttpRequest(http_host(params)?)],
        // A request whose answer the host forwards as it reads it: to the route's caller, or,
        // piped, to a channel's subscribers, which is sending on that channel too.
        "http_stream" => match params.iter().find(|p| p.name() == "pipe").and_then(|p| p.value()).filter(|v| !v.is_empty()) {
            None => vec![HttpRequest(http_host(params)?)],
            Some(_) => vec![HttpRequest(http_host(params)?), ChannelSend(ident(params, "pipe")?)],
        },
        // Request/response (P2.5.1): `ingress` is the driver (a request arrives), `egress` carries
        // the reply out. §13 merges the old `email`/`respond` into this one `egress` sink — both
        // were NetworkEgress, and the host dispatches on the *capability*, not the sink name, so the
        // destination (mailbox, connection token, …) belongs in the payload, not in a distinct
        // primitive. The correlation token rides as ordinary data; isolation is purity's
        // (independent pure paths can't cross), so the token is only a boundary label.
        "ingress" => vec![NetworkIngress],

        // Unknown leaf ⇒ fail closed. The engine does not vouch for it.
        _ => return None,
    };
    Some(Capabilities(caps.into_iter().collect()))
}

/// The tables a database leaf's statement reads and writes, as capabilities naming the database
/// (its frozen `database` param) and the table: `DbRead("shop/orders")`.
fn db_tables(params: &[Param], write: bool) -> Option<Vec<Capability>> {
    let database = ident(params, "database")?;
    let sql = params.iter().find(|p| p.name() == "sql")?.value()?;
    let tables = crate::sql::tables(sql, write).ok()?;
    // The host's own table, where a commit marks a run done: never a flow's.
    if tables.reads.iter().chain(&tables.writes).any(|t| t.eq_ignore_ascii_case("holon_commits")) {
        return None;
    }
    if write {
        expect_of(params)?;
    }
    // A statement that touches no table asks the database nothing a flow can't compute itself,
    // and would need no permission: refused.
    if tables.reads.is_empty() && tables.writes.is_empty() {
        return None;
    }
    let reads = tables.reads.iter().map(|t| Capability::DbRead(format!("{database}/{t}")));
    let writes = tables.writes.iter().map(|t| Capability::DbWrite(format!("{database}/{t}")));
    Some(reads.chain(writes).collect())
}

/// The collection a document database leaf reads and writes, as capabilities naming the database
/// and the collection (`DbRead("notes/notes")`), once each of its templates says only what its
/// part may (documents.rs).
fn db_collection(name: &str, params: &[Param]) -> Option<Vec<Capability>> {
    use crate::documents::{check, is_collection_name, Part};
    let database = ident(params, "database")?;
    let collection = params.iter().find(|p| p.name() == "collection")?.value()?;
    if !is_collection_name(collection) {
        return None;
    }
    let mut holes = std::collections::BTreeSet::new();
    let mut template = |param: &str, part: Part| -> Option<()> {
        let text = params.iter().find(|p| p.name() == param)?.value()?;
        holes.extend(check(part, text).ok()?.1);
        Some(())
    };
    let optional = |param: &str| params.iter().find(|p| p.name() == param).map_or(Some(""), |p| p.value());
    let at = format!("{database}/{collection}");
    let caps = match name {
        "db_find" | "db_find_one" => {
            template("filter", Part::Filter)?;
            match optional("sort")? {
                "" => {}
                text => check(Part::Sort, text).ok().map(|_| ())?,
            }
            if name == "db_find" {
                limit_of(params)?;
            }
            vec![Capability::DbRead(at)]
        }
        "db_insert" => {
            template("document", Part::Document)?;
            vec![Capability::DbWrite(at)]
        }
        _ => {
            template("filter", Part::Filter)?;
            if name == "db_update" {
                template("update", Part::Update)?;
            }
            expect_of(params)?;
            vec![Capability::DbRead(at.clone()), Capability::DbWrite(at)]
        }
    };
    crate::documents::holes_in_order(&holes).ok()?;
    Some(caps)
}

/// How many documents a `db_find` answers at most: `Some(None)` for all (`limit` absent or ""),
/// `Some(Some(n))` for `n` from 1 to 100000; `None` (fail closed) for anything else.
pub(crate) fn limit_of(params: &[Param]) -> Option<Option<u64>> {
    match params.iter().find(|p| p.name() == "limit").map(|p| p.value()) {
        None | Some(Some("")) => Some(None),
        Some(Some(n)) if n.len() <= 6 && n.bytes().all(|b| b.is_ascii_digit()) => n.parse().ok().filter(|n| (1..=100_000).contains(n)).map(Some),
        Some(_) => None,
    }
}

/// The rows a `db_exec` must change: `Some(None)` for any number (`expect` absent or ""),
/// `Some(Some(n))` for exactly `n`; `None` (fail closed) for anything but a whole number.
pub(crate) fn expect_of(params: &[Param]) -> Option<Option<u64>> {
    match params.iter().find(|p| p.name() == "expect").map(|p| p.value()) {
        None | Some(Some("")) => Some(None),
        Some(Some(n)) if !n.is_empty() && n.len() <= 9 && n.bytes().all(|b| b.is_ascii_digit()) => n.parse().ok().map(Some),
        Some(_) => None,
    }
}

/// Read the required `"key"` frozen param for `store_read`/`store_write` — the state
/// key that capability now names. `None` (routing the caller to `capabilities_of`'s
/// own fail-closed `None` path) if the param is missing, still an unfilled `Open`
/// hole, or frozen to an empty string — a state-cell leaf with no concrete key
/// identifies nothing to key by.
fn state_key(params: &[Param]) -> Option<String> {
    let key = params.iter().find(|p| p.name() == "key")?.value()?;
    // A key starting with `#` is the host's (a collection's root lives at `#collection:<name>`).
    if key.is_empty() || key.starts_with('#') {
        None
    } else {
        Some(key.to_string())
    }
}

/// The required `"host"` frozen param of `http_request`: a hostname, optionally with `:port`.
/// `None` (fail closed) when missing, open, empty, or anything but a plain host: no scheme, path,
/// credentials or spaces, so the capability can only ever name a host.
pub(crate) fn http_host(params: &[Param]) -> Option<String> {
    let host = params.iter().find(|p| p.name() == "host")?.value()?;
    let (name, port) = match host.rsplit_once(':') {
        Some((name, port)) => (name, Some(port)),
        None => (host, None),
    };
    let name_ok = !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        });
    let port_ok = port.is_none_or(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u16>().is_ok_and(|n| n > 0));
    (name_ok && port_ok).then(|| host.to_ascii_lowercase())
}

/// The required `"name"` frozen param of `secret`: letters, digits, `-` and `_`, 1 to 64 of
/// them. `None` (fail closed) otherwise.
pub(crate) fn secret_name(params: &[Param]) -> Option<String> {
    ident(params, "name")
}

/// The frozen param `key` when it is a name: letters, digits, `-` and `_`, 1 to 64.
pub(crate) fn ident(params: &[Param], key: &str) -> Option<String> {
    let name = params.iter().find(|p| p.name() == key)?.value()?;
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    ok.then(|| name.to_string())
}

/// The coarse [`Kind`] of a **primitive leaf**, *projected* from its capability
/// manifest ([`capabilities_of`]) — the single source of leaf classification:
///
/// - unknown leaf, or a state-cell leaf with no resolvable key ⇒ `None` (fail-closed);
/// - pure leaf (∅ manifest) ⇒ `Some(Pure)`;
/// - effectful leaf ⇒ the [`Kind`] its capabilities project to.
///
/// A single-bucket **convenience**, exact for the current ≤1-axis vocabulary. The
/// general truth — a leaf may touch several axes — is the *set* [`effects_of`]
/// returns; when a multi-axis leaf first exists, this convenience yields to it.
pub fn kind_of(name: &str, params: &[Param]) -> Option<Kind> {
    let caps = capabilities_of(name, params)?;
    let first = caps.iter().next();
    Some(match first {
        None => Kind::Pure,
        Some(cap) => cap.kind(),
    })
}

/// The set of effect axes a unit **may** touch — its kind lifted from a single
/// bucket to the honest *set*, because a composite can drive, hold state, *and*
/// sink at once; collapsing that to one "worst" bucket would conceal real axes (a
/// soft cousin of laundering).
///
/// **Purity is the empty set.** [`Kind::Pure`] is never a member — it is the
/// *absence* of effect — so [`is_pure`](Effects::is_pure) is exactly "no axes".
/// This mirrors the capability model (a pure unit declares ∅), and is the shape
/// capability-union (P2.3.2) will reuse.
///
/// The set is a **may-touch upper bound** — a permission/assumption surface, not a
/// record of what ran — so an unclassified or unresolvable reference *saturates*
/// to all three axes. Over-approximating danger is the sound direction;
/// under-approximating to `Pure` would be the laundering hole.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Effects(BTreeSet<Kind>);

impl Effects {
    /// No effect axes ⇒ pure. The empty set *is* purity.
    pub fn is_pure(&self) -> bool {
        self.0.is_empty()
    }
    /// Alias of [`is_pure`](Effects::is_pure) (the empty set is pure).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Does the may-touch set include this axis?
    pub fn contains(&self, kind: Kind) -> bool {
        self.0.contains(&kind)
    }
    /// Number of distinct effect axes (`0` ⇒ pure).
    pub fn len(&self) -> usize {
        self.0.len()
    }
    /// Iterate the axes in deterministic order.
    pub fn iter(&self) -> impl Iterator<Item = Kind> + '_ {
        self.0.iter().copied()
    }
}

/// Derive the **capability manifest** of a unit by folding [`capabilities_of`]
/// over its closure (P2.3.2) — the fine-grained, security-relevant union of
/// everything the unit *may* touch, **computed not stored**, surfaced *before* a
/// unit is run or imported (I9). This is the single closure walk; [`effects_of`] is
/// its projection.
///
/// A primitive contributes its own manifest; a composite the union over its
/// children; **and any hash-valued param is followed as a reference**, so a `map`
/// over an effectful `fn` cannot hide its capabilities.
///
/// Fail-closed throughout — an unknown leaf, an unresolvable child, or a
/// hash-shaped param that does not resolve each *saturate* to the full set. The
/// analysis never assumes innocence it cannot verify, and does not depend on
/// validation having run first. Termination is free: content-addressing forbids a
/// cycle (a hash cannot contain itself), so the closure is a finite DAG.
pub fn capabilities(unit: &Unit, resolver: &impl Resolver) -> Capabilities {
    let mut acc = BTreeSet::new();
    collect_caps(unit, resolver, &mut acc);
    Capabilities(acc)
}

/// Derive the kind-set of a unit — the coarse **projection** of its
/// [`capabilities`]. There is one source of truth and one closure walk (the
/// capability union); kind is each capability's [`Kind`](Capability::kind) lifted
/// to the set. (Saturated capabilities project to the saturated kind-set
/// `{Driver, State, Sink}`, so fail-closed behaviour is preserved.)
pub fn effects_of(unit: &Unit, resolver: &impl Resolver) -> Effects {
    let mut acc = BTreeSet::new();
    for cap in capabilities(unit, resolver).iter() {
        acc.insert(cap.kind());
    }
    Effects(acc)
}

/// Insert every capability — the saturated, maximally-suspicious manifest. The two
/// state capabilities use [`SATURATED_STATE_KEY`] — "touches state at some
/// unspecified key" — since an unresolvable reference can't name a real one.
fn saturate_caps(acc: &mut BTreeSet<Capability>) {
    use Capability::*;
    for c in [
        Clock,
        NetworkIngress,
        FileRead(SATURATED_STATE_KEY.to_string()),
        SecretAccess,
        SecretRead(SATURATED_STATE_KEY.to_string()),
        Await(SATURATED_STATE_KEY.to_string()),
        Random,
        StateRead(SATURATED_STATE_KEY.to_string()),
        StateWrite(SATURATED_STATE_KEY.to_string()),
        CollectionRead(SATURATED_STATE_KEY.to_string()),
        CollectionWrite(SATURATED_STATE_KEY.to_string()),
        NetworkEgress,
        HttpRequest(SATURATED_STATE_KEY.to_string()),
        FileWrite(SATURATED_STATE_KEY.to_string()),
        ChannelSend(SATURATED_STATE_KEY.to_string()),
        ChannelRead(SATURATED_STATE_KEY.to_string()),
        Spawn,
        DbRead(SATURATED_STATE_KEY.to_string()),
        DbWrite(SATURATED_STATE_KEY.to_string()),
    ] {
        acc.insert(c);
    }
}

/// Every unit of the closure contributes once, however many references reach it, walked with a
/// stack rather than recursion: a closure is anyone's who can stage it, and neither a chain nested
/// deep nor a diamond of shared children may cost more than the units it holds.
fn collect_caps(root: &Unit, resolver: &impl Resolver, acc: &mut BTreeSet<Capability>) {
    let mut seen: BTreeSet<Hash> = BTreeSet::new();
    let mut stack: Vec<&Unit> = vec![root];
    while let Some(unit) = stack.pop() {
        unit_caps(unit, acc);
        // (2) Every reference — a composite's children and the `fn` param — via the shared
        //     `child_refs` (the one definition of what a unit references). A capability routed
        //     through a reference cannot hide; an unresolvable reference fails closed rather than
        //     being read as inert data.
        let server = crate::declaration_of(unit) == Some("server");
        for h in crate::inspect::child_refs(unit) {
            if server && resolver.resolve(&h).is_some_and(|c| crate::declaration_of(c) == Some("endpoint")) {
                acc.insert(Capability::NetworkIngress);
            }
            if !seen.insert(h) {
                continue;
            }
            match resolver.resolve(&h) {
                Some(u) => stack.push(u),
                None => saturate_caps(acc),
            }
        }
    }
}

/// What `unit` itself contributes to its closure's capabilities, its references aside.
fn unit_caps(unit: &Unit, acc: &mut BTreeSet<Capability>) {
    // (1) This unit's own leaf contribution, if it is a primitive: its capability
    //     manifest, read from its name **and its own params** (the state-cell leaves'
    //     key lives there). A pure leaf's ∅ contributes nothing; an unknown leaf, or a
    //     state-cell leaf with no resolvable key, fails closed.
    if let UnitBody::Primitive { name } = &unit.body {
        match capabilities_of(name, &unit.params) {
            Some(caps) => {
                for c in caps.iter() {
                    acc.insert(c);
                }
            }
            None => saturate_caps(acc), // unknown leaf ⇒ fail closed
        }
    }

    // (1b) A server declaration with a route listens for requests: the inbound side of its
    //      manifest (`collect_caps`, which sees its children; a server of clocks alone listens for
    //      none). Its handlers contribute the rest through (2).
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::to_hex;
    use crate::{Hash, MapResolver, Param, Unit, UnitBody};

    fn prim(name: &str) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: name.to_string() },
            params: vec![],
            inputs: vec![],
            outputs: vec![],
        }
    }

    /// A `store_read`/`store_write` primitive with a real `"key"` param — both now
    /// require one to classify at all (`capabilities_of` fails closed without it).
    fn keyed_state_prim(name: &str, key: &str) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: name.to_string() },
            params: vec![Param::frozen("key", key)],
            inputs: vec![],
            outputs: vec![],
        }
    }

    /// A composite over the given children. Edges are irrelevant to effect
    /// derivation (it walks children + params, never wiring), so they stay empty.
    fn composite(children: Vec<Hash>) -> Unit {
        Unit {
            body: UnitBody::Composite { children, edges: vec![] },
            params: vec![],
            inputs: vec![],
            outputs: vec![],
        }
    }

    /// A `map` primitive whose `fn` param references `target`.
    fn map_over(target: &Hash) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "map".to_string() },
            params: vec![Param::frozen("fn", to_hex(target))],
            inputs: vec![],
            outputs: vec![],
        }
    }

    /// Every leaf `compute` implements, plus `map`, classifies `Pure`. (Listed
    /// explicitly so a drift between this list and the table is a test failure.)
    #[test]
    fn collections_are_read_and_written_by_name() {
        let named = |n: &str| [Param::frozen("collection", n)];
        for (leaf, cap) in [
            ("collection_get", Capability::CollectionRead("notes".into())),
            ("collection_list", Capability::CollectionRead("notes".into())),
            ("collection_put", Capability::CollectionWrite("notes".into())),
            ("collection_delete", Capability::CollectionWrite("notes".into())),
        ] {
            assert_eq!(capabilities_of(leaf, &named("notes")).unwrap().iter().collect::<Vec<_>>(), vec![cap.clone()], "{leaf}");
            assert_eq!(cap.kind(), Kind::State);
            assert!(capabilities_of(leaf, &named("not a name")).is_none(), "{leaf} fails closed");
        }
        assert!(Capability::CollectionRead("x".into()).is_sourced() && !Capability::CollectionWrite("x".into()).is_sourced());
        // A collection's root lives in a host-reserved state cell no flow can name.
        for leaf in ["store_read", "store_write"] {
            assert!(capabilities_of(leaf, &[Param::frozen("key", "#collection:notes")]).is_none(), "{leaf}");
        }
    }

    #[test]
    fn folders_are_read_and_written_by_name() {
        let named = |n: &str| [Param::frozen("folder", n)];
        for (leaf, cap, kind) in [
            ("file_read", Capability::FileRead("inbox".into()), Kind::Driver),
            ("file_list", Capability::FileRead("inbox".into()), Kind::Driver),
            ("file_handle", Capability::FileRead("inbox".into()), Kind::Driver),
            ("file_lines", Capability::FileRead("inbox".into()), Kind::Driver),
            ("file_slice", Capability::FileRead("inbox".into()), Kind::Driver),
            ("file_copy", Capability::FileWrite("inbox".into()), Kind::Sink),
            ("file_write", Capability::FileWrite("inbox".into()), Kind::Sink),
            ("file_delete", Capability::FileWrite("inbox".into()), Kind::Sink),
            ("file_create", Capability::FileWrite("inbox".into()), Kind::Sink),
            ("file_create_copy", Capability::FileWrite("inbox".into()), Kind::Sink),
        ] {
            assert_eq!(capabilities_of(leaf, &named("inbox")).unwrap().iter().collect::<Vec<_>>(), vec![cap.clone()], "{leaf}");
            assert_eq!(cap.kind(), kind);
            assert!(capabilities_of(leaf, &named(r"C:\drop")).is_none(), "{leaf}: a folder is a name, never a path");
        }
        assert!(Capability::FileRead("x".into()).is_sourced() && !Capability::FileWrite("x".into()).is_sourced());
    }

    #[test]
    fn database_leaves_name_the_tables_their_statement_touches() {
        let leaf = |sql: &str, expect: &str| [Param::frozen("database", "shop"), Param::frozen("sql", sql), Param::frozen("expect", expect)];
        let caps = |name: &str, sql: &str, expect: &str| capabilities_of(name, &leaf(sql, expect)).map(|c| c.iter().collect::<Vec<_>>());
        use Capability::{DbRead, DbWrite};
        assert_eq!(caps("db_query", "select o.id from orders o join lines l on l.order_id = o.id", ""), Some(vec![DbRead("shop/lines".into()), DbRead("shop/orders".into())]));
        assert_eq!(caps("db_one", "select total from orders where id = $1", ""), Some(vec![DbRead("shop/orders".into())]));
        assert_eq!(caps("db_exec", "insert into refunds (id) values ($1)", ""), Some(vec![DbWrite("shop/refunds".into())]));
        assert_eq!(caps("db_exec", "update stock set n = n - $1 where sku = $2", "1"), Some(vec![DbRead("shop/stock".into()), DbWrite("shop/stock".into())]));
        assert_eq!(caps("db_exec", "update stock set n = 0", "one"), None, "expect is a whole number");
        assert_eq!(caps("db_query", "delete from stock", ""), None, "a query reads");
        assert_eq!(caps("db_exec", "drop table stock", ""), None, "no DDL");
        assert_eq!(caps("db_query", "select 1", ""), None, "a query reads a table");
        assert!(capabilities_of("db_query", &[Param::frozen("database", "a/b"), Param::frozen("sql", "select 1")]).is_none(), "a database is a name");
        assert_eq!(DbRead("x".into()).kind(), Kind::State);
        assert!(DbRead("x".into()).is_sourced() && !DbWrite("x".into()).is_sourced());
    }

    #[test]
    fn document_leaves_name_their_collection_once_their_templates_check() {
        use Capability::{DbRead, DbWrite};
        let caps = |name: &str, params: &[(&str, &str)]| {
            let mut all = vec![Param::frozen("database", "notes"), Param::frozen("collection", "notes")];
            for (k, v) in params {
                all.retain(|p| p.name() != *k);
                all.push(Param::frozen(*k, *v));
            }
            capabilities_of(name, &all).map(|c| c.iter().collect::<Vec<_>>())
        };
        let read = Some(vec![DbRead("notes/notes".into())]);
        assert_eq!(caps("db_find", &[("filter", r#"{"owner":$1}"#), ("sort", r#"{"at":-1}"#), ("limit", "20")]), read);
        assert_eq!(caps("db_find_one", &[("filter", r#"{"_id":$1}"#)]), read);
        assert_eq!(caps("db_insert", &[("document", r#"{"owner":$1,"text":$2}"#)]), Some(vec![DbWrite("notes/notes".into())]));
        assert_eq!(caps("db_update", &[("filter", r#"{"_id":$1}"#), ("update", r#"{"$set":{"text":$2}}"#), ("expect", "1")]), Some(vec![DbRead("notes/notes".into()), DbWrite("notes/notes".into())]));
        assert_eq!(caps("db_delete", &[("filter", r#"{"owner":$1}"#)]), Some(vec![DbRead("notes/notes".into()), DbWrite("notes/notes".into())]));
        assert_eq!(caps("db_find", &[("filter", r#"{"$where":"true"}"#)]), None, "no code on the server");
        assert_eq!(caps("db_find", &[("filter", "{}"), ("limit", "0")]), None, "a limit is a whole number above 0");
        assert_eq!(caps("db_find", &[("filter", "{}"), ("sort", r#"{"at":$1}"#)]), None, "a sort is the flow's");
        assert_eq!(caps("db_update", &[("filter", "{}"), ("update", r#"{"text":$1}"#)]), None, "an update is operators");
        assert_eq!(caps("db_insert", &[("document", r#"{"$set":{}}"#)]), None, "a document is data");
        assert_eq!(caps("db_delete", &[("filter", "{}"), ("expect", "many")]), None, "expect is a whole number");
        assert_eq!(caps("db_update", &[("filter", r#"{"_id":$1}"#), ("update", r#"{"$set":{"text":$3}}"#)]), None, "holes skip none");
        assert_eq!(caps("db_find", &[("collection", "holon_claims"), ("filter", "{}")]), None, "the host's collections");
        assert_eq!(caps("db_find", &[("collection", "system.users"), ("filter", "{}")]), None, "the server's");
    }

    #[test]
    fn pure_leaves_are_pure() {
        const PURE: &[&str] = &[
            "const", "add", "multiply",
            "subtract", "divide", "compare", "and", "or", "not", "select",
            "concat", "uppercase", "lowercase", "text_length", "slice", "index_of",
            "list", "get", "append", "length", "sum",
            "join", "cons", "head", "tail", "is_empty", "pair", "some", "none", "to_text", "as_text", "is_truthy", "guard", "default", "coalesce", "equals", "fold", "map",
        ];
        for name in PURE {
            assert_eq!(kind_of(name, &[]), Some(Kind::Pure), "{name} should be Pure");
        }
    }

    #[test]
    fn effectful_exemplars_classify() {
        let key = [Param::frozen("key", "var1")];
        assert_eq!(kind_of("store_read", &key), Some(Kind::State));
        assert_eq!(kind_of("store_write", &key), Some(Kind::State));
        assert_eq!(kind_of("clock", &[]), Some(Kind::Driver));
        assert_eq!(kind_of("random", &[]), Some(Kind::Driver));
        assert_eq!(kind_of("egress", &[]), Some(Kind::Sink));
    }

    /// The security-relevant direction: every effectful leaf classifies non-`Pure`.
    /// (The cross-crate half — that the host never executes a `Pure`-classified
    /// name — lands with host execution, P2.4.1 / P2.6.1.)
    #[test]
    fn effectful_exemplars_are_not_pure() {
        let key = [Param::frozen("key", "var1")];
        for (name, params) in [
            ("store_read", &key[..]),
            ("store_write", &key[..]),
            ("clock", &[][..]),
            ("egress", &[][..]),
        ] {
            assert!(kind_of(name, params).unwrap().is_effectful(), "{name} must be effectful");
        }
    }

    /// `map` is higher-order but carries no effect — it must read as `Pure`.
    #[test]
    fn map_is_pure() {
        assert_eq!(kind_of("map", &[]), Some(Kind::Pure));
        assert!(!kind_of("map", &[]).unwrap().is_effectful());
    }

    /// An unknown leaf fails closed: `None`, never `Some(Pure)`.
    #[test]
    fn unknown_is_none() {
        assert_eq!(kind_of("xyzzy", &[]), None);
        assert_eq!(kind_of("", &[]), None);
        assert_ne!(kind_of("definitely_not_a_primitive", &[]), Some(Kind::Pure));
    }

    #[test]
    fn pure_is_the_only_non_effectful_kind() {
        assert!(!Kind::Pure.is_effectful());
        assert!(Kind::Driver.is_effectful());
        assert!(Kind::State.is_effectful());
        assert!(Kind::Sink.is_effectful());
    }

    // ── effects_of: the derived (composite) kind ────────────────────────────

    #[test]
    fn pure_only_closure_is_pure() {
        let mut store = MapResolver::new();
        let n = store.insert(prim("const")).unwrap();
        let m = store.insert(prim("multiply")).unwrap();
        let comp = composite(vec![n, m]);
        let fx = effects_of(&comp, &store);
        assert!(fx.is_pure());
        assert_eq!(fx.len(), 0);
    }

    #[test]
    fn sink_leaf_at_depth_surfaces() {
        // An `egress` (Sink) buried three levels down must surface at the top —
        // and the pure sibling must add nothing.
        let mut store = MapResolver::new();
        let egress = store.insert(prim("egress")).unwrap();
        let inner = store.insert(composite(vec![egress])).unwrap();
        let mid = store.insert(composite(vec![inner])).unwrap();
        let pure = store.insert(prim("const")).unwrap();
        let top = composite(vec![mid, pure]);
        let fx = effects_of(&top, &store);
        assert!(!fx.is_pure());
        assert!(fx.contains(Kind::Sink));
        assert_eq!(fx.len(), 1);
    }

    #[test]
    fn a_closure_costs_its_units_however_it_shares_or_nests_them() {
        // Each level reaches the one below twice (as a child, and as a `map`'s `fn`): 2^300
        // paths over 901 units. A walk that followed every path wouldn't finish.
        let mut store = MapResolver::new();
        let mut below = store.insert(prim("clock")).unwrap();
        for _ in 0..300 {
            let child = store.insert(composite(vec![below])).unwrap();
            let mapped = store.insert(map_over(&below)).unwrap();
            below = store.insert(composite(vec![child, mapped])).unwrap();
        }
        let top = composite(vec![below]);
        let caps = capabilities(&top, &store);
        assert!(caps.contains(Capability::Clock));
        assert_eq!(caps.len(), 1);
        // A deep chain of single references, walked on a small stack.
        let mut store = MapResolver::new();
        let mut below = store.insert(prim("clock")).unwrap();
        for _ in 0..20_000 {
            below = store.insert(composite(vec![below])).unwrap();
        }
        let top = composite(vec![below]);
        let caps = std::thread::Builder::new().stack_size(256 * 1024).spawn(move || capabilities(&top, &store)).unwrap().join().unwrap();
        assert!(caps.contains(Capability::Clock));
    }

    #[test]
    fn mixed_axes_are_preserved_not_collapsed() {
        // clock (Driver) + egress (Sink) ⇒ {Driver, Sink}, not one "worst" bucket.
        let mut store = MapResolver::new();
        let clock = store.insert(prim("clock")).unwrap();
        let egress = store.insert(prim("egress")).unwrap();
        let comp = composite(vec![clock, egress]);
        let fx = effects_of(&comp, &store);
        assert!(fx.contains(Kind::Driver));
        assert!(fx.contains(Kind::Sink));
        assert!(!fx.contains(Kind::State));
        assert_eq!(fx.len(), 2);
    }

    #[test]
    fn map_over_effectful_fn_is_effectful() {
        // The laundering test: `map` is itself Pure, but its `fn` references an
        // egress-bearing unit — following the hash-valued param is what keeps the
        // whole thing from reading as pure.
        let mut store = MapResolver::new();
        let egress = store.insert(prim("egress")).unwrap();
        let map = map_over(&egress);
        let fx = effects_of(&map, &store);
        assert!(!fx.is_pure());
        assert!(fx.contains(Kind::Sink));
    }

    #[test]
    fn map_over_pure_fn_is_pure() {
        let mut store = MapResolver::new();
        let dbl = store.insert(prim("multiply")).unwrap();
        let map = map_over(&dbl);
        assert!(effects_of(&map, &store).is_pure());
    }

    #[test]
    fn unknown_leaf_saturates() {
        let mut store = MapResolver::new();
        let unknown = store.insert(prim("frobnicate")).unwrap();
        let comp = composite(vec![unknown]);
        let fx = effects_of(&comp, &store);
        assert!(fx.contains(Kind::Driver));
        assert!(fx.contains(Kind::State));
        assert!(fx.contains(Kind::Sink));
        assert_eq!(fx.len(), 3);
    }

    #[test]
    fn unresolvable_child_saturates() {
        // A child hash absent from the resolver fails closed, never silently pure.
        let store = MapResolver::new();
        let missing: Hash = [0u8; 32];
        let comp = composite(vec![missing]);
        let fx = effects_of(&comp, &store);
        assert_eq!(fx.len(), 3);
        assert!(!fx.is_pure());
    }

    #[test]
    fn pure_is_never_a_set_member() {
        // The empty-set invariant: purity is ∅, and `Pure` is never inserted.
        let store = MapResolver::new();
        let fx = effects_of(&prim("add"), &store);
        assert!(fx.is_pure());
        assert!(!fx.contains(Kind::Pure));
        assert_eq!(fx.iter().count(), 0);
    }

    // ── P2.3.1: the capability manifest (primary classification) ────────────

    #[test]
    fn pure_leaf_declares_empty_manifest() {
        // A positive empty manifest — Some(∅), not None — "touches nothing".
        let caps = capabilities_of("add", &[]).unwrap();
        assert!(caps.is_pure());
        assert_eq!(caps.len(), 0);
    }

    #[test]
    fn effectful_leaves_declare_their_caps() {
        let egress = capabilities_of("egress", &[]).unwrap();
        assert!(egress.contains(Capability::NetworkEgress));
        assert_eq!(egress.len(), 1);

        let store_read = capabilities_of("store_read", &[Param::frozen("key", "var1")]).unwrap();
        assert!(store_read.contains(Capability::StateRead("var1".to_string())));
        assert_eq!(store_read.len(), 1);

        let store_write = capabilities_of("store_write", &[Param::frozen("key", "var1")]).unwrap();
        assert!(store_write.contains(Capability::StateWrite("var1".to_string())));
        assert_eq!(store_write.len(), 1);

        let clock = capabilities_of("clock", &[]).unwrap();
        assert!(clock.contains(Capability::Clock));
        assert_eq!(clock.len(), 1);

        // Request/response (P2.5.1): the in/out pair. `ingress` (a request arrives) is a driver —
        // effect-bearing yet *replayable* (no State); `egress` carries the reply (checked above,
        // since the merged sink is the same leaf). The correlating token is plain data, never a
        // capability.
        let ingress = capabilities_of("ingress", &[]).unwrap();
        assert!(ingress.contains(Capability::NetworkIngress));
        assert!(ingress.is_replayable()); // a driver, not state
    }

    #[test]
    fn a_unit_deployed_on_a_schedule_runs_on_the_clock() {
        // A clock flow that never reads the time still runs on the clock: deployed on a
        // schedule, its manifest has `Clock`, its trigger; unscheduled, only what it derives.
        let state = capabilities_of("store_write", &[Param::frozen("key", "beats")]).unwrap();
        assert!(!state.clone().deployed(false).contains(Capability::Clock));
        let scheduled = state.deployed(true);
        assert!(scheduled.contains(Capability::Clock) && scheduled.contains(Capability::StateWrite("beats".into())));
        assert_eq!(scheduled.len(), 2);
        // One that reads it already is the same either way.
        let clock = capabilities_of("clock", &[]).unwrap();
        assert_eq!(clock.clone().deployed(true), clock);
    }

    #[test]
    fn is_sourced_splits_input_from_output() {
        use Capability::*;
        // sourced (input-side): drivers and the state *read*
        for c in [Clock, NetworkIngress, FileRead("inbox".to_string()), FileArrival("inbox".to_string()), SecretAccess, Random, StateRead("var1".to_string())] {
            assert!(c.is_sourced(), "{c:?} should be sourced");
        }
        // performed (output-side): sinks and the state *write*
        for c in [NetworkEgress, FileWrite("reports".to_string()), Spawn, StateWrite("var1".to_string())] {
            assert!(!c.is_sourced(), "{c:?} should be performed");
        }
    }

    #[test]
    fn capability_projects_to_expected_kind() {
        assert_eq!(Capability::NetworkEgress.kind(), Kind::Sink);
        assert_eq!(Capability::Clock.kind(), Kind::Driver);
        assert_eq!(Capability::StateRead("var1".to_string()).kind(), Kind::State);
        assert_eq!(Capability::StateWrite("var1".to_string()).kind(), Kind::State);
    }

    #[test]
    fn fine_distinguishes_what_coarse_loses() {
        // The motivating case: Clock and SecretAccess both project to Driver, yet
        // are distinct capabilities — kind is lossy where capabilities are not.
        assert_eq!(Capability::Clock.kind(), Kind::Driver);
        assert_eq!(Capability::SecretAccess.kind(), Kind::Driver);
        assert_ne!(Capability::Clock, Capability::SecretAccess);
    }

    #[test]
    fn capability_kind_is_never_pure() {
        use Capability::*;
        for cap in [
            Clock, NetworkIngress, FileRead("inbox".to_string()), SecretAccess, Random, StateRead("var1".to_string()),
            StateWrite("var1".to_string()), NetworkEgress, HttpRequest("api.example.com".to_string()),
            FileWrite("reports".to_string()), Spawn,
        ] {
            assert_ne!(cap.kind(), Kind::Pure, "{cap:?} must project to an effect");
        }
    }

    #[test]
    fn unknown_leaf_has_no_manifest() {
        assert_eq!(capabilities_of("frobnicate", &[]), None);
        assert_eq!(capabilities_of("", &[]), None);
    }

    #[test]
    fn state_cell_leaf_without_a_key_is_unrecognized() {
        // A real, deliberate decision, not an incidental side effect of adding the
        // param: `store_read`/`store_write` with no resolvable key fail closed the
        // *same way* an unknown name does — not "some anonymous state," not ∅.
        assert_eq!(capabilities_of("store_read", &[]), None); // no key param at all
        assert_eq!(capabilities_of("store_write", &[]), None);
        assert_eq!(
            capabilities_of("store_read", &[Param::frozen("key", "")]),
            None // frozen to an empty string — no real key
        );
        assert_eq!(
            capabilities_of("store_read", &[Param::open("key", "some_binding")]),
            None // an unfilled hole — not yet a concrete key
        );
        // A key param present under the wrong name doesn't accidentally satisfy it.
        assert_eq!(capabilities_of("store_read", &[Param::frozen("wrong_name", "var1")]), None);
    }

    // ── P2.3.2: capability union, surfaced before import ────────────────────

    #[test]
    fn buried_egress_surfaces_before_import() {
        // The governing I9 test: a `NetworkEgress` buried three levels down
        // surfaces at the top — nothing exfiltrates silently.
        let mut store = MapResolver::new();
        let egress = store.insert(prim("egress")).unwrap();
        let inner = store.insert(composite(vec![egress])).unwrap();
        let mid = store.insert(composite(vec![inner])).unwrap();
        let pure = store.insert(prim("const")).unwrap();
        let top = composite(vec![mid, pure]);
        let caps = capabilities(&top, &store);
        assert!(caps.contains(Capability::NetworkEgress));
        assert_eq!(caps.len(), 1);
    }

    #[test]
    fn pure_closure_has_empty_manifest() {
        let mut store = MapResolver::new();
        let n = store.insert(prim("const")).unwrap();
        let m = store.insert(prim("multiply")).unwrap();
        let caps = capabilities(&composite(vec![n, m]), &store);
        assert!(caps.is_pure());
    }

    #[test]
    fn mixed_caps_union_over_closure() {
        let mut store = MapResolver::new();
        let clock = store.insert(prim("clock")).unwrap();
        let egress = store.insert(prim("egress")).unwrap();
        let store_read = store.insert(keyed_state_prim("store_read", "var1")).unwrap();
        let store_write = store.insert(keyed_state_prim("store_write", "var1")).unwrap();
        let comp = composite(vec![clock, egress, store_read, store_write]);
        let caps = capabilities(&comp, &store);
        assert!(caps.contains(Capability::Clock));
        assert!(caps.contains(Capability::NetworkEgress));
        assert!(caps.contains(Capability::StateRead("var1".to_string())));
        assert!(caps.contains(Capability::StateWrite("var1".to_string())));
        assert_eq!(caps.len(), 4);
    }

    #[test]
    fn distinct_state_keys_surface_as_distinct_capabilities() {
        // The actual point of keying: two store_read leaves at DIFFERENT keys don't
        // collapse into one manifest entry the way two bare StateReads used to.
        let mut store = MapResolver::new();
        let var1 = store.insert(keyed_state_prim("store_read", "var1")).unwrap();
        let var2 = store.insert(keyed_state_prim("store_read", "var2")).unwrap();
        let caps = capabilities(&composite(vec![var1, var2]), &store);
        assert!(caps.contains(Capability::StateRead("var1".to_string())));
        assert!(caps.contains(Capability::StateRead("var2".to_string())));
        assert_eq!(caps.len(), 2); // two distinct entries, not deduped to one

        // The same key, referenced twice, IS one entry — a BTreeSet, not a multiset.
        let var1_again = store.insert(keyed_state_prim("store_read", "var1")).unwrap();
        let repeated_caps = capabilities(&composite(vec![var1, var1_again]), &store);
        assert_eq!(repeated_caps.len(), 1);
    }

    #[test]
    fn map_fn_capabilities_surface() {
        // A capability routed through a `map` fn-param cannot hide.
        let mut store = MapResolver::new();
        let egress = store.insert(prim("egress")).unwrap();
        let caps = capabilities(&map_over(&egress), &store);
        assert!(caps.contains(Capability::NetworkEgress));
    }

    #[test]
    fn unknown_in_closure_saturates_caps() {
        let mut store = MapResolver::new();
        let unknown = store.insert(prim("frobnicate")).unwrap();
        let caps = capabilities(&composite(vec![unknown]), &store);
        assert_eq!(caps.len(), 19); // every capability — fail closed
        assert!(caps.contains(Capability::HttpRequest("<unknown>".to_string())), "any host");
        assert!(caps.contains(Capability::CollectionWrite("<unknown>".to_string())), "any collection");
        assert!(caps.contains(Capability::FileWrite("<unknown>".to_string())), "any folder");
        assert!(caps.contains(Capability::ChannelSend("<unknown>".to_string())), "any channel");
        assert!(caps.contains(Capability::DbWrite("<unknown>".to_string())), "any database");
    }

    #[test]
    fn a_channel_names_itself_in_both_capabilities() {
        let on = |leaf: &str| capabilities_of(leaf, &[Param::frozen("channel", "updates"), Param::frozen("from", "new")]).unwrap();
        assert!(on("channel_send").contains(Capability::ChannelSend("updates".into())));
        assert!(on("channel_subscribe").contains(Capability::ChannelRead("updates".into())));
        assert_eq!(Capability::ChannelSend("x".into()).kind(), Kind::Sink);
        assert!(!Capability::ChannelRead("x".into()).is_sourced(), "a subscription is the host's to hold, not an input");
        assert!(capabilities_of("channel_send", &[]).is_none(), "a channel names itself");
    }

    #[test]
    fn a_waiting_point_and_its_token_name_the_same_capability() {
        let wait = Unit { params: vec![Param::frozen("name", "paid"), Param::frozen("expires", "3600")], ..prim("await") };
        let token = Unit { params: vec![Param::frozen("await", "paid")], ..prim("resume_token") };
        let want = vec![Capability::Await("paid".into())];
        assert_eq!(capabilities_of("await", &wait.params).unwrap().iter().collect::<Vec<_>>(), want);
        assert_eq!(capabilities_of("resume_token", &token.params).unwrap().iter().collect::<Vec<_>>(), want);
        assert!(Capability::Await("paid".into()).is_sourced());
        assert!(capabilities_of("await", &[Param::frozen("name", "a b")]).is_none());
    }

    #[test]
    fn a_secret_names_itself_and_is_sourced() {
        let named = |n: &str| Unit { params: vec![Param::frozen("name", n)], ..prim("secret") };
        let caps = capabilities_of("secret", &named("stripe_key").params).unwrap();
        assert_eq!(caps.iter().collect::<Vec<_>>(), vec![Capability::SecretRead("stripe_key".into())]);
        assert!(Capability::SecretRead("x".into()).is_sourced());
        assert_eq!(kind_of("secret", &named("k").params), Some(Kind::Driver));
        for bad in ["", "a b", "a/b", &"x".repeat(65)] {
            assert!(capabilities_of("secret", &named(bad).params).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn http_request_names_its_host_and_nothing_else() {
        let req = |host: &str| Unit { params: vec![Param::frozen("method", "GET"), Param::frozen("host", host)], ..prim("http_request") };
        let caps = capabilities_of("http_request", &req("Hooks.Slack.com").params).unwrap();
        assert_eq!(caps.iter().collect::<Vec<_>>(), vec![Capability::HttpRequest("hooks.slack.com".into())]);
        assert_eq!(kind_of("http_request", &req("a.com").params), Some(Kind::Sink));
        assert!(capabilities_of("http_request", &req("localhost:8080").params).is_some());
        for bad in ["", "https://a.com", "a.com/x", "user@a.com", "a .com", "-a.com", "a..com", "a.com:", "a.com:99999"] {
            assert!(capabilities_of("http_request", &req(bad).params).is_none(), "{bad:?}");
        }
        assert!(capabilities_of("http_request", &[]).is_none(), "no host, no capability");
    }

    #[test]
    fn effects_is_the_projection_of_capabilities() {
        // The structural invariant: effects_of is exactly capabilities projected
        // through Capability::kind — one source of truth, one walk.
        let mut store = MapResolver::new();
        let clock = store.insert(prim("clock")).unwrap();
        let egress = store.insert(prim("egress")).unwrap();
        let comp = composite(vec![clock, egress]);
        let projected: std::collections::BTreeSet<Kind> =
            capabilities(&comp, &store).iter().map(|c| c.kind()).collect();
        let fx = effects_of(&comp, &store);
        assert!(fx.contains(Kind::Driver) && fx.contains(Kind::Sink));
        assert_eq!(fx.iter().collect::<std::collections::BTreeSet<_>>(), projected);
    }
}