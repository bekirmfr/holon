# Changelog

## Unreleased

- **Agents and services are principals of their own, with owners and tokens.** Whoever acts on a
  server is a person, an agent or a service, each name unique across them. A person signs in with a
  password or a provider; an agent (role agent or viewer) and a service, a script or CI (role viewer
  or editor, never admin), sign in with tokens of their own and act by their own name. Each agent and
  service has an owner, a person, who makes and revokes its tokens as an admin may; a person who
  still owns one can't be removed until it has another owner, and only a person decides an
  approval. `holon people` replaces `holon user` and `holon token` (`add --person|--agent|--service`,
  `set --owner`, `token NAME`, `revoke NAME ID`), and the `/people` routes replace `/admin/users` and
  `/admin/tokens`. The editor's People and access lists people, then agents and services with their
  owners and tokens; `holon editor`'s admin page does too (`a` adds one, `o` sets its owner). An
  accounts file from before keeps its people, but its API tokens are not read and a person with role
  agent is dropped, which the server says when it starts: add each as an agent or a service, and make
  it a token. No sample changes: this is who signs in, not what a flow does.
- **A flow can write log lines, and the host keeps them beside its runs.** `console.log`, `info`, `debug`,
  `warn` and `error` work in a flow, with no import: the standard library's own form, which is how the
  editor's text reads. A line is no effect a run performed: `holon verify` checks a run without it, and a
  replay writes none. The host keeps each deployment's lines in a rolling file (three of 2 MB, a line cut
  at 4 KiB, 1,000 lines a second at most, what is dropped counted), written about once a second, in memory
  on a server that keeps nothing, and forgotten with the deployment. A line computed from a secret is
  withheld. `holon logs <slug>` reads them (`--level`, `--run`, `--until`, `--limit`, `--follow`), as the
  flow page's **Logs** tab does, and a run's page shows the lines up to its end; `holon editor` has a Logs
  section, and `L` on a run opens its lines. `holon serve --log-stdout` also prints each line, and
  `--log-size`, `--log-files` and `--log-rate` (or `HOLON_LOG_*`) set the caps. An agent reads them only
  where a person let it: the deploy settings' **agents may read its logs** asks for `AgentReadsLogs`, and
  `holon mcp` has a `logs` tool that answers only where it holds. The sample `traced-payout` shows it.
- **`holon mcp` answers while a call is running, and a call can be cancelled or report progress.**
  Each tool call runs on a thread of its own, so a `ping`, or the cancel of the call itself, is
  answered at once rather than after it (a call that waits on a person used to hold the whole
  connection). A request that carries a progress token is told every second that a waiting call is
  alive, which hosts take as a reason to keep waiting, so such a `deployments` with `wait` may wait
  up to 600 seconds, not 45. `notifications/cancelled` stops a call within a fraction of a second
  and leaves it unanswered, as the protocol asks. When a client closes the input, calls that were
  already sent get three seconds to finish and the rest are cancelled, so a script that pipes its
  requests in still gets its answers.
- **`holon mcp` follows the protocol's lifecycle and says what is wrong with an argument.** Until
  a client has sent `initialize`, only `ping` is answered (anything else is an error that says to
  initialize first). An argument of the wrong kind (`text: 5`, `headers: "x"`) comes back as a
  failed tool that says so, where it used to read "`text` is missing", so the model that sent it
  can correct itself. `reference` with no topic keeps each sample to a line (the samples were
  nearly half of its 88 KB) and `reference` with `samples` gives each in full. The server's
  instructions now say a proposal is decided later, with a reason on a decline.
- **`holon wait <slug>` holds until a person decides a proposal.** An agent whose deploy is only
  proposed runs it in the background and is told when it ends: how they decided, with the reason
  of a decline, and a code (0 approved, 1 declined, 3 replaced, 4 timed out, 2 no proposal).
  `holon mcp`'s answer to a proposed deploy now carries the exact command.
- **An agent can wait for a person's decision.** `deployments` with a slug and `wait` (seconds,
  up to 45) holds its answer while that flow's proposal waits, answers the moment a person
  approves or declines (with their reason), and says it is still waiting if they haven't. Holon
  has no way to push a decision into a host's model, so the call is the wait. And `explain` shows
  the lines a run reached with three around each, and says how many it left out: the run of a
  15-line handler was read among a whole flow (471 lines for one of the Tidewheel sample's ticks,
  now 30).

