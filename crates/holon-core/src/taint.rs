//! Which nodes of a run compute with a secret's value, read off the graph before it runs.
//!
//! The host's log redacts a secret by its value: every literal occurrence becomes
//! `‹secret:name›`. A value computed from a secret (base64 of it for a Basic auth header, a field
//! of a JSON secret, a slice) no longer holds it literally, so redaction can't see it, and the
//! log would hand it to whoever may read the deployment. So each port is followed from the
//! `secret` leaves through the graph:
//!
//! - **literal**: the value may hold the secret as it is (a record built with it, a concatenation,
//!   a field read back out): redaction finds it there;
//! - **derived**: the value may hold something computed from it: the log keeps no outputs for
//!   that node, and an outbound request built from it is logged with those parts withheld. An
//!   effect it reaches is refused if it would keep it (a state, collection or channel write, as a
//!   secret's value is), else logged withheld; so are the run's outputs and its answer.
//!
//! What cuts the chain: a one-way leaf (`sha256`, `hmac_sha256`, `secure_equals`) fed the secret
//! whole (as it is, or in another spelling: its base64, its bytes), `jwt_verify` for a key that is (the token's payload comes out, nothing of the key; fed
//! a part of the secret, or anything else computed from it, each is derived, since a hash of one
//! character names it), and an outbound request's
//! response or a database read's rows (what the far side said, not what was sent). Nothing else
//! does: a Boolean computed from a secret (`k.startsWith(p)`) says something of it, so it is
//! derived, as is whatever it chooses (`c ? a : b`, a write under `if (c)`). A leaf that isn't
//! known to pass its inputs through as they are makes anything tainted derived, and so does one
//! that is, for an input that only chooses what comes out ([`selects`]), and one that orders what
//! it is handed ([`ORDERS`]). Which steps a run takes is chosen too: a choice decided by a
//! secret (`guard`, `coalesce`, `and`, `or`) runs only the branch it takes, so the steps only it
//! needs are [chosen](SecretFed::chosen), and the choice is one of the
//! [deciders](SecretFed::deciders). A run's steps show which way it went, as any choice's do:
//! the person approving a flow is told what it chooses by a secret.
//!
//! The same walk, one secret at a time, finds what a route may answer its caller with
//! ([`answered_secrets`]): each such secret is a permission of its own, `SecretAnswer("name")`,
//! that a person approves at deploy. It is core's, so the host grants by the rule the editor and
//! `holon check` show. It also finds where each secret is sent ([`sent_secrets`]): the hosts of
//! the requests its value, or something computed from it, may reach. That isn't a permission of
//! its own (the hosts and the secret are each approved, and the graph fixes which goes where
//! before the flow runs); the approval says it beside the secret, so a person sees it.
//!
//! Two things a flow makes with a secret are clean, and must be, yet are the secret's to answer
//! for: a hash or a signature of it ([`MAKES`]: a caller handed `hmac_sha256(k, what they sent)`
//! signs what they like with the key), and what a host answered a request that carried it (the
//! token a token exchange answers, as good as the secret until it expires). Neither is withheld or
//! refused; each is followed from where it is made ([`SecretFed::made_of`],
//! [`SecretFed::answers_of`]) to where it leaves, and the approval says that too, beside the
//! secret: "…; a hash or signature made with it goes to its callers".
//!
//! A walk follows every path through the graph, and a function used twice is followed twice, each
//! use at its own path, since the host asks of a node by its path: so a function that uses one that
//! uses one… twice each is 2^k paths, and twenty such levels are a million. The walks one question
//! about a flow takes (one for each secret, for what is answered to it and what is made with it,
//! and each route's of those) visit at most [`MAX_VISITS`] nodes between them ([`Visits`]); past
//! it, a function not yet followed is taken as not followed at all, as a `map`'s function is:
//! everything under it derived, every effect it could make carrying the secret, every host it could
//! call sent it. That says more than the flow does, never less (audit R17: `POST /deploy`, the
//! preview, `holon check` and each run that reads a secret walk its flow).

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::{capabilities, capabilities_of, child_refs, declaration_of, kind::ident, read_server, Capabilities, Capability, Hash, Resolver, Type, Unit, UnitBody, BOUNDARY};

/// How much of a secret a value may carry.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub enum Taint {
    #[default]
    Clean,
    /// It may hold the secret as it is: redaction finds it.
    Literal,
    /// It may hold the secret whole in another spelling ([`RECODES`]: its base64, its bytes, the
    /// JSON it is, read). Redaction can't find it, so to the log, an answer or a write it is as
    /// anything computed from the secret ([`Taint::is_derived`]). To a one-way leaf it is the
    /// secret whole: a hash of it gives nothing of the secret back.
    Recoded,
    /// It may hold something computed from the secret: a part of it, a test of it.
    Derived,
}

impl Taint {
    /// Whether it may hold something of the secret that redaction can't find by the secret's
    /// value: a re-spelling of it whole, or anything else computed from it.
    pub fn is_derived(self) -> bool {
        self >= Taint::Recoded
    }
}

/// Leaves that spell their one input another way, whole: an encoding, a decoding, the value a
/// JSON text is. Nothing is chosen out of it and nothing mixed in, so what a one-way leaf makes of
/// the result gives no more of a secret back than what it makes of the secret: signing with a key
/// kept as base64 (`hmac_sha256(text_to_bytes(key, { encoding: "base64" }), message)`) is signing
/// with the key. Fed only at their first input, and that the secret whole.
const RECODES: &[&str] = &["base64", "hex", "text_to_bytes", "bytes_to_text", "uri_encode", "uri_decode", "json_parse"];

/// Leaves whose output is computed one way from their inputs: nothing of a secret is recovered
/// from it, when it is the secret whole they are fed: as it is (literal), or in another spelling
/// (recoded: [`RECODES`]). Fed anything else computed from it (derived: a slice, a character),
/// what comes out is derived too: a hash of one character names the character, and a comparison
/// of a prefix says how the secret begins.
const ONE_WAY: &[&str] = &["sha256", "hmac_sha256", "secure_equals"];

/// The one-way leaves that make something with what they are fed: a hash, a keyed hash (a
/// signature). What they make of a secret whole is clean (the log keeps it, a request carries it:
/// signing is what a key is for), and it is still the secret's to answer for, since whoever is
/// handed it can check a guess of a short secret against it, or holds a signature the key's owner
/// never meant to give. So where it goes is followed ([`SecretFed::made_of`]) and said. The other
/// one-way leaf, `secure_equals`, answers only whether two values are the same, which every
/// guarded route says by its status: nothing is made, and nothing followed.
const MAKES: &[&str] = &["sha256", "hmac_sha256"];

/// Leaves whose output holds their inputs as they are, or a part of them unchanged: a secret in
/// an input is in the output literally, where redaction finds it. (`json_stringify` escapes, and
/// so does a second one over it; the host finds a secret in every spelling that gives, and
/// withholds whole a text that holds it in any other: `secrets::spellings` in holon.) The list leaves that rearrange a list's
/// items by their places are here; those that take a text too (`at`, `slice`) are not, since a
/// part of a text isn't the secret, nor those that place items by what they hold (`sort`,
/// `unique`): where a secret lands among the others, or whether it is left out as another's
/// like, says how it compares with them. So are the record leaves that move values between records and lists
/// (`values`, `from_entries`, `merge`); `has` and `same_value` answer a Boolean, which is derived.
/// `add` joins texts as `concat` does (a number it adds is no secret's text); the other text
/// leaves (`trim`, `replace`, `pad`, …) make a text from part of theirs, which is derived. An
/// input of one of these that only chooses ([`selects`]) isn't passed on: what it chose is
/// derived.
const PASSES_LITERALLY: &[&str] = &[
    "const", "select", "cons", "head", "tail", "concat", "add", "list", "get", "append", "extend", "join", "pair", "some",
    "entries", "reverse", "to_spliced", "with_item", "fill", "copy_within", "flat",
    "values", "from_entries", "merge",
    "record", "field", "with_field", "without_field", "lookup", "insert", "remove", "as_text", "guard",
    "default", "coalesce", "json_stringify", "store_write", "collection_put", "file_write", "file_create",
    "channel_send", "mail_send", "queue_send", "egress",
];

/// Whether `port`, an input of `leaf` (one of [`PASSES_LITERALLY`]), only chooses what comes out
/// and is never part of it: a condition (`select`, `guard`, any Boolean), a position in a list, a
/// key to look up or leave out, what starts a sink. A secret there isn't in the output, where
/// redaction would find it, but what was chosen says something of it: derived.
fn selects(leaf: &str, port: &crate::Port) -> bool {
    port.ty == Type::Boolean
        || port.name == crate::TRIGGER
        || matches!(
            (leaf, port.name.as_str()),
            ("get" | "with_item", "index") | ("to_spliced", "start" | "count") | ("fill", "start" | "end") | ("copy_within", "target" | "start" | "end") | ("lookup" | "remove", "key")
        )
}

/// The leaves of [`PASSES_LITERALLY`] that put their items in an order read off what the items
/// hold: the order of two values computed from a secret says something of them that neither
/// value does, and so does the order of a secret and a value it isn't (`[k, p].toSorted()[0]`
/// is `k` just when `k` comes first), or whether `unique` kept both. What they give from a
/// tainted input is derived, never only literal.
const ORDERS: &[&str] = &["sort", "unique"];

/// How much of a secret the inputs of `leaf` (one of [`PASSES_LITERALLY`]) that only choose
/// ([`selects`]) carry.
fn chose(leaf: &str, ports: &[crate::Port], inputs: &[Taint]) -> Taint {
    inputs.iter().zip(ports).filter(|(_, port)| selects(leaf, port)).map(|(t, _)| *t).max().unwrap_or_default()
}

/// What a leaf does with a value it is handed whole, for a check that runs a recorded run again
/// with a mark where the record never holds the value (`holon verify`: a secret's, a token's, a
/// tainted value's). Each is exact on a mark: what it gives with the mark is what it gives with
/// the value, every occurrence of the value then written as the mark. So such a check computes
/// the leaf, and needn't take its result from the record.
///
/// That holds as far as the record's marks do: for a value the run made of other parts that
/// happens to be the secret's, nothing is marked, and the mark and it read as different.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OnAMark {
    /// It moves the value: into a list, a record, a longer text, or on. True of any mark.
    Moves,
    /// It answers whether the value is another, a true or false. True only where every mark in
    /// its inputs tells one value from another (a fingerprint): two tainted values read alike.
    Compares,
}

/// The leaves that move a value and never look inside it: [`PASSES_LITERALLY`] less the sinks
/// (they act, and a check performs nothing), `const` and `record` (no input), and the ones that order
/// or compare by what a text holds (`sort`, `unique`) or by a key that may be the value
/// (`lookup`, `insert`, `remove`, `merge`, `from_entries`, `entries`, `values`). `as_text` and
/// `json_stringify` move a text as it is, and write anything else as JSON, where a mark is
/// escaped as its value wouldn't be: `as_text` is exact for a text alone, which the check looks
/// at, and `json_stringify` isn't here.
const MOVES: &[&str] = &[
    "select", "cons", "head", "tail", "concat", "add", "list", "get", "append", "extend", "join", "pair", "some",
    "reverse", "to_spliced", "with_item", "fill", "copy_within", "flat",
    "field", "with_field", "without_field", "as_text", "guard", "default", "coalesce",
];

/// The leaves that say whether a value is another, as a whole. `same_value` does too, but what it
/// says is computed from a secret as the taint walk has it, so the log withholds whatever follows
/// from it and a check has nothing to hold the rest of the run to: it stays off this list until
/// the log marks such values (docs/engineering/capabilities/secrets.md, "What is derived is
/// marked, or withheld").
const COMPARES: &[&str] = &["secure_equals"];

/// How `leaf` treats a value handed to it whole, if computing it on a mark is exact
/// ([`OnAMark`]); none for a leaf that computes with what the value holds (`hmac_sha256`, a
/// slice, `sort`), and for one nobody has put on a list: a check takes its result as recorded.
pub fn on_a_mark(leaf: &str) -> Option<OnAMark> {
    if MOVES.contains(&leaf) {
        Some(OnAMark::Moves)
    } else if COMPARES.contains(&leaf) {
        Some(OnAMark::Compares)
    } else {
        None
    }
}

/// Leaves whose output is what the far side answered: an outbound request's response, a
/// database read's rows. (One that echoes a secret back holds its value, where the host finds it.)
const ANSWERED: &[&str] = &["http_request", "http_upload", "http_stream", "flow_call", "flow_start", "db_query", "db_one", "db_find", "db_find_one"];

/// The [`ANSWERED`] leaves whose answer is an HTTP response: `{status, headers, body}`.
const HTTP_ANSWERED: &[&str] = &["http_request", "http_upload", "http_stream"];

/// Whether `leaf` is a call ([`ANSWERED`]): it moves what it is fed into its request, and what it
/// gives is what was answered. `holon verify` computes one fed a mark on its marks, so that it
/// asks, rather than taking what the log recorded for it.
pub fn is_answered(leaf: &str) -> bool {
    ANSWERED.contains(&leaf)
}

/// Deeper than this, a composite is taken as computing with whatever reaches it.
const MAX_DEPTH: usize = 128;

/// The nodes the walks over one flow may visit between them ([`Visits`]): far more than a flow
/// written to be read has paths (the samples' most, every walk of a question together, is 2,065:
/// dexpaprika's), and a few tens of milliseconds of walking.
pub const MAX_VISITS: usize = 1 << 16;

/// What the walks over one flow have visited, of the [`MAX_VISITS`] they may: one for all the
/// walks a question about the flow takes (each secret's, and what is answered to it), so a flow
/// reading many secrets doesn't walk its paths once for each.
#[derive(Debug, Default)]
pub struct Visits(Cell<usize>);

impl Visits {
    /// One visit taken. Counted past [`MAX_VISITS`] too: a walk that goes on once they are spent
    /// shows in what was taken ([`tally`]).
    fn take(&self) {
        self.0.set(self.0.get().saturating_add(1));
    }

    /// Whether all are taken: a function not yet followed isn't.
    fn spent(&self) -> bool {
        self.0.get() >= MAX_VISITS
    }

    /// How many the walks took.
    pub fn taken(&self) -> usize {
        self.0.get()
    }
}

