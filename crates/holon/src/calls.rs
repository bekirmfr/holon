//! Flow calls (docs/L4s/triggers-L4.md §9, slice 6): a flow runs another deployed flow's action,
//! in-process, as its `flow_call` nodes ask.
//!
//! The call is an outbound request of a kind: core builds `{flow, action, input}`, the host
//! performs it between rounds, and the answer is recorded in the run's log with the request, so
//! a retry, a replay and `holon verify` answer it from the record and never call again. What is
//! performed is an ordinary action run of the other flow (`deploy::run_action`), logged there as
//! run `by` the calling flow and its run, under the other flow's own grant and limits.
//!
//! The answer is `{answer, run}` when it ran (`answer` absent for an action that only writes),
//! and `{error, message}` when it didn't: `refused`, `not_found`, `not_running`, `no_action`,
//! `waiting` (the action parked at an `await`: its run goes on when the event arrives, `run`
//! says which), `failed` and `too_deep`. A call is a permission (`RunsAction("billing/charge")`),
//! checked when it is made, so revoking it refuses the next call at once.
//!
//! Three rules keep calls from running away. A flow isn't called from its own chain (`shop` calls
//! `billing`, which can't call `shop` or `billing` again): a call back would wait for the run that
//! made it. A chain is at most `max_call_depth` calls long. And what is sent is a value, never a
//! secret, whose value or whose derivation would reach the other flow's log.

use crate::*;

/// How the runs of this server reach its deployments: set once, when the server starts, and kept
/// on the store handle every run already has.
#[derive(Default)]
pub(crate) struct Hub {
    host: std::sync::OnceLock<Host>,
}

struct Host {
    deployments: Arc<Mutex<BTreeMap<String, Deployment>>>,
    deployments_path: String,
}

impl Hub {
    /// Let runs reach `deployments`, kept at `deployments_path`.
    pub(crate) fn connect(&self, deployments: Arc<Mutex<BTreeMap<String, Deployment>>>, deployments_path: String) {
        let _ = self.host.set(Host { deployments, deployments_path });
    }
}

/// A run's side of its calls: who it is, what it may call and how it got here.
pub(crate) struct Caller<'a> {
    pub(crate) store: &'a SharedStore,
    /// What the run's secrets reach, to refuse a call what one was made into.
    pub(crate) fed: &'a taint::SecretFed,
    pub(crate) permit: &'a Permit,
    pub(crate) vault: &'a Vault,
    /// The deployment making the calls, and the run (its id) that makes them.
    pub(crate) slug: &'a str,
    pub(crate) run: &'a str,
    /// The flows whose calls led to this run (empty for a run a request, a clock, a file or a
    /// person started).
    pub(crate) chain: &'a [String],
}

/// An answer that says why nothing ran.
pub(crate) fn failure(error: &str, message: impl Into<String>) -> Value {
    Value::Record(BTreeMap::from([("error".to_string(), Value::Text(error.into())), ("message".to_string(), Value::Text(message.into()))]))
}

impl Caller<'_> {
    /// Perform the call `request` (`{flow, action, input}`) that the node at `path` makes, and
    /// answer as the node does.
    pub(crate) fn call(&self, path: &[u32], request: &Value) -> Value {
        let Value::Record(r) = request else { return failure("failed", "a call names a flow and an action") };
        let (Some(Value::Text(flow)), Some(Value::Text(action)), Some(input)) = (r.get("flow"), r.get("action"), r.get("input")) else {
            return failure("failed", "a call names a flow, an action and what it sends");
        };
        if !self.permit.allows(&Capability::RunsAction(format!("{flow}/{action}"))) {
            return failure("refused", format!("this deployment may not run the action {action:?} of {flow:?}"));
        }
        if flow == self.slug || self.chain.iter().any(|f| f == flow) {
            return failure("refused", format!("{flow:?} is already in this chain of calls: a flow isn't called back"));
        }
        let depth = self.chain.len() as u64 + 1;
        if depth > self.permit.durable.max_call_depth {
            return failure("too_deep", format!("a chain of calls may be {} long (max_call_depth)", self.permit.durable.max_call_depth));
        }
        if secrets::secret_in(input, self.vault).is_some() || self.fed.input(path, 0) != taint::Taint::Clean {
            return failure("refused", "a secret, or what is computed from one, can't be sent to another flow");
        }
        let Some(host) = self.store.1.host.get() else { return failure("failed", "this server has no deployments to call") };
        let by = format!("flow {}, run {}", self.slug, self.run);
        let chain: Vec<String> = self.chain.iter().cloned().chain(std::iter::once(self.slug.to_string())).collect();
        let ran = deploy::run_action(self.store, &host.deployments, &host.deployments_path, flow, action, Ok(input.clone()), Some(&by), false, &chain);
        let run = ran.seq.map(|s| Value::Number(s as f64));
        if ran.waiting {
            let mut waiting = BTreeMap::from([("error".to_string(), Value::Text("waiting".into())), ("message".to_string(), Value::Text(format!("{action:?} of {flow:?} is waiting for an event; its run goes on when it arrives")))]);
            waiting.extend(run.map(|n| ("run".to_string(), n)));
            return Value::Record(waiting);
        }
        match (ran.error, ran.why) {
            (None, _) if (200..300).contains(&ran.status) => {
                let mut answered = BTreeMap::new();
                answered.extend(ran.answer.map(|a| ("answer".to_string(), a)));
                answered.extend(run.map(|n| ("run".to_string(), n)));
                Value::Record(answered)
            }
            (message, why) => {
                let mut failed = BTreeMap::from([
                    ("error".to_string(), Value::Text(why.unwrap_or("failed").into())),
                    ("message".to_string(), Value::Text(message.unwrap_or_else(|| format!("the action answered {}", ran.status)))),
                ]);
                failed.extend(run.map(|n| ("run".to_string(), n)));
                Value::Record(failed)
            }
        }
    }
}

/// Whether `request` is a call to another flow's action (`{flow, action, input}`).
pub(crate) fn is_call(request: &Value) -> bool {
    matches!(request, Value::Record(r) if matches!((r.get("flow"), r.get("action")), (Some(Value::Text(_)), Some(Value::Text(_)))))
}