- **A new sample, Tidewheel: a sea nobody hosts, that everyone can check.** The flow never runs
  the world and never sends a picture. Each second its clock chains the touches people planted
  (kept two ticks ahead, as a lockstep game keeps its inputs) onto a sha256 and sends only those on
  a channel; every page runs the same law (a cellular automaton served as a script, its hash in
  the chain's first link) and steps its own sea, and even the weather is read from the chain's
  bytes. The pages vouch for the sea at each fossil, one a minute, and a page that opens late joins
  from the newest fossil and the touches since. A fossil can be replayed in the browser against
  the hash the flow kept, and forked into a sea of one's own. It shows what the flow is for when
  the compute doesn't belong in it: the order of things, their record, and who agrees.

## 0.1.0-rc.34

- **Flows that compute run faster, and a fold that builds a list no longer slows with its square.**
  A fold that grows what it carries (`xs.reduce((acc, x) => [...acc, x], [])`, a record built key
  by key) moves it into each step instead of copying it: a filter by fold over 5,000 items answers
  18 times as many requests a second as before, and its time grows with its length, not its
  length squared. Folds and maps of numbers and small records are 6 to 9% faster than before
  (small values copied, not moved; each composite's children kept with the flow). A ternary now
  answers as JavaScript does when its chosen side has no value; its extra check takes about 15%
  off a filter by fold, already counted in the 18 times. Measured through release servers ([performance](docs/engineering/themes/performance.md#through-a-server-again-after-the-kept-children-and-small-values-490d03e-2026-11-07)).

- **Installing works on the first try.** The install commands in the README and the manual named
  `releases/latest/…`, which GitHub answers 404 while every release is a pre-release: they name
  the release now, and a test holds them to the version, so each release commit updates them.
  On Windows the script is downloaded, then run (`irm … -OutFile`, `powershell -ExecutionPolicy
  Bypass -File`): run straight from the download (`irm … | iex`), Windows PowerShell 5.1 can
  hand it over a line at a time and fail ("The terminator '#>' is missing"), and Microsoft
  Defender flags the one-line `powershell -ExecutionPolicy Bypass -c "irm … | iex"` the release
  page gave. The release workflow now rewrites that line in each release's notes.
- **A handler's log level is its author's choice.** `lazy` and `unrecorded` may be written on any
  handler, one that writes, calls out, sends or waits too: `unrecorded` for runs whose record
  isn't worth keeping (a health check, a read, what would only fill the log), `lazy` for what must
  answer fast where losing the last second of records to a crash is fine. A person still approves
  an unrecorded handler at deploy, and the host's policy still decides. A run that waits keeps its
  handler's level in each later part. The `status-board` sample adds a lazy handler that writes
  (`POST /notes/:id/seen`).
- **What a lazy or unrecorded writer leaves out is said, not taken for a break.** `holon verify`
  leaves unchecked the cells an unrecorded handler, or across a restart a lazy one, may have
  written where the log doesn't hold it; a cell none of them writes is still a break. A restore's
  dry run names the unrecorded handlers granted that act outside the state, and `holon restore`
  and the editor say their runs aren't counted in the runs since.
- **An agent that changes an unrecorded handler's code has its runs logged.** A person approves
  `Unrecorded("GET /health")` for that handler's code: an agent's version that changes it deploys
  with the grant withheld and offered back to a person.
- **An unrecorded run's packs go out live.** They were dropped with its entry; subscribers now get
  them as they are sent, redacted as a logged run's, with no id and never kept. A flow it calls
  logs the run as `unrecorded: not in its log`.
- **A logged run's packs go out once its record is on disk.** A lazy run's packs went out at once,
  so after a power loss a subscriber could be given a pack id it already had, and skip the next
  pack when it reconnected. Packs are now held in the log's order and go out with the sync that
  puts their record on disk; a lazy run still answers at once.
- **A waiting run doesn't send again what it sent as it parked.** After a restart, an unrecorded
  run that waited sent its earlier packs again: the parked run's record now keeps what it sent.
- **Paths, read as each platform writes them.** Every path given to the server (the data folder,
  the path options, the backup folder, a folder or database to bind) is read as its machine reads
  it: on Windows `/` and `\` both separate and a whole path has a drive or a share, so
  `/mnt/archive` isn't one; on Unix `..` stays, as the folder it leads up from may be a link. A
  path that isn't whole says what one is on this machine (`D:\archive` on Windows).
- **Windows: the host reads its disks and memory.** `holon host`, the admin page and the host's
  sweeps read no disk and no memory on Windows, so a full disk was never warned of and
  `when_full = "pause"` or `"roll"` never acted. They now read each folder's volume, and the
  process's working set and its peak.
- **The editor: undo starts off when a flow opens.** A flow just opened had undo on with nothing
  to undo: the canvas measuring its nodes was taken for an edit.

## 0.1.0-rc.33

What each log level costs can now be measured from the command line: `holon load` puts a flow's
routes under load and says, beside the requests a second and the latency, what the server did
with their runs; the `stress-test` sample has one route of each level. Every command's help is
written one way, with examples, and `holon remove` now says what it deletes. Nothing changes for
deployed flows; the deployments listing and a deployment's log only gain fields.

- **`holon load`: a flow's routes under load.** `holon load <slug>` sends each of a deployed flow's
  GET routes (or those `--route` names) as many requests as it answers, from connections kept
  open (`--connections`, default 8) for `--for` seconds each (default 10), with a line a second,
  then the routes side by side: requests a second, p50, p90, p99, the slowest, the errors, and
  from the server the runs it recorded, the times it synced the log to disk for them and the runs
  it only counted. `--json` gives all of it, each second's too. The `stress-test` sample has one
  route of each log level, to see what each costs.
- **Every command's help, written one way.** `holon <command> --help` says what the command does
  in a sentence or two, lists each option on a line of its own with its default and limits
  (and a subcommand's options under it), and ends with examples, each run against a server
  before it was written down. `holon new`, `check`, `show` and `fmt` have help of their own
  (they took `--help` for a file's name and printed every command), and so do `update`,
  `uninstall` and `version` (they refused it). `folder` and `database` say their `--lock`
  takes `commit`, the default, as well as `run`.
- **`holon remove` says what it deletes.** Removing a deployment deletes its runs' log, its
  timeline, its state and its secrets, as the editor's button says; the command's warning, its
  help and the manual said its runs' log stayed on disk. They now say what goes, and that
  `holon export` keeps the runs first. Removing a deployment there isn't says so (`no deployment
  "x"`), where it printed `answered 404:` and nothing.
- **What a deployment says of itself.** The deployments listing names a server's routes, each with
  how its runs are recorded as granted (`routes`: method, path, `log`), and a deployment's log
  says how many times it was synced to disk since the server started (`syncs`); `limit=0` gives
  the counts without the runs.

## 0.1.0-rc.32

Every run's record is on disk before its answer, and a handler says when it needn't be (`lazy`,
`unrecorded`); a slow deployment no longer slows the others. The server now looks after what it
keeps: it sweeps and packs its store, seals its logs' older runs gzipped, says what each flow
holds and warns before its disk fills, does what an admin chose when it is full, and can back up
the runs it lets go of. A security pass closes what a review of all this found. Deployed flows
keep running as they are; a proxy in front of a flow needs the name it forwards (`--allow-host`),
links inside a bound folder aren't followed on Linux, and an identity provider must say an email
is verified.

- **Signing in through an identity provider needs a verified email.** An account is matched by
  its email, so an ID token's email is taken only when the provider says it verified it
  (`email_verified` true); before, one was taken unless the provider said it wasn't, and a
  provider that lets a person type any email signed them in as whoever had it. A token that
  doesn't say signs no one in, and the refusal says why.
- **The setup code can't be guessed.** Five wrong setup codes in a row refuse setting up for a
  minute, as five wrong passwords lock a name; before, a server waiting for its first admin
  checked codes as fast as they came.
- **What the server keeps is its own.** Every file it writes is readable by its owner only
  (`0600`), whatever the umask: the content store's units and values, the packs, the kept file
  texts, the flows, the activity log, the witness's queue, the backup's batches and exports, and
  `holon export`'s file joined what was owner-only already (accounts, secrets, logs, the
  deployments). The data folder and the store's folders it makes are `0700`. A data folder there
  already that other users may read is said at start, with the `chmod 700` that makes it yours
  alone; the server doesn't change it.
- **No folder may be the server's state.** With the data folder outside `~/.holon`, an admin could
  bind the folder of state heads beside the deployments file (`deployments.state/`), and a flow
  given it read-write could rewrite another deployment's state; it is refused now, as the logs'
  folder was, and so are the arrivals, the heads, the activity log and the secrets' audit.
- **A waiting run is always sealed on disk.** What it keeps that may hold a secret (the request
  that started it, the requests it made) is sealed in the deployments file under the secrets key;
  but once a deployment's secrets were read again (one set, or a source refreshed) its vault lost
  the key, and waiting runs were written in the clear from then on. They are sealed now whatever
  happened to the vault, and with `--secrets :memory:` under a key never kept (so after a restart
  such a run says it can't be opened, as its secrets are gone too).
- **A flow's file stays in its folder, whatever else writes there.** On Linux a file name is walked
  to a part at a time with no link followed, and the file is read or written through its folder as
  opened, so a program that can write in a bound folder can't swap in a link, between the check
  and the write, that leads the server outside it; and a write no longer makes folders on its way
  before checking where they are. A link inside a folder isn't followed now, even to inside it
  (elsewhere than Linux, one inside it still is).
- **A flow's port answers only its address, localhost, or a name it was given.** A web page whose
  name was pointed at your machine (DNS rebinding) could call a flow on 127.0.0.1 and read its
  answers as its own; a request under any other name now answers 421, saying what to do. **A
  proxy that forwards the name it was reached by** (Caddy and nginx do) **needs the name given**:
  `holon serve --allow-host hooks.example.com`, as the control plane already did, or the proxy
  told to send the address (`header_up Host {upstream_hostport}`); a tunnel likewise
  (`cloudflared … --http-host-header 127.0.0.1`). [Public HTTPS](docs/manual/guides/public-https.md)
  says how.
- **A flood of requests or sign-ins can't take the server down.** Five wrong passwords lock a
  name for the address they came from only, so someone who knows a name can't keep its owner
  out; tries under ever new names are forgotten after a minute. A few passwords are checked at
  once, on threads kept for it (each check takes 19 MiB), the rest waiting their turn for up to
  thirty seconds before they are told the server is busy: 600 sign-ins sent 200 at a time left
  the server at 178 MB, where running each on its request's thread left it near 1 GB. Past 512
  requests being answered at once, one more is answered 503 at once.
- **Every run's record is on disk before its answer.** A run of a route, clock, file handler or
  action is answered (or counts as done) once its log entry is synced, not only written: an
  answered run is recorded, a power loss or not. Runs at once share the sync. A handler that only
  reads and answers may say `lazy` on its line (`route GET /notes (req) lazy { … }`) to be synced
  within a second instead, as every run was before, and faster for it; one that writes, calls out
  or waits can't, and `holon check` says why at the line. Requests no route took are lazy. A run
  whose record couldn't be written or synced is answered with a 500. New sample:
  `status-board`; `notes-api` and `orders-api` list lazily.
- **A handler can keep its runs out of the log.** `unrecorded` on a handler's line (`route GET
  /health (req) unrecorded { … }`), for one that only reads and answers, asks for
  `Unrecorded("GET /health")` at deploy: "keep runs of GET /health out of its log, failed or not
  (they are counted)". Approved, its runs leave no entry, failed or not, and are counted instead
  (runs, failures, time, the last one, and the last failure's status and error, never its request),
  shown where its runs would be in view mode, the deployment card, the terminal editor and `holon
  history`. A failed one isn't run again; to see why it fails, make it `lazy` for a while. Taken
  back, its next run is recorded and the flow runs on. A host can refuse it whole with `[grants]
  allow_unrecorded = false`. `holon check` says it, and an agent's `call` says a run was counted,
  not logged. `status-board` shows it.
- **A slow deployment no longer slows the others.** A request runs on the data plane's own
  worker only when its run can never wait: an unrecorded handler that reads only its request, the
  clock and random numbers, its flow in memory. Any other runs on the blocking pool, where waiting
  holds up nothing else: a lazy handler (it appends to its log), one reading state, a collection or
  a secret (behind a lock a writer holds while it syncs to disk). Before, a deployment whose reads
  waited on its log or on a write's sync could slow every other deployment's routes a hundredfold.
  A run that panics there is answered 500, as on the pool, not dropped.
- **The caches keep to their sizes in memory.** `--flow-cache` and the new `--value-cache` (or
  `HOLON_VALUE_CACHE`, default 64M) count what a loaded flow or a read value takes in memory, not its
  encoding: measured, a server of flows without a js node held about 8 times its `--flow-cache`,
  and the value cache about 3 times its 64 MiB. Each now keeps a little less than its size, never
  more.
- **The host says what it holds.** `holon host`, the admin page's **the host** and the terminal
  editor's admin view: the server's memory and its caches against their sizes; each disk it keeps
  things on, how full it is for the server (a quota counts) and what it keeps there; the content
  store's values and the disk they take, the logs and the folders' kept texts (counted at start
  and after each round of sweeps); and the sweeps (`GET /admin/host`).
- **Logs seal their older runs, gzipped.** A log keeps its newest 128 runs in its file as
  written, as before, and the older ones it keeps in segments beside it,
  `deployments.logs/<slug>.sealed/<first>-<last>.jsonl.gz`, sealed as it cuts back; a whole
  segment goes once none of its runs is kept (to the backup first, when there is one). The
  soaks' logs went from 192 MB to 37 MB on start; runs, pages, exports and `holon verify` read
  them as before. A log keeping no more than 128 runs seals nothing.
- **The store packs its long-lived values.** Each value was a file of its own, a 358-byte one
  taking a 4 KiB block, and a store writing state left thousands. The server's own sweep now
  moves the values it keeps, once an hour old, into packs deflated together, removing their files:
  a copy of the soaks' store went from 140,026 files and 562 MB on disk to one pack of 1 MiB.
  Packed values read back faster than files did (no value cache: 811–825 requests a second
  against 636–672), keeping up to 8 MiB of inflated blocks; what nothing names any more is swept
  from the pack. `holon host` says what is packed.
- **The runs a log lets go of can be backed up, compressed.** With `[storage] backup` set to a
  folder (an external drive, a cloud client's synced folder), the runs a log cuts back are
  written there first and made into gzipped exports `holon verify` checks (it reads `.json.gz`
  as it reads `.json`): about 360 bytes a run for a small route, 13 to 30 times smaller than the
  log's lines. A backup that can't be written to keeps the runs in the log, and the host warns.
  Every file the server writes there starts with `holon-archive-`, and whatever else the folder
  holds is left alone: only the server's own batches are exported and removed. Set on the admin page or with `holon policy --backup`.
- **An admin chooses what happens when the disk fills.** Past `[storage] full_at` (`95%` unless
  said), `when_full` is `warn` (the default: nothing changes), `pause` (every flow that records
  runs is paused by the host, and resumed once there is room; unrecorded ones go on) or `roll`
  (the biggest log on disk is cut to half its runs every ten seconds, never below 20, and the
  store swept; each keeps its runs as granted again once there is room). Each on the flow's
  timeline. Set on the admin page or with `holon policy --full-at --when-full`.
- **The host warns before its disk fills.** Past `[storage] warn_at` (`85%` full for the server
  unless said, or space left: `5G`), a disk is warned of with what to do and the flow growing
  fastest; so is a cache at its size most of the last hour, and sweeping off while the store
  takes a tenth of its disk. First on the admin page, on the home page for admins, in `holon host`
  and the terminal editor; once in the server's log and its activity. Set it from the admin page
  or with `holon policy --warn-at`.
- **A deployment's log keeps only its newest runs in memory.** It kept every run it keeps (500 by
  default) in memory, whole, beside its file: 1.1 to 3.6 times their lines, counted by no cache's
  size. Now it keeps its newest 32 there and reads older ones from its file when a page asks,
  without holding up its runs. A server on the soaks' data folder starts in 39 MB instead of 320,
  and answers faster for it (the `notes` listing 1,258–1,934 → 2,368–2,390 requests a second).
- **Each flow says what it holds.** In memory, its log's kept runs, its live state and its flow in
  the cache; on disk, its log's file and its states and file texts in the store (as the store's
  last sweep found them). A row each on the admin page and in `holon host`, the most first; a
  **holds** fold on its card on the deployed page; on its page in the terminal editor; `holon host
  --for SLUG`; `GET /deployments/<slug>/holds`.
- **What a crash left is cleared.** A server stopped in the middle of a write (each is made aside,
  then renamed over) left its temporary file behind for good: a copy of the soaks' data folder
  held 60 MB of them. The server now removes them once they are older than any write takes, and
  says so.
- **How often the server sweeps is a setting.** `[storage] sweep_every` in `holon.toml` (`10m` by
  default, a minute at the least, or `off`), set from the admin page or with `holon policy
  --sweep-every`, covers both sweeps: the content store's and the folders' kept file texts. The
  admin page, the terminal editor and `holon policy` say how often, when the next is due and what
  the last of each removed.
- **The server sweeps its content store on its own.** Every ten minutes it removes the values
  nothing kept names, as `holon store sweep` does (an hour after they were put, as before), and
  logs what it freed in its activity. Before, only an admin's sweep did: a server writing state
  filled its disk, by about 5 GB an hour at 50 writes a second. A sweep is skipped when nothing
  was put since the last one and it spared nothing, and paces what it removes (at most about
  2,500 values a second), so a deployment's runs lose 7–12% while it runs, not two thirds. An
  admin's `holon store sweep` still removes as fast as it can.
- **Sweeps no longer hold up a deployment's runs.** The sweeps of file texts and of the store
  read each log's file a line at a time with its lock released, instead of whole while holding
  it: a 60 MB log held up the deployment's runs while it was read. The file texts' sweep is
  skipped when there is nothing it could remove (no folder keeps texts), and parses only the
  lines that name one.
- **No run waits while its log is cut back.** A log's file is cut back to what it keeps by the
  periodic sync, with its lock released for all but the last few lines, instead of by the run that
  found it full, holding the lock: no run waits the 100–150 ms a large log's cut-back takes. The
  directory is synced after the file is replaced, so a crash can't bring back the file it replaced.
- **`chess`: a Sphinx that doesn't answer in time keeps its turn.** When its move call times out,
  the rules no longer play for it at once: the page says so and offers Try again (ask it once
  more) or Let the rules play. While its turn waits, `POST /move` takes only `again` or `rules`,
  never a move, so the player can't play Black's for it.

## 0.1.0-rc.31

A flow's header names everything that outlives a run, its secrets and state keys too, and the
approval says where each secret goes, held at run time and checked offline. Before a deploy or a
restore, everything a flow touches is one list of rows, the same in the editor, the terminal and
the agent tools; the editor itself now runs in a terminal too (`holon editor`), and every page of
the web editor has a command. Deployed flows keep running as they are; a saved `.holon` text
without the new header lines stops compiling until `holon fmt` adds them.

- **A flow's header declares its secrets and state keys.** `secret signing_key` and `state beats`
  are header lines, as `collection notes` is, and the body uses only names the header gave
  (`secret.signing_key`, `state.beats`); a use the header didn't declare is a mistake, marked
  where it is used. `holon fmt` declares what a file uses and says so ("declared secret
  signing_key, state beats"), `holon lsp` offers it as a quick fix, and the web editor's Format
  does it. A saved flow shown by the editor, `holon flows show` or the agent tools gets its lines
  added as fmt would; deployed flows keep running, since the server runs their documents. Every
  sample declares its names.
- **The approval says where each secret goes**: "read the secret "stripe_key", and send it to
  https://api.stripe.com", worked out from the graph before the flow runs, a database it is
  written to and what a call answers to it included; answering a route's caller with one is a
  permission of its own. The canvas marks every place a secret reaches. At run time a request
  carrying a secret's value to a host it isn't shown going to isn't sent (`blocked_secret`), a
  redirect to another host that would resend a body isn't followed (`blocked_redirect`), and
  `holon verify` checks from an export that every request carrying a secret went where its
  approval said.
- **The server's own secrets.** `holon secret bind <name> [--to HOST,…]` (or admin → resources →
  secrets) keeps a secret for every deployment that reads one of that name and has none of its
  own; `--to` says where it may go, and a deploy whose flow would send it elsewhere is refused
  before anything is approved. Sample: `shared-key`.
- **Everything a flow touches, before a deploy or a restore.** The deploy dialog lists each thing
  a flow touches as a row (a host, a secret and where it goes, a folder, a database, a state key,
  a limit), marked new, gone, changed or as now, and **!** where this server isn't ready for it
  and why. `holon editor`, `holon deploy`, `holon restore --dry-run` and the agent tools show the
  same rows, worked out by the server.
- **`holon editor`, the editor in a terminal**: the flows on the server and what waits for you,
  approvals reviewed as diffs in hunks, a flow's status, runs (over its text, with each
  statement's value), history, state, access and graph, flows' text in tabs checked as you type,
  the samples, and the admin pages. With no server it still edits, checks and formats files.
- **The `holon` command does what the editor does.** Everything the editor's deployed page,
  flows page and admin page do now has a command, against the server running here or one
  elsewhere (`--server URL`, `HOLON_TOKEN`), and those that list take `--json`:
  - `holon deployments` lists what is deployed, and with a slug, one's details and the runs
    waiting for an approval or an event; `holon run` (`--fresh`, `--session N`), `pause`, `stop`
    and `remove --yes` start, hold, end and delete one.
  - `holon call <slug> METHOD PATH` sends a deployed flow a request, `holon action <slug> <name>
    --input JSON` runs one of its actions, `holon trigger` runs a flow with no route by hand, and
    `holon tick <slug> "every 2s"` fires one of its clocks now.
  - `holon state <slug>` prints its live state as the language writes values; `--collection`
    lists a collection's items (one with `--key`, every one with `--all`); `--sessions`, each
    session and the state it left.
  - `holon grant <slug>` shows a grant and changes it without a redeploy (`--revoke`, `--grant`,
    `--lower LIMIT=N`, `--keep-running`); `holon grants` is who holds what, and `holon policy`
    the host's policy, `--reload` holding every deployment to it again.
  - `holon secret list|history|set|rollback|remove|env`: a value is piped in or named by its
    source (`--from env:…`, `vault:…`), never an argument and never printed; a rollback's dry run
    names values by fingerprint.
  - `holon flows list|show|save|remove`, the saved flows (`save` compiles a file and keeps its
    text, not deployed), and `holon activity`, what was done on the server, by whom.
  - `holon user`, `holon token` and `holon oidc`, the admin page's people and sign-in: an invite's
    link and a new token are printed once.
  - `holon database tables <name>` lists a bound database's tables.
- **`holon restore --dry-run` shows the code it would deploy**, beside the code deployed now, as
  a line diff, as the agents' `restore` tool does.
- **`holon mcp` has a `state` tool**: what a deployment keeps now, and a collection's items, read
  only.
- **The admin page sweeps storage.** Under resources, **storage** sweeps the content store (it
  counts what it would remove first, then asks) and the file texts runs read and wrote.
- **Every sample says, in its comments, what it does and how to try it.** Each `.holon` file in
  the samples opens with what the flow is, what it shows, the secrets it needs and the requests
  to send it, and each route, action and handler has a line on what it does. `holon new --sample`
  and the editor's copy of a sample keep them (`holon fmt` does), so the heading `holon new` used
  to add is now the file's own.
- **`limit` lines**: a flow's text sets any limit of the deploy settings by its name there,
  `limit run_budget_ms 1m`, `limit request_timeout_ms 50s`, `limit max_requests 100`, as `keep`
  sets the log's two. A `_ms` or `_seconds` limit takes a duration; more than the host's default
  asks for approval at deploy, as from the settings. `fmt` writes the limits a flow's settings
  have as these lines. The `chess` sample uses them to give its thinking models a minute.
- **A new sample, `chess`**: an agent held to the interface the app gives it. You play White in
  the browser (drag a piece, or click it and its square) against a Gemini agent, the model picked
  for each game from the newest few the app lists. The rules are a js node (legal moves, check,
  mate, the draws), and your move goes through them: an illegal one is answered 400 and changes
  nothing. The agent is sent the whole game, the position, the material and the legal moves, and
  answers `{move, comment, verdict, mood, say}` under a schema; the flow checks the answer itself,
  and a move not in the list, an answer that isn't the JSON asked for, or a call Gemini refuses
  plays the rules' own choice, which the page says. It judges your moves (brilliant to insane),
  shows its mood, and speaks up when it struggles or wonders. Its chat replies stream: the call is
  piped to a channel, and the page writes the words as Gemini sends them. Chat with it never
  reaches its moves. The page says how a game is won, lost or drawn, and how it ended, and you can resign;
  while the Sphinx thinks, it says your move is sent and counts the seconds. The board shows its
  files and ranks, each move in the list opens the position after it, and the chat is in bubbles,
  the Sphinx's asides typed in.
  Every proposal and refusal is in the run log.

## 0.1.0-rc.30

A deployment has a history: every deploy is a version kept for good, its code and its state go
back to any version or run after a dry run says what changes, and what was done to it is on a
timeline of its own. How long its log keeps runs is a deploy setting, and a flow's text can say
it. Deployments, logs and state from rc.29 load and run as they are: their kept canvases become
versions that open but don't restore, and runs from before keep no state to restore.

- **`holon deploy <file>` deploys a flow from the command line.** It compiles the text and deploys
  it, as the editor's deploy does: to the server running here, signed in as you, or one elsewhere
  with `--server URL` and an API token in `HOLON_TOKEN`; `--slug` names it. A deploy that needs a
  person's approval prints what it asks for and stops, deploying nothing; `--approve` approves it
  there, as you, and prints what it approved.
- **`holon store sweep` clears the states no kept run names.** Retention cuts old runs from a
  log, but the states they left stayed in the content store; an admin's sweep now removes every
  value nothing kept names: not a kept run, a version, an event, a session, a waiting run or the
  live state, nor any value those name in turn. Anything put in the last hour is spared, and code
  is never swept. `--dry-run` says what it would remove (`POST /admin/store/sweep`). The
  **rolling-log** sample says so.
- **An agent can ask for a restore, and a person approves it.** `holon mcp`'s `restore` tool takes
  `ask`: after its dry run, the restore is saved on the deployment's timeline as an `ask` event
  ("asks to put back the state run 41 left", by *you* (agent)), and nothing changes. In the
  editor's history, that row restores what it asks, code, state or both, after the same dry run
  as any restore. The server answers an agent's restore this way (202) instead of refusing it.
- **A flow's text says how long its log keeps runs: `keep 5 runs for 1d`.** A header line for the
  two retention limits, `keep_runs` and `keep_run_days`, either alone (`keep 500 runs`, `keep runs
  for 30d`). It is the deploy settings' two fields: typed in the text, ⚙ shows it; changed under
  ⚙, the line is written again. Flows deployed from their text (`holon mcp`, `holon new`) carry
  it. The **rolling-log** sample says its retention this way now.
- **`holon verify` checks that each run's state follows from the one before.** It began from the
  state the run before it left, and left what its writes make of the state it began with, each
  state by its snapshot's hash (the one `holon restore --to-run` puts back), made again from the
  log. A state changed between runs, or a write dropped, shows as `run N's state DOESN'T FOLLOW`,
  even in a log chained again after the change. What it can't check it says, by why: the first run
  here, a run after a deploy, a stop or a restore of a version's state, a state holding a
  placeholder; after a restore of a run's state, the next run is checked against what that run
  left. The **ledger** sample's runs check this way, a restore among them.
- **How long a deployment keeps its runs is a deploy setting.** Two limits, under ⚙: **runs the
  log keeps** (`keep_runs`; the host's `--keep-runs`, 500 without it) and **days the log keeps
  runs** (`keep_run_days`; any age without it). Keeping fewer than the host's default asks nobody;
  keeping more asks for approval, as it is the host's disk, and `holon.toml` sets the defaults and
  the ceilings (20000 runs, ten years). A grant lowered later holds the log at once. Deploys,
  restores and grant changes are events, kept for good either way. The new **rolling-log** sample
  ticks every second and keeps its newest 5 runs, while its state counts every beat.

- **A run is labelled with the version it ran on, not the newest of its code.** A version
  deployed again (a restore of its code) is a new number with the same code: the timeline and a
  cell's history now name the number that was deployed when the run happened.

- **A cell's history: which run changed a value.** `holon history <slug> --cell NAME [--item
  KEY]` lists the runs that changed one cell, a collection or one of its items, each with the
  value before and after it, a restore or a deploy that moved it between two runs as a row of its
  own; a run that only read it isn't named. In the editor, ⧗ beside a cell or an item in view
  mode's Flow state lists it, a run's row opening that run; `holon mcp`'s `history` tool takes a
  `cell`; `GET /deployments/<slug>/changes` answers it. The new **balance-trail** sample moves
  money between accounts: its e2e restores a state between moves and reads one account's history.

- **An agent reads a deployment's history and sees what a restore would change.** `holon mcp`
  has two more tools. `history` is the timeline `holon history` prints: the runs, each with the
  version it ran on, and what was done between them, by whom. `restore` is a restore's dry run:
  the code's text beside today's, as a line diff, the permissions, the state's keys and
  collection items now → then, the runs since and the files they wrote. It changes nothing: the
  server answers an agent's restore only as a dry run, and the agent tells a person, who restores
  it in the editor or with `holon restore`.

- **A state restore's dry run names the files the runs since wrote, and an edit made outside a
  run.** A restore never puts a bound folder's files back; its dry run (`holon restore
  --dry-run`, and the editor's window) now names each file the runs since that point wrote, once
  however many times: how many writes, by which runs, and whether it is on disk as the last one
  left it (`changed outside a run since run 48`, `gone from disk`). The editor's diff shows a
  file's text then beside now when the log has it (a run read that content); else the two hashes.
  The new **stock-sheet** sample keeps counts in a collection and a line per change in a file:
  its e2e edits the file by hand and restores the counts, the file left as it is.

- **A dry run says what it changes, and its diff compares it with what is there now.** The
  dry-run window opens with what it changes, a line each: the code deployed (as which version, in
  place of which), the permissions added and dropped, the state's keys and a collection's items;
  for a secret, each version by its fingerprint. Its **diff** button shows each part beside what
  is there now: the flow's text line by line, with its author's names; the grant item by item; the
  state's values, a collection's items by key (`prices["tea"]: 40` → `4`), redacted as the
  control plane is; a secret's `v3 #c9027077 → v4 #0820d784`, never a value. A restore's dry run
  answers the grant now (`granted_now`) and the values and items it would put back; a secret's,
  each value's fingerprint (`to_print`, `current_print`).

- **What the history changes is a dry run first, its result shown before you approve it.** A
  restore (code, state, both, ↺) shows its dry run in a window of its own, part by part: the code
  deployed again and the permissions to approve, the keys the state changes and what the runs
  since did outside; nothing changes until it is approved. A secret's **roll back** (⚙ → ▸) does
  too: `POST …/secrets/<name>/rollback` takes `"dry_run": true` and says which version comes back,
  as what, and the one kept, never a value. The **guarded-api** sample's e2e rolls its token back
  through it.

- **The editor shows a deployment's history, and restores from it.** View mode's **history** button,
  beside the runs' filter, shows what was done to it and its versions newest first, filtered by
  kind and by words (its runs too, when asked). Any row picked shows its details, and any row is
  gone back to: a run or a version to itself, a restore, a grant or a pause to how it was right
  after (the version then, the state then). Picked, it restores its code, its state or both; **↺**
  on a row, there or in the run list, restores what it can: a dry run first, confirmed in a dialog
  that says what changes and asks to approve what today's grant doesn't cover; **canvas** shows a
  version's. `GET /deployments/<slug>/versions` also answers each version's `first_seq`, and a
  restore's event the point it put back (`from`). The **price-rollback** sample's e2e restores
  through it, a restore's row with ↺ too.

- **What was done to a deployment is on its timeline.** Deploys, restores, grant changes (revoked,
  granted back, lowered), secrets set, removed or rolled back (name, version and source, never a
  value), folders and databases bound for it, and pausing, stopping and running, each with who and
  when; the host's own pause when a grant no longer covers it too. `holon history <slug>` shows
  them among the runs (`GET /deployments/<slug>/events`). Recorded, never restored: a restore puts
  back code and state, and the grant, secrets and bindings stay as they are. Events are kept for
  good, in a hash chain of their own beside the run log (`<slug>.events.jsonl`); an export carries
  them, `holon verify` checks their chain, and the witness gets their heads (kind `event`).

- **Every deploy is a version, kept for good, and an earlier one restores.** A deployment keeps
  each version it was deployed as (its unit, its canvas, when and by whom, what its deploy asked),
  not only the last ten canvases, numbered from 1 (`GET /deployments/<slug>/versions`). `holon
  restore <slug> --to N --code` deploys version N's code again as a new version that names it,
  checked against today's grant (`--approve` approves what isn't covered now), the state left as it
  is (`POST /deployments/<slug>/restore`, an editor's or an admin's). Sample: **price-rollback**.
  Deployments from rc.29 load as they are: their kept canvases become versions that open but don't
  restore, and canvases move from the deployments file into the content store. Versions are
  written under a new name (`versions_kept`), so an older release reading the file finds none
  rather than failing.
- **An earlier state restores too: a version's or a run's.** Each run's line in the log names the
  state it left (`state_after`, a snapshot in the content store holding only what it changed), and
  each version the state it was deployed with. `holon restore <slug> --to N --state` puts back the
  state version N left, `--to-run SEQ --state` the state run SEQ left; `--code --state` both, the
  code first. A state put back starts a new session named after it (`run 41`), the state it
  replaces kept as the last session's, so a restore can be restored past. The deployment must be
  running or paused. `--dry-run` lists the keys it would change, and the runs since with what they
  did outside (calls, file and database writes, messages), which a restore doesn't undo
  (`POST /deployments/<slug>/restore`, `{"run": SEQ, "state": true}`). Runs and versions from
  before keep no state to restore. The **price-rollback** sample puts its prices back.

## 0.1.0-rc.29

A flow can have several things that start it, and one flow can run another's action. Deployments,
logs and the state they keep from rc.28 load and run as they are, and their logs still verify. A
flow deployed again from its text is a new version, which asks for nothing more: every write and
call out gained a last input, `trigger` (below), so every unit that has one is hashed differently.
Saved flows in the editor get that port when they are opened.

- **A write or a call out belongs to what started the run.** A route's `state.hits = n` that reads
  nothing of the request is still the route's: it is tied to the request, as a `respond` is, so a
  statement no longer has to use `req` (one tied to nothing is still an error, saying what would
  tie it). `state.last = id after found` ties a statement to a value instead: it writes only once
  `found` has one. A holon with a write in it gets a `trigger` port it doesn't declare. `holon fmt`
  writes none of the ties it can infer.
- **A flow with several handlers.** Beside its routes a flow can have `every 2s { … }` (a clock; `now`
  in it is the tick's time), `on file inbox (f) { … }` (a run for each file new, changed or, with
  `deleted`, removed; `matching "*.csv"` picks names) and `action reset (input) { … }`. Each block
  is a run of its own, and the flow's state is shared by them all. A flow of clocks or actions alone
  listens on no port. `every 10s` and `on file inbox` as headers stay for a flow with no handler;
  beside one, they are an error that names the block to write.
- **Actions: runs by hand, by name.** An action runs when someone runs it, never from the network:
  from the editor's server panel or the deployed page (a box for what to send, the answer, **view
  run**), or `POST /trigger/<slug>/<name>` with a JSON body and `Content-Type: application/json`,
  as an editor or an admin. An agent runs one only after a person ticks it under ⚙ → **agents may
  run** and approves `AgentRuns("reset")`, with `holon mcp`'s new `run` tool; taking that back
  leaves the flow running. Each run is logged under its label (`action reset`) with who ran it.
  `POST /deployments/<slug>/tick` runs a clock now (**tick now**).
- **A flow nothing would start is refused**, with the fix: put its statements in
  `action run (input) { … }`. The editor offers it as **make it runnable by hand**. A flow that reads
  `ingress` and has no handler still runs by hand, by `POST /trigger/<slug>`.
- **One flow runs another's action.** `calls billing` in the headers, then `billing.charge(x)`: the
  server runs billing's action in-process, as an ordinary run of billing logged `by` "flow shop, run
  …", under billing's own grant. It answers `{answer, run}`, or `{error, message}` when nothing
  ran (`refused`, `not_found`, `not_running`, `no_action`, `waiting`, `too_deep`, `failed`), so
  `respond paid.answer, missing: 502` handles every failure. The call is a permission a person
  approves, `RunsAction("billing/charge")` ("run the action charge of the flow billing"), and is in
  the flow's hash. The answer is kept in the caller's log, so a retry, `holon verify` and the
  editor's replay never make the call again. A flow isn't called from its own chain (itself
  included), a chain is at most `max_call_depth` calls long (a new limit: 3 by default, 8 at
  most), a secret or a value computed from one is never sent, and the calls of one round to one
  flow go one at a time, in order. Revoking the permission pauses the caller like any required one,
  or, with **keep running**, makes its next call answer `refused`.
- **The editor.** The server panel lists a flow's clocks, file handlers, actions and routes; ⚙
  shows its clocks where a flow without handlers has its interval; view mode places a run by what
  started it (`every 2s`, `on file inbox`, `action reset`) and replays it over its block; the
  canvas draws the trigger ports lighter, ties a write dropped on it to its route, and flags one
  tied to nothing.
- **Samples.** `counter`, `ticker` (a clock beside routes, and an action), `drop-board` (file
  handlers beside routes), and `billing` with `shop` (deploy billing first).
- **Docs.** A new actions guide, and the language reference, tour, files, editor, deploying,
  permissions, concepts and agents pages, LANGUAGE.md and the guide `holon mcp`'s `reference` tool
  gives agents, all say handlers, ties and calls. A state key is written once per run, not once per
  flow, so two handlers may each write one.

## 0.1.0-rc.28

A security release: the second review of the code, fixed in order. Nothing to redeploy: no unit is
hashed differently, and deployments, logs and waiting runs from rc.27 load as they are (an entry
logged before the marks below named versions still verifies).

- **The log says which value of a secret a run had.** A secret shows as
  `‹secret:name v3 #a1b2c3d4›`: its version and a fingerprint of the value, keyed by the server's
  master key so a guess can't be checked against the log. An old key and the one just set read
  differently, the same value reads the same, and each entry lists the secrets the run was given
  (`secrets`), so verify and the editor's replay stand in the same marks and a run still checks
  after the secret rotates.
- **A token a call answers with a secret is kept out of the log.** An `access_token`,
  `refresh_token`, `id_token`, `session_token`, `token`, `client_secret`, `api_key`, `secret` or
  `password` in a JSON answer to a request that held a secret reads
  `‹token:answer access_token #a1b2c3d4›` wherever it appears, and a write of it to state, a
  collection, a database, a channel or a file is refused, as a secret's is. A run that uses the
  token still checks; one that tried to keep it is skipped. A flow that cached such a token in
  state is now refused.
- **A secret stays out of files, and of what the log keeps of them.** A file, state, collection or
  channel write of a secret's value is refused, a part of a file read that holds one isn't kept,
  and an export doesn't ship a file text that holds one; verify asks for the file, and skips a run
  that wrote a secret's value. A secret used as a record's key is redacted like a value.
- **A redirect to another host leaves the credentials behind.** A call that follows a redirect to a
  different host no longer sends its `authorization`, cookies or other headers there, and a call
  that fails on the way says why without its URL (a path or query can hold a secret).
- **SQL brackets are code.** In `[…]` a subquery now reads its table, so a table behind a bracket
  needs the permission it should. Quote names with `"…"`.
- **A reply is framed by the host.** A flow's, or a proxied upstream's, `content-length`,
  `transfer-encoding`, `connection` and the like are dropped, the rest kept, and a status is 200 to
  599 (`101` switched the connection to another protocol).
- **Sign-in.** Wrong passwords sent at once count before they are checked, so a name locks after
  five tries rather than after however many were in flight, and a name with no account is refused
  as slowly as a wrong password.
- **An agent doesn't open a flow to new sites.** Its deploy that adds an origin to `browsers` that
  the deployment doesn't already answer waits for a person's approval, like any new permission.

## 0.1.0-rc.27

Nothing to redeploy: no unit is hashed differently (every sample rc.26 compiles hashes as it
did), and deployments, logs and waiting runs from rc.26 load as they are. A server that binds no
database runs as before; `--databases` (default `~/.holon/data/databases.json`) is where it keeps
the ones an admin binds.

- **Databases.** A flow names a database an admin binds on the host (`database shop`), never
  where it is: a SQLite file, or a Postgres, MySQL or MongoDB (Atlas included) database whose URL
  the server fetches from a secret's source (`env:HOLON_SECRET_SHOP_URL`, `vault:…`), never typed
  in, stored or shown. `shop.query(sql, v…)`, `shop.one(…)` and `shop.exec(…, expect: n)` take
  SQL, its values as `$1`, `$2`, …; a MongoDB takes record literals (`notes.find("notes", { … })`,
  `find_one`, `insert`, `update`, `delete`), whose shapes are frozen at deploy so a request's
  values never become operators. Each table or collection a flow touches is its own permission
  (`DbRead("shop/orders")`, `DbWrite(…)`). A run's writes land when it finishes, in one
  transaction, only if what it read is as it read it; otherwise it runs again. Bind one with
  `holon database add shop ~/shop.db --schema "create table …"` (`--read-only`, `--for <slug>`,
  `--lock run`) or the admin page. A connection URL whose password is still a placeholder
  (Atlas's `<db_password>`) is refused when it's bound. Samples `orders-api`, `refund-once`,
  `ticket-numbers`, `approved-payout` and `atlas-notes`; the guide is
  `docs/manual/guides/databases.md`.
- **Runs that share what they write.** Runs of several deployments that write one file commit
  one at a time, so none writes over another (sample `shared-ledger`). A run that reads a row or
  file it may write and then calls out holds it until it commits, so two refunds of one order at
  once pay once (`refund-once`); on Postgres, MySQL and MongoDB that holds across servers. An admin
  can hold a database or folder for each run's whole length (`--lock run`), for a counter or a
  queue (`ticket-numbers`). A run that waited (`await`) checks what it shares when it finishes, and
  runs again from where it last waited if it changed (`approved-payout`).
- **The admin page has tabs**: people and sign-in; resources (folders and databases, each
  database's tables, which deployments use each); and permissions (the host's policy, read only,
  with "read the policy again", and who may touch what, by table and file, each linked to its
  deployment's card, where its grant changes).
- **verify trusts only the marks the server wrote.** The log writes `‹secret:name›` and
  `‹token:name›` in place of values it never holds; a caller's text that looks like one is now
  escaped as it is logged, so verify computes with it instead of taking the log's word. A request
  that carried a real secret or token (a resume route's path, a bearer token) keeps the run id the
  log holds, and the report says so; before, verify said such a run's id differed.
- **An env secret is picked from a list.** Under ⚙ → secrets, **env** offers the server's
  `HOLON_SECRET_…` names (its environment and its `--dotenv` file), the one the secret's name
  suggests chosen; **another name…** types one. Opening ⚙ reads the dotenv file again, so a line
  just added is offered and can be set at once. The names (never values) come from
  `GET /admin/secrets/env`, for admins only.
- **New opens a draft.** **new** opens an untitled canvas without asking anything; the flow is
  named on its first save or deploy. A name another saved flow or a deployment already has asks
  before that one is replaced.

## 0.1.0-rc.26

Nothing to redeploy: no unit is hashed differently, and deployments, logs and waiting runs from
rc.25 load as they are. A flow that stores or sends to a channel something computed from a
secret now has that write refused: keep a one-way form of it instead (`sha256`, `hmac_sha256`).

- **What is computed from a secret is never kept.** A state, collection or channel write of a
  value computed from a secret (base64 of it, a slice, a field of a JSON secret) is refused, as a
  write of the secret itself was; the run's other writes go through. Its outputs, its response
  body, any other effect built from one and a clock run's history read `‹withheld: computed from
  a secret›` in the log; the caller still gets its answer. An effect doesn't say which node made
  it, so a key written from such a value has all its writes in that run refused. One-way leaves,
  true/false answers and HTTP answers still cut it, so no sample changes.
- **A viewer stays a viewer.** `X-Holon-Role: agent` made anyone signed in an agent, and an agent
  may save flows, deploy a version needing no new approval and call a deployment, so a viewer
  could replace live code. Only an editor or an admin now steps down to an agent.

## 0.1.0-rc.25

Nothing to redeploy: no unit is hashed differently, and deployments, logs and waiting runs from
rc.24 load as they are (a waiting run is sealed the next time the deployments file is written).
This release fixes what a security review of the server found.

- **An agent's changed code doesn't inherit a secret.** A grant approved what the code did with
  a secret, so when an agent deploys a new version of a flow that reads one, a person approves it
  again: a required secret saves the version for approval, an optional one deploys withheld (its
  reads refused, shown as revoked) until a person grants it back from the deployment's
  permissions. An agent no longer starts a paused or stopped deployment, and a redeploy keeps the
  limits a person lowered.
- **What a run computes from a secret stays out of the log.** The log redacted a secret by its
  value, so base64 of it (a Basic auth header), a field of a JSON secret or a slice reached the
  log, the export and the editor's replay as is. The log now keeps no outputs for a node that
  computes with a secret, and an outbound request built from one reads `‹withheld: computed from
  a secret›` there. One-way leaves (`sha256`, `hmac_sha256`, `secure_equals`) still log, so signing
  flows replay and verify as before; `holon verify` counts the other runs as not run again.
- **A clock run's history is redacted** like its log entry, and the server no longer prints a
  tick's effects.
- **A resume token resumes its run once**, however many requests carry it at the same time; the
  others answer 404 while it runs.
- **A waiting run's record is sealed on disk**, under the secrets master key, and the deployments
  file is written owner-only.
- **Outbound requests: the host sets `Host` and the framing headers.** A flow setting `Host`,
  `Content-Length`, `Transfer-Encoding`, `Connection`, `Keep-Alive`, `TE`, `Trailer`, `Upgrade`,
  `Expect` or a `Proxy-*` header gets `bad_header`, and nothing is sent. NAT64 addresses
  (64:ff9b::/96) count as private.
- **Accounts:** a password reset stops the old password at once; setting a new password ends
  the account's other sessions (a change keeps the one making it).
- **Revoking what starts a deployment's runs pauses it**, even with "keep running when something
  required is revoked" ticked.
- **Folders:** none may be or hold the `--dotenv` file, the secrets key's file, or the flow
  logs, arrivals and heads the server keeps beside its deployments file.
- **Bounded units:** a deploy whose unit holds more than 20,000 units or nests deeper than 64 is
  refused (400); working out a unit's permissions visits each unit once. A module that claims an
  answer past its memory fails as bad output instead of making the server allocate it.

## 0.1.0-rc.24

Flows with a streamed reply (`respond sent.body, stream: true`, the `chat-proxy` sample) are
hashed differently: redeploy them to get the change below. Every other flow is the unit it was,
and keeps running.

- **A streamed reply answers with its call's status.** A call that fails (not 2xx: a bad key, no
  credit, a limit) forwards nothing, and the reply is its answer, whole; it went out as a 200, so
  a page reading the stream showed nothing. Now the lowering wires the call's `sent.status` to
  the respond's status input, and the caller gets the 401 or 400 with the API's own error; the
  log, a replay and `holon verify` say the same. `chat-proxy`'s page shows such an answer ("the
  model's API answered 400: …").
- **A new sample, `gemini-chat`**: chat-proxy's streamed reply against Google's Gemini API (the
  Interactions API, `gemini-3.5-flash-lite`), whose free tier needs no credit. The answer goes to
  the page as it is written, the flow stores it once it ends, and a refusal (a bad key, the free
  tier's rate limit) shows on the page with its status. On the free tier Google may use what is
  sent to improve its products.
- **`respond` takes a computed status:** `respond body, status: req.json.code`, on the respond
  template's status input (`status_input: true`, there only when asked). A number out of 100–599
  answers 500.
- **A plain-HTTP host the server refuses says which.** A browser's password manager could fill
  the deploy settings' "plain HTTP to" with a saved login's name (it takes the field before the
  secret's value box for a username), and the field was then greyed out with it, so a deploy was
  refused with "this host never grants plain HTTP" and no way to see why. The refusal now names
  the host and the field, the field stays editable while it holds a host the server refuses, and
  the secret's value box tells password managers it isn't a sign-in. If yours filled it, its
  password may have gone into that secret: set it again from an empty box.

## 0.1.0-rc.23

Nothing to redeploy: no unit is hashed differently (the catalog only gains `channel_send`,
`channel_subscribe` and `http_stream`), and flows deployed with rc.22 keep running, their logs
read as they are.

- **Channels** (docs/L4s/channels-L4.md): packs a flow streams to whoever listens. A flow
  declares `channel updates`; `updates.send(x)` sends a pack once the run is over
  (`ChannelSend("updates")`), and a route that answers `respond updates.subscribe()` is an event
  stream of them (`ChannelRead("updates")`), `subscribe(from: "kept")` with the kept packs
  first.
  - A pack's id is its run's place in the log and its place among the run's packs (`12.0`), so a
    subscriber that comes back with `Last-Event-ID` gets what it missed, across a restart too;
    one that fell too far behind hears `event: missed`, and a paused, stopped or revoked
    deployment ends its streams with `event: closed`. Subscribers stay across a redeploy.
  - Limits, granted like a deployment's others and in the deploy bar: `max_pack_bytes`,
    `max_packs_per_run`, `channel_keep`, `channel_buffer` and `max_subscribers` (per channel).
- **Streamed replies.** `http.post(…, stream: true)` with `respond sent.body, stream: true`
  forwards the upstream's answer to the caller as it arrives (a model's tokens, a long export),
  with the upstream's content type and nothing buffered on the way; the flow still gets the whole
  answer, and the log keeps it, so the run replays. It takes `missing:`, not `status:` or
  `headers:`.
- **Pipes.** `updates.pipe(http.post(…))` sends a call's answer to the channel's subscribers piece
  by piece as it arrives, cut where a character ends, without an id and never kept; it asks
  `ChannelSend` beside the call's `HttpRequest`, and needn't be in a route.
- **A waiting run's progress goes out as it parks:** the packs it has settled that don't wait are
  sent then and logged with the parked part (`progress`, checked by `holon verify`), and the rest
  when it finishes, none twice.
- **Two new samples**, `live-board` (a board that updates as tickets arrive) and `chat-proxy` (a
  streamed `/chat` and a piped `/room` with a page that listens); `approval` now sends its order's
  status before and after the payment and serves it at `GET /orders/live`.
- **`holon verify` checks a run that computed with a secret.** A node fed a secret or a resume
  token can't be computed again from the log, which holds a placeholder; verify used to count
  such a run as not re-run, so an entry with its answer rewritten passed as the real one did.
  Now the outputs the log kept for that node stand in for it, as the editor's replay does, and
  the rest of the run is checked against them: the answer, the effects, the status. The report
  says which runs were checked that way and for how many nodes; what such a node produced is
  still trusted, and the checking-runs guide says so.
- **The admin page's browse…** chooses the folder to bind: the OS's own folder dialog when the
  editor is on the server's machine, otherwise a list of the server's folders to click through
  (`GET /admin/folders/browse`, admin-only). `HOLON_NO_FOLDER_DIALOG=1` keeps the dialog from
  opening. A listing that arrives late no longer replaces a path being typed, which is what failed
  CI after rc.22.
- **`holon serve` warns when it listens beyond loopback over plain HTTP**, naming which (the
  editor, the flows' ports); the manual says TLS ends at the operator's ingress or proxy.

## 0.1.0-rc.22

Nothing to redeploy: no unit is hashed differently, and flows deployed with rc.21 keep running.
A js node is now spelled `holon f(a) -> b js { … }`; the old `js f = (a) -> b { … }` is an error
that names the new form, and lowers to the same unit.

- **Named folders** (docs/L4s/named-folders-L4.md): files on the host that a flow reads and writes by
  name, as a collection's items, never seeing a path. A flow declares `folder inbox` and asks
  `FileRead("inbox")` or `FileWrite("reports")`; only an admin binds the name to a real folder,
  read-only or read-write, for the whole server or for one deployment (`holon serve --folders`,
  `holon folder add|list|remove`, the admin page). The server's own data can't be bound, a file
  name is checked before it reaches the OS, and its real path must stay inside the folder.
  - Reads are recorded and checked again before the run commits; writes, copies and deletes land
    when it finishes, written beside the file and renamed over it. A run whose read fails, or
    whose write doesn't land, answers 500 and performs nothing.
  - Files of any size: a handle (`logs.file(n)`: name, size, hash), a copy by handle, host to host
    (`archive[n] = h`), and ranges (`logs.lines(n, from: -20, limit: 20)`, `logs.slice(n, offset,
    length)`), kept in the log with what proves them part of the file's hash, so `holon verify`
    checks them without it.
  - What runs read is kept by content, in chunks, as long as a folder's `--keep` says (`bytes`,
    `hash`, `bytes for 30d`), and swept after (`holon folder sweep`); a folder may be capped
    (`--cap 10M`). `holon export --max-files-bytes N` or `--without-files` makes a smaller export,
    and `holon verify --files DIR` answers the reads it left out.
  - Create-only writes: `reports.create(n, text)` and `reports.create_copy(n, h)` never replace a
    file; a taken name fails the run.
  - A deployment's file limits, granted like its others: `inline_file_bytes` (1 MiB),
    `max_file_read_bytes` (64 MiB), `max_file_write_bytes` (1 GiB) and `max_files_per_run`
    (1,000). The error names the limit a run passed.
- **Runs on files.** `on file drop` runs a flow once for each file that arrives in a folder, once
  it stops changing, its handle the run's `ingress`; `deleted` runs on files removed, `matching
  "*.csv"` on some names, and a flow may have several lines, for several folders. The watcher
  acts on the OS's change notices and walks every folder every 30 s anyway. Files there at the
  deploy, and files a flow's own runs wrote or deleted, start nothing; files that arrive while
  it is down or paused run when it is back. `http.put(host, path, file: h)` sends a file as a
  request's body, streamed from the folder and checked against its hash, never through the flow.
- **Five new samples**: `inbox-report`, `log-archive`, `journal`, `drop-box` and `file-audit` for
  folders and runs on files.
- **`every` makes the clock a flow's trigger**: a flow on an interval that never reads `now` (a
  heartbeat that only counts) deploys and runs on the clock, where it couldn't deploy before.
- **The manual** (docs/manual): getting started, the concepts, a tour of the language and its
  reference, a generated page of every leaf, guides and reference pages. The editor has it in a
  docs tab, where each flow in it opens as an unsaved copy.
- A sample opened from the flows page deploys with its text, so its runs are explained by the
  names the text gives them; view mode's run pane resizes by dragging its top edge; the admin
  page says which list is empty.

Nothing to redeploy: no unit is hashed differently, and flows deployed with rc.20 keep running.

- **`respond` takes `headers:`**, a record of response headers: `respond page, headers: {
  "content-type": "text/html; charset=utf-8" }` answers a page instead of JSON, and `respond "",
  headers: { location: "/" }, status: 303` redirects. It is a third input on the template only
  when a flow asks for it, so a respond without it is the unit it always was (docs/LANGUAGE.md
  §5.11).
- **A new sample, `guestbook`**: a web page served by the flow itself. A js node renders the
  guestbook as HTML from the entries in a collection, with the clock wired in to say how long ago
  each was signed and what people type escaped; another parses the posted form; the flow stores a
  signature and redirects back to the page, or answers 400 to an incomplete form. Signed through
  a real browser, the page came back with the new entry.
- **Three things a js body may hold, fixed.** A regex literal: the lexer knows a `/` where a value
  can't be starts one, so the quotes in `/[&<>"']/g` no longer open a string. A port called after
  a leaf (`clock`): the printer keeps it as written, where it came back renamed and the body broke.
  A `#` in JavaScript (a CSS colour): `fmt`'s comment pass leaves a js body alone, where it
  re-inserted the rest of the line as a comment.
- The e2e suite no longer trips over a disabled account that the editor's own poll signed out
  before the test's next step, which is what failed rc.20's CI run (its release built fine).

## 0.1.0-rc.20

Nothing to redeploy: no unit is hashed differently, and flows deployed with rc.19 keep running.
A log written by rc.20 has fields an older reader skips; a policy may now set `claim_wait_ms`.

- **Key claims** (docs/L4s/key-claims-L4.md). Two requests with one idempotency key, sent in
  the same instant, both called the processor: the optimistic commit and its retry kept one call
  per *run*, never one per *key*, because an outbound call leaves mid-run, before the host knows
  whether the attempt will be kept. Now a run about to send its first outbound request claims
  every collection key it read that the flow may write, re-checks those reads against the live
  state, and holds the keys until it commits, is discarded or parks. One that finds a read changed
  as it claims it runs again from the new state having sent nothing; one holding keys and needing
  another run's lets go, waits and runs again, so nothing deadlocks; a commit waits for foreign
  claims on what it writes. Nothing is declared and nothing is in the manifest: the claim is
  derived from the run's reads and the grant, and a run that sends nothing claims nothing, so a
  read-modify-write with no call (the `likes` sample) stays as it was. State cells are never
  claimed: a cell is one per deployment, and claiming it would serialize every calling run.
  `claim_wait_ms` (10 s by default, ceiling 60 s) bounds the wait; past it the run answers 503 as
  busy, naming the key. Verified live: the race that sent two now sends one, the second run
  waiting 907 ms and replaying the first's record.
- **Claims shown.** `runs` says "retried ×1" and "waited 681 ms for a key" on a line that has
  them. `explain` says under the read it protects what the run claimed, how long it waited and
  held the key, or that it found what it read had changed and ran again having sent nothing.
  `holon check` and `show` say, apart from what a flow may do, *while it runs: holds the keys it
  reads in "charges" from its first outbound call until it commits*, for a flow that calls out
  over a collection it both reads and writes.
- **A new sample, `idempotent`**: one charge per Idempotency-Key. The same key with the same body
  answers the first charge again, the same key with a different body is refused (409), a key
  expires after a day, and the processor is called only when there is no prior charge, since
  `prior ?? fresh` never asks for the call on a replay.
- **A new sample, `tally`**: post a CSV and get each column's kind, the min, max and mean of a
  number column, the distinct values and top three of a text column; parsing text into structure
  is a js node's work, and the flow around it keeps the last summary in its state.
- The js engine's cost measurement (`what_a_call_costs`, on request) prints wall time too, cold
  and warm, and says which build it ran on.

## 0.1.0-rc.19

Flows deployed with rc.18 keep running. A flow with a `js` node redeploys as a new version
(the engine module was rebuilt for `check`), with nothing new to approve; a flow saved by rc.18
opens as before and saves smaller.

- **A js node that gives nothing says why.** `explain` names the engine's reason under the
  statement that placed the node: a body's exception as the engine words it ("`deep` gave
  nothing: TypeError: cannot convert 'null' or 'undefined' to object (the body's line 2, col
  19)"), or the engine's own failure ("out of fuel (200000000 instructions)"), for a node bound
  by name or inlined into `respond { … }`. The run records it beside the engine's step.
- **`holon check` compiles each js body** once, running nothing, and says a syntax error at its
  line in the file with the line and a caret, like any other mistake. A body that compiles passes;
  what it does at run time is the run's to say.
- **A flow document carries a module by engine and params, not by its binary.** The js engine's
  preimage is 3.2 MB (6.5 MB as hex) and each budget made another copy in every saved flow,
  deploy body and deployment entry; a flow with two budgets could not deploy at all (the control
  plane's 8 MiB body limit). A document now lists `{hash, engine, params}` under `modules`,
  rebuilt on open from the engine the build carries and checked by hash; a deployment entry for
  the `nearest` sample is 21 KB instead of 6.5 MB, and two budgets deploy.
- **Measured** (docs/L4s/js-node-L4.md F6): on a release build a trivial js body costs 6 ms, the
  `nearest` search 20 ms, a request through the deployed sample 50 to 60 ms more than one
  without a js node; a debug build is eighty times slower.

## 0.1.0-rc.18

Nothing to redeploy: no unit is hashed differently, and flows deployed with rc.17 keep running.

- **JavaScript as one node** (docs/L4s/js-node-L4.md, `docs/LANGUAGE.md` §5.8a). `js name =
  (inputs) -> outputs { body }` is a top-level definition beside `holon`, for the one
  computation the vocabulary can't spell. It runs on a JavaScript engine (Boa) that is itself a
  module: a fresh realm per call, no clock, no randomness, no timers and no network, so the same
  call gives the same answer. Budgets default to 200M instructions and 16 MiB per call (`fuel N`,
  `memory N` change them). `fmt` writes the statement back with its body as typed. Not yet: a
  body that throws, doesn't parse or runs out of fuel makes the node absent, but `explain`
  doesn't show the engine's message, and `holon check` doesn't parse the body. The engine
  rides in the editor's bundle, which grows from 1.9 MB to 6 MB.
- **Modules run** (docs/L4s/wasm-module-L4.md). Everywhere a flow runs (a request, a clock run, a
  resumed durable run, a deploy's first run, `holon verify`'s rerun, and design mode in the
  editor), a module runs under its budget. A deploy's manifest carries the budgets as
  `Limit(module_fuel=…)` and `Limit(module_memory=…)`, `holon check` says "run code it carries"
  with them, and the policy holds them to ceilings (`[ceilings]` raises them).
- **A new sample, `nearest`**: a dictionary that answers the closest word to a query by edit
  distance, the one computation a `js` node does while the flow around it stores, routes and
  answers.
- **The text and the canvas, linked by expression.** The cursor in a flow's text picks the node
  under it (`base64` in `base64(uppercase(req.body))`, not the whole line), and a node picked on
  the canvas marks the expression it is written as. It works in view mode too, where the text
  also shows the run: what didn't run is dimmed (the values of a `match`'s arms not chosen), the
  step is marked as you step, and the values of what you named are written beside it. On a
  window 1600px wide or more, the text pane is open by default.

## 0.1.0-rc.17

**Run `holon migrate` where you used to start `holon serve`.** A server keeps its state in
`~/.holon/data` now, wherever it is started from, so one started as before begins empty; the
old `.holon-*` files are left where they are, and `holon serve` says so when it sees them.

- **Everything in one `~/.holon`** (or `HOLON_HOME`): `data/`, `keys/`, `run/`, `tokens.json`,
  `mcp-hosts.toml` and `bin/`. `holon serve` needs no `--data`; it prints the folder it uses.
  `--data`, `HOLON_DATA` and each per-file flag still win.
- **`holon migrate`** moves what an older release left elsewhere: a server's `.holon-*` files
  (from the current folder, or `--from DIR`) into `~/.holon/data`, and key files from
  `~/.config/holon/keys` into `~/.holon/keys`. It says what it will do and asks (`--yes` doesn't
  ask, `--check` exits 1 when there is something to do), refuses while a server is running,
  checks that the secrets open with their key before anything moves, and copies and checks
  before it removes.
- **A secrets file records which key it uses** (format 3, `key_id`), so a data folder can be
  moved or restored from a backup and its secrets still open. A format 2 file is upgraded on
  open, keeping its key where it is.
- **`uninstall --purge` removes all of `~/.holon`**: data, key, run files, tokens and the list
  of agent hosts set up.
- **A module is a unit** (docs/L4s/wasm-module-L4.md), underneath only: a WebAssembly body with
  no imports, admitted by its exports and budgets. No host runs one yet, so nothing a flow does
  changes, and no unit already encoded is hashed differently.

## 0.1.0-rc.16

Nothing to redeploy: no unit is hashed differently, and flows deployed with rc.15 keep running.
What changes for them is what runs (below): a request in a branch not taken is no longer sent.

- **A branch not taken doesn't run.** `guard` and `coalesce` evaluate only the inputs they need:
  `if c then a else b` runs `a` only when `c` is true and `b` only when it isn't (or `a` has no
  value), `a when c` runs `a` only when `c` is true, and `x ?? y` runs `y` only when `x` has no
  value. So a request in an untaken branch isn't sent, an `await` there doesn't park the run, a
  read there isn't made, and a holon there isn't entered. No value changes. A choice that waits on
  an answer waits a round instead of deciding early. `select` stays strict and runs both sides,
  and a holon's inputs are still computed before it is entered. View mode dims the nodes a run
  didn't run, and the Inspector names the choice that skipped them. New sample: `cache-aside`.
- **`match`: a choice by value.** `match op { "add" -> a + b, "div" -> a / b, else -> 0 }`
  chooses the arm whose pattern equals the subject. Only that arm runs, and if it has no value
  the match has none (so `respond … missing: 400` answers 400), where a chain of `if` would fall
  through to its `else`. Patterns are literals; an arm may take several (`"sub", "minus" ->`).
  It lowers to `equals`, `guard` and `coalesce`, and `holon fmt` prints those back as `match`.
  `match` is now a keyword. New sample: `calc`.
- **`range(n)`** is the list `[0, …, n-1]`, for a whole `n` up to 10000 (nothing otherwise), so
  a `map` can count and still ends. New sample: `tickets`.
- **`holon verify` checks runs that made requests.** It answered a run's requests only from the
  earlier parts of a durable run, so any run that sent a request failed to check.

## 0.1.0-rc.15

**Delete the flows you deployed before this release, then deploy them again.** Some units are
hashed differently from this release on (below), and a server that finds a deployment stored under
the old hash refuses to start:

```
holon: deployment "…" in "…deployments.json": store error: Corrupt { … }
holon: refusing to start rather than overwrite it — fix the file or move it aside
```

To delete them, stop the server, move `deployments.json`, `deployments.logs` and
`deployments.activity.jsonl` out of its data directory (`~/.holon/data` unless you gave `--data`),
and start it again. Accounts and saved flows stay; open each saved flow and deploy it again. Their
runs' logs go with the old deployments.

- **Same-hash children are told apart by where their values go.** Two identical nodes in one
  route used to be ordered by a search with a cap, which could fail on a large route. Each child
  is now ordered by a hash of the path its values take to the route's outputs; a composite without
  such twins keeps its hash. Two identical nodes whose values go nowhere are an error: "the same
  value is computed twice and never used, so nothing tells the two apart: remove one, or use it".
  This is the change that re-hashes units.
- **A lambda uses what is around it.** `map(keys(r), fn: fn(k) -> r[k])` now compiles: a name a
  function uses from the route or holon it is written in becomes an input of the function, and
  `map` and `fold` pass it after the item. The explicit form, `map(xs, r, fn: fn(k, r) -> r[k])`,
  is the same unit. A function of the wrong arity is said at the function. `holon fmt` prints
  one-expression functions back as lambdas. New sample: `quote`.
- **A read inside a mapped function is answered.** A collection read, an outbound request or a
  waiting point inside a `map` or `fold` function came back absent, silently. Each item's is now
  answered, recorded and replayed on its own, and a round sends every item's requests together.
  View mode replays such a `map` from what the run recorded. New sample: `basket`.
- **`holon mcp` reads back what is there:** `deployments`, `flows` and `save` tools, and a run
  records who sent it.
- A wiring-form file whose unit hash starts with a digit reads again.
