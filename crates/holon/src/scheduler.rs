//! The clock: fires each clock-driven deployment on its interval.

use crate::*;

/// A deployment due to tick: its slug, unit, state, interval, permit and secrets.
type DueTick = (String, Hash, Option<DeploymentState>, u64, Arc<Permit>, Vault);

/// The tick-firing logic for one scheduler pass, callable directly with a controlled `now` —
/// pulled out of `spawn_clock_scheduler`'s sleep loop specifically so it's testable without real
/// sleeping (the same "extract the testable logic out of the untestable wrapper" move the
/// editor's own `diagnostics.ts` extraction already used, for the identical reason). Every
/// deployment **due** at `now` fires **concurrently** with every other deployment due at the
/// same moment — see `fire_one_clock_tick` and the module doc for why, and for the honest limit
/// of this approach (unbounded per-tick fan-out, not a worker pool).
pub(crate) fn run_due_clock_ticks(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    deployments_path: &str,
    now: SystemTime,
) {
    let due: Vec<DueTick> = {
        let deployments = deployments.lock().unwrap();
        deployments
            .iter()
            .filter_map(|(slug, dep)| match (dep.interval_seconds, dep.next_due) {
                (Some(interval), Some(next_due)) if now >= next_due && dep.life.status == Status::Running => {
                    Some((slug.clone(), dep.unit, dep.state.clone(), interval, Arc::clone(&dep.permit), Arc::clone(&dep.secrets)))
                }
                _ => None,
            })
            .collect()
    };

    // `thread::scope` (stable since Rust 1.63): spawned threads may borrow `store`/
    // `deployments` directly — no `Arc` clone needed per thread — and every thread is
    // guaranteed joined before `scope()` returns, so this function is still synchronous from
    // its caller's point of view (the existing tests, written before this change, still hold:
    // by the time `run_due_clock_ticks` returns, every due tick from this call has completed).
    // What changes is internal: two deployments due in the same tick no longer block each
    // other — a slow flow only delays itself, not its neighbors.
    //
    // Deliberately NOT a bounded worker pool: a genuinely large number of simultaneously-due
    // deployments spawns that many OS threads at once, unbounded. Fine, and simpler, at the
    // scale this is actually for — a handful of local flows on one machine. Named honestly as
    // a real, open limitation rather than solved here: if per-tick fan-out ever gets large
    // enough to matter, the fix is a bounded worker pool (or a semaphore capping concurrent
    // fires), not more threads. Left open on purpose.
    thread::scope(|scope| {
        for (slug, unit_hash, state, interval, permit, secrets) in due {
            scope.spawn(move || {
                fire_one_clock_tick(store, deployments, deployments_path, slug, unit_hash, state, &permit, &secrets, interval, now)
            });
        }
    });
}

/// One deployment's single tick — settle it via `fire_sourced` with the real wall-clock time
/// injected, log the result, append it to that deployment's bounded history. Runs on its own
/// scoped thread when called from `run_due_clock_ticks`'s per-tick fan-out (see there), but has
/// no threading concerns of its own: it only ever touches `store` (internally locked,
/// `Sync`-safe) and this one deployment's own map entry (via the shared `deployments` lock,
/// held only briefly at the end to write the result back).
#[allow(clippy::too_many_arguments)]
pub(crate) fn fire_one_clock_tick(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    deployments_path: &str,
    slug: String,
    unit_hash: Hash,
    state: Option<DeploymentState>,
    permit: &Permit,
    secrets: &Vault,
    interval: u64,
    now: SystemTime,
) {
    let resolver = match materialize_closure(store, unit_hash) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("clock tick [{slug}]: store error while materializing closure: {e:?}");
            record_trigger(deployments, &slug, true);
            return;
        }
    };
    let Some(root_unit) = resolver.resolve(&unit_hash) else {
        eprintln!("clock tick [{slug}]: deployed unit no longer resolves in the store");
        record_trigger(deployments, &slug, true);
        return;
    };

    let epoch_seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let extra_sourced = [
        (Capability::Clock, Value::Number(epoch_seconds as f64)),
        (Capability::Random, Value::Number(rand::random::<f64>())),
    ];
    let is_state = state.is_some();
    let started = started_now();
    let (folders, channels) = deployments.lock().unwrap().get(&slug).map(|d| (Arc::clone(&d.folders), Arc::clone(&d.channels))).unzip();
    let mut waits = RunWaits { folders, channels, ..Default::default() };
    let resolver = holon_module::WithEngine::new(&resolver, holon_module::shared());
    let run = fire_with_state(root_unit, &[], &resolver, store, state.as_ref(), &extra_sourced, permit, true, secrets, &mut waits, None);
    let mut entry = run_entry("tick", started, &extra_sourced, &run);
    match durable::park_if_waiting(deployments, store, &slug, &unit_hash, "tick", None, &[], &extra_sourced, &run, &waits) {
        Ok(names) => entry.parked = names,
        Err(e) => entry.error = Some(e),
    }
    let parked_any = !entry.parked.is_empty();
    log_run(deployments, &slug, entry, Redactor::tokens(&waits.tokens));

    // What it produced isn't printed: a secret or a resume token in it would land in the server's
    // console, which nothing redacts.
    println!("clock tick [{slug}] at {epoch_seconds}: {} effect(s) performed", run.performed.len());

    // What may be computed from a secret withheld, as in its log entry.
    let mut tick = TickEntry {
        at_epoch_seconds: epoch_seconds,
        outputs: logged_outputs(&run, Value::clone),
        performed: logged_performed(&run, |_, p| p.clone()),
    };
    // Redacted as its log entry is: the history is read by whoever may read the deployment.
    redact_tick(&mut tick, &Redactor::of(secrets));
    redact_tick(&mut tick, &Redactor::tokens(&waits.tokens));
    redact_tick(&mut tick, &Redactor::answers(&run.answered.0, secrets));

    {
        let mut deployments = deployments.lock().unwrap();
        if let Some(dep) = deployments.get_mut(&slug) {
            dep.next_due = Some(now + Duration::from_secs(interval));
            dep.trigger_count += 1;
            dep.life.record(false);
            dep.history.push(tick);
            if dep.history.len() > 20 {
                dep.history.remove(0);
            }
        }
    }
    if is_state || parked_any {
        persist_deployments(deployments_path, deployments, store);
    }
}

/// Every value a tick's history keeps, redacted by `redactor`.
fn redact_tick(tick: &mut TickEntry, redactor: &Redactor) {
    for output in tick.outputs.iter_mut().flatten() {
        redactor.svalue(output);
    }
    for performed in &mut tick.performed {
        redactor.svalue(&mut performed.payload);
    }
}

/// The clock scheduler — the one genuinely new runtime shape in this whole project: a thread
/// that acts on its own, on a timer, with nobody asking. Spawned once at startup with its own
/// `Arc` handle onto `deployments` (no async runtime needed for this workload; a plain sleep
/// loop is enough). Every second, hands off to `run_due_clock_ticks`, which fires whatever's
/// due — concurrently within that one pass, see there for why and for the honest limit.
pub(crate) fn spawn_clock_scheduler(store: SharedStore, deployments: Arc<Mutex<BTreeMap<String, Deployment>>>, deployments_path: String) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(1));
        run_due_clock_ticks(&store, &deployments, &deployments_path, SystemTime::now());
        durable::expire_waiting(&store, &deployments, &deployments_path, epoch_now());
    });
}
