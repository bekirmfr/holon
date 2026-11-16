//! The clock: fires each clock of a deployment (its `every` handlers) on its period.

use crate::*;
use holon_core::Clock;

/// A clock due to tick: its deployment's slug, unit, state, permit and secrets, and the clock.
type DueTick = (String, Hash, Option<DeploymentState>, Arc<Permit>, Vault, Clock);

/// The clocks whose tick goes on now, by deployment and clock: one isn't fired again over its own
/// tick, so a slow clock is one thread and holds nothing but itself.
#[derive(Clone, Default)]
pub(crate) struct Ticking(Arc<Mutex<BTreeSet<(String, String)>>>);

/// A tick going on: its clock is free again when the tick ends, a panic included.
struct InFlight {
    ticking: Ticking,
    clock: (String, String),
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.ticking.0.lock().unwrap().remove(&self.clock);
    }
}

/// One pass of the clock loop at `now`, callable with a controlled `now` (the tests'): each clock
/// due fires on a thread of its own, unless its last tick still goes on (`ticking`), and the pass
/// doesn't wait for them: the handles are the caller's to join or let go. A tick that waits on a
/// call holds its own clock, not another deployment's, not the next pass, not a waiting run's
/// deadline (`spawn_clock_scheduler`). Threads are one per clock at most, the clock's own tick.
pub(crate) fn start_due_clock_ticks(
    store: &SharedStore,
    deployments: &Arc<Mutex<BTreeMap<String, Deployment>>>,
    deployments_path: &Place,
    now: SystemTime,
    ticking: &Ticking,
) -> Vec<thread::JoinHandle<()>> {
    let mut started = Vec::new();
    for (slug, unit_hash, state, permit, secrets, clock) in due_ticks(store, deployments, now) {
        let key = (slug.clone(), clock.label.clone());
        if !ticking.0.lock().unwrap().insert(key.clone()) {
            continue;
        }
        let in_flight = InFlight { ticking: ticking.clone(), clock: key };
        let (store, deployments, deployments_path) = (store.clone(), Arc::clone(deployments), deployments_path.clone());
        started.push(thread::spawn(move || {
            let _in_flight = in_flight;
            fire_one_clock_tick(&store, &deployments, &deployments_path, slug, unit_hash, state, &permit, &secrets, &clock, now);
        }));
    }
    started
}

/// A pass whose ticks have all ended when it returns, a tick's panic passed on: what a test
/// drives the clocks with (the loop doesn't wait: [`start_due_clock_ticks`]).
#[cfg(test)]
pub(crate) fn run_due_clock_ticks(
    store: &SharedStore,
    deployments: &Arc<Mutex<BTreeMap<String, Deployment>>>,
    deployments_path: &Place,
    now: SystemTime,
) {
    for tick in start_due_clock_ticks(store, deployments, deployments_path, now, &Ticking::default()) {
        if let Err(panic) = tick.join() {
            std::panic::resume_unwind(panic);
        }
    }
}

/// The clocks due at `now`, of the deployments that take runs.
fn due_ticks(store: &SharedStore, deployments: &Mutex<BTreeMap<String, Deployment>>, now: SystemTime) -> Vec<DueTick> {
    let deployments = deployments.lock().unwrap();
    let mut due = Vec::new();
    for (slug, dep) in deployments.iter().filter(|(_, d)| d.takes_runs(store)) {
        let clocks = dep.server.as_ref().map(|s| s.clocks.as_slice()).unwrap_or_default();
        for (clock, next_due) in clocks.iter().zip(&dep.clock_due) {
            if now >= *next_due {
                due.push((slug.clone(), dep.unit, dep.state.clone(), Arc::clone(&dep.permit), Arc::clone(&dep.secrets), clock.clone()));
            }
        }
    }
    due
}

/// `POST /deployments/<slug>/tick`'s body: which of its server's clocks ticks (`every 2s`).
#[derive(Deserialize)]
pub(crate) struct TickNowBody {
    pub(crate) clock: String,
}