thread_local! {
    /// The questions asked on this thread ([`Visits`] made and let go) and the visits their walks
    /// took between them ([`tally`]).
    static TALLY: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

/// A question's visits are added to its thread's [`tally`] as it ends.
impl Drop for Visits {
    fn drop(&mut self) {
        let taken = self.taken();
        TALLY.with(|t| {
            let (questions, visits) = t.get();
            t.set((questions + 1, visits.saturating_add(taken)));
        });
    }
}

/// The questions about flows asked on this thread so far, and the visits their walks took between
/// them: for tests to hold a reading of a text to its work by, counted rather than timed
/// (holon-author's `no_entry_doubles_its_work_with_each_level_a_text_nests`). Nothing else reads it,
/// and no limit is taken from it.
pub fn tally() -> (usize, usize) {
    TALLY.with(Cell::get)
}

/// The nodes of one run that a secret reaches, by path (child indices from the run's root, as
/// the timeline and the outbound exchanges name them).
#[derive(Default, Debug, Clone)]
pub struct SecretFed {
    /// Each node a secret reaches: the taint of its inputs and of its outputs.
    nodes: BTreeMap<Vec<u32>, (Vec<Taint>, Vec<Taint>)>,
    /// Nodes whose insides aren't followed (a `map` or `fold` over a function, a module, a level
    /// past [`MAX_DEPTH`]) that a secret reaches: everything under them counts as derived.
    opaque: Vec<Vec<u32>>,
    /// The effects whose payload may be computed from a secret: a sink fed one, and every effect
    /// a node not followed could make. An effect carries no node, only its capability, so a write
    /// to a key counts as derived when any such sink writes that key.
    effects: std::collections::BTreeSet<Capability>,
    /// Where a secret may be sent ([`destination_of`]): the hosts of the outbound requests it may
    /// reach, in their path, headers or body, as it is or computed from it, and the databases of
    /// the reads whose parameters it may reach. A node not followed counts every one it could.
    sent: BTreeSet<String>,
    /// Each sink a secret reaches, by the node's path, and what it does: a request it sends, a
    /// write, a send, a call. A node not followed counts every effect it could make. What the
    /// editor marks on the canvas.
    sinks: BTreeSet<(Vec<u32>, Capability)>,
    /// The nodes whose answer comes back to something a secret reached: a request or a database
    /// read fed one, and a node not followed that could make one. Where [`SecretFed::answers_of`]
    /// starts.
    asked: BTreeSet<Vec<u32>>,
    /// The leaves that compute something of their own from a secret: each one whose output is
    /// derived and isn't only what it was handed, moved ([`SecretFed::computes`]). The record marks
    /// what they give. Not [`SecretFed::made`], which is the other class: clean.
    computing: BTreeSet<Vec<u32>>,
    /// The leaves a secret reaches whose outputs a check running the run again on marks takes
    /// from its record ([`SecretFed::taken`]).
    taken: BTreeSet<Vec<u32>>,
    /// The nodes that make a hash or a signature of a secret whole ([`MAKES`]): clean, and where
    /// [`SecretFed::made_of`] starts. Not [`SecretFed::computing`], whose outputs are derived and
    /// marked in the record.
    made: BTreeSet<Vec<u32>>,
    /// The nodes a run takes only when a choice decided by a secret takes them ([`SecretFed::chosen`]).
    chosen: BTreeSet<Vec<u32>>,
    /// The choices decided by a secret ([`SecretFed::deciders`]).
    deciders: BTreeSet<Vec<u32>>,
    /// The nodes that read the flow's own data (a collection's items, a folder's files) by what
    /// a secret reaches, and a node not followed that could ([`SecretFed::reads_by_a_secret`]).
    own_reads: BTreeSet<Vec<u32>>,
}

/// What a walk follows in place of the secret itself, from the nodes where it begins.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Follow {
    /// What was answered to a request or a read the secret reached.
    Answers,
    /// A hash or a signature made of the secret whole.
    Made,
}

impl SecretFed {
    /// Follow the secrets through `root`, whose inputs are clean (a request, nothing).
    pub fn of(root: &Unit, resolver: &impl Resolver) -> SecretFed {
        SecretFed::of_within(root, resolver, &Visits::default())
    }

    /// [`of`](SecretFed::of), visiting no more than `visits` has left.
    pub fn of_within(root: &Unit, resolver: &impl Resolver, visits: &Visits) -> SecretFed {
        SecretFed::walk(root, resolver, None, visits)
    }

    /// Follow only the secret named `name`: what its value reaches, the others taken as clean.
    pub fn of_secret(root: &Unit, resolver: &impl Resolver, name: &str) -> SecretFed {
        SecretFed::of_secret_within(root, resolver, name, &Visits::default())
    }

    /// [`of_secret`](SecretFed::of_secret), visiting no more than `visits` has left: one of the
    /// walks a question about the flow takes.
    pub fn of_secret_within(root: &Unit, resolver: &impl Resolver, name: &str, visits: &Visits) -> SecretFed {
        SecretFed::walk(root, resolver, Some(name), visits)
    }

    /// Follow what is answered to the secret `name`: the answers of the requests and reads its
    /// value, or something computed from it, is sent in (the token a token exchange answers),
    /// from there on; the secret itself is taken as clean, and an answer to what was answered is
    /// followed too. A run-time token is a field of such an answer (`access_token`): the graph
    /// can't tell the field, so it follows the whole answer, but a response's `status` read
    /// straight off the request, which is a number the far side chose and no token.
    pub fn answers_of(root: &Unit, resolver: &impl Resolver, name: &str) -> SecretFed {
        SecretFed::answers_of_within(root, resolver, name, &Visits::default())
    }

    /// [`answers_of`](SecretFed::answers_of), visiting no more than `visits` has left.
    pub fn answers_of_within(root: &Unit, resolver: &impl Resolver, name: &str, visits: &Visits) -> SecretFed {
        let asked = SecretFed::of_secret_within(root, resolver, name, visits).asked;
        if asked.is_empty() {
            return SecretFed::default();
        }
        SecretFed::walk_from(root, resolver, Some(name), Some((&asked, Follow::Answers)), visits)
    }

    /// Follow what is made with the secret `name` and is clean: each hash or signature of it
    /// whole ([`MAKES`]), from where it is made on; the secret itself is taken as clean. A hash of
    /// such a hash is followed too (a signing key derived in steps, as AWS's is). A Boolean ends
    /// it, whatever computed it: whether a signature matches is what a signature is for, and a
    /// route that answers only that hands out none. (A flow comparing one a part at a time with
    /// what its caller sent would give it away slowly; that is left to whoever reads the flow.)
    /// What a host answers a request that carried one isn't followed: a signed request's answer
    /// is the far side's, not the signature.
    pub fn made_of(root: &Unit, resolver: &impl Resolver, name: &str) -> SecretFed {
        SecretFed::made_of_within(root, resolver, name, &Visits::default())
    }

    /// [`made_of`](SecretFed::made_of), visiting no more than `visits` has left.
    pub fn made_of_within(root: &Unit, resolver: &impl Resolver, name: &str, visits: &Visits) -> SecretFed {
        let made = SecretFed::of_secret_within(root, resolver, name, visits).made;
        if made.is_empty() {
            return SecretFed::default();
        }
        SecretFed::walk_from(root, resolver, Some(name), Some((&made, Follow::Made)), visits)
    }

    fn walk(root: &Unit, resolver: &impl Resolver, only: Option<&str>, visits: &Visits) -> SecretFed {
        SecretFed::walk_from(root, resolver, only, None, visits)
    }

    fn walk_from(root: &Unit, resolver: &impl Resolver, only: Option<&str>, from: Option<(&BTreeSet<Vec<u32>>, Follow)>, visits: &Visits) -> SecretFed {
        let mut fed = SecretFed::default();
        let mut walk = Walk { resolver, only, from, holds_secret: HashMap::new(), visits, capabilities: HashMap::new() };
        let inputs = vec![Taint::Clean; root.inputs.len()];
        walk.unit(root, &inputs, &mut Vec::new(), &mut fed);
        fed
    }

    /// A run in which input `port` of the node at `path` carries `taint`, and nothing else is
    /// reached: for testing what is done with one (the host's logging of an outbound request).
    #[doc(hidden)]
    pub fn reaching(path: &[u32], inputs: Vec<Taint>) -> SecretFed {
        let mut fed = SecretFed::default();
        fed.nodes.insert(path.to_vec(), (inputs, vec![Taint::Clean]));
        fed
    }

    /// Whether a secret reaches nothing in the run.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.opaque.is_empty()
    }

    /// Whether an effect with `capability` may carry something computed from a secret.
    pub fn derives_effect(&self, capability: &Capability) -> bool {
        self.effects.contains(capability)
    }

    /// Whether the run's output `port` (a request's answer is its first) may be computed from a
    /// secret.
    pub fn derives_output(&self, port: usize) -> bool {
        self.opaque.iter().any(Vec::is_empty) || self.nodes.get(&[][..]).is_some_and(|(_, outs)| outs.get(port).is_some_and(|t| t.is_derived()))
    }

    /// Whether the run's output `port` may carry a secret at all: as it is, or computed from it.
    pub fn reaches_output(&self, port: usize) -> bool {
        self.opaque.iter().any(Vec::is_empty) || self.nodes.get(&[][..]).is_some_and(|(_, outs)| outs.get(port).is_some_and(|t| *t > Taint::Clean))
    }

    fn under_opaque(&self, path: &[u32]) -> bool {
        self.opaque.iter().any(|o| path.starts_with(o) && path.len() > o.len())
    }

    /// Whether the node at `path` may give a secret on any of its outputs, as it is or computed
    /// from it, or is where the walk follows no steps. A run's record measures each such step
    /// as it writes it, each such value as its mark (`secrets::Measured` in holon), so that
    /// whether it is kept says nothing of the secret.
    pub fn holds(&self, path: &[u32]) -> bool {
        self.under_opaque(path) || self.opaque.iter().any(|o| o == path) || self.nodes.get(path).is_some_and(|(_, outs)| outs.iter().any(|t| *t > Taint::Clean))
    }

    /// Whether the node at `path` may produce something computed from a secret: the log keeps
    /// none of its outputs.
    pub fn derives(&self, path: &[u32]) -> bool {
        self.under_opaque(path) || self.opaque.iter().any(|o| o == path) || self.nodes.get(path).is_some_and(|(_, outs)| outs.iter().any(|t| t.is_derived()))
    }

    /// Whether the leaf at `path` computes something of its own from a secret: its output may be
    /// computed from one, and is a value of its own, not only what it was handed, moved. A leaf
    /// that passes its inputs on as they are (a record built, a text joined, a field read:
    /// [`PASSES_LITERALLY`]) computes nothing, unless a secret reaches an input that only chooses,
    /// or it orders what it is handed (`sort`, `unique`): then what comes out says something no
    /// input does. (Not [`MAKES`], the hashes and signatures of a secret whole: those are clean.)
    /// Every value computed from a secret in a run is what such a leaf gave, or holds it, moved
    /// there; so a log that marks each of those by its value marks every one
    /// ([`SecretFed::followed_to`]).
    pub fn computes(&self, path: &[u32]) -> bool {
        self.computing.contains(path)
    }

    /// Whether `holon verify`, running a run again with a mark where its record never holds the
    /// value, takes what the node at `path` gave from the record rather than computing it: a
    /// leaf a secret reaches, as it is or computed from it, but a call (asked again on its
    /// marks, [`is_answered`]) and one that only moves what it is handed by this walk's rule
    /// (it passes its inputs on as they are, orders nothing, and no input that only chooses is
    /// reached), which verify computes again; `as_text` is taken, as it writes anything but a
    /// text as JSON, where a mark is escaped as its value wouldn't be. And where the walk
    /// follows no steps. Wider than [`SecretFed::computes`]: a hash of a secret whole is clean,
    /// and taken. A run's record that marks every value computed from a secret keeps each such
    /// step's outputs (`secrets::born` in holon), and verify refuses a line that says so and
    /// keeps none for one. Verify's rule: what may hold a secret is [`SecretFed::holds`].
    pub fn taken(&self, path: &[u32]) -> bool {
        self.under_opaque(path) || self.opaque.iter().any(|o| o == path) || self.taken.contains(path)
    }

    /// The leaves that compute something of their own from a secret ([`SecretFed::computes`]),
    /// by path.
    pub fn computing(&self) -> impl Iterator<Item = &[u32]> {
        self.computing.iter().map(Vec::as_slice)
    }

    /// Whether every node that may produce something computed from a secret was followed by
    /// this walk and is no deeper than `depth`: none under a node whose insides aren't followed
    /// (a function applied by `map` or `fold`, a module), and none past the depth a host
    /// observes a run to. Then a host that keeps what each [such leaf](SecretFed::computes) gave has
    /// every value the run computed from a secret.
    pub fn followed_to(&self, depth: usize) -> bool {
        self.opaque.is_empty() && self.nodes.iter().all(|(path, (_, outs))| path.len() <= depth || !outs.iter().any(|t| t.is_derived()))
    }

    /// Each sink a secret reaches: the node's path, and the effect it makes with it.
    pub fn sinks(&self) -> impl Iterator<Item = (&[u32], &Capability)> {
        self.sinks.iter().map(|(path, cap)| (path.as_slice(), cap))
    }

    /// How much of a secret output `port` of the node at `path` may carry.
    pub fn output(&self, path: &[u32], port: usize) -> Taint {
        if self.under_opaque(path) || self.opaque.iter().any(|o| o == path) {
            return Taint::Derived;
        }
        self.nodes.get(path).and_then(|(_, outs)| outs.get(port).copied()).unwrap_or_default()
    }

    /// The most of a secret any input of the node at `path` may carry: what a node that asks the
    /// host with what it is handed (a read's key, a file's name, a query's parameters) asks with.
    pub fn fed(&self, path: &[u32]) -> Taint {
        if self.under_opaque(path) {
            return Taint::Derived;
        }
        self.nodes.get(path).and_then(|(ins, _)| ins.iter().copied().max()).unwrap_or_default()
    }

    /// Whether the node at `path` runs only when a choice decided by a secret takes it: inside a
    /// composite evaluated lazily, every road from what the composite gives (its outputs, and the
    /// children nothing consumes) to it goes through an input a choice reads only as its decision
    /// goes (`guard`'s value, `coalesce`'s second input, `and`'s and `or`'s after the first),
    /// and what decides there may carry a secret. Whether such a node ran says which way the
    /// choice went; so does anything inside it.
    pub fn chosen(&self, path: &[u32]) -> bool {
        self.chosen.iter().any(|c| path.starts_with(c))
    }

    /// The choices (`guard`, `coalesce`, `and`, `or`: a ternary, `??`, `&&`, `||`, an `if` that
    /// returns) whose decision may carry a secret, each by its path: what runs after them is
    /// chosen by it ([`SecretFed::chosen`]). Given one secret ([`SecretFed::of_secret`]), the
    /// choices it decides, for a person approving the flow to be told. A value a secret picks
    /// (`rec[k]`, `list[i]`) isn't a choice of what runs: it is derived ([`selects`]).
    pub fn deciders(&self) -> impl Iterator<Item = &[u32]> {
        self.deciders.iter().map(Vec::as_slice)
    }

    /// Whether the run reads its flow's own data (a collection's items, a folder's files) by
    /// what a secret reaches: a key, a file's name, a range. How such a read is answered says
    /// something of the secret to whoever asked: whether it found anything, whether it failed
    /// (a file too big, or not text: the run answers 500), whether it waited on another run's
    /// claim (503). A database's rows, and whether it answers, are the database's: approved as
    /// where the secret is sent, they aren't counted here.
    pub fn reads_by_a_secret(&self) -> bool {
        !self.own_reads.is_empty()
    }

    /// How much of a secret input `port` of the node at `path` may carry.
    pub fn input(&self, path: &[u32], port: usize) -> Taint {
        if self.under_opaque(path) {
            return Taint::Derived;
        }
        self.nodes.get(path).and_then(|(ins, _)| ins.get(port).copied()).unwrap_or_default()
    }
}

