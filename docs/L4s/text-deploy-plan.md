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
