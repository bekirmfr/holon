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
//! What cuts the chain: a one-way leaf (`sha256`, `hmac_sha256`, `secure_equals`), a Boolean
//! answer, and an outbound request's response or a database read's rows (what the far side said,
//! not what was sent). A
//! leaf that isn't known to pass its inputs through as they are makes anything tainted derived.
//!
//! The same walk, one secret at a time, finds what a route may answer its caller with
//! ([`answered_secrets`]): each such secret is a permission of its own, `SecretAnswer("name")`,
//! that a person approves at deploy. It is core's, so the host grants by the rule the editor and
//! `holon check` show. It also finds where each secret is sent ([`sent_secrets`]): the hosts of
//! the requests its value, or something computed from it, may reach. That isn't a permission of
//! its own (the hosts and the secret are each approved, and the graph fixes which goes where
//! before the flow runs); the approval says it beside the secret, so a person sees it.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::{capabilities, capabilities_of, child_refs, declaration_of, kind::ident, read_server, Capabilities, Capability, Hash, Resolver, Type, Unit, UnitBody, BOUNDARY};

/// How much of a secret a value may carry.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub enum Taint {
    #[default]
    Clean,
    Literal,
    Derived,
}

/// Leaves whose output is computed one way from their inputs: nothing of a secret is recovered
/// from it.
const ONE_WAY: &[&str] = &["sha256", "hmac_sha256", "secure_equals"];

/// Leaves whose output holds their inputs as they are, or a part of them unchanged: a secret in
/// an input is in the output literally, where redaction finds it. (`json_stringify` escapes, but
/// redaction matches a secret's JSON-escaped form too.) The list leaves that rearrange a list's
/// items are here; those that take a text too (`at`, `slice`) are not, since a part of a text
/// isn't the secret. So are the record leaves that move values between records and lists
/// (`values`, `from_entries`, `merge`); `has` and `same_value` answer a Boolean, which cuts it.
const PASSES_LITERALLY: &[&str] = &[
    "const", "select", "cons", "head", "tail", "concat", "list", "get", "append", "extend", "join", "pair", "some",
    "entries", "reverse", "to_spliced", "with_item", "fill", "copy_within", "sort", "flat", "unique",
    "values", "from_entries", "merge",
    "record", "field", "with_field", "without_field", "lookup", "insert", "remove", "as_text", "guard",
    "default", "coalesce", "json_stringify", "store_write", "collection_put", "file_write", "file_create",
    "channel_send", "egress",
];

/// Leaves whose output is what the far side answered: an outbound request's response, a
/// database read's rows. (One that echoes a secret back holds its value, where the host finds it.)
const ANSWERED: &[&str] = &["http_request", "http_upload", "http_stream", "flow_call", "flow_start", "db_query", "db_one", "db_find", "db_find_one"];

/// Deeper than this, a composite is taken as computing with whatever reaches it.
const MAX_DEPTH: usize = 128;

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
}

impl SecretFed {
    /// Follow the secrets through `root`, whose inputs are clean (a request, nothing).
    pub fn of(root: &Unit, resolver: &impl Resolver) -> SecretFed {
        SecretFed::walk(root, resolver, None)
    }

    /// Follow only the secret named `name`: what its value reaches, the others taken as clean.
    pub fn of_secret(root: &Unit, resolver: &impl Resolver, name: &str) -> SecretFed {
        SecretFed::walk(root, resolver, Some(name))
    }

    /// Follow what is answered to the secret `name`: the answers of the requests and reads its
    /// value, or something computed from it, is sent in (the token a token exchange answers),
    /// from there on; the secret itself is taken as clean, and an answer to what was answered is
    /// followed too. A run-time token is a field of such an answer (`access_token`): the graph
    /// can't tell the field, so it follows the whole answer.
    pub fn answers_of(root: &Unit, resolver: &impl Resolver, name: &str) -> SecretFed {
        let asked = SecretFed::of_secret(root, resolver, name).asked;
        if asked.is_empty() {
            return SecretFed::default();
        }
        SecretFed::walk_from(root, resolver, Some(name), Some(&asked))
    }

    fn walk(root: &Unit, resolver: &impl Resolver, only: Option<&str>) -> SecretFed {
        SecretFed::walk_from(root, resolver, only, None)
    }

    fn walk_from(root: &Unit, resolver: &impl Resolver, only: Option<&str>, answers: Option<&BTreeSet<Vec<u32>>>) -> SecretFed {
        let mut fed = SecretFed::default();
        let mut walk = Walk { resolver, only, answers, holds_secret: HashMap::new() };
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
        self.opaque.iter().any(Vec::is_empty) || self.nodes.get(&[][..]).is_some_and(|(_, outs)| outs.get(port) == Some(&Taint::Derived))
    }

