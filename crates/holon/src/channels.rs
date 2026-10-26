//! Channels (`docs/L4s/channels-L4.md`): packs a flow sends with `updates.send(v)`, to whoever
//! subscribes on one of its routes (`respond updates.subscribe()`), as server-sent events.
//!
//! - **When a pack goes out:** once its run is over and its writes have landed, as it is logged
//!   (`log_run`): a run that fails, parks, or is run again sends nothing, and what a subscriber
//!   gets is what the log holds, secrets redacted.
//! - **A pack's id** is its run's log seq and its place among the run's packs (`12.0`, `12.1`):
//!   it only grows, across restarts too, so a reconnecting `EventSource` (`Last-Event-ID`) gets
//!   the kept packs after the last one it saw.
//! - **Kept packs** (`channel_keep` of them) live in memory, rebuilt from the log at load.
//! - **A slow subscriber** whose queue (`channel_buffer`) is full is dropped with `event: missed`
//!   rather than holding the run or the other subscribers; it reconnects and catches up from
//!   the kept packs. Pausing or stopping the deployment ends every subscription with
//!   `event: closed`.

use std::collections::VecDeque;
use std::fmt;

use tokio::sync::mpsc;

use crate::*;

/// What a run's failure says when its packs were past its limits (it did nothing).
pub(crate) const SEND_FAILED: &str = "packs not sent: ";

/// Limits on a deployment's channels: the host's, like the file limits.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ChannelLimits {
    /// The largest pack, as JSON.
    pub(crate) max_pack_bytes: u64,
    /// The packs one run may send.
    pub(crate) max_packs_per_run: u64,
    /// The packs each channel keeps for subscribers that ask for them (`from: "kept"`, or
    /// catching up after `Last-Event-ID`).
    pub(crate) channel_keep: u64,
    /// The packs waiting for one subscriber before it is dropped as too slow.
    pub(crate) channel_buffer: u64,
    /// The subscribers of one channel at once.
    pub(crate) max_subscribers: u64,
}

impl ChannelLimits {
    /// What a deployment gets without asking.
    pub(crate) fn defaults() -> ChannelLimits {
        ChannelLimits { max_pack_bytes: 64 << 10, max_packs_per_run: 1000, channel_keep: 100, channel_buffer: 256, max_subscribers: 100 }
    }

    /// What no grant can exceed.
    pub(crate) fn ceilings() -> ChannelLimits {
        ChannelLimits { max_pack_bytes: 1 << 20, max_packs_per_run: 100_000, channel_keep: 10_000, channel_buffer: 4096, max_subscribers: 10_000 }
    }
}

/// A pack's id: its run's log seq, and its place among that run's packs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct PackId(pub(crate) u64, pub(crate) u32);

impl PackId {
    /// The id an `EventSource` sends back (`Last-Event-ID`), if it is one.
    pub(crate) fn parse(s: &str) -> Option<PackId> {
        let (seq, n) = s.trim().split_once('.')?;
        Some(PackId(seq.parse().ok()?, n.parse().ok()?))
    }
}

impl fmt::Display for PackId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.0, self.1)
    }
}

/// One pack, as it is sent: its id and its JSON.
#[derive(Debug, PartialEq)]
pub(crate) struct Pack {
    /// None for a piece of a piped call's answer: sent as it arrives, never kept.
    pub(crate) id: Option<PackId>,
    pub(crate) json: String,
}

/// What a subscriber reads: a pack, until the subscription ends (and why).
#[derive(Debug)]
pub(crate) struct Subscription {
    pub(crate) packs: mpsc::Receiver<Arc<Pack>>,
    end: Arc<Mutex<Option<End>>>,
}

/// Why a subscription ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum End {
    /// Its queue was full: it was too slow, and missed packs.
    Missed,
    /// The deployment was paused, stopped or removed.
    Closed,
}

impl Subscription {
    /// Why it ended, once its packs have run out.
    pub(crate) fn end(&self) -> End {
        self.end.lock().unwrap().unwrap_or(End::Closed)
    }
}

struct Subscriber {
    packs: mpsc::Sender<Arc<Pack>>,
    end: Arc<Mutex<Option<End>>>,
}

#[derive(Default)]
struct Channel {
    kept: VecDeque<Arc<Pack>>,
    subscribers: Vec<Subscriber>,
}

