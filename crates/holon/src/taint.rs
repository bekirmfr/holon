//! Which nodes of a run compute with a secret's value, read off the graph before it runs.
//!
//! The log redacts a secret by its value (`secrets::Redactor`): every literal occurrence becomes
//! `‹secret:name›`. A value computed from a secret (base64 of it for a Basic auth header, a field
//! of a JSON secret, a slice) no longer holds it literally, so redaction can't see it, and the
//! log would hand it to whoever may read the deployment. So each port is followed from the
//! `secret` leaves through the graph:
//!
//! - **literal**: the value may hold the secret as it is (a record built with it, a concatenation,
//!   a field read back out): redaction finds it there;
//! - **derived**: the value may hold something computed from it: the log keeps no outputs for
//!   that node, and an outbound request built from it is logged with those parts withheld.
//!
//! What cuts the chain: a one-way leaf (`sha256`, `hmac_sha256`, `secure_equals`), a Boolean
//! answer, and an outbound request's response (what the far side said, not what was sent). A
//! leaf that isn't known to pass its inputs through as they are makes anything tainted derived.

use std::collections::{BTreeMap, HashMap};

use holon_core::{child_refs, Hash, Resolver, Type, Unit, UnitBody, BOUNDARY};

/// How much of a secret a value may carry.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub(crate) enum Taint {
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
/// redaction matches a secret's JSON-escaped form too.)
const PASSES_LITERALLY: &[&str] = &[
    "const", "select", "cons", "head", "tail", "concat", "list", "get", "append", "extend", "join", "pair", "some",
    "record", "field", "with_field", "without_field", "lookup", "insert", "remove", "find", "as_text", "guard",
    "default", "coalesce", "json_stringify", "store_write", "collection_put", "file_write", "file_create",
    "channel_send", "egress",
];

/// Leaves whose output is what the far side answered: an outbound request's response.
const ANSWERED: &[&str] = &["http_request", "http_upload", "http_stream"];

/// Deeper than this, a composite is taken as computing with whatever reaches it.
const MAX_DEPTH: usize = 128;

/// The nodes of one run that a secret reaches, by path (child indices from the run's root, as
/// the timeline and the outbound exchanges name them).
#[derive(Default, Debug, Clone)]
pub(crate) struct SecretFed {
    /// Each node a secret reaches: the taint of its inputs and of its outputs.
    nodes: BTreeMap<Vec<u32>, (Vec<Taint>, Vec<Taint>)>,
    /// Nodes whose insides aren't followed (a `map` or `fold` over a function, a module, a level
    /// past [`MAX_DEPTH`]) that a secret reaches: everything under them counts as derived.
    opaque: Vec<Vec<u32>>,
}

impl SecretFed {
    /// Follow the secrets through `root`, whose inputs are clean (a request, nothing).
    pub(crate) fn of(root: &Unit, resolver: &impl Resolver) -> SecretFed {
        let mut fed = SecretFed::default();
        let mut walk = Walk { resolver, holds_secret: HashMap::new() };
        let inputs = vec![Taint::Clean; root.inputs.len()];
        walk.unit(root, &inputs, &mut Vec::new(), &mut fed);
        fed
    }

    /// Whether a secret reaches nothing in the run.
    pub(crate) fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.opaque.is_empty()
    }

    fn under_opaque(&self, path: &[u32]) -> bool {
        self.opaque.iter().any(|o| path.starts_with(o) && path.len() > o.len())
    }

    /// Whether the node at `path` may produce something computed from a secret: the log keeps
    /// none of its outputs.
    pub(crate) fn derives(&self, path: &[u32]) -> bool {
        self.under_opaque(path) || self.opaque.iter().any(|o| o == path) || self.nodes.get(path).is_some_and(|(_, outs)| outs.contains(&Taint::Derived))
    }

    /// How much of a secret input `port` of the node at `path` may carry.
    pub(crate) fn input(&self, path: &[u32], port: usize) -> Taint {
        if self.under_opaque(path) {
            return Taint::Derived;
        }
        self.nodes.get(path).and_then(|(ins, _)| ins.get(port).copied()).unwrap_or_default()
    }
}

struct Walk<'r, R> {
    resolver: &'r R,
    /// Whether a unit (by hash) reads a secret anywhere in its closure.
    holds_secret: HashMap<Hash, bool>,
}