    /// Whether the run's output `port` may carry a secret at all: as it is, or computed from it.
    pub fn reaches_output(&self, port: usize) -> bool {
        self.opaque.iter().any(Vec::is_empty) || self.nodes.get(&[][..]).is_some_and(|(_, outs)| outs.get(port).is_some_and(|t| *t > Taint::Clean))
    }

    fn under_opaque(&self, path: &[u32]) -> bool {
        self.opaque.iter().any(|o| path.starts_with(o) && path.len() > o.len())
    }

    /// Whether the node at `path` may produce something computed from a secret: the log keeps
    /// none of its outputs.
    pub fn derives(&self, path: &[u32]) -> bool {
        self.under_opaque(path) || self.opaque.iter().any(|o| o == path) || self.nodes.get(path).is_some_and(|(_, outs)| outs.contains(&Taint::Derived))
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
    /// Following what is answered to it instead ([`SecretFed::answers_of`]): the nodes whose
    /// answer is where it starts. The secret leaves are clean then.
    answers: Option<&'r BTreeSet<Vec<u32>>>,
    /// Whether a unit (by hash) reads a followed secret anywhere in its closure.
    holds_secret: HashMap<Hash, bool>,
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
        let most = inputs.iter().copied().max().unwrap_or_default();
        let reads = self.reads(unit) && self.answers.is_none();
        let asked = self.answers.is_some_and(|asked| asked.contains(path.as_slice()));
        // What a node not followed answers is where following what is answered starts: all of it.
        if asked && !matches!(&unit.body, UnitBody::Primitive { name } if ANSWERED.contains(&name.as_str())) {
            let outputs = vec![Taint::Derived; unit.outputs.len()];
            fed.nodes.insert(path.clone(), (inputs.to_vec(), outputs.clone()));
            return outputs;
        }
        // Following what is answered, a request's answer carries it: where it starts, and an
        // answer to a request fed it (an API echoing it back, a token exchanged for another).
        let answering = asked || self.answers.is_some() && most > Taint::Clean;
        let outputs = match &unit.body {
            UnitBody::Primitive { name } => {
                let outputs: Vec<Taint> = unit
                    .outputs
                    .iter()
                    .map(|port| match name.as_str() {
                        "secret" if reads => Taint::Literal,
                        _ if port.ty == Type::Boolean => Taint::Clean,
                        n if ANSWERED.contains(&n) && answering => Taint::Literal,
                        n if ONE_WAY.contains(&n) || ANSWERED.contains(&n) => Taint::Clean,
                        n if PASSES_LITERALLY.contains(&n) => most,
                        _ if most == Taint::Clean => Taint::Clean,
                        _ => Taint::Derived,
                    })
                    .collect();
                // A function applied inside (`map`, `fold`) isn't followed: one that reads a
                // secret, or is handed one, may compute anything with it.
                let applies_secret = self.answers.is_none() && child_refs(unit).iter().any(|r| self.holds(r));
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
                    }
                    // A sink fed something computed from a secret: its effect carries it.
                    if outputs.contains(&Taint::Derived)
                        && let Some(caps) = capabilities_of(name, &unit.params)
                    {
                        fed.effects.extend(caps.iter().filter(|c| !c.is_sourced()));
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
        if most > Taint::Clean || outputs.iter().any(|t| *t > Taint::Clean) {
            fed.nodes.insert(path.clone(), (inputs.to_vec(), outputs.clone()));
        }
        outputs
    }

    /// A node whose insides aren't followed: everything under it is derived, and so is every
    /// effect it could make.
    fn opaque(&mut self, unit: &Unit, path: &[u32], fed: &mut SecretFed) {
        fed.opaque.push(path.to_vec());
        let caps = capabilities(unit, self.resolver);
        if caps.iter().any(|c| destination_of(&c).is_some()) {
            fed.asked.insert(path.to_vec());
        }
        fed.sent.extend(caps.iter().filter_map(|c| destination_of(&c)));
        fed.effects.extend(caps.iter().filter(|c| !c.is_sourced()));
        fed.sinks.extend(caps.iter().filter(is_sink).map(|c| (path.to_vec(), c)));
    }

    fn composite(&mut self, unit: &Unit, children: &[Hash], edges: &[crate::Edge], inputs: &[Taint], path: &mut Vec<u32>, fed: &mut SecretFed) -> Vec<Taint> {
        let derived = vec![Taint::Derived; unit.outputs.len()];
        // Nothing tainted comes in and nothing inside reads a secret, or answers what one reached:
        // nothing to follow.
        let answers_inside = self.answers.is_some_and(|asked| asked.iter().any(|a| a.len() > path.len() && a.starts_with(path)));
        if inputs.iter().all(|t| *t == Taint::Clean) && !answers_inside && !children.iter().any(|c| self.holds(c)) {
            return vec![Taint::Clean; unit.outputs.len()];
        }
        if path.len() >= MAX_DEPTH {
            self.opaque(unit, path, fed);
            return derived;
        }
        let Some(order) = topo_order(children.len(), edges) else {
            self.opaque(unit, path, fed);
            return derived;
        };
        let mut outs: Vec<Option<Vec<Taint>>> = vec![None; children.len()];
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
            path.push(ci as u32);
            outs[ci] = Some(self.unit(child, &child_inputs, path, fed));
            path.pop();
        }
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
/// value, or something computed from it, may reach its response (output 0). An action's answer
/// isn't one: it goes back through Holon, withheld and redacted. Given a route's endpoint alone,
/// its own. Each is a permission, `SecretAnswer("name")`, which a person approves at deploy.
pub fn answered_secrets(root: &Unit, resolver: &impl Resolver) -> BTreeSet<String> {
    let endpoints: Vec<&Unit> = match read_server(root, resolver) {
        Ok(server) => server.routes.iter().filter_map(|r| resolver.resolve(&r.endpoint)).collect(),
        Err(_) if declaration_of(root) == Some("endpoint") => vec![root],
        Err(_) => vec![],
    };
    let mut answered = BTreeSet::new();
    for endpoint in endpoints {
        for cap in capabilities(endpoint, resolver).iter() {
            if let Capability::SecretRead(name) = cap
                && !answered.contains(&name)
                && SecretFed::of_secret(endpoint, resolver, &name).reaches_output(0)
            {
                answered.insert(name);
            }
        }
    }
    answered
}

/// Where each secret `root` reads is sent: the hosts of the outbound requests (`http.get`, an
/// upload, a stream…) its value, or something computed from it, may reach, in their path, headers
/// or body, and the databases of the reads whose parameters it may reach (`db:shop`,
/// [`destination_of`]). A one-way leaf, a Boolean and a call's answer cut it, as for [`answered_secrets`]; a
/// secret sent nowhere isn't listed. Every pair is known before the flow runs (a request's host is
/// a frozen param), so the approval says it beside the secret: "read the secret "k", and send it
/// to https://api.example.com".
///
/// Where what is answered to it goes ([`SecretFed::answers_of`]: the token a token exchange
/// answers, an API's echo) is listed with it, each such destination as `answer:` and the
/// destination (`answer:api.example.com`): "…; what is answered to it goes to
/// https://api.example.com".
pub fn sent_secrets(root: &Unit, resolver: &impl Resolver) -> BTreeMap<String, BTreeSet<String>> {
    let mut sent = BTreeMap::new();
    for cap in capabilities(root, resolver).iter() {
        if let Capability::SecretRead(name) = cap {
            let mut to = SecretFed::of_secret(root, resolver, &name).sent;
            to.extend(SecretFed::answers_of(root, resolver, &name).sent.into_iter().map(|d| format!("{ANSWER}{d}")));
            if !to.is_empty() {
                sent.insert(name, to);
            }
        }
    }
    sent
}

/// What marks a destination in [`sent_secrets`] as where what is answered to a secret goes.
pub const ANSWER: &str = "answer:";

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
        assert_eq!(fed.input(&[3], 1), Taint::Derived);
        assert_eq!(fed.input(&[3], 0), Taint::Clean);
        assert!(fed.derives(&[]), "the root's first output is derived");
    }

    /// A number computed from a secret (`to_number`, then `math {fn: "abs"}`, then written back
    /// with `number_format`) is derived from it; whether the secret reads as a number
    /// (`number_test`) is a Boolean, and cuts it.
    #[test]
    fn the_number_leaves_derive_and_a_number_test_cuts() {
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
        assert!(!fed.derives(&[4]), "a number_test answers a Boolean");
    }

    /// A record holding a secret, merged into another, still holds it as it is, and so do its
    /// values; whether it has a key is a Boolean, which cuts it; and a `group_by` applies a
    /// function, which isn't followed.
    #[test]
    fn the_record_leaves_keep_a_secret_literal_and_has_cuts_it() {
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
        assert!(!fed.derives(&[4]), "has answers a Boolean");
        assert!(fed.derives(&[5]), "a group_by's function isn't followed");
    }

    /// A list holding a secret, reversed, still holds it as it is; whether it holds an item is a
    /// Boolean, which cuts it; a position in it is derived from it; and a `filter` over it applies
    /// a function, which isn't followed.
    #[test]
    fn the_list_leaves_keep_a_secret_literal_and_a_search_cuts_it() {
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
        assert!(!fed.derives(&[3]), "includes answers a Boolean");
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
    /// body, and a third request fed nothing secret: only the first host is where a secret goes.
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
        assert_eq!(sent, BTreeMap::from([("token".to_string(), BTreeSet::from(["api.example.com".to_string()]))]), "the key is only hmac'd");
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
}
