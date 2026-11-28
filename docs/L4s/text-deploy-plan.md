<!-- holon:dev-phase-only -->
*A dev-phase document: a plan for this repository's development, not for the launch repository.*

# A flow travels as its text (2026-11-28)

Branch `text-deploy` in `.claude/worktrees/text-deploy`, from origin/main 271866ee.
Owner's choice (2026-11-28): option 4, deploys send the text and the server compiles it.

## Plan lines it serves

- `docs/L4s/one-deploy-plan.md` says a deploy "sends the flow and nothing else that changes a
  grant", and §4.2 "a version keeps its unit and document". This changes "document" to "text"
  there; part 1 (settings on the deployment) is not built and is not touched here (`requirements`,
  `browsers`, `uses` stay as body fields until part 1).
- `docs/LANGUAGE.md` P4 "the canvas is a view of the same program", P5 "one implementation ...
  in the server": the server compiles with holon-author as everything else does. §8 "the flow
  document keeps positions" changes: positions are never kept; the compiler lays out (decision
  below).
- Nothing in any plan covers the size of what a deploy sends.

## The rule

A flow travels and is kept as its text. Whoever needs its units, canvas, trace or names compiles
the text (server natively, editor through wasm, every command natively), the same code. The
compiled description (`FlowDocument`) is never sent and never kept.

## The map, and what each place gets from the text instead

Server:
- `POST /deploy`, `POST /deploy/preview`: body `text` replaces `document` and `made_with`; the
  server compiles it (`agent::plan`), puts its units in the store and deploys that unit. `unit`
  may still be sent: then it must be the text's (the unit a person reviewed). A staged unit with
  no text still deploys (tests and hand-built units), keeping no text, as a document-less deploy
  does today.
- `document_is_the_unit`, `other_release`, `made_with`, `canvas_checked`: gone (the text is the
  unit by construction).
- Versions: `document_ref` -> `text_ref` (text value in the content store); `Deployment.document`
  -> `text`; `DeploymentRecord.document` -> `text`. Old `document`/`document_ref` fields are
  dropped on load (dev phase: an old version shows no text).
- `GET /deployments/{slug}/document[?unit|?version]` -> `GET /deployments/{slug}/text[...]`,
  answering `{text, unit}`.
- Restore: deploys the version's unit with its text, not compiled again (as today).
- Proposals: `propose` keeps `text` (once: the saved flow holds it, `Proposal.text`).
- Saved flows: `POST /flows` takes `text`; `FlowRecord.text`; `MAX_DOCUMENT_BYTES` (4 MiB of
  document) -> `MAX_TEXT_BYTES` (4 MiB of text). A text that doesn't compile can still be saved
  (a draft).
- Sweep: keeps `text_ref`.

Clients (Rust):
- `holon deploy`: sends text, stages no units (`/unit` stays for replay); its answer cap
  4 MiB -> the MCP's 64 MiB. `--manifest --json`: deployed text from `/text`.
- MCP deploy/save/flows/deployments/explain/restore diff: send text; read `/text`, compile locally
  where they need trace or names (`agent::plan`).
- `holon flows`, `holon save`, `holon history --version`, `holon explain`, `holon restore`: same.
- Terminal editor (`workbench`): `text()`, `saved_text()`, `run_view()`, stage/commit, save,
  approvals: text in, compile locally.
- `agent::text_of` / `says_its_canvas` / `deploys_as` / `canvas_units`: gone or reduced to
  "compile this text".

Editor (web) and wasm:
- `deployUnit`, `previewDeploy`, `saveFlow`, `getFlow`, `getDeployedDocument` ->
  text in and out (`getDeployedText`).
- Deploy button (`useDeployFlow`): sends the text: the typed text, or the canvas printed
  (`printHolon`) when it was drawn; a canvas the text can't spell can't be deployed or saved
  (the editor says what to change, as it does now).
- Every drawing (view mode, run page, review, approvals, flow page, versions' manifests, placing
  a saved flow as a node, marketplace listing, export) compiles the text in wasm
  (`compile_holon`).

## Decisions taken (defaults, under "one rule" and "dev phase")

1. Hand-placed positions are not kept: every canvas is laid out by the compiler from the text.
2. A canvas the text can't spell can't be saved on the server (stays in the open tab).
3. Old deployments' versions keep running; their text shows only after a redeploy.

## Why it was a document, and what that must keep (checked 2026-11-28)

- July (JOURNAL 2026-07-04): saved flows were made to keep a canvas "exactly as left" (positions,
  viewport), opaque to the server; deploying stayed the content-addressed unit path, untouched.
  The document rode along with a deploy only for view mode.