struct Walk<'r, R> {
    resolver: &'r R,
    /// The one secret followed, if not every one.
    only: Option<&'r str>,
    /// Following something else instead: what is answered to it ([`SecretFed::answers_of`]) or
    /// what is made with it ([`SecretFed::made_of`]), from the nodes where that begins. The secret
    /// leaves are clean then.
    from: Option<(&'r BTreeSet<Vec<u32>>, Follow)>,
    /// Whether a unit (by hash) reads a followed secret anywhere in its closure.
    holds_secret: HashMap<Hash, bool>,
    /// What the walk may still visit.
    visits: &'r Visits,
    /// Each unit taken as not followed, by its place in memory (the resolver's, for the walk):
    /// what its closure could do, worked out once however many paths reach it.
    capabilities: HashMap<usize, Capabilities>,
}

impl<R: Resolver> Walk<'_, R> {
    /// Whether `unit` is a `secret` leaf this walk follows.
    fn reads(&self, unit: &Unit) -> bool {
        matches!(&unit.body, UnitBody::Primitive { name } if name == "secret") && self.only.is_none_or(|n| ident(&unit.params, "name").as_deref() == Some(n))
    }

    /// Whether the unit at `hash` reads a secret anywhere below it (an unresolvable reference
    /// counts as one).
    fn holds(&mut self, hash: &Hash) -> bool {
        if let Some(&known) = self.holds_secret.get(hash) {
            return known;
        }
        // Iterative, so a deep closure can't exhaust the stack.
        let mut stack = vec![(*hash, false)];
        while let Some((h, expanded)) = stack.pop() {
            if self.holds_secret.contains_key(&h) {
                continue;
            }
            let Some(unit) = self.resolver.resolve(&h) else {
                self.holds_secret.insert(h, true);
                continue;
            };
            let refs = child_refs(unit);
            if expanded {
                let own = self.reads(unit);
                let below = refs.iter().any(|r| self.holds_secret.get(r).copied().unwrap_or(true));
                self.holds_secret.insert(h, own || below);
            } else {
                stack.push((h, true));
                stack.extend(refs.into_iter().filter(|r| !self.holds_secret.contains_key(r)).map(|r| (r, false)));
            }
        }
        self.holds_secret[hash]
    }

    /// The taint of `unit`'s outputs, given its inputs'; what it reaches is recorded in `fed`.
    fn unit(&mut self, unit: &Unit, inputs: &[Taint], path: &mut Vec<u32>, fed: &mut SecretFed) -> Vec<Taint> {
        self.visits.take();
        let most = inputs.iter().copied().max().unwrap_or_default();
        let reads = self.reads(unit) && self.from.is_none();
        let follows = self.from.map(|(_, follow)| follow);
        let begins = self.from.is_some_and(|(at, _)| at.contains(path.as_slice()));
        // What a node not followed answers is where following what is answered starts: all of it.
        // (What is made starts only at a leaf.)
        if begins && !matches!(&unit.body, UnitBody::Primitive { name } if ANSWERED.contains(&name.as_str()) || MAKES.contains(&name.as_str())) {
            let outputs = vec![Taint::Derived; unit.outputs.len()];
            fed.nodes.insert(path.clone(), (inputs.to_vec(), outputs.clone()));
            return outputs;
        }
        // Following what is answered, a request's answer carries it: where it starts, and an
        // answer to a request fed it (an API echoing it back, a token exchanged for another).
        let answering = follows == Some(Follow::Answers) && (begins || most > Taint::Clean);
        // Following what is made, a hash or a signature carries it: where it starts, and a hash
        // of one (a key derived in steps is still made with the secret).
        let making = follows == Some(Follow::Made) && (begins || most > Taint::Clean);
        let outputs = match &unit.body {
            UnitBody::Primitive { name } => {
                // What a leaf that passes its inputs on gives: the most of what it holds of them,
                // and derived if what only chooses is tainted at all.
                let chose = chose(name, &unit.inputs, inputs);
                let passed = || {
                    let held = inputs.iter().zip(&unit.inputs).filter(|(_, port)| !selects(name, port)).map(|(t, _)| *t).max().unwrap_or_default();
                    if chose > Taint::Clean || (ORDERS.contains(&name.as_str()) && held > Taint::Clean) { Taint::Derived } else { held }
                };
                let outputs: Vec<Taint> = unit
                    .outputs
                    .iter()
                    .map(|_| match name.as_str() {
                        "secret" if reads => Taint::Literal,
                        n if ANSWERED.contains(&n) && answering => Taint::Literal,
                        n if ANSWERED.contains(&n) => Taint::Clean,
                        n if MAKES.contains(&n) && making => Taint::Literal,
                        // One way of a secret whole: nothing of it comes back. Of a part of it,
                        // or of anything else computed from it, what comes out says that part:
                        // `sha256(k.slice(i, i + 1))` is the character, by a table of 256, and
                        // `secure_equals(k.slice(0, n), p)` whether `p` is how it begins.
                        n if ONE_WAY.contains(&n) && most <= Taint::Recoded => Taint::Clean,
                        n if ONE_WAY.contains(&n) => Taint::Derived,
                        // The secret whole, spelled another way: still whole. Of a part of it,
                        // or fed it anywhere but as what it spells, it is computed like any other.
                        n if RECODES.contains(&n) && most > Taint::Clean => {
                            let whole = inputs.first().is_some_and(|t| *t <= Taint::Recoded) && inputs.iter().skip(1).all(|t| *t == Taint::Clean);
                            if whole { Taint::Recoded } else { Taint::Derived }
                        }
                        // The token's payload comes out, nothing of the key: a shared secret's
                        // taint stops here, as at hmac_sha256; the token's goes on, decoded. A key
                        // or a clock computed from a secret is a part of it, as above.
                        "jwt_verify" if inputs.first().is_some_and(|t| *t > Taint::Clean) || most == Taint::Derived => Taint::Derived,
                        "jwt_verify" => Taint::Clean,
                        n if PASSES_LITERALLY.contains(&n) => passed(),
                        _ if most == Taint::Clean => Taint::Clean,
                        _ => Taint::Derived,
                    })
                    .collect();
                // A function applied inside (`map`, `fold`) isn't followed: one that reads a
                // secret, or is handed one, may compute anything with it.
                let applies_secret = self.from.is_none() && child_refs(unit).iter().any(|r| self.holds(r));
                if applies_secret || (!child_refs(unit).is_empty() && most > Taint::Clean) {
                    self.opaque(unit, path, fed);
                    vec![Taint::Derived; unit.outputs.len()]
                } else {
                    // A request fed a secret, as it is or computed from it, sends it to its host;
                    // any sink fed one does something with it. (A piped stream's channel gets the
                    // far side's answer, not what was sent.)
                    if most > Taint::Clean
                        && let Some(caps) = capabilities_of(name, &unit.params)
                    {
                        fed.sent.extend(caps.iter().filter_map(|c| destination_of(&c)));
                        let sinks = caps.iter().filter(|c| is_sink(c) && !(name == "http_stream" && matches!(c, Capability::ChannelSend(_))));
                        fed.sinks.extend(sinks.map(|c| (path.clone(), c)));
                        if ANSWERED.contains(&name.as_str()) {
                            fed.asked.insert(path.clone());
                        }
                        if caps.iter().any(|c| reads_own(&c)) {
                            fed.own_reads.insert(path.clone());
                        }
                    }
                    // A hash or a signature of a secret whole: clean, and followed from here.
                    if self.from.is_none() && MAKES.contains(&name.as_str()) && most > Taint::Clean && most <= Taint::Recoded {
                        fed.made.insert(path.clone());
                    }
                    // A sink fed something computed from a secret: its effect carries it.
                    if outputs.iter().any(|t| t.is_derived())
                        && let Some(caps) = capabilities_of(name, &unit.params)
                    {
                        fed.effects.extend(caps.iter().filter(|c| !c.is_sourced()));
                    }
                    // What it gives is its own, unless it only moves what it was handed.
                    let moves = PASSES_LITERALLY.contains(&name.as_str()) && !ORDERS.contains(&name.as_str()) && chose == Taint::Clean;
                    // (The secret spelled another way is its own too: `base64` of it is no
                    // longer the text redaction finds. Only the walk of the secret itself keeps
                    // these: one that follows an answer or a hash marks nothing.)
                    if self.from.is_none() && outputs.iter().any(|t| t.is_derived()) && !moves {
                        fed.computing.insert(path.clone());
                    }
                    // What a check running the run again on marks takes from its record: all a
                    // secret reaches but a call and a leaf that only moves it ([`SecretFed::taken`]).
                    if self.from.is_none() && most > Taint::Clean && !ANSWERED.contains(&name.as_str()) && (!moves || name == "as_text") {
                        fed.taken.insert(path.clone());
                    }
                    outputs
                }
            }
            UnitBody::Module { .. } => {
                if most > Taint::Clean {
                    self.opaque(unit, path, fed);
                    vec![Taint::Derived; unit.outputs.len()]
                } else {
                    vec![Taint::Clean; unit.outputs.len()]
                }
            }
            UnitBody::Composite { children, edges } => self.composite(unit, children, edges, inputs, path, fed),
        };
        // Following what is made, a Boolean ends it: whether a signature matches is what a
        // signature is for, however the flow compares it (`secure_equals`, `===`, a check of
        // each of several, as Stripe's), and a route that answers only that hands out none.
        let outputs = match follows {
            Some(Follow::Made) => outputs.into_iter().zip(&unit.outputs).map(|(t, port)| if port.ty == Type::Boolean { Taint::Clean } else { t }).collect(),
            _ => outputs,
        };
        if most > Taint::Clean || outputs.iter().any(|t| *t > Taint::Clean) {
            fed.nodes.insert(path.clone(), (inputs.to_vec(), outputs.clone()));
        }
        outputs
    }

    /// A node whose insides aren't followed: everything under it is derived, and so is every
    /// effect it could make.
    fn opaque(&mut self, unit: &Unit, path: &[u32], fed: &mut SecretFed) {
        fed.opaque.push(path.to_vec());
        let resolver = self.resolver;
        let caps = self.capabilities.entry(std::ptr::from_ref(unit) as usize).or_insert_with(|| capabilities(unit, resolver));
        if caps.iter().any(|c| destination_of(&c).is_some()) {
            fed.asked.insert(path.to_vec());
        }
        fed.sent.extend(caps.iter().filter_map(|c| destination_of(&c)));
        fed.effects.extend(caps.iter().filter(|c| !c.is_sourced()));
        fed.sinks.extend(caps.iter().filter(is_sink).map(|c| (path.to_vec(), c)));
        if caps.iter().any(|c| reads_own(&c)) {
            fed.own_reads.insert(path.to_vec());
        }
    }

    fn composite(&mut self, unit: &Unit, children: &[Hash], edges: &[crate::Edge], inputs: &[Taint], path: &mut Vec<u32>, fed: &mut SecretFed) -> Vec<Taint> {
        let derived = vec![Taint::Derived; unit.outputs.len()];
        // Nothing tainted comes in and nothing inside reads a secret, or answers what one reached:
        // nothing to follow.
        let answers_inside = self.from.is_some_and(|(at, _)| at.iter().any(|a| a.len() > path.len() && a.starts_with(path)));
        if inputs.iter().all(|t| *t == Taint::Clean) && !answers_inside && !children.iter().any(|c| self.holds(c)) {
            return vec![Taint::Clean; unit.outputs.len()];
        }
        // Too deep, or the walks have visited all they may: not followed.
        if path.len() >= MAX_DEPTH || self.visits.spent() {
            self.opaque(unit, path, fed);
            return derived;
        }
        let Some(order) = topo_order(children.len(), edges) else {
            self.opaque(unit, path, fed);
            return derived;
        };
        let mut outs: Vec<Option<Vec<Taint>>> = vec![None; children.len()];
        let follows_answers = self.from.is_some_and(|(_, follow)| follow == Follow::Answers);
        // Each choice among the children, and the taint of each of its inputs.
        let mut choices: Vec<Option<(&'static str, Vec<Taint>)>> = vec![None; children.len()];
        for ci in order {
            let Some(child) = self.resolver.resolve(&children[ci]) else {
                self.opaque(unit, path, fed);
                return derived;
            };
            let mut child_inputs = vec![Taint::Clean; child.inputs.len()];
            for e in edges.iter().filter(|e| e.to_child as usize == ci) {
                let from = if e.from_child == BOUNDARY {
                    inputs.get(e.from_port as usize).copied().unwrap_or_default()
                } else {
                    outs.get(e.from_child as usize).and_then(|o| o.as_ref()).and_then(|o| o.get(e.from_port as usize)).copied().unwrap_or_default()
                };
                if let Some(slot) = child_inputs.get_mut(e.to_port as usize) {
                    *slot = (*slot).max(from);
                }
            }
            // A response's status is a number the far side chose among a few, not what it
            // answered with: `sent.status`, read straight off a request, carries no token.
            let status_of_a_response = follows_answers
                && matches!(&child.body, UnitBody::Primitive { name } if name == "field")
                && ident(&child.params, "name").as_deref() == Some("status")
                && edges.iter().filter(|e| e.to_child as usize == ci).all(|e| {
                    children.get(e.from_child as usize).and_then(|h| self.resolver.resolve(h)).is_some_and(|from| matches!(&from.body, UnitBody::Primitive { name } if HTTP_ANSWERED.contains(&name.as_str())))
                });
            if status_of_a_response {
                outs[ci] = Some(vec![Taint::Clean; child.outputs.len()]);
                continue;
            }
            if let UnitBody::Primitive { name } = &child.body
                && let Some(choice) = CHOICES.iter().find(|c| **c == name.as_str())
            {
                choices[ci] = Some((*choice, child_inputs.clone()));
            }
            path.push(ci as u32);
            outs[ci] = Some(self.unit(child, &child_inputs, path, fed));
            path.pop();
        }
        chosen_among(children.len(), edges, &choices, path, fed);
        let mut outputs = vec![Taint::Clean; unit.outputs.len()];
        for e in edges.iter().filter(|e| e.to_child == BOUNDARY) {
            let from = if e.from_child == BOUNDARY {
                inputs.get(e.from_port as usize).copied().unwrap_or_default()
            } else {
                outs.get(e.from_child as usize).and_then(|o| o.as_ref()).and_then(|o| o.get(e.from_port as usize)).copied().unwrap_or_default()
            };
            if let Some(slot) = outputs.get_mut(e.to_port as usize) {
                *slot = (*slot).max(from);
            }
        }
        outputs
    }
}

/// The secrets a server's routes may answer their callers with: each secret a route reads whose
/// value, or something computed from it, may reach its response (output 0), or by which it reads
/// its flow's own data ([`SecretFed::reads_by_a_secret`]: how that read is answered, found,
/// failed or kept waiting, is how the route answers). An action's answer isn't one: it goes back
/// through Holon, withheld and redacted. Given a route's endpoint alone, its own. Each is a
/// permission, `SecretAnswer("name")`, which a person approves at deploy.
pub fn answered_secrets(root: &Unit, resolver: &impl Resolver) -> BTreeSet<String> {
    let mut answered = BTreeSet::new();
    let visits = Visits::default();
    for endpoint in endpoints_of(root, resolver) {
        for cap in capabilities(endpoint, resolver).iter() {
            if let Capability::SecretRead(name) = cap
                && !answered.contains(&name)
            {
                let fed = SecretFed::of_secret_within(endpoint, resolver, &name, &visits);
                if fed.reaches_output(0) || fed.reads_by_a_secret() {
                    answered.insert(name);
                }
            }
        }
    }
    answered
}

