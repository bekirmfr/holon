# Changelog

## Unreleased

- **Changing a secret while a run waits no longer sends its earlier requests again** where the
  request holds the secret as it is: in a header, a URL or a JSON body. When the run goes on, such
  a request is recognised as sent and answered from its record; the same holds for a run tried
  again after a secret changed between its tries. Still sent again, with the new value: a request
  holding a value computed from the secret in the flow (a signature, a Basic credential, another
  encoding); change such a secret when `holon runs <slug> --waiting` lists none. `holon verify`
  and replays read the record the same way.

## 0.1.0-rc.36

- **Known limits of this release.** The first three are the first work after it.
  - Changing a secret while a run waits sends that run's earlier requests again. When the run goes
    on, the requests it made before its wait that carried the secret go out a second time, with the
    new value (`holon secret set`, or an env or vault secret that changes). Change a secret when
    `holon runs <slug> --waiting` lists none.
  - A warning for the admin is also written to the activity record, where everyone who views every
    flow reads it: a viewer or an editor bound on the host, an agent with such a role. Its text can
    say where the data folder is and the system's words for the failure, which the admin's
    warnings keep for whoever views the host.
  - The approval-mail sample mails the asker "Your payout … was approved, and is recorded" when its
    approval only ran out, and nothing is recorded then. Copying it, return before the mail when
    the answer says it expired, as the trading-bot sample does:
    `if (approved.expired ?? false) return …`.
  - A file whose writing pauses for a second or more is read part way, and the rest starts a
    second run, `changed`, with the whole file. Write a file under another name, or elsewhere on
    the same disk, and rename it into the folder once it is complete.
  - `holon verify` reads DIFFERS for a run that read a never-kept value under six characters out
    of its key, though nothing in its record was changed.
  - A deployment whose run records are truly damaged can't be removed until the damaged file is
    restored from a backup of the data folder. Moving it out by hand, with the server stopped, is a
    last resort and only for the oldest of its sealed files: from anywhere else the kept records
    then fail `holon verify` for good.
  - A client whose upload stops is told 408 when the server's 60-second idle limit ends the read,
    up to a minute after it stopped, not at the body's own deadline.
- **A mail or a queue message waiting to be sent is no longer dropped as the server starts.** A
  mail a run sent while its mail server was down, or a message waiting for its queue, was deleted
  at the next start when the run log could no longer vouch for its run: the run had aged out of
  what the log keeps, or the log couldn't be read. Each is now marked once its run's line is on
  disk, and a marked one is sent without asking the log. One the log can't vouch for because the
  log can't be read is kept back, unsent, with a warning for the admin that says what to do.