- October 24: the language came as a second surface over the same IR; documents gained `source`;
  agent deploys sent "the document the editor would" so a person sees the same canvas.
- November 11-21 (08f112dc, 7d4ddd00 R24/R25, 1744616b): what a person reads must be what runs, so
  the server checks the document builds the unit, records `canvas_checked` and `made_with`.
- No register row says it must be a document (the codebase review session, 2026-11-28). What the
  rows protect is that the person approves what runs. With the server compiling the text it keeps,
  that holds by construction.

What the change must keep, each with a test:

- R25: for every proposal, the text a reviewer is shown is the text compiled into the unit that
  deploys. The 11-mutation class test is rewritten for text, not dropped.
- R79: approving deploys exactly the proposed unit or refuses. Approval deploys the proposal's own
  stored unit and text, not compiled again; never compiled silently into another unit when the
  server's release changed between proposal and approval.
- R24: the preview compiles through the same entry as the deploy (`compiled`), one entry.
- Agent versions wait with a full diff, and MCP `replay`: both read the server-compiled unit.
- R36/R58 (nesting.rs): every server entry goes through the nesting count first; the server's
  compile is added to `no_way_of_writing_a_text_gets_it_past_the_count`'s list of entries; a panic
  in compile answers 400/500 and never takes the server down.
- R17 / audit F2 (one request exhausting the server), R122/R105 (control port accept path): the
  compile is bounded: who may call it (as deploy/preview are), how many at once, off the accept
  path; no change to the tiny_http copy beyond that.
- R11: the deploy body (its text) is never written whole to a log or record.
- R85/R87: compile errors answered are the compiler's words only.
- Two compilers, one unit: wasm/native hash parity (wasm-pack test); a drawing whose unit is not
  the version's says so.
- The compiler is changing on branch s2-2-catch (unmerged): no edits in holon-author/src/lang/ or
  graph.rs here; call the existing entries; unit hashes may move when it lands.
- Restore and unit-only deploys by hash stay.

## Revision (owner, 2026-11-28): keep hand-drawn canvases, text plus layout, compressed

The owner: "we want to keep the hand drawn canvas data. but the current size is not acceptable."
Measured on the workroom: document 8.0 MB (0.68 MB gzipped); text 0.62 MB (0.16 MB); positions of
all 11,631 boxes 0.32 MB (0.07 MB). So a flow travels and is kept as its text plus an optional
`layout` (node id -> position, only when the canvas was moved by hand); everything else is rebuilt
from the text. Checked with the codebase review session:

- Gzip bodies: one cap, `MAX_BODY_BYTES`, for the encoded and the decoded size; inflate stops at the
  cap while streaming (holon-core `gunzip_within`, its visibility widened: a small core change);
  decoded in the handler after `read_body`, nothing under vendor/tiny_http; only the routes that take
  a flow accept it; unknown encoding, bad gzip and past-the-cap each answer their own 4xx in fixed
  words; neither body logged whole. Class test: every such route x {plain, gzip, bad gzip, bomb,
  unknown encoding}.
- Layout is presentation: never in the unit; one text with two layouts deploys as one unit hash
  (tested). An agent's deploy carrying a layout waits like any agent version (owner rule: agent
  versions always wait).
- Ids move with the compiler (s2-2-catch renumbers): a layout is stored with the release that made
  its ids, and applied only when that release is the compiler's or every id matches; otherwise it is
  dropped whole and the editor says so ("positions from release X no longer fit; laid out again").
  Never a partial match.
- The layout is capped (entries, bytes), finite numbers only, ids of the compiler's form.
- One function applies a layout, used natively and through wasm; the parity test covers it.
- Run view and explain match a run's record to a drawing compiled from the text: when that drawing's
  unit isn't the run's, they say so, as the canvas does.
- Storage, backups, history and restore read the new shape; old versions keep no text (dev phase).
- Open, the owner's: a canvas the text can't spell. Proposed: it can't be saved, and the editor says
  which box stops it, at save time (text and canvas both ways expressible).

## Tests (class)

- Size: a generated flow whose old description was over 8 MiB deploys from the command line and
  is proposed by an agent (MCP), and reads back as the same text.
- Round trip: every sample deployed as text reads back as that text, and compiles to the unit
  that runs.
- Drawing: every editor view that draws a deployed or saved flow is built from the text alone
  (the server stub answers text only).

## Steps (one commit each)

1. Plan doc in the repo.
2. Server: text in deploy/preview/proposal/save, kept per version, `/text` route; old path removed.
3. Rust clients: `holon deploy`, MCP, other commands, terminal editor.
4. Editor and wasm.
5. Class tests, samples/e2e, manual and LANGUAGE.md §8.
6. Live: the workroom flow and an over-8 MB branch on a scratch server.