/// `POST /deployments/<slug>/tick`: that clock ticks now, as the scheduler would tick it (its
/// next tick a period from now), and the tick's history entry answers. The editor's "tick now".
pub(crate) fn handle_post_tick(state: &AppState, slug: &str, request: &Request) -> Response {
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let body: TickNowBody = match serde_json::from_slice(&bytes) {
        Ok(b) => b,
        Err(e) => return err(400, format!("not a valid tick JSON: {e}")),
    };
    let found = {
        let deployments = state.deployments.lock().unwrap();
        let Some(dep) = deployments.get(slug) else { return Response::empty_404() };
        if dep.life.status != Status::Running {
            return err(409, format!("{slug:?} is {}", status_word(dep.life.status)));
        }
        let Some(clock) = dep.server.as_ref().and_then(|s| s.clocks.iter().find(|c| c.label == body.clock)) else {
            return err(404, format!("{slug:?} has no clock {:?}", body.clock));
        };
        (dep.unit, dep.state.clone(), Arc::clone(&dep.permit), Arc::clone(&dep.secrets), clock.clone())
    };
    let (unit, cell, permit, secrets, clock) = found;
    fire_one_clock_tick(&state.store, &state.deployments, &state.deployments_path, slug.to_string(), unit, cell, &permit, &secrets, &clock, SystemTime::now());
    let deployments = state.deployments.lock().unwrap();
    let Some(dep) = deployments.get(slug) else { return Response::empty_404() };
    let mut reply = serde_json::to_value(dep.history.last()).unwrap_or_default();
    Redactor::of(&dep.secrets).json(&mut reply);
    Response::json(&reply)
}