- **The editor gives up on a request the server never answers, and asks a read again.** A read is
  given up after 8 seconds and asked again twice, after 1 and after 3 seconds; meanwhile the
  buttons that wait on the server say "couldn't reach the server: trying again". A read that has
  the server work first (a replay, a database's tables, an export, the logs) and a change are
  given 2 minutes and are never sent twice; a change that ran out says it may or may not have been
  done. The server tells a browser to keep its answer to the cross-origin check for ten minutes
  (`Access-Control-Max-Age: 600`).
- **When a file of the server's own fails, whoever asked is told what failed; only the admin is
  told where and why.** Answers that passed on a file's path on the server's disk and the system's
  error text (a table that couldn't be written, the policy file, the access history, `keys.json`,
  a sealed file that doesn't open) now say what failed in fixed words; where and why go to the
  server's output and to the admin's warnings. A listener held because its record of what it has
  seen can't be opened says what to do. `GET /admin/secrets/env` gives the `.env` file's path only
  to whoever views the host.
- **A deployment's past runs that can't be read are an error, never fewer runs.** The runs page and
  `holon runs` reaching back, a cell's history, a restore's preview, an export, a replay and a
  collection read at an earlier point answered with what they could read when part of the run log
  couldn't be read (a permission, a disk fault, a damaged file). Each now fails in fixed words,
  `holon export` writes no file, and the admin's warnings name the file and why; a part that
  couldn't be read as the server started stays an error until a restart reads it. Removing a
  deployment keeps its records whole or removes nothing. The store's clean-up removes nothing while
  it can't read every log, and a store value not found while a pack can't be read is an error, not
  "missing".
- **What a flow reads from a reply that sent its secret back is treated as the secret.** A
  service that answers with the secret it was sent (its headers echoed, an account record with
  the key in it) handed the flow the secret again in a reply the server took as ordinary. A
  route that returned the reply whole was refused, but one that returned a slice of it, or a
  field of it, answered its caller with a piece of the secret, wrote it to a collection, and
  left it in the run's log. Now, once a run's calls are answered, the server follows what the
  flow read out of any reply that held a secret: a route that answers with a text of six
  characters or more from it is refused (500) unless the flow may answer with that secret, a
  write of one doesn't land, and the log marks each piece of six or more of the secret's
  characters as `‹derived #…›`. A number, a true or false, or a shorter text read from such a
  reply passes as before (a balance, a count, a currency code); a longer harmless text (a
  customer's name) is refused too, because the server decides by where a value came from and
  what kind it is and never by looking for the secret's characters, which would let a caller
  test guesses. The log of a refused run says what to do. Only flows that call a service which
  sends their secret back are affected. Still open, by design: a piece shorter than six
  characters, a number worked out from a piece, a flow that compares the reply with its
  caller's text itself, and a reply holding only a hint of the secret. `holon verify` refuses
  the same writes when it runs such a run again.
- **The editor's first reads are always answered.** Opening a page sends a burst of requests,
  and the browser opens several new connections to the server at once for them; now and then
  some of those connections were never read, so their requests waited without an answer until
  another connection closed (minutes, on a quiet server), and a button that waits for one stayed
  disabled. The server's HTTP library (tiny_http, under rouille) queued them for threads already
  busy. Holon now carries a patched copy of it (`vendor/tiny_http`, what changed and why in its
  `PATCHED.md`), which also closes a connection that is owed no answer and has sent nothing for
  60 seconds since its last one, answers one past 1,024 open (512 from one address, room for
  every request the server answers at once behind a proxy) with a 503 at once rather than giving
  every socket a thread for ever (a refused one is kept up to 2 s for its client to read the
  503, at most 256 at once, so the control port holds at most 1,280 connections however many come), and
  closes the connection after an answer that says
  `Connection: close` (the 408 for a body that stopped, which used to leave it open).
- **A flow can never keep a key, however short its value.** `flow("charge", { taint:
  ["number"], never_keep: ["cvc"] })`: a value under a key it never keeps is replaced where it
  stands, by `‹never_keep:cvc›`, at any length, in every form a run's record holds it (a record's
  field, a header, a JSON text's field, a query or a form, either inside a JSON text's string as a
  host's echo has it, and a request body over 64 KiB kept by hash), and a write or a send that holds it is
  refused (state, a collection, a file, a database, a channel, a mail, a queue, a line of its log,
  another flow's action); a request to a host is sent. A tainted value is removed by its text,
  which a short one can't be (a CVC `737` is in an amount `1737`), so one under six characters is
  kept; a key never kept is guarded by its place, and the amount stays. For now a flow with it
  can't wait, and a value read out of its key into one of its own (`const c = req.json.cvc`) is
  followed no further. `holon check` says `it never keeps: cvc`, the deploy settings list it under
  "never keeps", and an agent's version that drops a key waits for a person (`KeepAgain`). A text
  computed from a secret that holds such a value under its key gets no hash in the run's record
  (a CVC's would be a thousand tries from it): the record withholds what the run computed from a
  secret, and `holon verify` says why it doesn't run it again. A run's id hashes such a value as
  its mark, so that the id beside its nonce isn't a CVC a thousand tries away either, and verify
  makes it again from the record; a step is kept or left out, for its size, as the record writes
  it, the value as its mark. The **card-check** sample shows it.
- **A run's id hashes a guarded value as its record writes it, where the run knows it at the
  start.** The id is in a run's record beside its nonce
  and everything else it hashes, and it hashed what started the run as it was: a value the record
  marks could be found from the record by guessing until the id came out the same. A weak password
  in an `authorization` header, a tainted card number whose first six digits are a bank's and
  whose last four the flow answers (about 100,000 guesses), a secret a caller sent. Now the id
  hashes each input, sourced value and state cell as the record writes it: a secret by its text, a
  tainted key's value (those of the four credential headers every flow taints included, and a
  credential's parts) and a value never kept, each as its mark. A short tainted value the record
  keeps in clear is hashed as kept. `holon verify` makes the id again from the record, so a run
  that received a secret (a bearer token equal to one) is now checked rather than taken as the
  record says; one that received a value the run finds only as it goes still is: a resume link's
  token, or a tainted value it meets first in a call's answer or a database row (an email it
  looked up as `?q=`, answered under `email`). For that value the id still hashes it as it was,
  so a guess at it can be checked against the id: a limit, which a salted id would close. A
  record made before this change, of a run whose request carried a credential header or a guarded
  key's value under its key, now reads "its run id differs" (its id hashed the value as it was);
  a run with no mark in what started it still checks.
- **`holon serve --emit-test-unit` is gone.** An undocumented flag from the server's first days,
  for checking `POST /unit` by hand: it wrote a fixed test unit to whatever path it was given, from
  the binary that ships. Nothing used it.
- **A change to a deployment that isn't saved says so, and a crash doesn't run a part twice.** A
  deploy, a removal, a grant or a revoke, a pause, a stop or a run, a deployment's settings, its
  state put back, a decision on a waiting run and a run that waits (its token) each answered 200
  even when `deployments.json` couldn't be written, so a revoke or a pause was undone at the next
  start, and a removal came back, after its maker was told it was done. Each now answers 500 when
  the file doesn't take it: the change is in effect now, and won't hold past a restart. A waiting
  run is saved as going on before its next part runs: not saved, it doesn't run and waits on; cut
  off by a crash, it is ended at the next start, its timeline saying so, instead of being given
  its event again and making its effects (a payout) twice.
- **Smaller at-rest and record fixes.** A deployment's state head (the hash of its state's
  snapshot) is sealed: in clear, a small state (a counter, a flag) could be confirmed offline by
  guessing it. The server's console no longer says a given-up mail's subject or recipients, or a
  refused recipient's address: the deployment's events, which are sealed, say them. A log cut back
  holding its lock tells the witness of the trim only once its file is written. `receipts.jsonl`
  rolls past 16 MiB, keeping the newest. A run's copy of the secrets' values, made to redact its
  record, is wiped when it goes. The manual says what a data key's rotation doesn't do (nothing is
  sealed again), and that each run-log line keeps the hash of the line before it in clear.
- **The root key and the backup, three rough edges.** `holon key export --server PORT`, with no
  `--root-key`, reads the root where that server reads it (`holon key status` now names its
  `--root-key` as `source`), once its keys are the folder's; it looked in the default place and
  found nothing for a server given `--root-key file:…`. The backup folder's copy of `keys.json` is
  written again with each write of `keys.json`, at once: for a second or so after a forget, the
  copy still opened the forgotten deployment's archives with the old root. And a root rotated in
  place, `--to` the key file in use spelled another way, stays in the file as it was spelled (on a
  disk that ignores case, the file had taken the new spelling's), and `--root-key` is printed so.
- **Upgrading from 0.1.0-rc.35 or earlier: this release doesn't read that data folder.** The
  server now seals everything in its data folder (below), and a folder written in clear by an
  older release is refused at start, saying so; nothing in it is changed (only `keys.lock` is
  written). There is no conversion: keep running the release that wrote it on it, or move it aside
  and start over, which leaves its accounts, tokens, secrets, state, waiting runs and run logs
  behind. `--secrets-key` and `--new-secrets-key` are gone (`--root-key`, `--new-root-key`), and
  say so. `holon migrate` no longer moves a server's `.holon-*` files (every release that kept
  them wrote them in clear): it names them and refuses; it still moves old key files.
  `holon uninstall --purge` removes the key such a release kept its secrets under (the OS key
  store's `holon / secrets-key-<id>`) with the folder.
- **A failure is no longer retried every second for good.** With the backup unwritable or the
  disk full, a log's cut-back is tried again after a pause that doubles up to five minutes, said
  once, and no longer made again holding the log's lock, which read the whole file each second as
  its runs waited. A waiting run past its deadline that can't be resumed ends, its log saying why,
  instead of being tried each second. A server starting gives each log its backup before cutting
  back the runs past their age, which went unbacked.
- **A slow lock holder no longer stalls the server.** A data-plane worker no longer takes the
  deployments table's lock: what a request needs of it is read with its run, on the blocking pool,
  so a holder writing or sealing can't leave every port deaf. An export, a replay and a removed
  deployment's records read and open the log with its lock released, so exporting a big log no
  longer holds up its runs. With a witness, a run queues its head in memory only; the queue file
  is written with the logs' sync each second, never under the lock every run takes.
- **A removed deployment's life ends whole, and its slug used again starts apart.** A backup of
  the runs a log lets go of is named by its last run's hash as well as its runs' numbers, so a slug
  removed and deployed again, numbering its runs from 0 again, no longer overwrites the old one's
  archives (the manual said they stayed). A run still going as its deployment is removed writes no
  line after the removed deployment's records are taken, and a strict one is answered 500, not 200
  with no record anywhere. Those records are counted by `holon host` and the console, kept as long
  as the deployment's retention would have kept its runs, then moved to the backup or let go of;
  before, they were kept for good, unseen. The witness is sent a `removed` head with the old
  chain's last, and a queue cut while it is down keeps each life's newest heads apart.
- **What is sealed opens only where it was sealed.** A run log's line, sealed apart from its seq and
  the hash before it so the log's lock isn't held while it is sealed, now carries a keyed hash
  binding the two to its body (`r2.` lines): before, the last line's body could be swapped for an
  earlier one's and still open, the chain not showing it. And a deployment's listener records and
  spool files open only where that deployment's belong, not wherever their own header says.
- **A forget says what it reaches, and sweeps the content store.** Forgetting a removed
  deployment's keys makes its log, events, records and backups open nowhere again; the content
  store, which every deployment shares and the server's keys seal, isn't forgotten. The forget now
  sweeps it at once: the values nothing else names are removed (those written in the last hour by
  the next sweep, as any sweep spares them), and it says how many. The flow's code stays in the
  store. The CLI, the editor, the README and the manual said nothing it left opened again.
- **A write that fails is never taken as done.** An event the server couldn't write (a full disk)
  no longer breaks the events' chain for good: the chain goes on from the last line written, the
  next event says which were lost, and `holon verify` lists them (and calls events missing that
  none says were lost removed). A run whose line couldn't be written sends nothing: its packs,
  queue messages and mail are dropped, as a crash would have, instead of going out with no line
  of it in the log. A queue message or mail that can't be spooled fails its run: its record says
  so, its caller gets 500 and nothing else it sends goes out, where before the message was
  dropped and the record said it was sent. A deployment's keys are made as it is first deployed,
  and a deploy whose keys can't be made is refused, instead of going live with nothing it runs
  sealable.
- **Waiting runs are sealed once, and capped by bytes.** A run waiting at `await` is sealed as it
  parks or parks again, and every save of the deployments file writes that sealed form, instead
  of sealing every waiting run of every deployment again under the table's lock. A new limit,
  `max_waiting_bytes` (64 MiB by default, 1 GiB at most), caps what a deployment's waiting runs
  keep in all: past it a run that would wait fails as one past `max_waiting_runs` does, and a
  waiting run whose next part would keep more waits on as it was, its event deliverable again.
- **A caller decides nothing of how big a run log gets.** A request body longer than 64 KiB, as
  the log's line writes it, is kept by hash beside the log (sealed, with the file texts runs
  read), and the entry names it; `holon verify` and a replay put it back from the export. Before,
  a 1 MiB body made a line of up to 17 MB, held in memory and read whole at start. A request that
  runs nothing (no route takes it, a method its route doesn't, a body that isn't text) is counted
  under `(ran nothing)` in the log's tallies instead of logged, so it can't push a deployment's
  runs out of its log. A server starting reads a log's runs a line at a time to rebuild its
  channels and its spooled mail and messages, instead of three whole copies.
- **Everything in the data folder is sealed at rest, under a root key kept apart from it.** Every
  table, the content store (its files named by a keyed hash), the run logs, events and console
  lines (a line at a time), secrets, waiting runs, the host key, spools, removed records and
  backups are sealed (XChaCha20-Poly1305) under data keys, one per scope (`server`, `secrets`,
  `names`, `d:<slug>` for each deployment), kept in `keys.json` wrapped by a root key. The root
  is kept where `--root-key` says (`os`, the default on Windows and macOS; a file in
  `~/.holon/keys` elsewhere; `env:VAR`; `file:PATH`), never in the data folder: the first start
  makes it and says where, and a root file inside the folder is refused. Back it up apart:
  `holon key export --to file:PATH` copies it, from the OS key store too. The backup folder keeps
  a copy of `keys.json` (`holon-archive-keys.json`), so a backup and the root open its archives
  alone; `holon verify` finds it beside an archive, or takes `--data` and `--root-key`. A run
  log's line is sealed outside the log's lock; sealing costs lazy runs about 11-14% of their
  throughput (docs/engineering/performance.md).
- **`holon key`: status, rotate, forget, export, and a new root.** `holon key status` says where
  the root is and each scope's versions; `rotate <scope>` makes a new data key that seals from
  then on; `forget <slug> --yes` (or `holon remove <slug> --forget`, or the editor's remove box)
  drops a removed deployment's keys, so its records, backups and leftovers open nowhere again and
  say they were forgotten, then rotates the root where it is kept, and says to back the new one
  up. `rotate --root --to <SOURCE>`, with the server stopped, wraps every key under a new root: it
  never writes over a key it didn't make, a rotation cut short at any write is finished or undone
  at the next start, and a root that another data folder opens too (a copy, or a second server on
  the same `--root-key`) is never rotated in place, naming that folder.
- **A `keys.json` older than its data is refused, never taken as empty.** A run log, events or a
  sealed segment holding a line whose key `keys.json` doesn't have stops the start, naming the
  deployment, the line and the version, and a deploy onto it is refused; nothing of it is skipped,
  numbered again from 0 or cut. A log that is there but can't be read refuses the same way. A
  mailbox, queue, feed or folder watch whose record of what it has seen doesn't open holds: it
  runs nothing and marks nothing seen, and says why, where before it ran everything again or
  marked what waited as seen.
- **Removing a flow keeps its run records.** `DELETE /deployments/<slug>` first writes its log,
  timeline, units and file texts as an export `holon verify` checks (`removed/<slug>-runs-<ms>.json.gz`),
  and removes nothing if it can't. One run exports alone: `GET /deployments/<slug>/export?run=N`,
  `holon export --run N`, and `holon verify` checks it. A witnessed head now says which runs it
  proves ("no run from 3 to 5 was changed or removed"), and that it says nothing of runs a trimmed
  log let go.
- **Who started a waiting run, when each account was last seen, when each flow last ran.** A run
  that parks keeps who started it (`by` in `/runs`; "started by ada" in `holon decide`,
  `holon runs --waiting` and the MCP tools). Each account keeps `last_seen` and each token
  `last_used`, written at most once a minute. `/summary` gives each flow's `last_ms`, which the
  Overview's and Flows' Last run show. `GET /deployments/<slug>/channels` counts each channel's
  kept packs and subscribers, which Resources shows.
- **The editor's canvas.** Nodes is the flow, edited; Statements and Grouped are read-only views
  of it. A node clicked brings its code into view, and the code's cursor its node; the mouse is
  the same in Edit and View (middle drag pans, left drag selects). A failed run draws red only
  where it failed, and what never ran is faint; a waiting run replays as itself. View runs shows
  the runs and the run picked side by side; the Inspector and bottom pane fold. Every button asks
  the server what it may send through one hook, and a run started by another flow's run is named
  by its first eight characters.
- **Dialogs answer Enter with the focused control, and keep the focus.** Enter answered every
  in-page dialog yes wherever the focus was: tabbing to Cancel on a delete or a deploy's consent
  and pressing Enter went ahead. Now it does what a click on the focused control would, and every
  modal keeps Tab inside it.
- **No other page can frame the editor or the API, and the editor's fonts come with it.** The
  server answers with `X-Frame-Options: DENY` and a `frame-ancestors 'none'` policy, so a hostile
  page can't show the signed-in editor and lure a click onto Approve; the fonts are served by the
  server, not fetched from Google.
- **Text nested past 128 levels is refused at its place.** 1,000 nested parentheses overflowed
  the stack of `holon check`, `holon lsp`, the MCP server's `check` and the editor; now it is an
  error at the line and column where it goes too deep.
- **A long answer is cut after its secrets are marked.** A run's line keeps 64 KiB of what a
  route answered. It was cut there as the run had it, and marked afterwards: a secret lying
  across the cut left its first part in the line, since the whole value was no longer there to
  find. That took an answer of more than 64 KiB with a secret at exactly that place (a route
  that echoes what it is sent, to a caller who sends the secret). Now the answer is marked whole
  and cut as the line writes it, never inside a mark, and `holon verify` cuts what a run answers
  again the same way and compares the two whole. A line written before, whose answer was cut,
  reads as changed to this verify: its last characters may differ.
- **A secret with a quote, a backslash or a newline in it is kept out in every spelling JSON
  gives it.** Written out as JSON, such a value is spelled another way, and again another the
  next time. A run's record found a secret as it is and written out once; the guards that refuse
  a write of a secret, or a send to a host its approval doesn't show, found it only as it is. So
  a password with a quote in it, put into a JSON text and written to a collection, reached the
  store; put into a stringified payload inside a JSON body, it stood in the run's record in the
  clear, and in the record of every later run that read it back. Now the record marks a secret
  at each depth it is written out, the guards find it however deep and however spelled
  (`\u0022` for a quote too), and a text that holds one where no mark can be put is kept out
  whole, as `‹respelled:secret:name›`. A secret with none of those characters, as most tokens and keys
  are, was never affected. If a secret of yours has one and your flow wrote it into JSON, treat
  it as stored where the flow wrote it, and rotate it.
- **A run that computed with a secret is checked offline, and its record marks what it computed.**
  Everything a run computed from a secret was left out of its record whole, by the graph alone
  (the step's outputs, the header it was sent in, the answer), and `holon verify` never ran such a
  run again: it said the step was "too large, or too deep". Now a text of 6 characters or more
  that a step computed from a secret is written as `‹derived #7c1e9a02›` wherever the run's record
  holds it, found by its value as a secret is, and the entry lists each such value by a keyed hash
  only the server can make (`derived`); one value has one mark through a run and another in the
  next. `holon verify` runs the run again on the marks. A value no mark stands for (a true or
  false, a number, bytes, a short text) is withheld as before with what follows from it, and the
  check says that is why the run wasn't run again. A hash or a signature of a part of a secret is
  such a value and is marked; one of the secret whole is kept as it is.
- **`holon verify` says what it took from the log, and holds it to itself.** A step fed a secret
  can't be computed without it, so its record is taken. The check now computes on the marks the
  steps that only move a secret (into a text, a list, a record) or compare it whole
  (`secure_equals`), and names each one it did take, by its leaf and what it was fed:
  "hmac_sha256 fed the secret api_token v1 #4fb07bc6: 1 node in 1 run: 0, as recorded". What it
  takes must be the leaf's: as many outputs as the leaf has, each of its type, a `sha256` or an
  `hmac_sha256` of 32 bytes in the node's encoding (or the one mark of a value computed from a
  secret), and one record for one leaf fed the same inputs across the export. A line that says
  other differs.
- **A run's entry commits to each secret it was given.** A run's entry named a secret's value by
  a version and a 32-bit fingerprint only the server can make: nothing held the server to that
  value. Each value now has a salt of its own (drawn once, sealed in the secrets store, the same
  after a restart), and the entry carries a commitment to the value beside its list of secrets
  (`committed`: SHA-256 over the secret's owner, name and version, the salt and the value).
  `holon verify` holds an export to it: every mark of a secret in a line reads the fingerprint
  its list has, and one value has one commitment across the export. The export's format stays
  `holon-runs/1`, and a line logged before reads as it did.
- **A `#` in a flow's code no longer hangs what reads it.** The count that bounds how deep a
  text nests read nothing at a `#` outside a string or a comment (`a #b`, a private name) and
  read the same place again, forever: `holon check`, the language server as `#` was typed, an
  agent's `check` and the editor's tab hung. It reads a `#` as the word it begins, and a place it
  can't read past is refused, as Holon's fault to report, rather than read again.
- **A text nested past the bound is refused however it is written.** The count that refuses a
  text nested past 128 levels read some texts otherwise than the parser: `x as⏎any as⏎any…`, an
  `else if` chain whose statements end with `;`, a `//` comment ended by a lone `\r`, U+2028 or
  U+2029, a hashbang, Unicode's spaces, a string going on over `\r\n`, and a `/` after `}`, `of`,
  a type's `>`, TypeScript's `!` or a `++` begun on a line. Such a text got past it as deep as it
  liked and crashed `holon check`, the language server or an agent's `check`, or trapped the
  editor's tab. The count now reads a text as oxc's lexer does, and refuses a `/` TypeScript
  reads either way by what came before it: put one side in parentheses.
- **Checking, deploying and formatting a flow take time in proportion to it.** A function using
  one that uses one, twice each, with a secret through them, made `holon check`, a deploy and each
  run walk every path: twenty-four such levels took minutes, and anyone who may propose a deploy could
  hold the server's CPU with one. Now the walks that find where a secret goes stop after 65,536
  nodes for one question, and what they haven't followed is taken as computing with the secret
  (the approval asks for more, never less). Sums of sums as a collection's key no longer double
  the compile at each line. And `holon fmt` refuses a text whose calls, templates and functions
  written in place nest more than 20 in each other, a function counting twice (seven
  `xs.map((x) => String(…))` inside each other took half a minute to lay out): name a part with a
  `const`, as fmt says where.
- **The server keeps to its memory and its open files.** A run was held to its own `max_memory`,
  and nothing held the runs together: many at once, or a few parsing a megabyte or two of JSON
  each (a parse makes about 80 times its text, all in one step), took the server's memory until
  the system ended it. Now the server holds every run's memory together, with its caches, to
  `--max-memory` (`HOLON_MAX_MEMORY`; by default half the machine's memory, or three quarters of
  its container's): past it a run fails, saying so, and a request is answered `503` with
  `Retry-After: 1` until the runs let some go. `JSON.parse`, `csv_parse` and `xml_parse` stop as
  they pass the run's memory or the server's, not once they are done. Connections are held to
  4,096 in all (fewer where the open-file limit leaves less room; on Linux and macOS the server
  raises its own limit as it starts), an open WebSocket's among them, and a connection the server
  can't take is tried again a little later rather than at once, over and over.
- **A deployment answers at most its share of runs at once, and a caller that stops reading is
  let go.** Every deployment's runs share the data plane's 512 threads, so one flooded with
  requests, or whose runs wait on a slow upstream, held the others' requests queued with no
  answer; and a caller that stopped reading a big file held a thread for good. Now a deployment
  runs at most `max_concurrent_runs` at once (64 by default, at most 448, the data plane's total
  for runs): past it, or past the total, a request is answered `503` with `Retry-After: 1` at once,
  and a WebSocket's message waits its turn. A write a caller doesn't take within 30 s closes its
  connection, whatever the answer (whole, a file, a streamed reply, a subscription), and a file is
  sent without a thread waiting on its caller. A flow that answers more than 64 slow requests at
  once asks for more: `flow("api", { port: 8080, limits: { max_concurrent_runs: 200 } })`.

- **A step is a unit of work, and a run's time is read as it computes.** A step was a node, or an
  item a function is applied to, whatever its leaf did, so a leaf that scans what it is handed,
  called for each item of a list, was the square of its work in a count of steps:
  `ids.filter(x => ids.indexOf(x) === ids.lastIndexOf(x))` over 100,000 ids held a server's
  thread for 26 seconds within 10 million steps, and the run's time budget, read only after a
  round that called out, never stopped it. Now a leaf also takes a step for each 32 items or fields
  and each 128 bytes of what it reads and makes, at every level of it (a list scanned for a value,
  each item against the value; `sort` n log n times; a number written as text, a step; a key a
  signature is checked with, and a compressor's set-up, what they take), so a step is of the order
  of a node's work whatever the node does: no pure leaf's step takes 20 times what a node of two
  numbers does, about 2 µs. A leaf that answers an item, a field or a length reads only that, and
  one that adds to a value it alone holds (`append` or `+` in a `reduce`, `insert` into a record)
  takes the steps of what it adds. And the time budget (`run_budget_ms`) is read as a round steps,
  so a run that only computes stops at it too, failing with `the run took longer than …` and
  performing nothing. A run that builds or scans large values takes more steps than it did, and
  one that took near its `max_steps` may need more (`limits: { max_steps: n }`). Leaves that copied
  what they then dropped copy only what they answer (`with_item`, `fill`, `to_spliced`,
  `with_field`, `insert`, `remove`, `merge`; `flat` and `copy_within` check their params first), and
  `json_stringify` of control characters, `uri_encode` and `replace` with a long replacement are
  many times faster.

- **The It may strip is short pills.** Above the canvas, each thing a flow may do is an icon, a
  small second one for what is done to it (written, read and written, sent on, a run on each
  one that comes) and its name: `↗ api.example.com`, `orders`, a key and `stripe_key`. Every kind
  has an icon of its own. The whole sentence, as the approval says it, shows at once in a tip
  when the pointer is on a pill or the keyboard's focus is, a line for each part of it, and is
  what a screen reader hears. A thing both read and written is one pill.
- **A secret's lines stand out wherever what a flow may do is listed.** In the editor, each line
  of a secret (reading one, where it is sent, where what is answered to it or made with it goes,
  a route that answers with one) has a 🔑 before it and is in the secret's colour: in the It may
  strip, the deploy question, a flow's status and settings, the Inspector, the Library and the
  host's holdings. The canvas's marks on the nodes a secret reaches use the same colour. The It
  may strip now says where the draft sends each secret, as the approval will, before anything is
  sent to a server.
- **The approval says where a hash, a signature or a host's answer made with a secret goes.** A
  route answering `hmac_sha256(key, req.query.msg)` signs whatever its caller sends, and one
  answering the token a host gave for a secret hands out a live token; both are clean (a flow
  must sign what it sends, and use what it is answered), and until now `holon check`, the editor
  and the approval said only `read the secret "key"`. Each is now followed from where it is made
  to where it leaves, and said beside the secret's read: "…; a hash or signature made with it
  goes to its callers", "… goes to https://api.example.com", "…; what is answered to it goes to
  its callers". `holon check` names the route under "with its secrets:", and the canvas marks
  the node. Nothing new is refused or withheld. A route that only says whether a signature
  matches says nothing, and a response's `status` alone isn't counted. Flows that sign a request
  or answer with what an API answered read one clause longer; the **sign-payload** sample shows
  it.
- **`holon verify` exits 3 when a run's state couldn't be checked.** A state not checked (a
  resumed part with nothing to hold it to, a commit whose numbering can't be followed, a run with
  no run before it in an export that begins mid-log) ended `ok`, exit 0, as if checked; a commit's
  cell or number changed, with no witness, could make verify check less and still end 0. Now it
  ends `INCOMPLETE` and exits 3, as an export with a run not run again does. The log's first run,
  which began from nothing, is checked in an export from the first line, so a whole log can still
  end 0. ([Checking runs](docs/manual/guides/checking-runs.md#the-state-run-to-run))
- **`holon verify` holds each run's line to the run, and says what only the chain holds.** A line
  changed and the lines after it chained again (by someone holding the server's data, with no
  witness) passed wherever verify read the line rather than making it again: an error and a
  status put in, a step's outputs or rounds, what a run answered, the points it waits at, the
  state it left, what a resumed run was delivered. Now every line must be one the server writes
  (`run 7 ISN'T A LINE THE SERVER WRITES: …`, exit 1), and verify makes each of those again
  from the run: its steps round by round, its answer as the line keeps it, where it waits, the
  state it left (a resumed part's and a run's beside others' too: the state the commit before
  its own left, its writes applied), and an approval's decisions, replayed by the server's own
  rules against the steps the run asked for and whoever started it. What no run again can make
  (when it ran and for how long, its session, its commit's cell and number, its tries, who
  called it, its grant) is the server's account, held by the chain alone: without `--head`, the
  report says so, and the guide lists each. A run whose snapshot the store couldn't take names
  the state it left by its hash all the same. ([Checking runs](docs/manual/guides/checking-runs.md#what-only-the-chain-holds))
- **A run's record keeps out what a secret chose to ask by.** In a run whose log can't mark
  what it computed from a secret, a read by one (a collection's key, a file's name computed from
  it) keeps which collection, folder or database it read and withholds the rest of what it asked
  and found, but a database's rows; a header named by one withholds the request's headers whole;
  and `sort` and `unique` of a list holding a secret give something computed from it. Which way
  a choice by a secret went (`key.startsWith("live_") ? a : b`) shows in the run's steps, as any
  choice's does: whoever approves a flow reads what it chooses by. In any run, a key a secret chose is withheld where the log lists what the run claimed and
  in the 503 a run kept waiting on it answers, and a read that failed by one says only that it
  failed. A route that reads
  its flow's own data by a secret asks `SecretAnswer`, as whether it found, waited or failed tells
  its caller something of it; a deployed flow that does waits for it to be granted. A database's
  answer to a secret stays the database's, its failing included, as its approval says.
- **A replay is held to what the deployment's runs are granted, and as a whole.** Each replayed
  run gets the run time and the count of requests a run of its kind is granted, where it had an
  hour and any number; a run that sends or reads what its run didn't stops there; a replay runs for
  30 seconds at most, counting the runs it didn't reach (`not_reached`: "12 not reached in time" in
  `holon replay`, the agent's tool and the editor); and the server runs two at a time, a third
  answered 503. A replay needs no approval and anyone who may read a deployment starts one, an
  agent too: one could hold a thread of the control plane for hours.
- **One mail stops no mailbox.** A message is read in time its size bounds, whatever it holds:
  an encoded word in a subject, a name or a file name (`=?utf-8?q?…?=`) is read in one pass,
  where one of many `=?` that didn't close read on to the end of the header for each (a 5 MB
  subject took minutes), and a message's first 1,000 parts are read, the rest left out. Each
  mailbox is looked at on a thread of its own, which the watcher doesn't wait for: a server that
  doesn't answer, or a long message or run, holds that mailbox's next look and no other's, where
  every mailbox's look waited for the slowest. And a message is noted taken up before it is read: a look cut off
  before its run is over (a restart, a failure reading it) tries it once more, then moves it
  without a run, on the timeline, where before it was read again at every look and after every
  restart.
- **Postgres requires TLS to another machine.** A connection that named no `sslmode` took
  libpq's `prefer`, so anyone on the path could answer "no TLS" and read it, a cleartext password
  too. It is now required unless the database is on the same machine or the URL says otherwise;
  a server without TLS is refused saying so, and that `?sslmode=disable` is the owner's to write.
- **A value inside a MongoDB `$expr` is data.** A text starting with `$` there names a field:
  `{ $expr: { $eq: ["$owner", who] } }` with `who` = `"$owner"` matched every document. Each value
  computed into an expression goes in as `$literal`.
- **A cloud's metadata service is never reached.** With private addresses granted, a flow could
  call 169.254.169.254, where a cloud hands out the machine's credentials. It, the rest of
  169.254.0.0/16, `100.100.100.200` and `fd00:ec2::254` are refused however written (mapped,
  6to4, NAT64, and the forms that hide an IPv4 address).
- **A redirect to another host carries none of the flow's headers.** It carried `content-type`,
  `accept`, `accept-language` and `user-agent`, and a secret a flow put in one of them went to a
  host its approval doesn't name. No body goes there either, so none says what one is.
- **A header no request can carry is named, never echoed.** A token with a stray line break or
  control character (a `VAULT_TOKEN`, an SQS session token, a header a flow computed from a
  secret) came back whole in the error, which reaches a run's record, the timeline, the live status
  and the console: ureq's message is the header's whole line. Every failed call is now told one
  way (`outbound::said`), naming no header's value. A Vault that answers with a redirect isn't
  followed, as its token went along, and a plain `http://` `VAULT_ADDR` to another machine (`HTTP://`
  too) is said once on the server's output, naming its host and never a name or password before an
  `@`.
- **A slow clock tick holds its own clock alone.** The clock loop waited for every tick it started
  before the next pass, and looked at waiting runs' deadlines after it on the same thread: one tick
  waiting 30 s on a call held every deployment's clocks, and every deadline, as long. Each tick now
  goes on by itself (a clock isn't fired again over its own tick), and the deadlines have a thread
  of their own.
- **Where a resource is goes only to whoever may bind it, and a flow's answers are sent
  `nosniff`.** A feed's, a queue's and a mail server's address, and the sources their URLs, headers
  and logins are read from, were shown to anyone who could view resources, and a flow's timeline
  and its feeds' and queues' listings showed them to anyone who could view the flow. Now they go
  only to whoever may bind (`resource.bind`), as a folder's path and a database's address already
  did: anyone else is told what each does, and a connection's error names the binding where it
  named the address. Deleting a deployment says where its records are kept in full to whoever
  views the host, and within the data folder to anyone else, and so does where the run logs'
  witness is (`GET /summary`'s `witness.address`). `GET /summary`'s last check and `POST
  /summary/check` name only the deployments whose runs their caller may view; they named every
  deployment and its head. Every answer of a flow's port carries `X-Content-Type-Options: nosniff`
  (a flow's own kept). The timeline says `listens to the feed prices`, without its address.
- **`/health` says when the server can't keep what it is asked to, and a stop has a deadline.**
  `GET /health` answers 503 `not ok: ` and what fails while the deployments file can't be
  written or the run logs can't be synced to disk (before, it said `ok` throughout, and a deploy
  answered while the file couldn't be written was lost on the next start). Each, and a witness
  that doesn't take the runs' heads, is an admin's warning with why and what to do (`holon host`,
  the admin page, `GET /admin/warnings`), where before each was one line on stderr. A stop waits
  for the runs going on up to `--stop-within` (default `1m`; `HOLON_STOP_WITHIN`), then syncs
  what is written and exits with status 1, the runs it cut off taken as a crash's by the next
  start; before, one long run held it until a service manager killed it, unsynced. A new guide,
  [running it as a service](docs/manual/guides/running-as-a-service.md): systemd, containers and
  Windows, where a service's root key goes, and how long each should wait for a stop.
- **`holon fmt` formats the texts it refused.** An ordinary text was refused as "a mistake in fmt":
  `((1 + 1) + 1)`, whose parentheses Prettier takes out, where in a flow they break a chain (two
  `add` nodes, where `1 + 1 + 1` is one); `a && (b && c)` and `a ?? (b ?? c)`, which Prettier
  nests the other way; and `0.10`, `1.50`, `.5` or `1E3`, which Prettier rewrites, where a number
  lowered as it was written. fmt now keeps the parentheses around a side that holds the same
  operator (not in a `js()` body, JavaScript, which it lays out as Prettier does), and a number is
  its value however it is spelled: `0.10` and `0.1` are one unit, `1e3`, `1_000` and `0x3e8`
  another, and `({ 1e3: "a" })[1000]` is `"a"`, as in JavaScript. A flow that spelled a number
  otherwise than JavaScript does (`1e3`, `0.50`, `1_000`) deploys as a new version.
- **Access changes that wait for people are decided in the editor and from the command line.**
  The editor's Approvals has **Access changes**: each that waits, what it changes and gives, who
  proposed it and why, who could act as whom it gives more, and who approved it so far, with
  **Approve…** and **Decline…** to whoever may (its author withdraws it), and those decided lately;
  People and access › History lists those that wait above the versions, and a binding, a role or a
  group changed in People and access that waits says so, linking to Access changes, where before
  the form failed reading the binding it expected. The `approved-payout` sample walks through
  binding its roles in the editor's form, and a binding that waits for another admin. `holon access proposals` lists them, `holon access approve <id>` and
  `holon access decline <id> --reason TEXT` decide one. `holon mcp` says an agent never approves
  one.
- **No one takes over someone whose access waits for people to be given.** While `--changes` is
  on, a way in to someone else who decides runs or holds anything of an admin's isn't made from
  another account: resetting their password, setting their email (a provider matches people by
  it), making a token of their agent or service (but by its owner), giving it another owner, or
  setting the identity provider. Whoever did could sign in as them, and count as a second person
  where people are counted, so it is refused (403), saying why. They do it themselves, or it is
  done on the server's machine through its run file, which holds the data folder anyway: the
  machine is the root here, and no account is. Or what they hold is taken away first, at once,
  and given back after, which waits. A proposal now says whom it gives more (`to`) and who could
  act as them (`ways_in`: "ada made bob's reset link"): none of those approves it, and a way in
  made to one of them while it waits makes it stale. `GET /people` says each one's `ways_in`.
- **An access change that gives more can wait for people: `holon access approvals --changes N`.**
  With `changes` set (1 to 5; off by default), a change that gives someone `run.decide` or
  anything of an admin's isn't made: binding a role that holds it, adding someone to a group bound
  to one, changing a role so that it holds it, setting someone's role on the host to one, and
  loosening the approval settings (`--exclude-deployer off`, `--agents on`, `--starter on`,
  `--changes` lower or off). It is answered 202 with a proposal saying what it changes and what it
  gives to whom, kept beside the access document (`access.proposals.json`), and listed at
  `GET /access/proposals`. N people other than its author approve it
  (`POST /access/proposals/<id>/approve`), each a person holding `people.assign` and what it
  gives, so two who hold less don't give more together; the last approval makes it as its author
  asked, its version's why naming who approved it, and one that no longer applies (its role
  removed since) is stale, saying why. `…/decline` takes a reason; its author withdraws it so.
  Taking away applies at once, as does a change that gives no one more, and a change to the
  approval settings through the server machine's run file: that is how a host whose approvers
  are gone gets its access back. A new principal with a role that would wait is refused (409),
  saying to give the role after. The commands and the editor's page for proposals come with the
  next step; the settings table shows `changes`, and the editor's version diff names it.
- **A step of an approval may let whoever started the run take it: `starter: true`.** Whoever
  started a run took no step of any of its approvals: right for a request check, as whoever asks
  for a payout never approves it, and wrong for an output review, where a person asks agents for
  a draft and is its natural reviewer. The rule is now the step's: `steps: [{ role: "drafter",
  kind: "agent" }, { role: "proofreader", starter: true }]` lets the one who started the run take
  the second step. Left out, they may not, as before. It is part of what the flow asks for
  (`Approval("shape/agent:drafter>person:proofreader^")`, said "…, who may be whoever started the
  run"), so letting the starter in, or no longer, is a changed row in the version's review. An
  agent never decides a run it started, whatever a step says (an agent's step can't say it).
  `holon access approvals --starter off` lets no step take whoever started a run, on this host:
  a version with such a step is refused at deploy, saying how to let it, and a run already
  waiting goes by it at once (`approvals.starter`, on by default). The deploy review sets the
  starter aside only from the steps that don't take them, and the editor's Runs waiting says of a
  step "whoever started it may take it". The `reviewed-draft` sample shows it: an agent drafts
  what you asked for, you approve it as it reads, and it is written to a folder. `holon.d.ts`
  types a decision's `note` as any JSON, so a flow reads the draft from `note.text`.
- **Deploying a flow whose approval no one here can decide asks again, naming it.** The deploy
  preview already marked such an approval **!** (a step no principal of its kind here may take, or
  too few, or none once whoever starts the run is set aside), but deploying went on as for any
  other row, and a flow could be deployed whose runs would wait at an approval nobody could
  decide. Now the editor's deploy dialog, and Approve and deploy on a version's review, ask again,
  naming each approval and why (`touches.ts` `askAgain`); the version deploys if you go on (the
  server doesn't refuse it: someone may be bound after), and its runs wait there until someone is.
  The review lists those approvals first, under *Can't be decided here*. It asks only when no run
  could be decided there: each approval row of the preview now says which of its runs would wait
  undecided (`waits`: `every run`, or `some runs`, those a principal starts who takes no step of
  them), and an approval whose only approver is the admin, as the approved-payout sample binds
  it, is marked but not asked about, since the runs its route starts can be decided.
- **The editor's buttons follow what the server would answer you.** A viewer saw **Pause**,
  **Revoke**, **run** and the rest as an admin did, and learned only on pressing one that it was
  refused; the host's pages were shown by the built-in role alone (an admin's, or People and
  access to whoever held `people.view`). Now each button that changes something asks the server's
  `can` for its flow (`GET /auth/me/can`, every action at once, once per flow) and, refused, is
  disabled, saying why in the server's words on hover (`cy may not do that: a viewer only reads`):
  deploying and approving a deploy, run, pause, resume and stop, removing a deployment and deleting
  a saved flow, declining a proposal, revoking and granting back, whether agents may propose,
  setting, removing and rolling back a secret (a flow's and the server's own), running an action,
  reading the policy again, binding and unbinding. Each host page is shown to whoever may view it:
  **Resources** to `resource.view`, **People and access** to `people.view`, **Policy** to
  `policy.view`, which a viewer holds, so a viewer reads the host's policy now. `/auth/me` answers
  `access_version`, which the editor reads every few seconds and on coming back to the window, and
  it asks again when that moves, when it changes something and when the server refuses it.
  Someone who may deploy a flow but not approve what a version asks is offered **Propose**: it
  waits in Approvals for someone who may, as the editor then says (it had sent an acknowledgement
  the server refuses from them, and asked nothing of a flow named just then). A server test writes what `can` answers five callers into the editor's fixture
  (`HOLON_BLESS=1`), and the editor's table of callers and buttons is held to it.
- **People and access binds roles, makes groups and roles, and says what each principal may do.**
  Binding a role on one flow, a tag or an approval point, defining a role, making a group, and
  reading what the access document's versions changed took the command line: the editor's People
  and access gave only a role on the host. Now it is in tabs, **People**, **Agents and tokens**,
  **Groups**, **Roles**, **Bindings**, **History** and **Signing in**. *Bindings* lists every role
  bound, each with **unbind** (which asks why), and binds one: to a person, an agent, a service or
  a group, on the host, a flow, a tag or one approval point (its flow, then one of the approvals
  its deployed version has), until a date or for a while (`7d`), and why; a refusal says the
  server's words (`editor holds flow.edit on "payout", which you don't: a binding gives only what
  its author holds where it is made`). A binding at an approval point alone lets its holder decide
  from Runs waiting but not open the flow, and the form says so and offers viewer on the flow too,
  as `holon bind` does. A name in these tables opens the principal's page (`#people.who.NAME`):
  their groups, tokens and bindings (and through which group), a form binding one more, and what
  it all lets them do, worked out as the server's `can` decides it (an end-to-end test compares
  the two for each way of being bound): on the host, on every flow, on the flows they saved, on
  each tag's flows, and on each flow what more, with the approvals they decide there. *Roles*
  defines and changes a role by ticking its actions, by resource, and who may hold it; *Groups*
  makes one, adds and takes out members, and binds a role to it; *History* lists each version,
  who made it, what and why, and a version's page what it changed from the one before. Someone
  who isn't an admin and holds `people.view` (a role of your own) has People and access too, and
  binds what their role lets them; anyone else keeps Your agents and services.
- **Runs waiting finds every run waiting for you, draws an approval in steps as its chain, and a
  run's page says each step's decision.** Someone bound only at an approval point decided it with
  `holon decide`, but the editor's Approvals, which read the runs of each flow it listed, showed
  them nothing: they may list no flow. Now `GET /runs/waiting` lists, across flows, the runs
  whoever is signed in may see or decide, as each flow's own listing gives them, and Runs waiting
  and the nav's count read it. An approval in steps is drawn as its chain, `✓ classifier (scout,
  invoice, 0.93) → ◻ reviewer → ◻ approver (not open yet)`: who took a step and what their note
  says in a word or two, the open steps, the steps not open yet dimmed; hovering over a step shows
  each decision on it (who, kind, role, when, why, and the binding that let them, with the access
  version `holon access show --version N` prints). Declining a step, or an agent's proposal, needs
  a reason: the button waits for one and says so, where an empty one used to close the dialog
  silently. The run's page, and the details pane of a run stepped through in the editor, say what
  the resumed part was given: the approval's decision, then each step's decision a line.
- **A person signs in to a server elsewhere from the command line: `holon login`.** On another
  machine a person's only way in was the editor; the commands took an agent's or a service's
  token. Now `holon login --server https://holon.example.com` asks their name and password (not
  shown as typed, or piped in), signs them in, and keeps the session for that server in
  `~/.holon/tokens.json`, readable only by them; it says who they are and until when, a week on.
  Every other command reaching that server signs in with it unless `HOLON_TOKEN` is set, and with
  no server running here they reach the one signed in to without `--server`, so `holon decide
  payouts` lists their steps and takes one as them. `holon logout` ends the session there and
  forgets it. A refused sign-in says what to do by where it came from: a session that ended,
  `holon login` again; `HOLON_TOKEN` not taken; none, how to sign in. A password goes over https
  only, as a token does. The sign-in answer says when the session ends (`expires_at`). An agent's
  token `holon mcp setup --server` keeps for a server elsewhere is now kept as `agent:<URL>`, apart
  from a person's session: set it up again. Signing in through a provider isn't in the command
  line yet.
- **Those who decide an approval see what they approve, and nothing else of the flow's runs.**
  An approval listed its steps and who had taken them, never what the run would do: a reviewer
  approved a payout without seeing its amount or payee. Now `await approval("payout", { shows:
  { amount, payee } })` names what it shows, any value the run has before it waits (the
  `approval` leaf's optional `shows` input); with no `shows`, it shows what the run was started
  with (an action's argument, a request without its headers, what a source delivered; nothing
  for a clock). Either is kept when the run parks as its log keeps it: a field tainted by a
  secret's mark redacted (`‹taint:card›`), a value a secret may have made withheld whole, one
  past 16 KiB its size instead, sealed on disk with the run. Runs waiting in the editor's
  Approvals and the flow's Waiting list, `holon decide` and `holon mcp`'s `runs` show it under
  the approval (`it shows {"amount":30,"payee":"acme"}`). Listing a flow's runs took `run.view`;
  now `run.decide` on its approvals lists the runs waiting at an approval the caller may decide,
  those points alone, with nothing delivered. The approved-payout sample shows the account, the
  amount and the balance it read.
- **A version runs and sends only what someone who may has looked at, and the deploy review says
  which approvals can't be decided here.**
  - **A flow that runs another flow's action is deployed only by someone who may run it.** A
    call runs the other flow's action under that flow's grant, with what the caller sends, and
    only the caller's grant was asked: an editor bound on the `marketing` tag deployed a flow of
    their own that drove `payments`' payout with any amount. Now deploying a version that runs
    another flow's action takes `run.call` on it (for an agent, the other flow letting agents run
    it too), whether the call is new or granted already: anyone else is refused (403, naming the
    action), an agent's version waits as a proposal for someone who may, and the review marks the
    row (`! flow payments: run payout — bob may not run it: someone who may deploys this version`).
  - **An agent's change to what a flow sends out waits for a person.** Only the secrets a version
    read were asked again: an agent's version posting whole CRM rows instead of a count to the
    same granted host deployed without anyone looking. Now each handler whose code an agent
    changed, and each fed by one (through a key, collection, table or folder one writes and the
    other reads, an action of its own it starts, a queue it handles, a folder it watches), asks a
    person again for what it sends out through (a host, a mail, a channel, a queue, another flow,
    a process, a folder or a database), and a route among them that reads a database or folder
    the host binds, for its answers. The review says `asked again` on those rows. What the host
    grants without asking isn't asked again, nor what a route answers from the flow's own state.
  - **The deploy review says when an approval can't be decided here.** A row for an approval
    whose steps can't all be taken is marked not ready, saying why: `approver: 1 person holds it
    here; it needs 2`, `classifier: no agent holds it here`, `its steps take 2 different
    principals, one a step, and fewer can here`, or `a run pat starts can't be decided` when the
    only one who may take a step may also start the run (whoever starts a run decides none of
    it). Holders are counted where the approval is as the version's tags would make it, not
    disabled, and without whoever deploys it on a host that keeps them out.
  - `holon deploy` and `holon editor` say when a deploy was kept as a proposal, rather than
    saying it deployed.
- **Signing in, setting up and reading units hold against what the launch review found.**
  - **A flood of sign-ins under ever new names no longer takes the server down.** Each waited its
    turn for a password check, up to 30 seconds, until the 512 requests the server answers at
    once were all sign-ins, and everything was answered 503. Now at most 64 wait, and 8 from one
    address (an IPv6 address by its /64), whatever the names; one more is told the server is
    busy at once. Sign-ins wait while it lasts, never the rest of what the server answers. An
    invite link is checked before its password is hashed, so links made up wait for nothing.
  - **Setting up is locked per address.** Five wrong setup codes locked setting up for everyone,
    so anyone could keep the first admin out a minute at a time; now only from the address that
    sent them. A `HOLON_SETUP_CODE` with lowercase letters never matched (what was entered was
    put in capitals, and it wasn't): it is entered in any case now.
  - **Signing out ends a provider's sign-in.** An ID token stayed good after signing out, until
    it expired. The server now keeps one signed out of refused until then (its hash, forgotten
    once it has expired); another sign-in of the person's at the provider still is one.
  - **The command line sends no token over plain http to another machine.** `--server
    holon.example.com` was reached over http, the token with it; a host given bare is reached
    over https now, and `http://` to another machine with a token to send is refused, saying to
    give its https address or reach it through an SSH tunnel. This machine stays plain http.
  - **What the run file makes before anyone is tied to it is owned by `local:<user>`.** It was
    owned by the OS user's bare name, so a person invited under that name owned the agents and
    services it made (and made their tokens). `local:` is no principal's name.
  - **`GET /unit/<hash>` reads only a flow you may view.** Anyone viewing one flow read any
    flow's code by its hash. Now a unit is answered to someone who views every flow, or one who
    views a deployment that places it; anyone else is answered 404, as for a hash nothing has.
- **A role that manages people no longer makes its holder an admin.** No one gave more than they
  held, but anyone holding `people.manage` changed anyone: an admin's email (and, setting the
  identity provider to one they ran, signed in as them), a service's owner (and then made its
  tokens), or disabled, demoted and removed every admin. Now whoever changes a principal
  (`PATCH` or `DELETE /people/<name>`, its email, owner, role, display or whether it is enabled,
  its password reset, making or revoking its tokens) holds all it holds, or is refused (403, `ada
  holds admin on this host, which holds flow.edit there, which you don't: you change, or act as,
  only someone who holds no more than you`); its owner still makes and revokes an agent's or a
  service's tokens. Taking is held to it as giving is: a binding is unbound, a member taken out
  of a bound group, and a defined role changed only by someone holding what it gives. Setting the
  identity provider takes holding what each person holds, since it says who each of them is.
  And a provider's sign-in is matched by the email it verified only the first time: from then on
  by its issuer and subject, so another subject given that email is refused, saying whose it is,
  until an admin sets that person's email again. A sweep of the route table holds every route
  that changes a principal or what it holds to the rule, with a case for each.
- **A flow's taint keys are a deploy setting, listed in the editor.** `taint: [...]` stays in the
  text's `flow(...)` line, beside `tags`, and is held in the flow's deploy settings and frozen on
  its server unit from there: the route nodes have no `taint` param any more. The editor's
  deployment settings list the keys under "keeps out of its records", the four every flow taints
  shown fixed: one is added by name, checked by core's rule as it is (one word, at most 128
  characters, not listed already, in either case), and removed with its ×; the text's `flow(...)`
  line is written again, as for the port. A key is listed once (`"number" is tainted once`). A
  version that drops a key is a weakening: a person's review says the change (`keeps out of its
  records: x-api-key number → x-api-key`), and an agent's version waits for a person as a
  proposal, `Untaint("number")`, worded "record the values of "number" again". Typing `tags` or
  `taint` in the text pane now reaches the flow's settings (`tags` typed there were dropped). The
  `card-vault` sample's code is unchanged, and its try-it says what its run shows: the key it is
  sent as the secret it is (`‹secret:merchant_key …›` wins over the taint mark), another key as
  `‹taint:x-merchant-key›`. Run live: deployed with its two keys and no taint param on any node,
  its run recorded with `‹taint:number›`; ada's review of a version dropping `number` said so, an
  agent's was kept as a proposal with the deployment's keys unchanged, and once ada deployed it
  the next card's number was recorded. In the editor, a key added to the list and removed with its
  × rewrote the `flow(...)` line, one typed in the text joined the list, and `NUMBER` beside
  `number` was refused as listed already.
- **The run file is the account that set the server up.** Commands on the server's machine
  (`holon decide`, `holon deploy`…) sign in through its run file as the person who set the server
  up, holding what is bound to them and admin on the host, so that person is one principal
  whichever way they sign in: they take a step bound to them with `holon decide`, count once in
  `people: 2`, and a run they start or a version they deploy from the command line is theirs,
  which the starter rule and `exclude_deployer` refuse them in the editor too. The access document
  keeps the tie (`local`), set at setup, which `holon access local --as NAME` (or
  `PUT /access/local`, `people.assign`) moves to another person, as a version; `holon access local`
  (`GET /access/local`) says it and whether it holds now, and `holon access show` prints it. Removing that account clears it; while it is disabled, the run file is the OS user's
  admin, `local:<user>`, and decides no approval. Each activity entry made through the run file
  says `via: "local:<user>"`, `/auth/me` answers `local`, and an agent the OS user made before
  setup (`holon mcp`'s) is the account's. A decision's `binding` is always an id: `"local"` goes.
  The `approved-payout` sample's approver is you, approving with `holon decide`.
- **Each decision keeps what let it.** A decision on an approval is kept, and given to the run,
  with `binding`, the id of the binding its decider took the step through (the step's role, or
  for a step with none what holds `run.decide` there), and `access`, the version of the access
  document it was read in. So the
  resumed part's log entry, the run's export and `holon verify` carry them, for an auditor to
  check offline against `holon access show N`. `holon.d.ts` types them on `Decided`. The
  `approved-payout` sample pays in three steps: an agent bound to `classifier` on the `payments`
  tag classifies the payout with a note, sent with it, then a person bound to `reviewer`, then one
  bound to `approver`. `holon roles --help` and the docs say the fixed rule as it stands (an agent
  takes only a step written for an agent), and the VS Code sidebar says a waiting approval's steps
  taken, which it had stopped saying when `people` left the listing.
- **An agent decides its step.** `holon mcp` has a `decide` tool: an agent approves, or declines
  with a reason, a step of an approval written for an agent, bound to its role where the flow is,
  with a `note` the run is given. For a person's step it decides nothing, and says who decides it
  and where. `runs` and `holon decide <slug>` list the steps yours to take, and `holon decide`
  takes `--slot N` and `--note JSON`, with an agent's token too. A caller bound to no role that
  decides there is told so ("no role claude holds on "payouts" holds run.decide").
- **The host's approval settings.** The access document keeps `approvals: { exclude_deployer,
  agents }`, `false` and `true` by default, set with `PUT /access/approvals` (`people.assign`) or
  `holon access approvals --exclude-deployer on|off --agents on|off --why …`, each change a
  version with who and why. With `exclude_deployer`, whoever deployed the version a run is on
  decides none of its approvals; with `agents` off, no agent decides one, a run already waiting
  included, and a deploy (and its preview, `refused`) whose approval has a step for an agent is
  refused. `holon access approvals` alone shows them, `holon access show` too, and a waiting
  run's `yours` follows them.
- **The server decides an approval by its steps.** Each step opens once every stage before it is
  filled, and takes a principal of its kind bound to its role where the approval is (on the host,
  the flow or the point; an admin holding everything doesn't hold `reviewer`); each principal
  decides one step, once, and never one of a run they started; one decline ends it. A decision
  names its step (`slot`), or takes the one open step its decider may fill: none is 403 saying
  what each open step needs, several is 409 asking which. It may carry a `note`, any JSON up to
  16 KiB, given to the run with it. An agent decides an agent's step through a role that allows
  `run.decide`. A deploy with steps is no longer refused. The decide route answers
  `{name, slot, slots, outcome}` and a waiting run's point lists `steps`, `said`, `slots` (each
  step: its role, kind, people, who approved it, whether it is open), `decisions` and `yours`,
  the open steps whoever asks may take, in place of `people` and `approved_by`. `holon decide`,
  the `runs` tool and the editor's Runs waiting say each step as it stands; the editor offers the
  steps that are yours, named when there are several, and says "you approved as reviewer".
- **An approval's permission names its steps.** The `approval` leaf keeps who decides it as one
  canonical text, its `steps` param (`person*2`: two people), in place of `people`, and its
  permission names them: `Approval("payout/person*2")`, which was `Approval("payout/2")`, so a
  deployment granted the old item asks for the new one. The language is unchanged:
  `{ people: 2 }` lowers to `person*2`, and prints back as it was written. The review says
  "approved by 2 people", the permission "wait for approval "payout" by 2 people, and continue
  later", and a waiting run's point lists its `steps`. This host decides an approval by people
  alone for now, and a deploy refuses any other steps, which only a hand-edited canvas can write
  so far. Approvals in steps are `docs/L4s/approval-steps-plan.md`. Run live: approved-payout was
  reviewed and deployed with `Approval("payout/person*2")`, and its payout was decided by two people.
- **An approval in steps, in the language.** `await approval("payout", { steps: [{ role:
  "classifier", kind: "agent" }, { role: "reviewer" }, { role: "approver", people: 2 }] })` names
  who takes each step by the role they hold where the approval is, each in turn; `order: "any"`
  takes them in any order, and a list inside `steps` is a stage taken in any order. A step with
  no role is anyone holding `run.decide`, a person; an agent's step names its role; a service
  never takes one; up to 5 steps. They lower to the canonical text the permission names,
  `Approval("payout/agent:classifier>person:reviewer>person:approver*2")`, so a version that
  changes them asks anew, and `holon fmt` writes one form for each. `holon.d.ts` types them
  (`ApprovalOptions`, `Step`), the editor's hover says each option, and a role an admin defines
  is held to the same name rule (letters, digits, `-` and `_`, up to 64), so every role can be
  named in one. This host doesn't decide one yet: a deploy with steps is refused, saying so.
  Run live: a flow in steps checked, and left as written by `holon fmt`; a one-line
  `order: "any"` written over lines; a service's step refused; the steps flow's review and deploy
  refused, saying why; the people form deployed; a role named `ops.lead` refused.
- **A run is given every decision on its approval.** The decision an approval delivers has
  `decisions` in place of `approvals`: every decision taken, a decline included, in the order they
  came, each `{slot, who, kind, role, at, approved, reason}`, the step it took (0, and no role,
  for `people`), who took it and as what. `by`, `declined_by` and `reason` stay. A waiting run's
  point lists the same records (`decisions`, which was `approved`), and the editor, `holon decide`
  and the `runs` tool count its approvals from them. `holon.d.ts` types it (`Decision.decisions`,
  `Decided`). Run live: approved-payout deployed; a payout's waiting point listed ada's approval
  with its slot and kind; approved by ada and bob, it paid, and its run was given both decisions,
  each with its slot and kind; a second, declined by bob with a reason, was given the decline
  among its decisions, and paid nothing.
- **What a run throws with a secret in it no longer reaches the log.** Why a node gave nothing (a
  `throw`'s message, a module's error) was the one text of a log entry the redactor never went
  over: `throw new Error("key is " + k)` wrote the secret into the log and the export in the
  clear, and `explain` printed it. Now it is redacted with the rest of the entry, and withheld
  (`‹withheld: computed from a secret›`) where the node computes with a secret, as its outputs
  are. So are a request's method, the keys a run claimed and an effect's sink, and the bytes of a
  file kept around a part of it a run read: when they hold a secret, the part isn't kept, and
  `holon verify` asks for the file.
- **A true or false computed from a secret is no longer clean.** A Boolean was clean whatever
  computed it, so `route.get("/b", req => k.startsWith(req.query.p))` answered anyone with no
  approval asked, and a secret could be read off a character at a time. Now it is derived from the
  secret, as what a secret chooses is: the value `c ? a : b`, `list[i]` or `rec[k]` gives when
  `c`, `i` or `k` comes from one. The log withholds it, as anything computed from a secret,
  answering it asks a person to approve "answer its callers with the secret" before the flow
  deploys, and a write it chooses is refused as a write of the secret is. The one-way leaves
  (`sha256`, `hmac_sha256`, `secure_equals`) are still clean: check a key with
  `secure_equals(given, key)`, not `given === key`.
- **`holon verify` no longer takes a line that says its run failed as one it can't check.** Any
  error on an entry made every difference from what its run does again "not run again" (exit 3,
  INCOMPLETE): a line given an error could change its outputs, what it did or its run id and pass
  as unchecked, not as changed. Now a failed run's entry may say less than the run does again
  (a limit stopped it), never other: the same run id, each output it keeps as computed, what it
  did and sent the start of what it does again, and the status it answered either that or a
  failure's. One that says other differs (exit 1). A panicked run's entry must say it did
  nothing, and the state check no longer takes the empty state it records as where it began.
- **A run's computing is bounded: `max_steps`.** A run's time budget was read only between its
  outbound calls, so one request whose handler computed and called nothing out
  (`range(10000).map(i => range(10000).reduce(…))`) held a server worker for as long as it took.
  Now each node a run evaluates, and each item a function is applied to, is a step, and a run
  that takes more than `max_steps` between two outbound calls (10 million by default, about a
  second or a few of computing) fails, answering 500 and performing nothing. It is a limit like
  the others: a flow asks for more with `limits: { max_steps: n }`, which a person approves, up
  to 200 million. Steps are counted, not timed, and `holon verify` runs a run again with up to
  200 million, so it finishes every run that finished; a policy may lower that ceiling, not raise
  it. The editor's design mode stops at the
  default too, rather than hang the tab.
- **A run's `js()` calls are bounded together, not only each one: `max_fuel`.** A `js()`
  function's fuel bounds one call, and a function mapped over a request's items is as many calls
  as items: each call within its 200 million instructions, `range(10000).map(score)` computed for
  minutes in one request. Now a run's calls between two of its outbound calls run on `max_fuel`
  instructions in all (4 billion by default, two to five seconds by what the code does; chess's
  costliest move burns 0.93 billion): each call gets its own fuel or what is left, the less, and a
  run that wants more fails, answering 500 and performing nothing. A call that fails after
  computing (a throw, a trap) is charged what it ran. It is a limit like `max_steps`: a flow asks
  for more with `limits: { max_fuel: n }`, which a person approves, up to 20 billion, which
  `holon verify` runs a run again with; a policy may lower that ceiling, not raise it. The editor's
  design mode stops at the default too, and a replay computing with a secret (twice, with two
  placeholders, outside the round's count) is held to twice a round's steps and fuel, where before
  nothing bounded it.
- **A replay computes no more than the deployment is granted.** Replaying a version on a
  deployment's recent runs needs no approval, runs it on up to 200 runs, and nothing bounded its
  computing but each `js()` call's fuel. Now each round is held to the deployment's own
  `max_steps` and `max_fuel`, not the higher ceilings `holon verify` runs a run again with; a
  version that asks for more has it once deployed, and replayed, a run that needs it says
  `it doesn't finish within what the deployment is granted`, with the limit it ran past.
- **A run's memory is bounded: `max_memory`.** Steps and fuel bound what a run computes, not
  what it keeps: a request copying a 64 KiB body into each of ten thousand items held gigabytes
  within its steps, and `range(40).reduce((s) => s + s, "x")` doubled a text past what the
  machine has. Either aborted the server, every deployment with it. Now the server counts the
  bytes each run makes as it makes them, and a run that comes to hold more than `max_memory`
  between two outbound calls (256 MiB by default) fails, answering 500 and performing nothing. It
  is a limit like `max_steps`: a flow asks for more with `limits: { max_memory: n }`, which a
  person approves, up to 4 GiB; a policy may lower that ceiling, not raise it. What a run holds
  isn't counted alike on every build, so `holon verify` reads a run stopped for it as not run
  again to its end (exit 3), not as changed. A `js()` call's own memory is still its `memory`.
- **No leaf makes a value past the most one may be, before it makes it.** A run's memory is
  looked at between steps, so one step must not make gigabytes: `+`, `concat` and `join` give
  nothing past 16,777,216 UTF-16 units (`join` with a long separator between many items made
  gigabytes from megabytes), `bytes_concat` past 64 MiB, `append`, `cons`, `extend`,
  `to_spliced` and `flat` past 1,048,576 items, `fill` past 64 MiB of copies of its value, and
  `split`, `json_parse`, `csv_parse`, `xml_parse` and `bytes_to_list` when they would read more
  than 1,048,576 values out of one text or bytes. `pad`, `gunzip` and a number's text (`js_string`)
  make what they make with less held on the way.
- **One request no longer stops every flow on the server from answering.** A short route (one
  that is unrecorded and reads only its request, the clock and random numbers) ran on the thread
  that read its request, which was also the one watching the network for every flow's port. A
  route that computed a while held it: a fold of 3 s kept another deployment's requests, and a
  413 on its own port, waiting 1.65 s, until it ended. Every run now goes to the server's pool of
  run threads, and a request beside 24 such runs is answered in milliseconds. A trivial route
  called one request at a time takes about 20–30 µs longer; under load it is about even, and
  ahead at 64 connections.

## 0.1.0-rc.35

- **Tags: a binding on a tag holds on every flow deployed with it.** A flow's text tags it,
  `flow("payout", { port: 8153, tags: ["payments"] })` (lowercase letters, digits and `-`, kept
  sorted, once each), and the tags are part of its unit, so a version that adds or drops one is a
  new version; an untagged flow's hash is as it was. `holon bind editor --to bob --tag payments`
  (`{"flows": {"tag": "payments"}}`) covers each flow while its deployed version has the tag:
  restoring an untagged version or deleting the flow takes it out of reach, and
  `GET /deployments` says each flow's tags. Each tag is an item of the manifest, `Tag("payments")`,
  which a person approves and the policy never grants; the review groups it under "Who may act on
  it" and says who the tag lets in (`joins payments: bob as editor, the group ops as viewer`). A
  deploy asks the caller to hold the action on the flow as it is and as the version would make it:
  bob, bound on `payments`, deploys a new flow tagged so, but doesn't tag one he holds nothing on
  nor untag one he holds by the tag. An agent's version adding a tag waits as a proposal. The
  `approved-payout` sample is tagged `payments`, and its try-it binds bob on the tag before he
  decides; run live: deployed with the tag, approved by ada; bob, bound on the tag only, decided
  its run and was refused a flow without it; an agent's version adding a tag was kept as a
  proposal.
- **Roles, groups and bindings from the command line, `holon mcp` and the People page.**
  `holon roles` lists the roles and defines or removes an admin's own, `holon groups` makes groups
  and adds or drops members, `holon bind` binds a role to a principal or a group on the host, on
  one flow (`--flow`) or at one approval point (`--approval FLOW:POINT`), until a date or for a
  while (`--until 2026-12-01`, `7d`), and `holon unbind` takes a binding away; `holon access`
  lists every version of who may do what, and `show` prints one whole, for an auditor.
  `holon people` lists what each principal holds where it showed a role, and takes a role of an
  admin's own as `--role`, as `POST` and `PATCH /people` now do. `holon mcp` has `whoami`: who
  the agent is signed in as, and what each of its bindings allows, where (`/auth/me` answers
  each binding's `allows`). The editor's People page offers each kind the roles it may hold, an
  admin's own included, and lists beside each principal what else holds for them; Approvals
  offers Approve and Decline wherever the server says the person may decide, so one bound at a
  flow or an approval point decides there, whatever their role on the host. The terminal editor
  reads the roles from the server too. No sample changes: this is who may call the control
  plane, not what a flow does.
- **An admin defines roles of their own, and groups.** A role (`PUT /roles/<name>`) allows the
  actions it names from the catalogue, to the kinds of principal it names; its author holds on
  the host everything it allows. A group (`PUT /groups/<name>`) is a set of principals, bound as
  one is (`{"who": {"group": "payments"}, …}`); its binding holds for each member whose kind the
  role allows. `GET /roles` and `GET /groups` list them, with how many bindings hold each, and
  neither is removed while bound (409). No one gives more than they hold, whichever way: binding,
  inviting or setting someone with a role, adding a member to a bound group, or resetting
  someone's password or making their tokens (an owner still makes its agent's or service's), so a
  role that manages people can't make an admin. Whatever role it holds, an agent never does what is a
  person's: approving a deploy, starting, stopping, deleting or restoring a flow, writing a
  secret, managing people, roles, resources or the host. `GET /auth/me/can?actions=…&flow=…`
  answers, for each action, `true` or the words its refusal would say, as the routes ask it; the
  editor's buttons will read it. `/auth/me` and `GET /people` answer a group's bindings too, with
  the group's name. No sample changes: this is who may call the control plane, not what a flow
  does.
- **A role can be bound at one flow, or at one approval point, and until a time.** An admin binds
  it with `POST /bindings` (`{who, role, scope, expires_at, why}`), and unbinds it with
  `DELETE /bindings/<id>`. Bound at a flow, it holds there and nowhere else, and the lists (flows,
  deployments, proposals, the overview, the activity) keep the flows its holder may view; a flow
  they saved that no one has deployed is theirs wherever they hold the action. Bound at an approval
  point, it decides that point only. A binding that has expired holds nothing, and stays listed
  until it is unbound. A binding gives only what its author holds where it is made. Each change is
  a version with who made it, what it changed and why: `GET /access/versions` lists them, and
  `GET /access?version=N` answers one. Every caller holding a role on the host may do what it did
  before; a person who proposes a version without approving it is told so in their name, not as an
  agent. No sample changes: this is who may call the control plane, not what a flow does.
- **The roles each principal holds are kept apart from the accounts, with every change to them.**
  A role is now bound to a principal on the server, in `access.json` beside `accounts.json`
  (`.holon-access.json` beside `.holon-accounts.json`), and `can` reads what is bound. Each change
  (setting up, inviting or adding with a role, changing one, removing a principal) is a new
  version, kept for good in `access.versions.jsonl` with who made it, when and what it changed; it
  holds no password or token. No one may do more or less than before. `GET /people` and
  `/auth/me` answer each principal's bindings beside its `role`. An accounts file from before
  keeps its principals but not their roles: each holds none, the server names them when it
  starts, and the OS user gives them roles again (`holon people set NAME --role ROLE`). No sample
  changes: this is who may call the control plane, not what a flow does.
- **An agent reads a flow's log lines only where the flow lets agents, whatever role it holds.**
  One holding the viewer role read them without `AgentReadsLogs`, as a person does; it is now
  refused until a person approves it, as one holding the agent role is. What the flow lets agents
  do is decided by the principal's kind, as the manual says, not by its role.
- **`holon verify` exits 3 when it couldn't run every run again, and runs a failed run again.**
  It exited 0 for an export some of whose runs it hadn't run again (a file whose text it wasn't
  given, a run that panicked), as if all of it checked; now it says `INCOMPLETE` and exits 3, and
  exits 0 only when every run was run again. A run that failed was never run again, so its line
  could name any state it left, and the run after it begin from that state unchecked: now what a
  failed run computed is checked, and the state it left, as any run's (not its status, which is
  its failure's). A request no route took, and a run turned away before it began, ran nothing:
  their lines must say so. A line that names no unit differs, where it was passed over as one
  logged before lines named theirs.
- **A panic holding a lock no longer stops what the lock guards until a restart.** A run that
  panics is a run that failed, but a panic while it held one of the server's locks (the
  deployments table, a log, a state, a vault, the policy, the content store's packs) poisoned
  it, and every run and request after it that took that lock failed too, until the server was
  started again. Now the next one takes the lock as the panic left it.
- **A run a crash cut off reads as cut off, not as the state changed outside holon.** A run's
  state lands before its line is written, so a crash between the two left the state ahead of the
  log, and `holon verify` reported the next run's state as not following from the one before. Now
  a stop leaves a mark, and a start without one (after a crash or a power loss) notes on each
  deployment whose state its last run didn't leave that a run may have been cut off, naming the
  state it loaded; `verify` leaves the next run unchecked where it began, if it began from that
  state. After a stop, such a state is still a break.
- **Ctrl+C finishes the runs going on before the server stops.** It ended the server where it
  stood: a run half done (its calls made, its line never written), and the last second of `lazy`
  runs and of the witness's queue unsynced. Now ctrl+c, SIGTERM or closing its console stops the
  server taking runs (its flows' requests and the editor's are answered 503, its clocks, folders,
  mailboxes, feeds and queues start none), lets the runs going on finish, each to its line,
  syncs the logs and the witness's queue, and exits. A second ctrl+c stops it at once.
- **A save that failed once no longer stops the next start.** A run kept the state it left by
  putting the values that differ from the state it began with, taking the others to be in the
  store. After a run whose save failed (a disk full for a moment), its state in place, the next
  run's head could name a value never put, and the server then refused to start: "its state
  can't be loaded from the store". Now each save puts what differs from the state last kept
  whole, so a head names only values the store has.
- **The witness's queue is synced, and has a bound.** The heads waiting for a witness were
  appended to `pending.jsonl` and never synced, and while the witness was down the queue grew by
  a head a run, in memory and on disk, without end. Now the queue file is synced with the logs,
  every second, and each receipt as it is kept; and past 10,000 heads waiting, each deployment's
  newest head of each kind stands for the ones before it, since it pins every run before it: the
  witness, back, is given fewer heads and loses no run. The batch being sent as the queue is cut
  is left as it is.
- **What the server writes by renaming it into place stays there after a power loss.** A state's
  head, a value in the store, the deployments file, a file a run wrote into a bound folder: each
  was synced and renamed in, but the folder holding its name wasn't synced, so a power loss could
  bring back the old one under a run already said done. Now the folder is synced after each
  rename (on Linux and macOS; NTFS journals a rename itself). `holon migrate` syncs the copy it
  makes before it removes the old files, where a power loss just after could have lost both.
- **A run whose log line couldn't be written reads as lost, not as the log changed.** A line the
  server couldn't write (a full disk) still moved the chain on, and its head went to the witness:
  the next line named one that wasn't in the file, so `holon verify` said the chain was BROKEN
  there, "changed, removed or inserted outside holon", the witness's head was NOT FOUND, and the
  run after it failed the state check. Now the chain stays on the last line written, its head goes
  to the witness only once it is in the file, and the next line says from which run on the lines
  before it were lost (`lost_since`): `verify` lists those runs as not in the file, the chain
  whole, and doesn't check where the run after them began. Lines lost just before a restart are
  said by nothing, as a crash's are. And as the server says each run it lost, `verify` now fails
  an export with runs missing that no line says were lost (`runs 5 to 7 MISSING`): runs removed
  and the lines after them chained again, which it passed before.
- **A log line cut short partway doesn't take the next with it.** A write to a flow's log that
  failed partway (a full disk) left part of its line, and the next line was written on to the end
  of it, unreadable: a run told it was on record had no record. Now the next line starts a line of
  its own, as after a crash, and the part is skipped when the log is read.
- **A run's writes land all or none, and a run whose state wasn't kept is told so.** A run whose
  writes were refused partway (a value a state can't hold) kept those before it, and a state the
  store couldn't keep (a full disk) was said only in the server's log, the run answered as done.
  Now none of the run's state writes land when one is refused, and a run that changed its state
  fails with "not every write landed" when the state, or the head that names it, can't be kept.
- **In SQLite, `$2` is the second value wherever it comes in the statement.** SQLite numbers
  parameters by where each first appears, so `UPDATE t SET v = $2 WHERE id = $1` set `v` to the
  id and looked for the row by the value: the wrong row changed, or none. Now each `$n` is bound
  by its name, on every engine alike.
- **A public key is never taken for a shared secret.** `jwt_verify` read a PEM key only when its
  text began with `-----BEGIN`: the same key after a byte-order mark, a blank line or a line of
  explanation, given as bytes, as a DER key, or as a JWK's JSON text was taken as an HMAC secret,
  so a token signed HS256 with the public key itself (which anyone may have) verified. Now a
  public key in any of those forms is that key or nothing, and a JWK or JWKS may be given as its
  JSON, as text or bytes.
- **The editor's HTML preview shows the page and nothing else.** A value shown as HTML (a run's
  output, a request's body: text from outside) ran its scripts in the preview's frame, and loaded
  whatever it named from any address: an image, a style sheet, a font, telling that address the
  value was looked at, and by whom. Now the frame runs no script and sends no form, and a content
  policy put before the page holds it to what it carries inline (`data:` images, sounds and fonts,
  inline styles). A value shown as an image or a sound is shown only when it is a `data:` URL of
  that kind.
- **A request a function makes is sent when the function is called, whatever reads its answer.**
  `const sent = await api.post(…)` in a function was sent only when the function's answer was
  needed, so `const got = await ask(x)` in a route, read only in a branch not taken, sent nothing,
  where a request bound in the route itself is sent. Now the request's answer leaves the function
  by an output of its own, `called`, which the route keeps as it keeps its own requests, and the
  function is entered for it. What the function returns, and what `ask(x)` reads, is unchanged.
  Flows with such a function get a new version (the `dexpaprika` and `stripe-to-slack` samples
  among them): redeploy them.
- **A call in a branch of a value bound under a condition is accepted, and held to it.** `if (d) {
  got = v ?? (await api.get(…)).body }`, the same after an early `return`, and `c ? ping(x) :
  undefined` in an `if` failed `holon check` with "`after` ties a write or a call to what it names:
  this statement makes none". Now the value is guarded by the statement's conditions, so the call
  is made only where they hold and the branch needs it. And with a call bound on the statement
  beside it, `if (d) { const got = ping(x) ?? ping(y) }`, the fallback was made where the `if`
  didn't hold, since the first call wasn't: it is held now too. Flows with a statement like that
  get a new version: redeploy them.
- **An `if` on a condition with no value runs neither branch.** It took its else, as `?:` does,
  and what followed a `return` in it ran: `if (req.json.amount > 1000) return status(403)` followed
  by `payout(…)` paid when `amount` was missing, or a text (`"5000"`). Now neither branch runs, a
  name either changes keeps the value it had, and if a branch can return, nothing after the `if`
  runs: the route answers 404, or what its `?? status(…)` says. What follows a `switch` that
  returns from a case is held so too, where its subject is absent. `?:` and `??` are unchanged.
  Where a missing value should count as false, say so with `?? false`: the trading-bot sample's
  `if (approved.expired)` now reads `approved.expired ?? false`, as an approval that came has no
  `expired`, and tidewheel's `/touch` and `/witness` do the same to answer 400 for a body without
  a touch or a hash. Flows with an `if` get a new version: redeploy them, and look at each `if` on
  a field a request may lack.
- **In a `try` block, nothing after a statement that failed runs.** A call bound after a failure
  was still made, and a function after it still entered, unless it read what failed: `const order
  = JSON.parse(req.body)` and then `await api.post(…)` sent the request for a body that wasn't
  JSON. Now what follows a failure in the block isn't run, as JavaScript skips it, so the requests
  of one try block go one after another, a round each. A function's returned effect, called under
  an `if` in a try block, is no longer lost (`return { put, … }`). And reading a folder's file
  (`docs.bytes(…)`) under a condition or after an early `return` no longer fails `holon check` with
  "`after` ties a write or a call to what it names". Flows with a try block of more than one
  statement get a new version: redeploy them.
- **A flow placed as a node is refused for any handler it has, by name.** Only routes were refused:
  an action's flow was told it "has routes", and a clock's, a file's, a mail's, a feed's or a
  queue's was placed with its period or its source lost.
- **`&&` and `||` stop at the operand that decides, as JavaScript's do.** Every operand was run:
  `ok && record(x)` wrote whatever `ok` was, and a request on the right of `||` was sent after a
  `true`. Now what follows the operand that decides isn't run, and `false && x` is `false`, `true
  || x` `true`, whatever `x` is; an operand reached that is absent still makes them absent. And a
  function called in a branch of an `if`'s condition (`if (c ? f() : undefined)`, `if (a && f())`)
  no longer fails `holon check` with "the wiring contains a cycle". Flows with a function on the
  right of `&&` or `||` get a new version: redeploy them.
- **A function that calls a function no longer loses what it writes.** A write, a put or a send
  made by a function was performed only when a handler called it. Called from another function
  that didn't return it by name, it was dropped without a word: the flow passed `holon check`, its
  approval listed the write, the run answered 200, and nothing was written. A function now passes
  on every effect it doesn't read, at any depth. And inside a `try`, a function called only for
  what it does performs the effect it returns first (`return { put, … }`): the block's test that
  the call arrived read that effect, and an effect that was read was no longer performed. A node
  that only tests an effect, and can't pass it on, now leaves it to be performed, in text and on
  the canvas alike. Flows that did either get a new version: redeploy them.
- **Who may do what is decided in one place.** Every route the control plane answers is a row of
  one table (`crates/holon/src/can.rs`), saying what it asks: nothing, that its caller is signed
  in, or an action on what its path names, as the catalogue of the access design writes it
  (`flow.approve-deploy`, `run.decide`, `host.check`…). The built-in roles (viewer, editor, admin,
  agent) are data made from that catalogue, and one function, `can`, answers whether a caller may.
  It also holds the rules no role decides: an agent replaces only a flow it saved, runs an action
  and reads its log lines only where the flow lets agents, and never approves or starts what it
  deploys; only a person decides an approval; a person makes the tokens of an agent or a service
  they own. Each role may do what it did before, refused in the same words. One change: a path no
  route has is answered 404 to anyone signed in, where a viewer's or an agent's write, and anything
  under `/admin/` but an admin's, was refused with 403. No sample changes: this is who may call
  the control plane, not what a flow does.
- **The registry, the shared cache and `/fire` are gone from the server.** `GET /registry`,
  `POST /registry/publish`, `POST /registry/fork`, `GET /cache/<key>`, `POST /cache/ingest` and
  `POST /fire` came before deployments: nothing in Holon called them (not the editor, the CLI or
  `holon mcp`), and what they kept lasted only until the server stopped. A request to one is now
  answered as any path no route has. `holon serve --emit-test-pair`, `--emit-test-replacement` and
  `--emit-test-sink`, which wrote units to try them with, are gone too.
- **Docs.** A new guide, [People, agents and services](docs/manual/guides/people.md): the first
  admin, a teammate, an agent of your own, a token for CI, taking access away, and who did what;
  the concepts page names the three kinds and their owners.
- **An owner who isn't an admin has a page of what they own.** People and access is an admin's;
  anyone else signed in now has **Your agents and services** (7) in its place: the agents and
  services they own, with their tokens, which they make (each shown once) and revoke there. An
  admin still adds one and names its owner. The server lists what the caller owns at
  `GET /auth/owned`.
- **`holon mcp` signs in as an agent, never as you, and `X-Holon-Role` is gone.** On the holon
  serve you run, `holon mcp` (or `holon mcp setup`, when the server is running) makes your agent the
  first time, `<your user>-agent` owned by you, through the run file, and keeps its token in
  `~/.holon/tokens.json`; `setup --agent NAME` picks another. What it saves, deploys, calls and asks
  is under that name ("by bma-agent", not "by bma (agent)"). On a server elsewhere it signs in with
  the agent's token `setup --server` kept, and a person's or a service's token is refused there and
  when it connects: a setup that kept one says to run `setup` again with an agent's. The header that
  made an editor or an admin an agent is ignored. A server is set up by its first person, so an
  agent made before then leaves the setup code as it was, and its token signs in meanwhile. No
  sample changes: this is who signs in, not what a flow does.
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
- **Only an admin writes a secret, even with no name in the path.** `POST` or `DELETE
  /deployments/<slug>/secrets/`, with nothing after the slash, got past the admin's gate and on
  to the secrets, where `{"from": "env:…"}` read the variable before its empty name was refused: an
  editor could learn whether a server variable or a vault entry was set, and whether it was short.
  Every write under a deployment's `/secrets/` is an admin's now, and a secret's name is checked
  before its source is read.
- **Changing a password no longer holds up every request, and guessing the current one is
  counted.** The current password was checked while the accounts were locked, which every request
  needs to say who it is: with sign-ins filling the checkers, one change could stall the server for
  up to thirty seconds. It is checked outside the lock now, and five wrong in a row refuse changing
  it for a minute, so a session someone else holds can't be used to guess it. In the tests, any
  password hashed or checked while an accounts lock is held now fails, whatever the path.
- **Signing in through an identity provider takes only keys that came over https, and checks
  whom a token was issued to.** The issuer, and every address its discovery document gives, must be
  https, or plain http to this machine; an `http://` issuer elsewhere is refused when it is set. A
  token naming a key the provider doesn't publish asked the provider again on every request when
  the provider didn't answer; now it is asked once a minute at most, answered or not. A token for
  several audiences must name this client as the party it was issued to (`azp`), and one that names
  another is refused.
- **A request body that stalls no longer holds the control plane.** A body was read for as long as
  the client took, holding one of the 512 places the server answers at once: 512 connections
  sending a long body slowly, or not at all, turned every other request away (503), signing in
  included. A body must arrive within 30 seconds now, as on a flow's port; past that the request is
  told 408 and its place goes back. (A body of 1 KiB or less never held a place.)
- **A flow can keep the values of its own keys out of the log: `flow("x", { taint: ["x-api-key", "number"] })`.**
  The values under those keys are replaced by `‹taint:key›` in everything the server keeps of the
  flow's runs, wherever a run has them, whatever started it: a request's header, query or path
  parameter or JSON field at any depth, a WebSocket's message, an action's input, what a call sends
  and what the far side answers, the run's own answer, a row it writes or reads. They are kept out
  of its log entry, its lines, its export, a replay, `holon verify`'s report, a channel's packs,
  and an unrecorded handler's last failure. The run computes with them and the caller is answered
  as always; no one, an admin included, can read them back. Every flow now does this for
  `authorization`, `proxy-authorization`, `cookie` and `set-cookie`: a bearer token, a Basic
  password, each cookie a caller sent and the one a far side set were logged as they were. An
  unrecorded handler's last failure was redacted of the deployment's secrets only, and is now
  redacted of its run's resume tokens and tainted values too. A value shorter than six characters
  is kept as it is. A new sample, `card-vault`, keeps a merchant's card under a token, the number
  never in the log.
- **A new version is replayed on every kind of run before it goes live.** A replay found the
  handler of a request, a tick, a file or an action, but not of a WebSocket's message, a mail, a
  feed's event or a queue's message: those runs were counted as answering differently whatever
  the version, and so was a request's run that waited and was resumed. `holon verify` and a
  replay now find a run's handler in one place, so each kind is run again alike.
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

- **A handler does what its text says: an action started, and a function that writes or calls
  out, are held to the `if` and the `return` they are written under.** Until now each ran on
  every path: `if (amount > 1000) return status(403)` followed by `payout(…)` answered 403 and
  started the payout. A function that writes, called in a branch (`c ? record(x) : undefined`,
  `kept ?? record(x)`, a `switch`'s case), now writes only when the branch is taken, and a
  `switch`'s default makes no call when its subject isn't there, as it already wrote nothing. A
  flow that has one of these compiles to another unit: **redeploy it** (`holon check` shows the
  same permissions). The rule a flow has of its own is unchanged, and now said in the reference
  where it surprises: a condition with no value doesn't hold, so a guard on a missing field lets
  the request through (`if (req.headers["x-admin"] !== key) return status(401)` with no header),
  and `a || b` has no value when either side has none. The same handler is now run as a flow and
  as JavaScript over every way a statement is reached, every kind of thing a handler does and
  requests with and without each field, and the two must do the same but for that rule.
- **A review shows the code that deploys.** A document carries its code as a canvas, which
  deploys, and as a text, which people read; an agent's proposal could carry a text that wasn't
  its canvas's, and the review, the deploy's diff, `holon flows`, `holon history`, the agent's own
  tools and the terminal editor showed it as it came. Each now shows a saved text only when it
  compiles to the unit the canvas deploys as, and the canvas printed otherwise.
- **SQL: forms that hid a table or a function from a flow's permissions are refused.** A name in
  SQLite's brackets holding a quote (`select 1 as ["], pan from cards -- "]` read `cards` unseen),
  a function called by a quoted name or as a schema's (`"query_to_xml"($1, …)`), `table x`, an
  update or a delete of several tables and `update ignore` (MySQL), `straight_join`'s table, a
  number run into a word (`1from t`) and Postgres's `U&"…"`. Each says why. What a statement
  touches is now checked against SQLite's own account of it, over some 17,000 generated
  statements.
- **One flow or one staged unit no longer takes the server, or its clocks, down.** A run that
  panics, whatever started it, is a run that failed, logged as one: a clock's used to end the
  thread that ticks every deployment's clocks and the deadlines of waiting runs, for good and
  with nothing in any log. A staged unit with an edge to port four billion is refused like any
  wiring that doesn't validate (it asked for 51 GiB first, which ended the server). A number
  that isn't finite, which only a `js` function makes (`1 / 0`), is written `Infinity` or `NaN`
  beside a text, where it panicked, and `toString(2)` of one gives nothing, where it asked for
  64 GiB.
- **A strict run whose record couldn't be written is told so, whichever run synced first.** With
  a write failing and runs side by side, a run could answer as recorded with no line on record.
- **A `js` function's fuel is its code's and its arguments' alone.** The first call in a process
  paid for translating the engine's functions (15.8M instructions against 12.1M after), so a run
  near its budget could pass on a server long up and fail when checked by `holon verify`. And a
  module's answer holding an effect is refused: a module computes, and only a leaf performs.
- **The trading-bot sample doesn't place an order nobody approved.** Its approval running out
  placed the order as approved by "someone"; it now leaves the order unplaced, and no longer
  waiting. The reference says what a wait gives when it runs out (`{ expired: true }`).

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