/// A deployment's channels, by name: kept across redeploys, as its state is.
#[derive(Default)]
pub(crate) struct Channels {
    by_name: Mutex<BTreeMap<String, Channel>>,
    /// The packs each waiting run sent as it parked (channel, JSON), by run id: the sends a later
    /// part of it settles again, and doesn't send twice.
    sent_waiting: Mutex<BTreeMap<String, Vec<(String, String)>>>,
}

impl fmt::Debug for Channels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Channels").field("names", &self.by_name.lock().unwrap().keys().collect::<Vec<_>>()).finish()
    }
}

impl Channels {
    /// The channels as the log left them: each one's newest packs kept.
    pub(crate) fn from_log(log: &Log, limits: &ChannelLimits) -> Channels {
        let channels = Channels::default();
        for entry in log.after(None, usize::MAX) {
            for (name, pack) in channels.packs_of(&entry, entry.seq) {
                channels.send(&name, pack, limits);
            }
        }
        channels
    }

    /// Send `pack` on `name`: kept, and queued for each subscriber; one whose queue is full is
    /// dropped (it missed packs), as is one that went away.
    pub(crate) fn send(&self, name: &str, pack: Pack, limits: &ChannelLimits) {
        let mut by_name = self.by_name.lock().unwrap();
        let channel = by_name.entry(name.to_string()).or_default();
        let pack = Arc::new(pack);
        channel.kept.push_back(Arc::clone(&pack));
        while channel.kept.len() as u64 > limits.channel_keep {
            channel.kept.pop_front();
        }
        channel.subscribers.retain(|s| match s.packs.try_send(Arc::clone(&pack)) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                *s.end.lock().unwrap() = Some(End::Missed);
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        });
    }

    /// Subscribe to `name`: the packs sent from now on, after the kept ones past `after` (a
    /// reconnect's `Last-Event-ID`), or all the kept ones first when `kept`. Refused past
    /// `max_subscribers`.
    pub(crate) fn subscribe(&self, name: &str, kept: bool, after: Option<PackId>, limits: &ChannelLimits) -> Result<Subscription, String> {
        let mut by_name = self.by_name.lock().unwrap();
        let channel = by_name.entry(name.to_string()).or_default();
        channel.subscribers.retain(|s| !s.packs.is_closed());
        if channel.subscribers.len() as u64 >= limits.max_subscribers {
            return Err(format!("the channel {name:?} has as many subscribers as it may ({})", limits.max_subscribers));
        }
        let first: Vec<Arc<Pack>> = match after {
            Some(after) => channel.kept.iter().filter(|p| p.id > Some(after)).cloned().collect(),
            None if kept => channel.kept.iter().cloned().collect(),
            None => Vec::new(),
        };
        let (tx, rx) = mpsc::channel(limits.channel_buffer.max(1) as usize + first.len());
        for p in first {
            let _ = tx.try_send(p);
        }
        let end = Arc::new(Mutex::new(None));
        channel.subscribers.push(Subscriber { packs: tx, end: Arc::clone(&end) });
        Ok(Subscription { packs: rx, end })
    }

    /// The packs a subscriber to `name` gets first: the kept ones when `kept`, else none.
    pub(crate) fn kept(&self, name: &str, kept: bool) -> Vec<Arc<Pack>> {
        let by_name = self.by_name.lock().unwrap();
        by_name.get(name).filter(|_| kept).map(|c| c.kept.iter().cloned().collect()).unwrap_or_default()
    }

    /// Send a piece of a piped call's answer on `name`, as it arrives: to the subscribers there
    /// now, never kept (the run's packs, and its log, are the record).
    pub(crate) fn send_live(&self, name: &str, json: String) {
        let mut by_name = self.by_name.lock().unwrap();
        let Some(channel) = by_name.get_mut(name) else { return };
        let pack = Arc::new(Pack { id: None, json });
        channel.subscribers.retain(|s| match s.packs.try_send(Arc::clone(&pack)) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                *s.end.lock().unwrap() = Some(End::Missed);
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        });
    }

    /// End every subscription with `closed` (the deployment was paused, stopped or removed);
    /// the kept packs stay.
    pub(crate) fn close(&self) {
        for channel in self.by_name.lock().unwrap().values_mut() {
            for s in channel.subscribers.drain(..) {
                *s.end.lock().unwrap() = Some(End::Closed);
            }
        }
    }

    /// How many subscribers `name` has now.
    #[cfg(test)]
    pub(crate) fn subscribers(&self, name: &str) -> usize {
        self.by_name.lock().unwrap().get_mut(name).map_or(0, |c| {
            c.subscribers.retain(|s| !s.packs.is_closed());
            c.subscribers.len()
        })
    }
}