impl<R: Resolver> Walk<'_, R> {
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
                let own = matches!(&unit.body, UnitBody::Primitive { name } if name == "secret");
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
        let outputs = match &unit.body {
            UnitBody::Primitive { name } => {
                let outputs: Vec<Taint> = unit
                    .outputs
                    .iter()
                    .map(|port| match name.as_str() {
                        "secret" => Taint::Literal,
                        _ if port.ty == Type::Boolean => Taint::Clean,
                        n if ONE_WAY.contains(&n) || ANSWERED.contains(&n) => Taint::Clean,
                        n if PASSES_LITERALLY.contains(&n) => most,
                        _ if most == Taint::Clean => Taint::Clean,
                        _ => Taint::Derived,
                    })
                    .collect();
                // A function applied inside (`map`, `fold`) isn't followed: one that reads a
                // secret, or is handed one, may compute anything with it.
                let applies_secret = child_refs(unit).iter().any(|r| self.holds(r));
                if applies_secret || (!child_refs(unit).is_empty() && most > Taint::Clean) {
                    fed.opaque.push(path.clone());
                    vec![Taint::Derived; unit.outputs.len()]
                } else {
                    outputs
                }
            }
            UnitBody::Module { .. } => {
                if most > Taint::Clean {
                    fed.opaque.push(path.clone());
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

    fn composite(&mut self, unit: &Unit, children: &[Hash], edges: &[holon_core::Edge], inputs: &[Taint], path: &mut Vec<u32>, fed: &mut SecretFed) -> Vec<Taint> {
        let derived = vec![Taint::Derived; unit.outputs.len()];
        // Nothing tainted comes in and nothing inside reads a secret: nothing to follow.
        if inputs.iter().all(|t| *t == Taint::Clean) && !children.iter().any(|c| self.holds(c)) {
            return vec![Taint::Clean; unit.outputs.len()];
        }
        if path.len() >= MAX_DEPTH {
            fed.opaque.push(path.clone());
            return derived;
        }
        let Some(order) = topo_order(children.len(), edges) else {
            fed.opaque.push(path.clone());
            return derived;
        };
        let mut outs: Vec<Option<Vec<Taint>>> = vec![None; children.len()];
        for ci in order {
            let Some(child) = self.resolver.resolve(&children[ci]) else {
                fed.opaque.push(path.clone());
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

/// Kahn's algorithm over the child→child edges: an order in which every child comes after those
/// it reads from, or `None` for a cycle (which core never evaluates either).
fn topo_order(n: usize, edges: &[holon_core::Edge]) -> Option<Vec<usize>> {
    let inner = |e: &&holon_core::Edge| (e.from_child as usize) < n && (e.to_child as usize) < n;
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

/// What a logged outbound request shows when the node that made it was fed something computed
/// from a secret: the parts built from it (its path, headers or body) withheld.
pub(crate) fn withhold_request(request: &mut holon_core::Value, path: &[u32], fed: &SecretFed) {
    const WITHHELD: &str = "‹withheld: computed from a secret›";
    let holon_core::Value::Record(r) = request else { return };
    let derived = |port: usize| fed.input(path, port) == Taint::Derived;
    // An `http_request`'s inputs: its path, its headers, its body (an upload's file handle).
    if derived(0)
        && let Some(holon_core::Value::Text(url)) = r.get_mut("url")
    {
        let origin_end = url.find("://").map_or(0, |i| i + 3);
        let path_start = url[origin_end..].find('/').map_or(url.len(), |i| origin_end + i);
        url.replace_range(path_start.., &format!("/{WITHHELD}"));
    }
    if derived(1)
        && let Some(holon_core::Value::Record(headers)) = r.get_mut("headers")
    {
        for value in headers.values_mut() {
            *value = holon_core::Value::Text(WITHHELD.into());
        }
    }
    if derived(2)
        && let Some(body) = r.get_mut("body")
    {
        *body = holon_core::Value::Text(WITHHELD.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_core::{interface_of, Edge, MapResolver, Param, Port};

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

    #[test]
    fn a_requests_derived_parts_are_withheld() {
        let mut fed = SecretFed::default();
        fed.nodes.insert(vec![2], (vec![Taint::Clean, Taint::Derived, Taint::Literal], vec![Taint::Clean]));
        let text = |s: &str| holon_core::Value::Text(s.into());
        let mut request = holon_core::Value::Record(BTreeMap::from([
            ("url".to_string(), text("https://api.example.com/v1/x")),
            ("headers".to_string(), holon_core::Value::Record(BTreeMap::from([("authorization".to_string(), text("Basic c2tf"))]))),
            ("body".to_string(), text("{\"k\":\"sk_live\"}")),
        ]));
        withhold_request(&mut request, &[2], &fed);
        let holon_core::Value::Record(r) = request else { panic!() };
        assert_eq!(r["url"], text("https://api.example.com/v1/x"), "its path isn't from a secret");
        assert_eq!(r["headers"], holon_core::Value::Record(BTreeMap::from([("authorization".to_string(), text("‹withheld: computed from a secret›"))])));
        assert_eq!(r["body"], text("{\"k\":\"sk_live\"}"), "literal: redaction takes care of it");
    }
}