/// One clock's single tick — its handler settled via `fire_sourced` with the real wall-clock time
/// injected, the result logged under its label and appended to its deployment's bounded history. Runs on its own
/// thread when started by a pass (`start_due_clock_ticks`), but has no threading concerns of its own: it only ever touches `store` (internally locked,
/// `Sync`-safe) and this one deployment's own map entry (via the shared `deployments` lock,
/// held only briefly at the end to write the result back).
#[allow(clippy::too_many_arguments)]
pub(crate) fn fire_one_clock_tick(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    deployments_path: &Place,
    slug: String,
    unit_hash: Hash,
    state: Option<DeploymentState>,
    permit: &Permit,
    secrets: &Vault,
    clock: &Clock,
    now: SystemTime,
) {
    let resolver = match closure(store, unit_hash) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("clock tick [{slug}]: store error while materializing closure: {e:?}");
            record_trigger(deployments, &slug, true);
            return;
        }
    };
    // A clock runs its handler, which is the unit its run is of.
    let unit_hash = clock.handler;
    let Some(root_unit) = resolver.resolve(&unit_hash) else {
        eprintln!("clock tick [{slug}]: deployed unit no longer resolves in the store");
        record_trigger(deployments, &slug, true);
        return;
    };
    let label = Some(clock.label.clone());

    let epoch_seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let extra_sourced = [
        (Capability::Clock, Value::Number(epoch_seconds as f64)),
        (Capability::Random, Value::Bytes(rand::random::<[u8; 32]>().to_vec())),
    ];
    let started = started_now();
    let (folders, channels) = deployments.lock().unwrap().get(&slug).map(|d| (Arc::clone(&d.folders), Arc::clone(&d.channels))).unzip();
    let declared = clock.log;
    // As a route's (listeners.rs): a run that won't be logged keeps no timeline.
    let unrecorded = permit.log_level(label.as_deref(), declared) == holon_core::LogLevel::Unrecorded;
    let mut waits = RunWaits { folders, channels, slug: Some(slug.clone()), unrecorded, ..Default::default() };
    let resolver = holon_module::WithEngine::new(&resolver, holon_module::shared());
    let run = fire_with_state(root_unit, &[], &resolver, store, state.as_ref(), &extra_sourced, permit, true, secrets, &mut waits, None);
    let mut entry = run_entry("tick", started, &extra_sourced, &run);
    entry.route = label.clone();
    match durable::park_if_waiting(deployments, store, &slug, &unit_hash, "tick", label.clone(), declared, &[], &extra_sourced, &run, &waits) {
        Ok(names) => entry.parked = names,
        Err(e) => entry.error = Some(e),
    }
    let parked_any = !entry.parked.is_empty();
    // As its handler's options say.
    if let Some(l) = log_run(deployments, &slug, entry, Redactor::tokens(&waits.tokens).and(Redactor::handed(&calls::handed(&run.rounds))), declared) {
        l.said(&slug);
    }

    // What it produced isn't printed: a secret or a resume token in it would land in the server's
    // console, which nothing redacts.
    let which = label.as_ref().map(|l| format!(" {l}")).unwrap_or_default();
    println!("clock tick [{slug}]{which} at {epoch_seconds}: {} effect(s) performed", run.performed.len());

    // What may be computed from a secret withheld, as in its log entry.
    let mut tick = TickEntry {
        at_epoch_seconds: epoch_seconds,
        label,
        outputs: logged_outputs(&run, Value::clone),
        performed: logged_performed(&run, |_, p| p.clone()),
    };
    // Redacted as its log entry is: the history is read by whoever may read the deployment.
    // Of everything at once, the longest value first: what the run computed from a secret may
    // hold the secret, and marked for the secret first it would no longer be found.
    let redactor = Redactor::of(secrets)
        .and(Redactor::tokens(&waits.tokens))
        .and(Redactor::handed(&calls::handed(&run.rounds)))
        .and(Redactor::answers(&run.answered.0, secrets))
        .and(Redactor::born(&run.born));
    redact_tick(&mut tick, &redactor);

    {
        let mut deployments = deployments.lock().unwrap();
        if let Some(dep) = deployments.get_mut(&slug) {
            let next = now + Duration::from_secs(clock.seconds);
            if let Some(i) = dep.server.as_ref().and_then(|s| s.clocks.iter().position(|o| o == clock))
                && i < dep.clock_due.len()
            {
                dep.clock_due[i] = next;
            }
            dep.trigger_count += 1;
            dep.life.record(false);
            dep.history.push(tick);
            if dep.history.len() > 20 {
                dep.history.remove(0);
            }
        }
    }
    // The state is kept by its head, written as the change landed (claims.rs); the deployments
    // file changes only with the deployment's definition, here its waiting runs.
    if parked_any {
        persist_deployments(deployments_path, deployments);
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

/// The clock scheduler: two threads that act on their own, every second, with nobody asking.
/// One starts the clocks due (`start_due_clock_ticks`) and doesn't wait for them; the other
/// expires the waiting runs past their deadlines, each resumed run going on in it. Apart, so
/// neither waits on a call the other's run makes: a tick waiting 30 s on a call held every
/// deployment's clocks and every deadline as long when one thread did both and waited for its
/// ticks (R34). Each ends once the table it serves is gone (a test's; the server's lives on).
pub(crate) fn spawn_clock_scheduler(store: SharedStore, deployments: Arc<Mutex<BTreeMap<String, Deployment>>>, deployments_path: Place) {
    let (clocks, at) = (Arc::downgrade(&deployments), deployments_path.clone());
    let clock_store = store.clone();
    thread::spawn(move || {
        let ticking = Ticking::default();
        loop {
            thread::sleep(Duration::from_secs(1));
            let Some(deployments) = clocks.upgrade() else { return };
            drop(start_due_clock_ticks(&clock_store, &deployments, &at, SystemTime::now(), &ticking));
        }
    });
    let deadlines = Arc::downgrade(&deployments);
    drop(deployments);
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(1));
        let Some(deployments) = deadlines.upgrade() else { return };
        expire_waiting(&store, &deployments, &deployments_path, epoch_now());
    });
}

/// The waiting runs past their deadlines go on, timed out (`durable::expire_waiting`), unless the
/// server is stopping (stopping.rs): then they wait on, and time out once it is back.
pub(crate) fn expire_waiting(store: &SharedStore, deployments: &Mutex<BTreeMap<String, Deployment>>, deployments_path: &Place, now: u64) {
    if !store.1.stopping() {
        durable::expire_waiting(store, deployments, deployments_path, now);
    }
}