impl Channels {
    /// The packs a logged run sends, by channel, with their ids (`seq` is the entry's): what a
    /// finished run performed, or, for a part that settled waiting, the sends that didn't wait
    /// (its progress), less the ones an earlier part of the same run sent as it parked: a later
    /// part runs again from the start and settles them again, the same (a run replays). None for
    /// a run that failed; what it sent while it waited stays sent.
    pub(crate) fn packs_of(&self, entry: &LogEntry, seq: u64) -> Vec<(String, Pack)> {
        let mut waiting = self.sent_waiting.lock().unwrap();
        let parked = !entry.parked.is_empty();
        let earlier = if parked { waiting.get(&entry.run_id).cloned() } else { waiting.remove(&entry.run_id) };
        if entry.error.is_some() {
            waiting.remove(&entry.run_id);
            return Vec::new();
        }
        let mut already = earlier.clone().unwrap_or_default();
        let mut fresh = Vec::new();
        for p in if parked { &entry.progress } else { &entry.performed } {
            let Some(name) = channel_named(&p.capability) else { continue };
            // As the run sent it: a look-alike of a mark the log escaped is the subscriber's text.
            let mut payload = p.payload.clone();
            if entry.escaped {
                crate::secrets::unescaped(&mut payload);
            }
            let Some(json) = holon_core::stringify_json(&payload.into_core()) else { continue };
            // One the run sent as it parked, settled again: matched one for one, so a pack the
            // rest of the run sends equal to it still goes.
            if let Some(i) = already.iter().position(|(n, j)| *n == name && *j == json) {
                already.remove(i);
                continue;
            }
            fresh.push((name, json));
        }
        if parked {
            let mut sent = earlier.unwrap_or_default();
            sent.extend(fresh.iter().cloned());
            waiting.insert(entry.run_id.clone(), sent);
        }
        fresh.into_iter().enumerate().map(|(n, (name, json))| (name, Pack { id: Some(PackId(seq, n as u32)), json })).collect()
    }
}

/// A piped call's answer, to its channel as it is read (`updates.pipe(http.post(…))`): each
/// piece a text pack, cut where a character ends.
pub(crate) struct Pipe<'a> {
    channels: &'a Channels,
    name: &'a str,
    /// The start of a character a piece cut short, kept for the next.
    rest: Mutex<Vec<u8>>,
}

impl<'a> Pipe<'a> {
    pub(crate) fn new(channels: &'a Channels, name: &'a str) -> Pipe<'a> {
        Pipe { channels, name, rest: Mutex::new(Vec::new()) }
    }
}

impl outbound::Forward for Pipe<'_> {
    fn head(&self, _content_type: &str) {}

    fn chunk(&self, bytes: &[u8]) {
        let mut rest = self.rest.lock().unwrap();
        rest.extend_from_slice(bytes);
        let whole = match std::str::from_utf8(&rest) {
            Ok(_) => rest.len(),
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(_) => rest.len(),
        };
        let text = String::from_utf8_lossy(&rest[..whole]).into_owned();
        rest.drain(..whole);
        if let Some(json) = holon_core::stringify_json(&Value::Text(text)).filter(|j| j != "\"\"") {
            self.channels.send_live(self.name, json);
        }
    }
}

/// The sends among a run's settled outputs, as performing them would make them: for a run that
/// settled waiting, which performs nothing yet, the packs that didn't wait.
pub(crate) fn settled_sends(outputs: &[Option<Value>]) -> Vec<(Capability, Value)> {
    let mut host = firing::RecordingHost { performed: Vec::new() };
    for output in outputs.iter().flatten() {
        holon_host::perform_effects(output, &mut host);
    }
    host.performed.retain(|(c, _)| matches!(c, Capability::ChannelSend(_)));
    host.performed
}

/// The channel a logged capability sends on (`ChannelSend("updates")`), if it sends.
fn channel_named(capability: &str) -> Option<String> {
    capability.strip_prefix("ChannelSend(")?.strip_suffix(')').and_then(|q| serde_json::from_str(q).ok())
}