/// Where each secret `root` reads is sent: the hosts of the outbound requests (`http.get`, an
/// upload, a stream…) its value, or something computed from it, may reach, in their path, headers
/// or body, and the databases of the reads whose parameters it may reach (`db:shop`,
/// [`destination_of`]). A one-way leaf and a call's answer cut it, as for [`answered_secrets`]; a
/// secret sent nowhere isn't listed. Every pair is known before the flow runs (a request's host is
/// a frozen param), so the approval says it beside the secret: "read the secret "k", and send it
/// to https://api.example.com".
///
/// Where what is answered to it goes ([`SecretFed::answers_of`]: the token a token exchange
/// answers, an API's echo) is listed with it, each such destination as `answer:` and the
/// destination (`answer:api.example.com`): "…; what is answered to it goes to
/// https://api.example.com". So is where a hash or a signature made with it goes
/// ([`SecretFed::made_of`]), as `made:` and the destination: "…; a hash or signature made with it
/// goes to https://api.example.com". For both, a route's own callers are a destination too
/// ([`CALLERS`]): a token or a signature handed to whoever calls leaves the flow as surely as one
/// sent to a host. A secret that only does that is listed, though it is sent nowhere itself.
pub fn sent_secrets(root: &Unit, resolver: &impl Resolver) -> BTreeMap<String, BTreeSet<String>> {
    sent_secrets_within(root, resolver, &Visits::default())
}

/// [`sent_secrets`], its walks visiting no more than `visits` has left between them.
pub fn sent_secrets_within(root: &Unit, resolver: &impl Resolver, visits: &Visits) -> BTreeMap<String, BTreeSet<String>> {
    let mut sent = BTreeMap::new();
    // What answers a caller: a route, and a websocket's open and message blocks.
    let mut endpoints = endpoints_of(root, resolver);
    if let Ok(server) = read_server(root, resolver) {
        let blocks = server.websockets.iter().flat_map(|w| [&w.open, &w.message]).flatten();
        endpoints.extend(blocks.filter_map(|b| resolver.resolve(&b.handler)));
    }
    for cap in capabilities(root, resolver).iter() {
        if let Capability::SecretRead(name) = cap {
            let mut to = SecretFed::of_secret_within(root, resolver, &name, visits).sent;
            to.extend(SecretFed::answers_of_within(root, resolver, &name, visits).sent.into_iter().map(|d| format!("{ANSWER}{d}")));
            to.extend(SecretFed::made_of_within(root, resolver, &name, visits).sent.into_iter().map(|d| format!("{MADE}{d}")));
            if endpoints.iter().any(|endpoint| SecretFed::answers_of_within(endpoint, resolver, &name, visits).reaches_output(0)) {
                to.insert(format!("{ANSWER}{CALLERS}"));
            }
            if endpoints.iter().any(|endpoint| SecretFed::made_of_within(endpoint, resolver, &name, visits).reaches_output(0)) {
                to.insert(format!("{MADE}{CALLERS}"));
            }
            if !to.is_empty() {
                sent.insert(name, to);
            }
        }
    }
    sent
}

/// Where each secret `root` reads is itself sent, and nothing else of [`sent_secrets`]: the
/// hosts and databases a run's requests holding its value are held to. One walk a secret, where
/// [`sent_secrets`] takes several (what is answered, what is made, each route's answer): a run
/// asks this, every time it reads a secret; an approval asks the other, once.
pub fn sent_itself(root: &Unit, resolver: &impl Resolver) -> BTreeMap<String, BTreeSet<String>> {
    let mut sent = BTreeMap::new();
    // Its walks are one question's, on one budget ([`Visits`]): a run reading many secrets
    // doesn't walk its paths once for each. They are a part of `sent_secrets`'s, taken in its
    // order, so what this says a secret is sent to is what the approval says or less.
    let visits = Visits::default();
    for cap in capabilities(root, resolver).iter() {
        if let Capability::SecretRead(name) = cap {
            let to = SecretFed::of_secret_within(root, resolver, &name, &visits).sent;
            if !to.is_empty() {
                sent.insert(name, to);
            }
        }
    }
    sent
}

/// Whether a destination [`sent_secrets`] lists is where the secret itself is sent: a host, a
/// database. The others are where what is answered to it, or made with it, goes.
pub fn is_own_destination(destination: &str) -> bool {
    !destination.starts_with(ANSWER) && !destination.starts_with(MADE)
}

/// What marks a destination in [`sent_secrets`] as where what is answered to a secret goes.
pub const ANSWER: &str = "answer:";

/// What marks a destination in [`sent_secrets`] as where a hash or a signature made with a secret
/// goes ([`SecretFed::made_of`]).
pub const MADE: &str = "made:";

/// The destination, in [`sent_secrets`], that is a route's own callers: its response. Only what
/// is answered to a secret (`answer:@callers`) or made with it (`made:@callers`) is listed as
/// going there; the secret itself answered to them is a permission of its own, `SecretAnswer`.
/// No host or database is named so.
pub const CALLERS: &str = "@callers";

/// The endpoints whose response goes to a caller: a server's routes, or `root` itself when it is
/// one.
fn endpoints_of<'r>(root: &'r Unit, resolver: &'r impl Resolver) -> Vec<&'r Unit> {
    match read_server(root, resolver) {
        Ok(server) => server.routes.iter().filter_map(|r| resolver.resolve(&r.endpoint)).collect(),
        Err(_) if declaration_of(root) == Some("endpoint") => vec![root],
        Err(_) => vec![],
    }
}

/// A unit's manifest as deployed: what its graph derives, and `SecretAnswer` for each secret its
/// routes may answer with ([`answered_secrets`]). The host deploys and grants by it, and
/// `holon check` and the editor show it.
pub fn deployed_capabilities(root: &Unit, resolver: &impl Resolver) -> Capabilities {
    let mut caps = capabilities(root, resolver);
    for name in answered_secrets(root, resolver) {
        caps.insert(Capability::SecretAnswer(name));
    }
    caps
}

/// Where a capability sends what its node is fed, as [`sent_secrets`] names it: an outbound
/// request's host (`api.example.com`), or a database read's database (`db:shop`, its parameters
/// go to the database's server). A database write isn't one: the host refuses to write a secret.
pub fn destination_of(capability: &Capability) -> Option<String> {
    match capability {
        Capability::HttpRequest(host) => Some(host.clone()),
        Capability::DbRead(table) => Some(format!("db:{}", table.split('/').next().unwrap_or(table))),
        _ => None,
    }
}

/// Whether a node with `capability` does something outside the run with what it is fed: an
/// effect, or a database read, whose parameters go to the database.
fn is_sink(capability: &Capability) -> bool {
    !capability.is_sourced() || matches!(capability, Capability::DbRead(_))
}

/// Whether `capability` reads the flow's own data by what it is handed: a collection's items, a
/// folder's files ([`SecretFed::reads_by_a_secret`]). Not a database's: it is where the approval
/// says a secret fed to it goes.
fn reads_own(capability: &Capability) -> bool {
    matches!(capability, Capability::CollectionRead(_) | Capability::FileRead(_))
}

/// The leaves the evaluator decides lazily (eval.rs, `choice_next`): each reads some of its inputs
/// only as what it has read so far decides.
const CHOICES: [&str; 4] = ["guard", "coalesce", "and", "or"];

/// Whether `choice` reads input `port` only as its inputs before it decide, and those may carry a
/// secret (`inputs`): `guard` its value by its condition, `coalesce` its second input by whether
/// the first is there, `and` and `or` each input by those before it.
fn decided_by_a_secret(choice: &str, port: usize, inputs: &[Taint]) -> bool {
    let deciding = match (choice, port) {
        ("guard" | "coalesce", 1) => &inputs[..1.min(inputs.len())],
        ("and" | "or", p) if p > 0 => &inputs[..p.min(inputs.len())],
        _ => return false,
    };
    deciding.iter().any(|t| *t > Taint::Clean)
}

/// The children of a composite at `path` (`n` of them, wired by `edges`) that are choices decided
/// by a secret ([`SecretFed::deciders`]), and those that run only when such a choice takes them
/// ([`SecretFed::chosen`]): those no road reaches from what the composite gives (the children
/// wired to its outputs, and those nothing consumes) but through an input such a choice reads
/// only as its decision goes. `choices`: each child that is a choice, and its inputs' taint.
fn chosen_among(n: usize, edges: &[crate::Edge], choices: &[Option<(&str, Vec<Taint>)>], path: &[u32], fed: &mut SecretFed) {
    let lazy = |to: usize, port: usize| choices[to].as_ref().is_some_and(|(choice, inputs)| decided_by_a_secret(choice, port, inputs));
    let at = |ci: usize| path.iter().copied().chain([ci as u32]).collect::<Vec<u32>>();
    let deciders: Vec<usize> = (0..n).filter(|&ci| choices[ci].as_ref().is_some_and(|(_, inputs)| (1..inputs.len()).any(|p| lazy(ci, p)))).collect();
    if deciders.is_empty() {
        return;
    }
    fed.deciders.extend(deciders.into_iter().map(at));
    let child = |c: u32| (c != BOUNDARY && (c as usize) < n).then_some(c as usize);
    let mut reached = vec![false; n];
    let mut stack: Vec<usize> = (0..n)
        .filter(|ci| edges.iter().any(|e| e.from_child as usize == *ci && e.to_child == BOUNDARY) || !edges.iter().any(|e| e.from_child as usize == *ci && e.from_child != BOUNDARY))
        .collect();
    while let Some(ci) = stack.pop() {
        if std::mem::replace(&mut reached[ci], true) {
            continue;
        }
        for e in edges.iter().filter(|e| child(e.to_child) == Some(ci)) {
            if let Some(from) = child(e.from_child)
                && !reached[from]
                && !lazy(ci, e.to_port as usize)
            {
                stack.push(from);
            }
        }
    }
    fed.chosen.extend((0..n).filter(|&ci| !reached[ci]).map(at));
}