/// Why a run's packs are past its limits, if they are: too many, one too large, or one with no
/// JSON form.
pub(crate) fn past_pack_limits(performed: &[(Capability, Value)], limits: &ChannelLimits) -> Option<String> {
    let packs: Vec<&Value> = performed.iter().filter(|(c, _)| matches!(c, Capability::ChannelSend(_))).map(|(_, p)| p).collect();
    if packs.len() as u64 > limits.max_packs_per_run {
        return Some(format!("{} packs, more than max_packs_per_run ({})", packs.len(), limits.max_packs_per_run));
    }
    for p in packs {
        match holon_core::stringify_json(p) {
            None => return Some("a pack has no JSON form".into()),
            Some(json) if json.len() as u64 > limits.max_pack_bytes => {
                return Some(format!("a pack of {} bytes, more than max_pack_bytes ({})", json.len(), limits.max_pack_bytes));
            }
            Some(_) => {}
        }
    }
    None
}

/// A pack as a server-sent event.
pub(crate) fn event_text(pack: &Pack) -> String {
    match pack.id {
        Some(id) => format!("id: {id}\ndata: {}\n\n", pack.json),
        // A piece: no id, so a reconnect's Last-Event-ID stays the last pack's.
        None => format!("data: {}\n\n", pack.json),
    }
}