/// Kahn's algorithm over the child→child edges: an order in which every child comes after those
/// it reads from, or `None` for a cycle (which core never evaluates either).
fn topo_order(n: usize, edges: &[crate::Edge]) -> Option<Vec<usize>> {
    let inner = |e: &&crate::Edge| (e.from_child as usize) < n && (e.to_child as usize) < n;
    let mut indegree = vec![0usize; n];
    for e in edges.iter().filter(inner) {
        indegree[e.to_child as usize] += 1;
    }
    let mut ready: Vec<usize> = (0..n).filter(|i| indegree[*i] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(node) = ready.pop() {
        order.push(node);
        for e in edges.iter().filter(inner).filter(|e| e.from_child as usize == node) {
            let to = e.to_child as usize;
            indegree[to] -= 1;
            if indegree[to] == 0 {
                ready.push(to);
            }
        }
    }
    (order.len() == n).then_some(order)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{interface_of, Edge, MapResolver, Param, Port};

    fn leaf(r: &mut MapResolver, name: &str, params: Vec<Param>) -> Hash {
        r.insert(Unit { params, ..interface_of(name).unwrap() }).unwrap()
    }

    /// `secret → base64 → concat("Basic ", ·)` into a header, `secret → hmac` into the answer.
    #[test]
    fn a_secret_is_followed_until_something_one_way_cuts_it() {
        let mut r = MapResolver::new();
        let secret = leaf(&mut r, "secret", vec![Param::frozen("name", "k")]);
        let b64 = leaf(&mut r, "base64", vec![]);
        let basic = leaf(&mut r, "const", vec![Param::frozen("value", "Basic "), Param::frozen("type", "Text")]);
        let concat = leaf(&mut r, "concat", vec![]);
        let hmac = leaf(&mut r, "hmac_sha256", vec![]);
        let e = |fc, fp, tc, tp| Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp };
        let root = Unit {
            body: UnitBody::Composite {
                children: vec![secret, b64, basic, concat, hmac],
                edges: vec![
                    e(0, 0, 1, 0),
                    e(2, 0, 3, 0),
                    e(1, 0, 3, 1),
                    e(0, 0, 4, 0),
                    e(BOUNDARY, 0, 4, 1),
                    e(3, 0, BOUNDARY, 0),
                    e(4, 0, BOUNDARY, 1),
                    e(0, 0, BOUNDARY, 2),
                ],
            },
            params: vec![],
            inputs: vec![Port { name: "message".into(), ty: Type::Text }],
            outputs: vec![Port { name: "auth".into(), ty: Type::Any }, Port { name: "sig".into(), ty: Type::Any }, Port { name: "raw".into(), ty: Type::Any }],
        };
        let fed = SecretFed::of(&root, &r);
        assert!(!fed.derives(&[0]), "the secret itself is literal: redaction finds it");
        assert!(fed.derives(&[1]), "base64 of it is derived");
        assert!(fed.derives(&[3]), "and so is what's built from that");
        assert!(!fed.derives(&[2]), "a constant isn't reached");
        assert!(!fed.derives(&[4]), "an hmac cuts it");
        assert_eq!(fed.input(&[3], 1), Taint::Recoded, "the secret whole, spelled as base64: derived to the log, whole to a hash");
        assert_eq!(fed.input(&[3], 0), Taint::Clean);
        assert!(fed.derives(&[]), "the root's first output is derived");
    }

    /// Guards: which leaves compute something of their own from a secret
    /// ([`SecretFed::computes`]), and when every node that may compute from one is followed
    /// ([`SecretFed::followed_to`]). A leaf that encodes, measures or reads a secret as a number
    /// computes a value of its own; one that joins such a value into a text or a list only moves
    /// it; one that orders such values, or picks by one, computes something again. A function
    /// applied by `map` isn't followed. And which a check running the run again on marks takes
    /// from its record ([`SecretFed::taken`]): each that computes, and a hash of the secret
    /// whole, clean as it is; not one that only moves.
    /// Why: a host marks what a run computed from a secret by keeping what each such leaf gave
    /// and finding those values in the run's record. A leaf left off (taken as only moving)
    /// gives a value nothing marks, written to the log as it is. And a record that says it
    /// marked every such value keeps each step the check takes from it (R66).
    /// If it fails: the node named is the leaf whose output the log would no longer mark, or
    /// would mark though it holds nothing of its own (then a check takes it from the record);
    /// or one the check takes from a record that may leave it out, or computes though it can't.
    #[test]
    fn a_leaf_computes_from_a_secret_unless_it_only_moves_what_it_was_handed() {
        let mut r = MapResolver::new();
        let secret = leaf(&mut r, "secret", vec![Param::frozen("name", "k")]);
        let b64 = leaf(&mut r, "base64", vec![]);
        let basic = leaf(&mut r, "const", vec![Param::frozen("value", "Basic "), Param::frozen("type", "Text")]);
        let concat = leaf(&mut r, "concat", vec![]);
        let list = leaf(&mut r, "list", vec![]);
        let sort = leaf(&mut r, "sort", vec![]);
        let number = leaf(&mut r, "to_number", vec![]);
        let get = leaf(&mut r, "get", vec![]);
        let hmac = leaf(&mut r, "hmac_sha256", vec![]);
        let children = vec![secret, b64, basic, concat, list, sort, number, get, hmac];
        let edges = vec![
            e(0, 0, 1, 0),
            e(2, 0, 3, 0),
            e(1, 0, 3, 1),
            e(3, 0, 4, 0),
            e(2, 0, 4, 1),
            e(4, 0, 5, 0),
            e(0, 0, 6, 0),
            e(BOUNDARY, 0, 7, 0),
            e(6, 0, 7, 1),
            e(0, 0, 8, 0),
            e(BOUNDARY, 1, 8, 1),
            e(5, 0, BOUNDARY, 0),
            e(7, 0, BOUNDARY, 1),
            e(8, 0, BOUNDARY, 2),
        ];
        let root = |children: Vec<Hash>, edges: Vec<Edge>| Unit {
            body: UnitBody::Composite { children, edges },
            params: vec![],
            inputs: vec![Port { name: "items".into(), ty: Type::list_any() }, Port { name: "message".into(), ty: Type::Text }],
            outputs: ["sorted", "picked", "sig"].map(|n| Port { name: n.into(), ty: Type::Any }).to_vec(),
        };
        let fed = SecretFed::of(&root(children.clone(), edges.clone()), &r);
        // (the node, whether it computes something of its own from the secret, whether a check
        // takes it from the record, why)
        let rows = [
            (0, false, false, "the secret itself: its value, found as it is, its mark given"),
            (1, true, true, "base64 of it is a value of its own"),
            (2, false, false, "a constant isn't reached"),
            (3, false, false, "a text joined with the encoded value only holds it"),
            (4, false, false, "a list of it only holds it"),
            (5, true, true, "sorted, the order says which made value comes first"),
            (6, true, true, "read as a number"),
            (7, true, true, "an item picked by a number computed from it"),
            (8, false, true, "an hmac cuts it: nothing computed from the secret comes out, and nothing computes it on a mark"),
        ];
        for (node, computes, taken, why) in rows {
            assert_eq!(fed.computes(&[node]), computes, "node {node}: {why}");
            assert_eq!(fed.taken(&[node]), taken, "node {node}, taken: {why}");
        }
        assert_eq!(fed.computing().map(<[u32]>::to_vec).collect::<Vec<_>>(), [vec![1], vec![5], vec![6], vec![7]]);
        assert!(fed.followed_to(1), "every node that derives is one of the root's own");
        assert!(!fed.followed_to(0), "they are one level down");
        // A function applied to each item isn't followed: what it makes inside isn't known.
        let truthy = leaf(&mut r, "is_truthy", vec![]);
        let filter = leaf(&mut r, "filter", vec![Param::frozen("fn", crate::hash::to_hex(&truthy))]);
        let (mut children, mut edges) = (children, edges);
        children.push(filter);
        edges.push(e(4, 0, 9, 0));
        let fed = SecretFed::of(&root(children, edges), &r);
        assert!(fed.derives(&[9]) && !fed.followed_to(8), "a filter over made values isn't followed");
    }

    /// Guards: a leaf that orders what it is handed (`sort`, `unique`) gives something derived
    /// from a secret among its items, not the items moved: `[k, "m"].toSorted()[0]` is the
    /// secret just when it sorts first, and `[...new Set([k, p])]` keeps one item just when
    /// they are equal.
    /// Why: taken as literal, what it picked was logged as the secret's mark or as the other
    /// value in the clear, and answered as either: an oracle for the secret's order.
    /// If it fails: the leaf named passes a secret on as literal, and a log or an answer says
    /// where it came in the order.
    #[test]
    fn ordering_a_secret_among_other_values_derives() {
        for name in ["sort", "unique"] {
            let mut r = MapResolver::new();
            let secret = leaf(&mut r, "secret", vec![Param::frozen("name", "k")]);
            let m = leaf(&mut r, "const", vec![Param::frozen("value", "m"), Param::frozen("type", "Text")]);
            let list = leaf(&mut r, "list", vec![]);
            let order = leaf(&mut r, name, vec![]);
            let e = |fc, fp, tc, tp| Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp };
            let root = Unit {
                body: UnitBody::Composite { children: vec![secret, m, list, order], edges: vec![e(0, 0, 2, 0), e(1, 0, 2, 1), e(2, 0, 3, 0), e(3, 0, BOUNDARY, 0)] },
                params: vec![],
                inputs: vec![],
                outputs: vec![Port { name: "first".into(), ty: Type::Any }],
            };
            let fed = SecretFed::of(&root, &r);
            assert_eq!(fed.output(&[2], 0), Taint::Literal, "{name}: a list holding the secret holds it as it is");
            assert_eq!(fed.output(&[3], 0), Taint::Derived, "{name}: what it gives says where the secret came");
            assert!(fed.computes(&[3]) && fed.derives(&[]), "{name}");
        }
    }

    /// Guards: which steps run only when a choice decided by a secret takes them
    /// ([`SecretFed::chosen`]), and which choices it decides ([`SecretFed::deciders`], all of
    /// them and given the secret alone). `c ? a : b` lowers to `coalesce(guard(c, a), b)`, and the
    /// evaluator runs `a` only when `c` is true, `b` only when the guard gave nothing; `c && d`
    /// runs `d` only when `c` is true. With `c` computed from the secret, `a`, `b` and `d` are
    /// chosen, and nothing else is; with `c` the caller's, nothing is.
    /// Why: a run's log lists the steps it took. One run listing `a` and another `b` says which
    /// way the secret went, though every value either shows is withheld.
    /// If it fails: the node named runs whatever the secret but is taken as chosen (the log
    /// drops a step it took), or runs as a secret decides and isn't (the log says which way).
    #[test]
    fn a_step_a_choice_decided_by_a_secret_takes_is_chosen() {
        let mut r = MapResolver::new();
        let secret = leaf(&mut r, "secret", vec![Param::frozen("name", "k")]);
        let truthy = leaf(&mut r, "is_truthy", vec![]);
        let guard = leaf(&mut r, "guard", vec![]);
        let live = leaf(&mut r, "const", vec![Param::frozen("value", "live mode"), Param::frozen("type", "Text")]);
        let coalesce = leaf(&mut r, "coalesce", vec![]);
        let test = leaf(&mut r, "const", vec![Param::frozen("value", "test mode"), Param::frozen("type", "Text")]);
        let and = leaf(&mut r, "and", vec![]);
        let asked = leaf(&mut r, "is_truthy", vec![]);
        let e = |fc, fp, tc, tp| Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp };
        // The condition is the secret's (from child 0) or the caller's (from the root's input).
        let root = |from: u32| Unit {
            body: UnitBody::Composite {
                children: vec![secret, truthy, guard, live, coalesce, test, and, asked],
                edges: vec![
                    e(from, 0, 1, 0),
                    e(1, 0, 2, 0),
                    e(3, 0, 2, 1),
                    e(2, 0, 4, 0),
                    e(5, 0, 4, 1),
                    e(4, 0, BOUNDARY, 0),
                    e(1, 0, 6, 0),
                    e(BOUNDARY, 0, 7, 0),
                    e(7, 0, 6, 1),
                    e(6, 0, BOUNDARY, 1),
                ],
            },
            params: vec![],
            inputs: vec![Port { name: "q".into(), ty: Type::Any }],
            outputs: vec![Port { name: "mode".into(), ty: Type::Any }, Port { name: "both".into(), ty: Type::Any }],
        };
        let fed = SecretFed::of(&root(0), &r);
        // (the node, whether it is chosen, why)
        let rows = [
            (0, false, "the secret: what decides is read first"),
            (1, false, "the condition: every choice reads it"),
            (2, false, "the guard: the coalesce reads it first"),
            (3, true, "the guard's value: read only when the secret's condition is true"),
            (4, false, "the coalesce: what the route answers"),
            (5, true, "the coalesce's second input: read only when the guard gave nothing"),
            (6, false, "the and: what the route answers"),
            (7, true, "the and's second input: read only when the secret's condition is true"),
        ];
        for (node, chosen, why) in rows {
            assert_eq!(fed.chosen(&[node]), chosen, "node {node}: {why}");
        }
        assert!(fed.chosen(&[3, 0]), "inside a chosen step is chosen");
        // The guard (by its condition), the coalesce (by the guard's value, chosen) and the and.
        let deciders = |fed: &SecretFed| fed.deciders().map(<[u32]>::to_vec).collect::<Vec<_>>();
        assert_eq!(deciders(&fed), vec![vec![2], vec![4], vec![6]], "the choices the secret decides");
        assert_eq!(deciders(&SecretFed::of_secret(&root(0), &r, "k")), deciders(&fed), "given the secret alone");
        assert!(deciders(&SecretFed::of_secret(&root(0), &r, "other")).is_empty(), "another secret decides none");
        let caller = SecretFed::of(&root(BOUNDARY), &r);
        assert!((0..8).all(|node| !caller.chosen(&[node])), "the caller's condition chooses nothing a secret decides");
        assert!(deciders(&caller).is_empty(), "nor decides any");
    }

    /// Guards: a route that reads its flow's own data by what a secret reaches (a collection's
    /// item under a key computed from it, a file named by it) may answer its caller with the
    /// secret ([`answered_secrets`]), whatever it returns; one that sends it to a database to
    /// read by doesn't, the database being where its approval says the secret goes.
    /// Why: such a read is answered by the secret: an item found or not, a file read or failed
    /// (the route answers 500), a key another run holds (503). The caller learns which.
    /// If it fails: the row's read is answered by the secret and the deploy asks no one, or a
    /// database's read asks what its approval already covers.
    #[test]
    fn a_read_of_the_flows_own_data_by_a_secret_answers_with_it() {
        for (name, params, answers) in [
            ("collection_get", vec![Param::frozen("collection", "seen")], true),
            ("file_read", vec![Param::frozen("folder", "notes")], true),
            ("db_query", vec![Param::frozen("database", "shop"), Param::frozen("sql", "select 1 where ? = ?")], false),
        ] {
            let mut r = MapResolver::new();
            let secret = leaf(&mut r, "secret", vec![Param::frozen("name", "k")]);
            let slice = leaf(&mut r, "base64", vec![]);
            let Some(read) = interface_of(name).map(|u| r.insert(Unit { params: params.clone(), ..u }).unwrap()) else { panic!("{name}") };
            let ok = leaf(&mut r, "const", vec![Param::frozen("value", "ok"), Param::frozen("type", "Text")]);
            let e = |fc, fp, tc, tp| Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp };
            let endpoint = Unit {
                body: UnitBody::Composite { children: vec![secret, slice, read, ok], edges: vec![e(0, 0, 1, 0), e(1, 0, 2, 0), e(3, 0, BOUNDARY, 0)] },
                params: vec![Param::frozen("declare", "endpoint")],
                inputs: vec![],
                outputs: vec![Port { name: "answer".into(), ty: Type::Any }],
            };
            let fed = SecretFed::of(&endpoint, &r);
            assert_eq!(fed.reads_by_a_secret(), answers, "{name}");
            assert_eq!(answered_secrets(&endpoint, &r).contains("k"), answers, "{name}: the route answers ok, whatever it read");
        }
    }

    /// `jwt_verify` keyed with a secret gives the token's payload, nothing of the key: the
    /// secret's taint stops there. A token that is itself a secret goes on, decoded.
    #[test]
    fn a_jwt_verifys_key_is_cut_and_its_token_goes_on() {
        let mut r = MapResolver::new();
        let key = leaf(&mut r, "secret", vec![Param::frozen("name", "jwt_key")]);
        let token = leaf(&mut r, "secret", vec![Param::frozen("name", "kept_token")]);
        let verify = leaf(&mut r, "jwt_verify", vec![Param::frozen("issuer", ""), Param::frozen("audience", ""), Param::frozen("leeway", "0")]);
        let e = |fc, fp, tc, tp| Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp };
        let keyed = |token_from: u32| Unit {
            body: UnitBody::Composite {
                children: vec![key, verify, token],
                edges: vec![e(token_from, 0, 1, 0), e(0, 0, 1, 1), e(BOUNDARY, 1, 1, 2), e(1, 0, BOUNDARY, 0)],
            },
            params: vec![],
            inputs: vec![Port { name: "token".into(), ty: Type::Text }, Port { name: "now".into(), ty: Type::Number }],
            outputs: vec![Port { name: "claims".into(), ty: Type::Any }],
        };
        let fed = SecretFed::of(&keyed(BOUNDARY), &r);
        assert_eq!(fed.input(&[1], 1), Taint::Literal, "the key is the secret");
        assert!(!fed.derives(&[1]) && !fed.derives(&[]), "the claims carry nothing of it");
        let fed = SecretFed::of(&keyed(2), &r);
        assert!(fed.derives(&[1]), "a secret token's claims are derived from it");
    }

    /// A number computed from a secret (`to_number`, then `math {fn: "abs"}`, then written back
    /// with `number_format`) is derived from it, and so is whether the secret reads as a number
    /// (`number_test`): a Boolean says something of it too.
    #[test]
    fn the_number_leaves_and_a_number_test_derive() {
        let mut r = MapResolver::new();
        let secret = leaf(&mut r, "secret", vec![Param::frozen("name", "pin")]);
        let read = leaf(&mut r, "to_number", vec![Param::frozen("grammar", "int")]);
        let abs = leaf(&mut r, "math", vec![Param::frozen("fn", "abs")]);
        let written = leaf(&mut r, "number_format", vec![Param::frozen("style", "fixed")]);
        let test = leaf(&mut r, "number_test", vec![Param::frozen("test", "finite"), Param::frozen("coerce", "number")]);
        let e = |fc, fp, tc, tp| Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp };
        let root = Unit {
            body: UnitBody::Composite {
                children: vec![secret, read, abs, written, test],
                edges: vec![e(0, 0, 1, 0), e(1, 0, 2, 0), e(2, 0, 3, 0), e(0, 0, 4, 0), e(3, 0, BOUNDARY, 0), e(4, 0, BOUNDARY, 1)],
            },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "text".into(), ty: Type::Text }, Port { name: "ok".into(), ty: Type::Boolean }],
        };
        let fed = SecretFed::of(&root, &r);
        for (node, what) in [(1, "to_number"), (2, "math"), (3, "number_format")] {
            assert!(fed.derives(&[node]), "{what} of a secret is derived");
        }
        assert!(fed.derives(&[4]), "a number_test of it is derived");
    }

    /// `"Bearer " + secret` holds it as it is, as a template literal's `concat` does; a trimmed
    /// secret is derived from it, and so is whether it begins with a text.
    #[test]
    fn a_secret_joined_by_add_stays_literal_and_a_trimmed_one_is_derived() {
        let mut r = MapResolver::new();
        let secret = leaf(&mut r, "secret", vec![Param::frozen("name", "k")]);
        let bearer = leaf(&mut r, "const", vec![Param::frozen("value", "Bearer "), Param::frozen("type", "Text")]);
        let add = leaf(&mut r, "add", vec![]);
        let trim = leaf(&mut r, "trim", vec![]);
        let starts = leaf(&mut r, "text_contains", vec![]);
        let e = |fc, fp, tc, tp| Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp };
        let root = Unit {
            body: UnitBody::Composite {
                children: vec![secret, bearer, add, trim, starts],
                edges: vec![
                    e(1, 0, 2, 0),
                    e(0, 0, 2, 1),
                    e(0, 0, 3, 0),
                    e(0, 0, 4, 0),
                    e(BOUNDARY, 0, 4, 1),
                    e(2, 0, BOUNDARY, 0),
                    e(3, 0, BOUNDARY, 1),
                    e(4, 0, BOUNDARY, 2),
                ],
            },
            params: vec![],
            inputs: vec![Port { name: "prefix".into(), ty: Type::Text }],
            outputs: ["header", "trimmed", "starts"].map(|n| Port { name: n.into(), ty: Type::Any }).to_vec(),
        };
        let fed = SecretFed::of(&root, &r);
        assert_eq!(fed.output(&[2], 0), Taint::Literal, "the header holds it as it is");
        assert!(fed.derives(&[3]), "a trimmed secret is derived");
        assert!(fed.derives(&[4]), "whether it starts with a text is derived");
    }

    /// A record holding a secret, merged into another, still holds it as it is, and so do its
    /// values; whether it has a key is derived from it; and a `group_by` applies a function,
    /// which isn't followed.
    #[test]
    fn the_record_leaves_keep_a_secret_literal_and_has_derives() {
        let mut r = MapResolver::new();
        let secret = leaf(&mut r, "secret", vec![Param::frozen("name", "k")]);
        let insert = leaf(&mut r, "insert", vec![]);
        let merge = leaf(&mut r, "merge", vec![]);
        let values = leaf(&mut r, "values", vec![]);
        let has = leaf(&mut r, "has", vec![]);
        let key = leaf(&mut r, "as_text", vec![]);
        let group_by = leaf(&mut r, "group_by", vec![Param::frozen("fn", crate::hash::to_hex(&key))]);
        let e = |fc, fp, tc, tp| Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp };
        let root = Unit {
            body: UnitBody::Composite {
                children: vec![secret, insert, merge, values, has, group_by],
                edges: vec![
                    e(BOUNDARY, 0, 1, 0),
                    e(BOUNDARY, 1, 1, 1),
                    e(0, 0, 1, 2),
                    e(BOUNDARY, 0, 2, 0),
                    e(1, 0, 2, 1),
                    e(2, 0, 3, 0),
                    e(2, 0, 4, 0),
                    e(BOUNDARY, 1, 4, 1),
                    e(3, 0, 5, 0),
                    e(2, 0, BOUNDARY, 0),
                    e(3, 0, BOUNDARY, 1),
                    e(4, 0, BOUNDARY, 2),
                    e(5, 0, BOUNDARY, 3),
                ],
            },
            params: vec![],
            inputs: vec![Port { name: "r".into(), ty: Type::Record }, Port { name: "k".into(), ty: Type::Text }],
            outputs: ["merged", "values", "has", "groups"].map(|n| Port { name: n.into(), ty: Type::Any }).to_vec(),
        };
        let fed = SecretFed::of(&root, &r);
        assert_eq!(fed.output(&[2], 0), Taint::Literal, "the merged record holds it as it is");
        assert_eq!(fed.output(&[3], 0), Taint::Literal, "and so do its values");
        assert!(fed.derives(&[4]), "whether it has a key is derived");
        assert!(fed.derives(&[5]), "a group_by's function isn't followed");
    }

    /// A list holding a secret, reversed, still holds it as it is; whether it holds an item is
    /// derived from it, as a position in it is; and a `filter` over it applies a function, which
    /// isn't followed.
    #[test]
    fn the_list_leaves_keep_a_secret_literal_and_a_search_derives() {
        let mut r = MapResolver::new();
        let secret = leaf(&mut r, "secret", vec![Param::frozen("name", "k")]);
        let list = leaf(&mut r, "list", vec![]);
        let reverse = leaf(&mut r, "reverse", vec![]);
        let includes = leaf(&mut r, "includes", vec![]);
        let index_of = leaf(&mut r, "index_of", vec![]);
        let truthy = leaf(&mut r, "is_truthy", vec![]);
        let filter = leaf(&mut r, "filter", vec![Param::frozen("fn", crate::hash::to_hex(&truthy))]);
        let e = |fc, fp, tc, tp| Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp };
        let root = Unit {
            body: UnitBody::Composite {
                children: vec![secret, list, reverse, includes, index_of, filter],
                edges: vec![
                    e(0, 0, 1, 0),
                    e(BOUNDARY, 0, 1, 1),
                    e(1, 0, 2, 0),
                    e(1, 0, 3, 0),
                    e(BOUNDARY, 0, 3, 1),
                    e(1, 0, 4, 0),
                    e(BOUNDARY, 0, 4, 1),
                    e(1, 0, 5, 0),
                    e(2, 0, BOUNDARY, 0),
                    e(3, 0, BOUNDARY, 1),
                    e(4, 0, BOUNDARY, 2),
                    e(5, 0, BOUNDARY, 3),
                ],
            },
            params: vec![],
            inputs: vec![Port { name: "x".into(), ty: Type::Any }],
            outputs: ["reversed", "has", "at", "kept"].map(|n| Port { name: n.into(), ty: Type::Any }).to_vec(),
        };
        let fed = SecretFed::of(&root, &r);
        assert_eq!(fed.output(&[1], 0), Taint::Literal, "the list holds it as it is");
        assert_eq!(fed.output(&[2], 0), Taint::Literal, "and so does the list reversed");
        assert!(fed.derives(&[3]), "whether it includes an item is derived");
        assert!(fed.derives(&[4]), "a position in it is derived");
        assert!(fed.derives(&[5]), "a filter's function isn't followed");
    }

    /// `secret → base64` into a state write and the answer, `secret → hmac` into another write: the
    /// first write and the answer carry what is computed from it, the second doesn't.
    #[test]
    fn the_effects_and_outputs_a_secret_reaches_are_known() {
        let mut r = MapResolver::new();
        let secret = leaf(&mut r, "secret", vec![Param::frozen("name", "k")]);
        let b64 = leaf(&mut r, "base64", vec![]);
        let hmac = leaf(&mut r, "hmac_sha256", vec![]);
        let kept = leaf(&mut r, "store_write", vec![Param::frozen("key", "kept")]);
        let signed = leaf(&mut r, "store_write", vec![Param::frozen("key", "signed")]);
        let e = |fc, fp, tc, tp| Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp };
        let root = Unit {
            body: UnitBody::Composite {
                children: vec![secret, b64, hmac, kept, signed],
                edges: vec![
                    e(0, 0, 1, 0),
                    e(1, 0, 3, 0),
                    e(0, 0, 2, 0),
                    e(BOUNDARY, 0, 2, 1),
                    e(2, 0, 4, 0),
                    e(1, 0, BOUNDARY, 0),
                    e(BOUNDARY, 0, BOUNDARY, 1),
                ],
            },
            params: vec![],
            inputs: vec![Port { name: "message".into(), ty: Type::Text }],
            outputs: vec![Port { name: "answer".into(), ty: Type::Any }, Port { name: "echo".into(), ty: Type::Any }],
        };
        let fed = SecretFed::of(&root, &r);
        assert!(fed.derives_effect(&Capability::StateWrite("kept".into())));
        assert!(!fed.derives_effect(&Capability::StateWrite("signed".into())), "an hmac cuts it");
        assert!(!fed.derives_effect(&Capability::SecretRead("k".into())), "reading it is no effect of what's computed");
        assert!(fed.derives_output(0));
        assert!(!fed.derives_output(1));
    }

    /// A node whose insides aren't followed counts every effect it could make, and every output.
    #[test]
    fn a_node_not_followed_derives_all_it_could_do() {
        let mut fed = SecretFed::default();
        assert!(!fed.derives_output(0));
        fed.opaque.push(vec![]);
        assert!(fed.derives_output(0) && fed.derives_output(3));
    }

    #[test]
    fn a_flow_without_secrets_reaches_nothing() {
        let mut r = MapResolver::new();
        let b64 = leaf(&mut r, "base64", vec![]);
        let root = Unit {
            body: UnitBody::Composite { children: vec![b64], edges: vec![Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 }, Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 }] },
            params: vec![],
            inputs: vec![Port { name: "in".into(), ty: Type::Text }],
            outputs: vec![Port { name: "out".into(), ty: Type::Text }],
        };
        assert!(SecretFed::of(&root, &r).is_empty());
    }

    fn e(fc: u32, fp: u32, tc: u32, tp: u32) -> Edge {
        Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp }
    }

    /// Two secrets, one answered as it is and one only hashed: followed one at a time, each is
    /// known for what it reaches.
    #[test]
    fn one_secret_is_followed_alone() {
        let mut r = MapResolver::new();
        let token = leaf(&mut r, "secret", vec![Param::frozen("name", "token")]);
        let key = leaf(&mut r, "secret", vec![Param::frozen("name", "key")]);
        let hmac = leaf(&mut r, "hmac_sha256", vec![]);
        let root = Unit {
            body: UnitBody::Composite { children: vec![token, key, hmac], edges: vec![e(1, 0, 2, 0), e(BOUNDARY, 0, 2, 1), e(0, 0, BOUNDARY, 0), e(2, 0, BOUNDARY, 1)] },
            params: vec![],
            inputs: vec![Port { name: "message".into(), ty: Type::Text }],
            outputs: vec![Port { name: "answer".into(), ty: Type::Any }, Port { name: "sig".into(), ty: Type::Any }],
        };
        assert!(SecretFed::of_secret(&root, &r, "token").reaches_output(0), "answered as it is");
        assert!(!SecretFed::of_secret(&root, &r, "key").reaches_output(0), "the other secret isn't followed there");
        assert!(!SecretFed::of_secret(&root, &r, "key").reaches_output(1), "an hmac cuts it");
        assert!(!SecretFed::of(&root, &r).derives_output(0), "literal: not derived, though it is reached");
    }

    /// A route's endpoint whose response a secret reaches answers with it; one that only hashes
    /// it doesn't; and its manifest as deployed asks for each one it answers with.
    #[test]
    fn a_route_answering_with_a_secret_asks_for_it() {
        let mut r = MapResolver::new();
        let token = leaf(&mut r, "secret", vec![Param::frozen("name", "token")]);
        let key = leaf(&mut r, "secret", vec![Param::frozen("name", "key")]);
        let b64 = leaf(&mut r, "base64", vec![]);
        let hmac = leaf(&mut r, "hmac_sha256", vec![]);
        let pair = leaf(&mut r, "pair", vec![]);
        let endpoint = |edges: Vec<Edge>| Unit {
            body: UnitBody::Composite { children: vec![token, key, b64, hmac, pair], edges },
            params: vec![Param::frozen("declare", "endpoint"), Param::frozen("method", "GET"), Param::frozen("path", "/x")],
            inputs: vec![Port { name: "request".into(), ty: Type::Any }],
            outputs: vec![Port { name: "response".into(), ty: Type::Any }],
        };
        // base64(token) and hmac(key, request) answered together.
        let leaks = endpoint(vec![e(0, 0, 2, 0), e(1, 0, 3, 0), e(BOUNDARY, 0, 3, 1), e(2, 0, 4, 0), e(3, 0, 4, 1), e(4, 0, BOUNDARY, 0)]);
        assert_eq!(answered_secrets(&leaks, &r), BTreeSet::from(["token".to_string()]));
        let caps = deployed_capabilities(&leaks, &r);
        assert!(caps.contains(Capability::SecretAnswer("token".into())) && !caps.contains(Capability::SecretAnswer("key".into())));
        assert!(caps.contains(Capability::SecretRead("key".into())), "both are still read");
        // Only the hmac answered.
        let signs = endpoint(vec![e(1, 0, 3, 0), e(BOUNDARY, 0, 3, 1), e(3, 0, BOUNDARY, 0)]);
        assert!(answered_secrets(&signs, &r).is_empty());
        // Not a route: nothing is answered to a caller.
        assert!(answered_secrets(&Unit { params: vec![], ..leaks.clone() }, &r).is_empty());
    }

    /// `concat("Bearer ", token)` into a request's headers, `hmac(key, request)` into another's
    /// body, and a third request fed nothing secret: only the first host is where a secret goes;
    /// the second is where a signature made with the key goes, and is said as that.
    #[test]
    fn where_each_secret_is_sent_is_known() {
        let mut r = MapResolver::new();
        let token = leaf(&mut r, "secret", vec![Param::frozen("name", "token")]);
        let key = leaf(&mut r, "secret", vec![Param::frozen("name", "key")]);
        let bearer = leaf(&mut r, "const", vec![Param::frozen("value", "Bearer "), Param::frozen("type", "Text")]);
        let concat = leaf(&mut r, "concat", vec![]);
        let empty = leaf(&mut r, "record", vec![]);
        let headers = leaf(&mut r, "with_field", vec![Param::frozen("name", "authorization")]);
        let hmac = leaf(&mut r, "hmac_sha256", vec![]);
        let request = |host: &str| Param::frozen("host", host);
        let api = leaf(&mut r, "http_request", vec![Param::frozen("method", "GET"), request("api.example.com")]);
        let hook = leaf(&mut r, "http_request", vec![Param::frozen("method", "POST"), request("hooks.example.com")]);
        let other = leaf(&mut r, "http_request", vec![Param::frozen("method", "GET"), request("other.example.com")]);
        let root = Unit {
            body: UnitBody::Composite {
                children: vec![token, key, bearer, concat, empty, headers, hmac, api, hook, other],
                edges: vec![
                    e(2, 0, 3, 0),
                    e(0, 0, 3, 1),
                    e(4, 0, 5, 0),
                    e(3, 0, 5, 1),
                    e(5, 0, 7, 1),
                    e(1, 0, 6, 0),
                    e(BOUNDARY, 0, 6, 1),
                    e(6, 0, 8, 2),
                    e(BOUNDARY, 0, 9, 0),
                    e(7, 0, BOUNDARY, 0),
                ],
            },
            params: vec![],
            inputs: vec![Port { name: "message".into(), ty: Type::Text }],
            outputs: vec![Port { name: "answer".into(), ty: Type::Any }],
        };
        let sent = sent_secrets(&root, &r);
        let expected = BTreeMap::from([
            ("token".to_string(), BTreeSet::from(["api.example.com".to_string()])),
            ("key".to_string(), BTreeSet::from([format!("{MADE}hooks.example.com")])),
        ]);
        assert_eq!(sent, expected, "the key is only hmac'd: its signature goes to the hook, the key nowhere");
        // The sinks each secret reaches, by node: the token's request; nothing for the key.
        let sinks = |name: &str| SecretFed::of_secret(&root, &r, name).sinks().map(|(p, c)| (p.to_vec(), c.clone())).collect::<Vec<_>>();
        assert_eq!(sinks("token"), vec![(vec![7], Capability::HttpRequest("api.example.com".into()))]);
        assert!(sinks("key").is_empty());
        // A request's answer is what the far side said: answering with it sends nothing back out.
        assert!(answered_secrets(&root, &r).is_empty());
    }

    /// A token exchange: the secret is posted to the auth host, and what that answers (its
    /// access token) is put in a header to the API. The secret goes to the auth host; what is
    /// answered to it, to the API, listed as `answer:api.example.com`. A request fed neither
    /// sends nothing, and the walk from the answers leaves the secret itself clean.
    #[test]
    fn where_what_is_answered_to_a_secret_goes_is_known() {
        let mut r = MapResolver::new();
        let secret = leaf(&mut r, "secret", vec![Param::frozen("name", "client_secret")]);
        let empty = leaf(&mut r, "record", vec![]);
        let headers = leaf(&mut r, "with_field", vec![Param::frozen("name", "authorization")]);
        let request = |r: &mut MapResolver, method: &str, host: &str| leaf(r, "http_request", vec![Param::frozen("method", method), Param::frozen("host", host)]);
        let auth = request(&mut r, "POST", "auth.example.com");
        let api = request(&mut r, "GET", "api.example.com");
        let other = request(&mut r, "GET", "other.example.com");
        let root = Unit {
            body: UnitBody::Composite {
                // 0 secret → 3 auth's body; 3's answer → 2 headers → 4 api; 5 other fed the input.
                children: vec![secret, empty, headers, auth, api, other],
                edges: vec![e(0, 0, 3, 2), e(1, 0, 2, 0), e(3, 0, 2, 1), e(2, 0, 4, 1), e(BOUNDARY, 0, 5, 0), e(4, 0, BOUNDARY, 0)],
            },
            params: vec![],
            inputs: vec![Port { name: "message".into(), ty: Type::Text }],
            outputs: vec![Port { name: "answer".into(), ty: Type::Any }],
        };
        let to = BTreeSet::from(["auth.example.com".to_string(), format!("{ANSWER}api.example.com")]);
        assert_eq!(sent_secrets(&root, &r), BTreeMap::from([("client_secret".to_string(), to)]));
        let answers = SecretFed::answers_of(&root, &r, "client_secret");
        let sinks: Vec<_> = answers.sinks().map(|(p, c)| (p.to_vec(), c.clone())).collect();
        assert_eq!(sinks, vec![(vec![4], Capability::HttpRequest("api.example.com".into()))], "the auth request isn't where the answer goes");
        assert_eq!(answers.output(&[0], 0), Taint::Clean, "the secret itself isn't followed from the answers");
        // The secret alone: its own request, and not the API's.
        let own: Vec<_> = SecretFed::of_secret(&root, &r, "client_secret").sinks().map(|(p, _)| p.to_vec()).collect();
        assert_eq!(own, vec![vec![3]]);
    }

    /// A secret in a database read's parameters goes to that database: `db:shop`, and the read is
    /// a sink the canvas marks. A read fed nothing secret sends nothing.
    #[test]
    fn a_secret_in_a_database_read_is_sent_to_its_database() {
        let mut r = MapResolver::new();
        let key = leaf(&mut r, "secret", vec![Param::frozen("name", "key")]);
        let params = leaf(&mut r, "list", vec![]);
        let query = |database: &str, sql: &str| vec![Param::frozen("database", database), Param::frozen("sql", sql)];
        let shop = leaf(&mut r, "db_query", query("shop", "select * from orders where api_key = $1"));
        let audit = leaf(&mut r, "db_one", query("audit", "select * from log"));
        let root = Unit {
            body: UnitBody::Composite { children: vec![key, params, shop, audit], edges: vec![e(0, 0, 1, 0), e(1, 0, 2, 0), e(BOUNDARY, 0, 3, 0), e(2, 0, BOUNDARY, 0)] },
            params: vec![],
            inputs: vec![Port { name: "nothing".into(), ty: Type::list_any() }],
            outputs: vec![Port { name: "answer".into(), ty: Type::Any }],
        };
        assert_eq!(sent_secrets(&root, &r), BTreeMap::from([("key".to_string(), BTreeSet::from(["db:shop".to_string()]))]));
        let sinks: Vec<_> = SecretFed::of_secret(&root, &r, "key").sinks().map(|(p, c)| (p.to_vec(), c.clone())).collect();
        assert_eq!(sinks, vec![(vec![2], Capability::DbRead("shop/orders".into()))]);
        assert!(answered_secrets(&root, &r).is_empty(), "the rows are what the database said");
        assert!(!SecretFed::of(&root, &r).derives_output(0));
        assert_eq!(destination_of(&Capability::DbWrite("shop/orders".into())), None, "a write isn't where a secret goes: the host refuses it");
    }

    /// `unit` alone under a root, a secret feeding its input `port` and the root's inputs the
    /// rest, its outputs the root's: what each of its outputs carries of the secret.
    fn fed_on(unit: &Unit, port: usize) -> Vec<Taint> {
        let mut r = MapResolver::new();
        let secret = leaf(&mut r, "secret", vec![Param::frozen("name", "k")]);
        let node = r.insert(unit.clone()).unwrap();
        let mut edges: Vec<Edge> = (0..unit.inputs.len() as u32).map(|i| if i as usize == port { e(0, 0, 1, i) } else { e(BOUNDARY, i, 1, i) }).collect();
        edges.extend((0..unit.outputs.len() as u32).map(|o| e(1, o, BOUNDARY, o)));
        let root = Unit { body: UnitBody::Composite { children: vec![secret, node], edges }, params: vec![], inputs: unit.inputs.clone(), outputs: unit.outputs.clone() };
        let fed = SecretFed::of(&root, &r);
        (0..unit.outputs.len()).map(|o| fed.output(&[1], o)).collect()
    }

    /// Guards: whatever a leaf is fed of a secret, on any of its inputs, comes out of it as
    /// something of the secret (literal or derived), but at the leaves named here, whose answer
    /// gives nothing of it back: a one-way hash or comparison, the key and clock `jwt_verify`
    /// checks a token with, what a call's far side answers.
    /// Why: a Boolean was clean whatever computed it, so `k.startsWith(req.query.p)` answered
    /// anyone true or false with no approval asked, and the secret could be read off a character
    /// at a time; the approval said only that the flow read it.
    /// If it fails: a leaf hands on something of a secret as clean: answered to callers with no
    /// approval, logged in the clear, written where a secret's value may not be. Follow it there,
    /// or name it here with why nothing of the secret comes out.
    #[test]
    fn no_output_fed_a_secret_is_clean_but_where_nothing_of_it_comes_out() {
        let mut clean: BTreeSet<String> = BTreeSet::new();
        for &name in crate::GENESIS {
            let Some(unit) = interface_of(name) else { continue };
            for port in 0..unit.inputs.len() {
                if fed_on(&unit, port).contains(&Taint::Clean) {
                    clean.insert(format!("{name}.{}", unit.inputs[port].name));
                }
            }
        }
        let named: BTreeSet<String> = [
            // One way: a hash, a keyed hash, a comparison in constant time.
            "sha256.text", "hmac_sha256.key", "hmac_sha256.message", "secure_equals.a", "secure_equals.b",
            // The token's payload comes out, nothing of the key it is checked with, nor the clock.
            "jwt_verify.key", "jwt_verify.now",
        ]
        .into_iter()
        .map(String::from)
        // What the far side answered (the host finds a credential it answers with by its value).
        .chain(ANSWERED.iter().flat_map(|&n| interface_of(n).unwrap().inputs.into_iter().map(move |p| format!("{n}.{}", p.name))))
        .collect();
        assert_eq!(clean, named);

        // A `default` typed Boolean (`x ?? false`, a condition's own) passes what it is given:
        // a Boolean, which is derived.
        let mut default = interface_of("default").unwrap();
        default.params = vec![Param::frozen("value", "false"), Param::frozen("type", "Boolean")];
        default.inputs[0].ty = Type::Boolean;
        default.outputs[0].ty = Type::Boolean;
        assert_eq!(fed_on(&default, 0), vec![Taint::Derived]);
    }

    /// As [`fed_on`], the input fed a part of the secret (`k.trim()`: something computed from it)
    /// in place of the secret whole.
    fn fed_a_part_on(unit: &Unit, port: usize) -> Vec<Taint> {
        fed_through("trim", unit, port)
    }

    /// As [`fed_on`], the input fed what the leaf `through` makes of the secret.
    fn fed_through(through: &str, unit: &Unit, port: usize) -> Vec<Taint> {
        let mut r = MapResolver::new();
        let secret = leaf(&mut r, "secret", vec![Param::frozen("name", "k")]);
        let part = r.insert(interface_of(through).unwrap()).unwrap();
        let node = r.insert(unit.clone()).unwrap();
        let mut edges: Vec<Edge> = vec![e(0, 0, 1, 0)];
        edges.extend((0..unit.inputs.len() as u32).map(|i| if i as usize == port { e(1, 0, 2, i) } else { e(BOUNDARY, i, 2, i) }));
        edges.extend((0..unit.outputs.len() as u32).map(|o| e(2, o, BOUNDARY, o)));
        let root = Unit { body: UnitBody::Composite { children: vec![secret, part, node], edges }, params: vec![], inputs: unit.inputs.clone(), outputs: unit.outputs.clone() };
        let fed = SecretFed::of(&root, &r);
        (0..unit.outputs.len()).map(|o| fed.output(&[2], o)).collect()
    }

    /// Guards: whatever a leaf is fed of something computed from a secret (a slice of it, a
    /// character), on any of its inputs, comes out of it derived, never clean: the one-way leaves
    /// and `jwt_verify`'s key and clock too. Only what a call's far side answers is clean.
    /// Why: a one-way leaf's answer was clean whatever it was fed, so
    /// `secure_equals(k.slice(0, n), req.query.p)` answered anyone whether `p` is how the secret
    /// begins, and `sha256(k.slice(i, i + 1))` a hash that names its character by a table of 256,
    /// with no approval asked: the secret, read off a character at a time, as a Boolean once let
    /// it be. A hash of a secret whole gives nothing of it back; of a part, it gives the part.
    /// If it fails: a leaf hands on what is computed from a secret as clean. Make it derived
    /// where the walk gives its outputs, unless nothing of what it is fed can come out of it, and
    /// then name it here with why.
    #[test]
    fn no_output_fed_a_part_of_a_secret_is_clean_but_what_the_far_side_answers() {
        let mut clean: BTreeSet<String> = BTreeSet::new();
        for &name in crate::GENESIS {
            let Some(unit) = interface_of(name) else { continue };
            for port in 0..unit.inputs.len() {
                if fed_a_part_on(&unit, port).contains(&Taint::Clean) {
                    clean.insert(format!("{name}.{}", unit.inputs[port].name));
                }
            }
        }
        // What the far side answered (the host finds a credential it answers with by its value).
        let named: BTreeSet<String> = ANSWERED.iter().flat_map(|&n| interface_of(n).unwrap().inputs.into_iter().map(move |p| format!("{n}.{}", p.name))).collect();
        assert_eq!(clean, named);

        // The flows this closes, as a person writes them: each answer is the secret's to approve.
        for leaf_name in ONE_WAY {
            let unit = interface_of(leaf_name).unwrap();
            for port in 0..unit.inputs.len() {
                assert_eq!(fed_a_part_on(&unit, port), vec![Taint::Derived], "{leaf_name}, a part of the secret at {}", unit.inputs[port].name);
                assert_eq!(fed_on(&unit, port), vec![Taint::Clean], "{leaf_name}, the secret whole at {}", unit.inputs[port].name);
            }
        }
        // The secret whole in another spelling is whole to a one-way leaf: a key kept as base64
        // signs as the key does, and its signature is a run's to keep and to check. To anything
        // else it is computed from the secret: no leaf gives it on as clean, or as the secret's
        // own text for redaction to find.
        for &spelled in RECODES {
            let recoder = interface_of(spelled).unwrap();
            assert_eq!(fed_on(&recoder, 0), vec![Taint::Recoded], "{spelled}, of the secret whole");
            assert_eq!(fed_a_part_on(&recoder, 0), vec![Taint::Derived], "{spelled}, of a part of it");
            for one_way in ONE_WAY {
                let unit = interface_of(one_way).unwrap();
                for port in 0..unit.inputs.len() {
                    assert_eq!(fed_through(spelled, &unit, port), vec![Taint::Clean], "{one_way}, the secret {spelled}-spelled at {}", unit.inputs[port].name);
                }
            }
        }
        for &name in crate::GENESIS {
            let Some(unit) = interface_of(name) else { continue };
            for port in 0..unit.inputs.len() {
                let out = fed_through("base64", &unit, port);
                let cut = ONE_WAY.contains(&name) || name == "jwt_verify" || ANSWERED.contains(&name);
                assert!(cut || !out.iter().any(|t| *t <= Taint::Literal), "{name}.{}: of the secret's base64 it gave {out:?}", unit.inputs[port].name);
            }
        }
        assert!(Taint::Recoded.is_derived() && Taint::Derived.is_derived() && !Taint::Literal.is_derived() && !Taint::Clean.is_derived());
        // Where a secret lands in a sorted list, or whether `unique` leaves it out, says how it
        // compares with the others.
        for placed in ["sort", "unique"] {
            assert_eq!(fed_on(&interface_of(placed).unwrap(), 0), vec![Taint::Derived], "{placed}");
        }
    }

    /// Guards: a leaf that passes its inputs on as they are ([`PASSES_LITERALLY`]) says of each
    /// input either that its output may hold it (literal: redaction finds a secret there) or, for
    /// one that only chooses what comes out ([`selects`]: a condition, a position, a key to look
    /// up or leave out, what starts a sink), that the output is derived. Running each leaf over
    /// its inputs shows which of them ever come out in what it gives.
    /// Why: an input that only chooses was taken as passed on as it is, so a secret used to choose
    /// (`rec[k]`, `list[i]`, `c ? a : b` with `c` a secret's text) left what it chose in the log in
    /// the clear, and that says something of the secret.
    /// If it fails: [`selects`] names an input a leaf holds in its output, or misses one it only
    /// chooses with. Correct it there.
    #[test]
    fn what_a_passing_leaf_holds_of_an_input_is_literal_and_what_only_chooses_is_derived() {
        use crate::Value;
        const MARK: &str = "MARK";
        const AT: f64 = 4242.25;
        fn holds(v: &Value) -> bool {
            match v {
                Value::Text(s) => s.contains(MARK),
                Value::Number(n) => *n == AT,
                Value::List(xs) => xs.iter().any(holds),
                Value::Record(r) => r.iter().any(|(k, x)| k.contains(MARK) || holds(x)),
                Value::Effect { payload, .. } | Value::Sum { payload, .. } => holds(payload),
                Value::Boolean(_) | Value::Bytes(_) => false,
            }
        }
        let t = |s: &str| Value::Text(s.into());
        let rec = |fs: &[(&str, Value)]| Value::Record(fs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect());
        // What an input of each type is given beside the one marked, and what marks it.
        let given = |ty: &Type| match ty {
            Type::Number => vec![Value::Number(0.0), Value::Number(1.0), Value::Number(2.0)],
            Type::Text => vec![t(""), t("id"), t("a")],
            Type::Boolean => vec![Value::Boolean(true), Value::Boolean(false)],
            Type::Record => vec![rec(&[]), rec(&[("id", Value::Number(1.0)), ("a", t("x"))])],
            Type::List(_) => vec![Value::List(vec![]), Value::List(vec![t("a"), t("b")]), Value::List(vec![Value::Number(1.0), Value::Number(2.0)])],
            _ => vec![t("id"), Value::Number(1.0), rec(&[("id", Value::Number(1.0))]), Value::List(vec![t("a")])],
        };
        let marked = |ty: &Type| {
            let record = rec(&[("id", t(MARK)), (MARK, t(MARK)), ("a", t(MARK))]);
            let list = Value::List(vec![t(MARK), t(MARK)]);
            let entries = Value::List(vec![Value::List(vec![t(MARK), t(MARK)])]);
            match ty {
                Type::Number => vec![Value::Number(AT)],
                Type::Text => vec![t(MARK)],
                Type::Record => vec![record],
                Type::List(_) => vec![list, entries],
                _ => vec![t(MARK), record, list, entries, Value::Number(AT)],
            }
        };
        let mut wrong = Vec::new();
        let mut not_computed = BTreeSet::new();
        for &name in PASSES_LITERALLY {
            let unit = interface_of(name).unwrap();
            let mut computed = false;
            for (i, port) in unit.inputs.iter().enumerate() {
                // A Boolean or a trigger is never part of what a leaf gives: it chooses.
                if port.ty == Type::Boolean || port.name == crate::TRIGGER {
                    assert!(selects(name, port), "{name}.{}", port.name);
                    continue;
                }
                // Every combination of the other inputs, the marked one at `i`.
                let mut sets: Vec<Vec<Value>> = vec![vec![]];
                for (j, p) in unit.inputs.iter().enumerate() {
                    let choices = if j == i { marked(&p.ty) } else { given(&p.ty) };
                    sets = sets.into_iter().flat_map(|s| choices.iter().map(move |c| [s.clone(), vec![c.clone()]].concat())).collect();
                }
                // What it gives, of the sets it gives anything for (a position or key not there
                // gives nothing: nothing of it is held).
                let outs: Vec<Value> = sets.iter().filter_map(|set| crate::primitives::compute(name, &set.iter().collect::<Vec<_>>(), &unit.params)).collect();
                computed |= !outs.is_empty();
                if outs.iter().any(holds) == selects(name, port) {
                    let says = if selects(name, port) { "held in what it gives, but taken as only choosing" } else { "never in what it gives, but taken as passed on" };
                    wrong.push(format!("{name}.{}: {says}", port.name));
                }
            }
            if !computed && !unit.inputs.is_empty() {
                not_computed.insert(name);
                wrong.retain(|w| !w.starts_with(&format!("{name}.")));
            }
        }
        // `coalesce` gives `b` when `a` is absent, which `compute` is never handed (eval decides
        // what is absent): it passes `b` on as it is.
        assert_eq!(wrong, vec!["coalesce.b: never in what it gives, but taken as passed on"]);
        // The sinks: eval builds their effect from what they are given, every input in it but
        // the trigger, which only starts it (`selects` says so by the port's name).
        assert_eq!(not_computed, BTreeSet::from(["collection_put", "file_create", "file_write", "mail_send", "queue_send"]));
    }
    /// As [`fed_on`], for what is clean and followed all the same: whether each output of `unit`,
    /// its input `port` fed the secret, is where a hash or a signature of it is made
    /// ([`SecretFed::made_of`]) or where what is answered to it comes back
    /// ([`SecretFed::answers_of`]).
    fn followed_on(unit: &Unit, port: usize) -> Vec<bool> {
        let mut r = MapResolver::new();
        let secret = leaf(&mut r, "secret", vec![Param::frozen("name", "k")]);
        let node = r.insert(unit.clone()).unwrap();
        let mut edges: Vec<Edge> = (0..unit.inputs.len() as u32).map(|i| if i as usize == port { e(0, 0, 1, i) } else { e(BOUNDARY, i, 1, i) }).collect();
        edges.extend((0..unit.outputs.len() as u32).map(|o| e(1, o, BOUNDARY, o)));
        let root = Unit { body: UnitBody::Composite { children: vec![secret, node], edges }, params: vec![], inputs: unit.inputs.clone(), outputs: unit.outputs.clone() };
        let (made, answers) = (SecretFed::made_of(&root, &r, "k"), SecretFed::answers_of(&root, &r, "k"));
        (0..unit.outputs.len()).map(|o| made.output(&[1], o) > Taint::Clean || answers.output(&[1], o) > Taint::Clean).collect()
    }

    /// A root in which the secret `k` feeds input 0 of the leaf `through` (a hash, a request),
    /// and what that gives feeds input `port` of `unit`, whose other inputs are the root's and
    /// whose outputs are the root's: the store, and the root. `unit` is its child 2.
    fn through_to(through: &str, unit: &Unit, port: usize, params: Vec<Param>) -> (MapResolver, Unit) {
        let mut r = MapResolver::new();
        let secret = leaf(&mut r, "secret", vec![Param::frozen("name", "k")]);
        let maker = r.insert(interface_of(through).unwrap()).unwrap();
        let node = r.insert(unit.clone()).unwrap();
        let mut edges: Vec<Edge> = vec![e(0, 0, 1, 0)];
        edges.extend((0..unit.inputs.len() as u32).map(|i| if i as usize == port { e(1, 0, 2, i) } else { e(BOUNDARY, i, 2, i) }));
        edges.extend((0..unit.outputs.len() as u32).map(|o| e(2, o, BOUNDARY, o)));
        let root = Unit { body: UnitBody::Composite { children: vec![secret, maker, node], edges }, params, inputs: unit.inputs.clone(), outputs: unit.outputs.clone() };
        (r, root)
    }

    /// Guards: nothing a leaf makes of a secret whole is clean and then lost sight of. Each leaf
    /// that gives something clean when fed the secret is either followed from there, to every
    /// way out of the flow (a hash or a signature: [`MAKES`]; what a far side answered:
    /// [`ANSWERED`]), or named here with why nothing made of the secret comes out of it. And
    /// what is followed is seen at every way out: each leaf that does something outside the run
    /// (a request, a write, a send, a call), fed a signature or an answer at any input, is a sink
    /// of that walk, its host or database a destination, and a route that answers with one sends
    /// it to its callers.
    /// Why: `hmac_sha256(k, req.query.msg)` answered to any caller, and the access token a host
    /// answered the secret with, were clean and followed nowhere: a route handed out signatures
    /// over whatever its caller chose, or a live token, and the approval said only "read the
    /// secret "k"". What was answered was followed to other hosts, but not to the route's own
    /// callers; what was made was followed nowhere.
    /// If it fails with a leaf in neither list: a new leaf gives something clean of a secret.
    /// Follow it (add it to [`MAKES`] or [`ANSWERED`]), or name it in `ends_here` with why what
    /// it gives is nothing made with the secret. If a sink is missing: the walk no longer records
    /// what a followed value reaches there.
    #[test]
    fn what_is_made_with_a_secret_and_clean_is_followed_to_every_way_out() {
        // 1. Every leaf, every input, the secret whole: clean out means followed, or named.
        let mut lost: BTreeSet<String> = BTreeSet::new();
        for &name in crate::GENESIS {
            let Some(unit) = interface_of(name) else { continue };
            for port in 0..unit.inputs.len() {
                let (taints, followed) = (fed_on(&unit, port), followed_on(&unit, port));
                if taints.iter().zip(&followed).any(|(t, f)| *t == Taint::Clean && !f) {
                    lost.insert(format!("{name}.{}", unit.inputs[port].name));
                }
            }
        }
        let ends_here: BTreeSet<String> = [
            // Whether two values are the same, in constant time: what every guarded route says by
            // its status. Nothing is made.
            "secure_equals.a", "secure_equals.b",
            // The token's payload comes out, nothing of the key it is checked with, nor the clock.
            "jwt_verify.key", "jwt_verify.now",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(lost, ends_here);
        // The lists say the same: a one-way leaf makes something, or is named above.
        for one_way in ONE_WAY {
            assert!(MAKES.contains(one_way) || ends_here.iter().any(|n| n.starts_with(&format!("{one_way}."))), "{one_way}");
        }
        // The secret in another spelling (a key kept as base64) is the secret to a hash: followed.
        for &spelled in RECODES {
            for &makes in MAKES {
                let (r, root) = through_to(spelled, &interface_of(makes).unwrap(), 0, vec![]);
                assert!(SecretFed::made_of(&root, &r, "k").output(&[2], 0) > Taint::Clean, "{makes} of the secret {spelled}-spelled");
            }
        }

        // 2. Every way out, fed what is followed at any input: a sink of the walk, and a
        // destination where it has one.
        let ways_out = |unit: &Unit, name: &str| -> BTreeSet<Capability> {
            capabilities_of(name, &unit.params).map(|caps| caps.iter().filter(is_sink).collect()).unwrap_or_default()
        };
        let mut outs = 0;
        for (through, follow) in [("hmac_sha256", Follow::Made), ("sha256", Follow::Made), ("http_request", Follow::Answers), ("db_query", Follow::Answers)] {
            for &name in crate::GENESIS {
                let Some(unit) = interface_of(name) else { continue };
                let expected = ways_out(&unit, name);
                if expected.is_empty() {
                    continue;
                }
                outs += 1;
                for port in 0..unit.inputs.len() {
                    let (r, root) = through_to(through, &unit, port, vec![]);
                    let walk = match follow {
                        Follow::Made => SecretFed::made_of(&root, &r, "k"),
                        Follow::Answers => SecretFed::answers_of(&root, &r, "k"),
                    };
                    let sinks: BTreeSet<Capability> = walk.sinks().filter(|(path, _)| *path == [2]).map(|(_, c)| c.clone()).collect();
                    assert_eq!(sinks, expected, "{name}.{}, fed what {through} gave", unit.inputs[port].name);
                    for to in expected.iter().filter_map(destination_of) {
                        let mark = if follow == Follow::Made { MADE } else { ANSWER };
                        let sent = sent_secrets(&root, &r);
                        assert!(sent.get("k").is_some_and(|d| d.contains(&format!("{mark}{to}"))), "{name}.{}, fed what {through} gave: {sent:?}", unit.inputs[port].name);
                    }
                }
            }
        }
        assert!(outs >= 4 * 20, "the ways out are found by what each leaf may do: {outs}");

        // 3. A route's own callers: a signature or an answer in its response goes to them; the
        // same graph that isn't a route answers no one.
        let endpoint = || vec![Param::frozen("declare", "endpoint"), Param::frozen("method", "GET"), Param::frozen("path", "/x")];
        let passes = interface_of("some").unwrap();
        for (through, mark) in [("hmac_sha256", MADE), ("sha256", MADE), ("http_request", ANSWER), ("db_query", ANSWER)] {
            let (r, route) = through_to(through, &passes, 0, endpoint());
            let callers = format!("{mark}{CALLERS}");
            assert!(sent_secrets(&route, &r).get("k").is_some_and(|d| d.contains(&callers)), "{through}, answered by a route: {:?}", sent_secrets(&route, &r));
            assert!(answered_secrets(&route, &r).is_empty(), "{through}: it isn't the secret, and asks no approval of its own");
            let (r, plain) = through_to(through, &passes, 0, vec![]);
            assert!(!sent_secrets(&plain, &r).get("k").is_some_and(|d| d.contains(&callers)), "{through}, not a route");
        }
        // A key derived in steps (AWS's: an hmac of an hmac) is still made with the secret.
        for &makes in MAKES {
            let (r, chained) = through_to("hmac_sha256", &interface_of(makes).unwrap(), 0, endpoint());
            assert!(sent_secrets(&chained, &r).get("k").is_some_and(|d| d.contains(&format!("{MADE}{CALLERS}"))), "{makes} of a signature: {:?}", sent_secrets(&chained, &r));
        }
        // A comparison ends it: a route that says only whether a signature matches hands out
        // none, however it compares (in constant time, with `===`, by any leaf that answers a
        // Boolean). Named, since it is a choice: a flow that compared a signature a part at a
        // time with what its caller sent would give it away slowly, and nothing here says so.
        let mut compared = 0;
        for &name in crate::GENESIS {
            let Some(unit) = interface_of(name) else { continue };
            if unit.outputs.len() != 1 || unit.outputs[0].ty != Type::Boolean || !child_refs(&unit).is_empty() {
                continue;
            }
            compared += 1;
            for port in 0..unit.inputs.len() {
                let (r, compares) = through_to("hmac_sha256", &unit, port, endpoint());
                assert!(!sent_secrets(&compares, &r).contains_key("k"), "{name}.{}: {:?}", unit.inputs[port].name, sent_secrets(&compares, &r));
            }
        }
        assert!(compared >= 10 && interface_of("secure_equals").is_some_and(|u| u.outputs[0].ty == Type::Boolean), "{compared}");
        // A response's status, read straight off the request, is no token: a route answering
        // with it sends its callers nothing answered; with its body, it does.
        let status_or = |field: &str| {
            let mut r = MapResolver::new();
            let secret = leaf(&mut r, "secret", vec![Param::frozen("name", "k")]);
            let asks = r.insert(interface_of("http_request").unwrap()).unwrap();
            let reads = leaf(&mut r, "field", vec![Param::frozen("name", field)]);
            let route = Unit {
                body: UnitBody::Composite { children: vec![secret, asks, reads], edges: vec![e(0, 0, 1, 1), e(1, 0, 2, 0), e(2, 0, BOUNDARY, 0)] },
                params: endpoint(),
                inputs: vec![Port { name: "request".into(), ty: Type::Any }],
                outputs: vec![Port { name: "response".into(), ty: Type::Any }],
            };
            sent_secrets(&route, &r).remove("k").unwrap_or_default()
        };
        assert_eq!(status_or("status"), BTreeSet::from(["example.com".to_string()]));
        assert_eq!(status_or("body"), BTreeSet::from(["example.com".to_string(), format!("{ANSWER}{CALLERS}")]));
        // A signed request's answer is the far side's: answering with it hands out no signature.
        let (r, signed) = through_to("hmac_sha256", &interface_of("http_request").unwrap(), 1, endpoint());
        assert_eq!(sent_secrets(&signed, &r).get("k"), Some(&BTreeSet::from([format!("{MADE}example.com")])));
        // What a run is held to is where the secret itself goes, and no more: the same as the
        // approval's own destinations, whatever else the approval says.
        for (through, port) in [("hmac_sha256", 1), ("http_request", 1), ("base64", 1)] {
            let (r, flow) = through_to(through, &interface_of("http_request").unwrap(), port, endpoint());
            let own: BTreeMap<String, BTreeSet<String>> =
                sent_secrets(&flow, &r).into_iter().map(|(k, to)| (k, to.into_iter().filter(|d| is_own_destination(d)).collect::<BTreeSet<_>>())).filter(|(_, to)| !to.is_empty()).collect();
            assert_eq!(sent_itself(&flow, &r), own, "{through}");
        }
        // Where the secret itself goes is told apart from where what it makes goes.
        assert!(is_own_destination("api.example.com") && is_own_destination("db:shop"));
        assert!(!is_own_destination(&format!("{MADE}api.example.com")) && !is_own_destination(&format!("{ANSWER}{CALLERS}")));
    }
}