/// The event that ends a subscription.
pub(crate) fn end_text(end: End) -> &'static str {
    match end {
        End::Missed => "event: missed\ndata: {}\n\n",
        End::Closed => "event: closed\ndata: {}\n\n",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(seq: u64, n: u32) -> Pack {
        Pack { id: Some(PackId(seq, n)), json: format!("{{\"n\":{seq}}}") }
    }

    #[test]
    fn a_subscriber_gets_what_is_sent_after_it_and_the_kept_ones_when_it_asks() {
        let limits = ChannelLimits { channel_keep: 2, ..ChannelLimits::defaults() };
        let ch = Channels::default();
        ch.send("u", pack(1, 0), &limits);
        ch.send("u", pack(2, 0), &limits);
        ch.send("u", pack(3, 0), &limits);
        let mut new = ch.subscribe("u", false, None, &limits).unwrap();
        let mut kept = ch.subscribe("u", true, None, &limits).unwrap();
        let mut after = ch.subscribe("u", false, Some(PackId(2, 0)), &limits).unwrap();
        ch.send("u", pack(4, 0), &limits);
        let ids = |s: &mut Subscription| std::iter::from_fn(|| s.packs.try_recv().ok()).map(|p| p.id.unwrap().0).collect::<Vec<_>>();
        assert_eq!(ids(&mut new), [4]);
        assert_eq!(ids(&mut kept), [2, 3, 4], "the two kept, then the new one");
        assert_eq!(ids(&mut after), [3, 4], "the kept ones past the last it saw");
    }

    #[test]
    fn a_slow_subscriber_is_dropped_as_missed_and_closing_ends_the_rest() {
        let limits = ChannelLimits { channel_buffer: 2, max_subscribers: 2, ..ChannelLimits::defaults() };
        let ch = Channels::default();
        let mut slow = ch.subscribe("u", false, None, &limits).unwrap();
        let mut quick = ch.subscribe("u", false, None, &limits).unwrap();
        assert!(ch.subscribe("u", false, None, &limits).unwrap_err().contains("as many subscribers"));
        for seq in 0..3 {
            ch.send("u", pack(seq, 0), &limits);
            while quick.packs.try_recv().is_ok() {}
        }
        assert_eq!(ch.subscribers("u"), 1, "the slow one is gone");
        while slow.packs.try_recv().is_ok() {}
        assert!(slow.packs.try_recv().is_err());
        assert_eq!(slow.end(), End::Missed);
        ch.close();
        assert!(matches!(quick.packs.try_recv(), Err(mpsc::error::TryRecvError::Disconnected)));
        assert_eq!(quick.end(), End::Closed);
        // A subscriber that went away frees its place.
        drop(ch.subscribe("u", false, None, &limits).unwrap());
        assert_eq!(ch.subscribers("u"), 0);
    }

    #[test]
    fn ids_read_back_and_a_run_that_failed_sent_nothing() {
        assert_eq!(PackId::parse("12.3"), Some(PackId(12, 3)));
        assert_eq!(PackId::parse("12"), None);
        assert_eq!(PackId(12, 3).to_string(), "12.3");
        assert!(PackId(12, 3) < PackId(13, 0));
        let sent = |cap: &str, v: Value| PerformedEntry { capability: cap.into(), payload: SValue::from_core(&v) };
        let mut entry = LogEntry {
            performed: vec![
                sent("ChannelSend(\"u\")", Value::Number(1.0)),
                sent("StateWrite(\"k\")", Value::Number(2.0)),
                sent("ChannelSend(\"v\")", Value::Text("hi".into())),
            ],
            ..Default::default()
        };
        let packs = Channels::default().packs_of(&entry, 7);
        assert_eq!(packs, [("u".to_string(), Pack { id: Some(PackId(7, 0)), json: "1".into() }), ("v".to_string(), Pack { id: Some(PackId(7, 1)), json: "\"hi\"".into() })]);
        assert_eq!(event_text(&packs[1].1), "id: 7.1\ndata: \"hi\"\n\n");
        // A look-alike of a mark the log escaped goes out as the run sent it.
        let escaped = LogEntry { performed: vec![sent("ChannelSend(\"u\")", Value::Text("‹‹token:paid›".into()))], escaped: true, ..Default::default() };
        assert_eq!(Channels::default().packs_of(&escaped, 8)[0].1.json, "\"‹token:paid›\"");
        entry.error = Some("busy: …".into());
        assert!(Channels::default().packs_of(&entry, 7).is_empty());
    }

    #[test]
    fn a_waiting_run_sends_what_didnt_wait_as_it_parks_and_never_again() {
        let sent = |v: f64| PerformedEntry { capability: "ChannelSend(\"u\")".into(), payload: SValue::from_core(&Value::Number(v)) };
        let ch = Channels::default();
        let jsons = |packs: Vec<(String, Pack)>| packs.into_iter().map(|(_, p)| (p.id.unwrap().to_string(), p.json)).collect::<Vec<_>>();
        // Parked at its first await, having settled 1; at its second, 1 again and 2.
        let first = LogEntry { run_id: "r".into(), parked: vec!["paid".into()], progress: vec![sent(1.0)], ..Default::default() };
        assert_eq!(jsons(ch.packs_of(&first, 3)), [("3.0".to_string(), "1".to_string())]);
        let second = LogEntry { run_id: "r".into(), parked: vec!["shipped".into()], progress: vec![sent(1.0), sent(2.0)], ..Default::default() };
        assert_eq!(jsons(ch.packs_of(&second, 5)), [("5.0".to_string(), "2".to_string())]);
        // Finished: 1 and 2 again, and 2 a second time after its last await, which still goes.
        let last = LogEntry { run_id: "r".into(), performed: vec![sent(1.0), sent(2.0), sent(2.0), sent(3.0)], ..Default::default() };
        assert_eq!(jsons(ch.packs_of(&last, 8)), [("8.0".to_string(), "2".to_string()), ("8.1".to_string(), "3".to_string())]);
        assert!(ch.sent_waiting.lock().unwrap().is_empty());
        // A run that fails after it parked sends nothing more; what it sent stays sent.
        assert_eq!(ch.packs_of(&first, 9).len(), 1);
        let failed = LogEntry { run_id: "r".into(), performed: vec![sent(1.0)], error: Some("expired".into()), ..Default::default() };
        assert!(ch.packs_of(&failed, 10).is_empty());
        assert!(ch.sent_waiting.lock().unwrap().is_empty());
    }

    #[test]
    fn packs_past_their_limits_fail_the_run() {
        let limits = ChannelLimits { max_pack_bytes: 8, max_packs_per_run: 2, ..ChannelLimits::defaults() };
        let send = |v: Value| (Capability::ChannelSend("u".into()), v);
        assert_eq!(past_pack_limits(&[send(Value::Number(1.0)), send(Value::Number(2.0))], &limits), None);
        assert!(past_pack_limits(&vec![send(Value::Number(1.0)); 3], &limits).unwrap().contains("max_packs_per_run"));
        assert!(past_pack_limits(&[send(Value::Text("a long text".into()))], &limits).unwrap().contains("max_pack_bytes"));
    }
}
