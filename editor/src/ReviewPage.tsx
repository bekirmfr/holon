// A version reviewed before it is deployed (gaps-web.md G9, C4, C15), at `#approvals.<slug>.v<n>`:
// three columns. Left, what it may do, grouped as a person decides (needs your approval /
// unchanged / no longer), each pointing at its line. Middle, the version itself: its changes
// beside the version deployed (hunks, as a deploy's consent shows them), the whole flow, or its
// statement graph, the rows new since the version deployed dashed (StatementGraph.tsx). Right, the decision: who proposed or saved it, and why (an agent's proposal, with its
// note), what approving it does, the gate ("I read …"), Approve, keep it saved without deploying,
// or decline a proposal with a reason; and the queue, `[` and `]` for the one before and after.
// A proposal is reviewed as it was proposed (its own document, GET /proposals/<id>), whatever was
// saved since. A version of a deployed flow is replayed on its recent runs (G11, ReplayRuns.tsx).
// Under what it may do (web-console-L4.md §7.3): while it runs (its port, run budget, the runs it
// keeps, what it holds between runs), and the lines of the host's policy it relies on, each
// checked. A note the approver writes is kept with the grant, on the deploy's event (events.rs).

import { useEffect, useMemo, useRef, useState } from "react";
import type { Reviewable } from "./approvals";
import { useDecline } from "./ApprovalsPage";
import { codeHunk, touchHunks } from "./hunks";
import { holonGraph, type FlowGraph } from "./holonText";
import { ReplayRuns } from "./ReplayRuns";
import { Review } from "./Review";
import { StatementGraph } from "./StatementGraph";
import { mayGroups, policyRows, textLines, whileItRuns } from "./reviewMay";
import { touchLines } from "./touches";
import { follow, format, type Route } from "./routes";
import {
  deployUnit,
  getDeployedDocument,
  getFlow,
  getPermissions,
  getProposal,
  getVersions,
  previewDeploy,
  type DeployPreview,
  type DeploymentEntry,
  type FlowDocument,
  type FlowRecord,
  type HostPolicy,
  type Proposal,
  type VersionEntry,
} from "./server";
import { useKeys } from "./ui/Keys";
import { useCan } from "./useCan";

const when = (seconds: number) => new Date(seconds * 1000).toLocaleString();
const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

type Loaded = {
  saved: FlowRecord;
  /** The proposal it is, when one waits for it. */
  proposal: Proposal | null;
  text: string;
  /** The version deployed now, as text (null: a first deploy, or it can't be written). */
  deployedText: string | null;
  current: VersionEntry | null;
  /** The number this version would be. */
  version: number;
  hash: string;
  document: FlowDocument;
  preview: DeployPreview;
};

type View = "changes" | "whole" | "canvas";

export function ReviewPage({
  slug,
  version,
  queue,
  deployments,
  navigate,
  prepare,
  textOf,
  onKeep,
  onDeployed,
  onDeclined,
}: {
  slug: string;
  version: number;
  /** The versions waiting, in the Deploys tab's order. */
  queue: Reviewable[];
  deployments: DeploymentEntry[];
  navigate: (r: Route, opts?: { replace?: boolean }) => void;
  /** A saved document made ready to deploy (App): its server unit staged, its hash, and the
   *  document to keep with the deployment. Throws why it can't be. */
  prepare: (slug: string, doc: FlowDocument) => Promise<{ hash: string; document: FlowDocument }>;
  /** A document as its text. Throws when it can't be written. */
  textOf: (slug: string, doc: FlowDocument) => string;
  onKeep: (slug: string, savedAt: number) => void;
  onDeployed: (slug: string, version: number) => void;
  /** A proposal was declined: what waits is read again. */
  onDeclined: () => void;
}) {
  const entry = deployments.find((d) => d.slug === slug);
  const waiting = queue.find((q) => q.slug === slug);
  const [data, setData] = useState<Loaded | null>(null);
  const [error, setError] = useState("");
  const [view, setView] = useState<View>("changes");
  const [line, setLine] = useState<number | null>(null);
  const [read, setRead] = useState(false);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState("");
  // The host's policy, for the lines this version relies on: null while unread, or why it can't be.
  const [policy, setPolicy] = useState<HostPolicy | string | null>(null);
  useEffect(() => {
    getPermissions().then(setPolicy, (e: unknown) => setPolicy(e instanceof Error ? e.message : String(e)));
  }, []);
  // Deploying it asks flow.deploy of the flow, and flow.approve-deploy for what it newly asks for;
  // declining a proposal, flow.approve-deploy (can.rs, deploy.rs).
  const can = useCan(["flow.deploy", "flow.approve-deploy"], { flow: slug });

  // Read again when another version is saved or proposed, or one is deployed.
  const proposalId = waiting?.proposal?.id;
  const key = `${slug}:${waiting?.at ?? ""}:${proposalId ?? ""}:${entry?.unit ?? ""}`;
  useEffect(() => {
    let fresh = true;
    setData(null);
    setError("");
    setRead(false);
    (async () => {
      // A proposal is reviewed as proposed; anything else, as saved.
      const proposal = proposalId === undefined ? null : await getProposal(proposalId);
      const saved: FlowRecord =
        proposal?.document !== undefined
          ? { name: slug, document: proposal.document, saved_at_epoch_seconds: proposal.saved_at ?? Math.floor(proposal.at_ms / 1000), saved_by: proposal.by }
          : await getFlow(slug);
      const deployedDoc = entry ? await getDeployedDocument(slug) : null;
      const versions = entry ? await getVersions(slug) : [];
      const { hash, document } = await prepare(slug, saved.document);
      const preview = await previewDeploy(slug, hash, saved.document.server?.requirements);
      const written = (doc: FlowDocument) => {
        try {
          return textOf(slug, doc);
        } catch {
          return null;
        }
      };
      return {
        saved,
        proposal,
        text: written(saved.document) ?? "",
        deployedText: deployedDoc ? written(deployedDoc) : null,
        current: versions.find((v) => v.current) ?? null,
        version: Math.max(0, ...versions.map((v) => v.number)) + 1,
        hash,
        document,
        preview,
      };
    })().then(
      (d) => fresh && setData(d),
      (e: unknown) => fresh && setError(e instanceof Error ? e.message : String(e)),
    );
    return () => {
      fresh = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  // The address names the number it would be.
  useEffect(() => {
    if (data && data.version !== version) navigate({ kind: "review", slug, version: data.version }, { replace: true });
  }, [data, version, slug, navigate]);

  const at = queue.findIndex((q) => q.slug === slug);
  const step = (by: 1 | -1) => {
    if (queue.length === 0) return;
    const next = queue[(((at < 0 ? 0 : at + by) % queue.length) + queue.length) % queue.length];
    if (next.version !== null) navigate({ kind: "review", slug: next.slug, version: next.version });
  };
  // The queue, and the whole flow or the diff: the review's d, not the docs.
  useKeys("review", { "]": () => step(1), "[": () => step(-1), w: () => setView("whole"), d: () => setView("changes") });

  const lines = useMemo(() => textLines(data?.text ?? ""), [data]);
  const groups = useMemo(() => (data ? mayGroups(data.preview.touches, data.preview.settings, lines) : []), [data, lines]);
  const asked = data?.preview.needs_approval.length ?? 0;
  const newer = !entry || (data !== null && data.saved.saved_at_epoch_seconds > entry.deployed_at_epoch_seconds);

  const show = (n: number) => {
    setView("whole");
    setLine(n);
  };

  const approve = async () => {
    if (!data) return;
    setBusy(true);
    setError("");
    try {
      await deployUnit(slug, data.hash, data.preview.needs_approval, data.document, data.saved.document.server?.requirements, note);
      onDeployed(slug, data.version);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  /** On to the next that waits, or back to the list. */
  const next = () => {
    const rest = queue.filter((q) => q.slug !== slug && q.version !== null);
    navigate(rest.length > 0 ? { kind: "review", slug: rest[0].slug, version: rest[0].version! } : { kind: "approvals", tab: "deploys" });
  };
  const keep = () => {
    if (!data) return;
    onKeep(slug, data.saved.saved_at_epoch_seconds);
    next();
  };
  const decline = useDecline(() => {
    onDeclined();
    next();
  });
  const gone = groups.find((g) => g.key === "gone")?.rows ?? [];
  const requirements = data?.saved.document.server?.requirements;
  const hostPolicy = typeof policy === "object" ? policy : null;
  const running = useMemo(
    () =>
      data
        ? whileItRuns({ port: data.saved.document.server?.port ?? entry?.port?.toString() ?? null, touches: data.preview.touches, limits: requirements?.limits, defaults: hostPolicy?.defaults })
        : [],
    [data, entry, requirements, hostPolicy],
  );
  const relies = data
    ? policyRows({
        touches: data.preview.touches,
        items: data.preview.items,
        limits: requirements?.limits,
        privateAddresses: requirements?.private_addresses,
        policy: hostPolicy,
        agent: data.proposal?.agent ?? false,
        may: can.may(asked > 0 ? "flow.approve-deploy" : "flow.deploy"),
        approving: asked > 0,
      })
    : [];
  const editor: Route = { kind: "editor", slug, view: false, seq: null };

  return (
    <div className="hl-flow hl-review-page">
      <header className="hl-flow__head">
        <h1 className="hl-flow__name">
          Review {slug} v{data?.version ?? version}
        </h1>
        <span className="hl-flow__status">{entry ? "a new version" : "its first deploy"}</span>
        <span className="hl-flow__spacer" />
        <a className="hl-flow__button" href={format(editor)} onClick={follow(editor, navigate)} title="its saved version, in the editor">
          Open in the editor
        </a>
        {queue.length > 0 && (
          <span className="hl-review-page__queue" aria-label="queue">
            <button className="hl-flow__button" onClick={() => step(-1)} aria-label="the review before" title="[">
              ‹
            </button>
            <span>{at < 0 ? `${queue.length} waiting` : `${at + 1} of ${queue.length}`}</span>
            <button className="hl-flow__button" onClick={() => step(1)} aria-label="the review after" title="]">
              ›
            </button>
          </span>
        )}
      </header>
      {error && <div className="hl-flow__banner">{error}</div>}
      {!data ? (
        !error && <p className="hl-flow__none">reading {slug}…</p>
      ) : !newer ? (
        <p className="hl-flow__none">
          Nothing waits: {slug}'s saved version is the one deployed.{" "}
          <a href={format({ kind: "flow", slug, tab: "status" })} onClick={follow({ kind: "flow", slug, tab: "status" }, navigate)}>
            Its page
          </a>
        </p>
      ) : (
        <div className="hl-review-page__columns">
          <section className="hl-review-page__may" aria-label="what it may do">
            <h2>What it may do</h2>
            {groups.length === 0 && <p className="hl-flow__note">It only computes.</p>}
            {groups.map((g) => (
              <div key={g.key} className={`hl-review-page__group hl-review-page__group--${g.key}`} role="group" aria-label={g.title}>
                <h3>{g.title}</h3>
                <ul>
                  {g.rows.map((r, i) => (
                    <li key={i}>
                      <span className="hl-review-page__said">{r.line.text}</span>
                      {r.line.note && <span className="hl-flow__note"> — {r.line.note}</span>}
                      {r.anchor !== null && (
                        <button className="hl-review-page__anchor" onClick={() => show(r.anchor!)}>
                          line {r.anchor}
                        </button>
                      )}
                    </li>
                  ))}
                </ul>
              </div>
            ))}
            {data.preview.missing_secrets.length > 0 && <p className="hl-flow__note">Not set yet: {data.preview.missing_secrets.join(", ")}. Until they are, those nodes stay empty.</p>}
            {data.preview.withheld && data.preview.withheld.length > 0 && <p className="hl-flow__note">Left out, for a person to grant back: {data.preview.withheld.join(", ")}.</p>}
            <div className="hl-review-page__group" role="group" aria-label="while it runs">
              <h3>While it runs</h3>
              <dl className="hl-flow__facts">
                {running.map(([k, v]) => (
                  <div key={k}>
                    <dt>{k}</dt>
                    <dd>{v}</dd>
                  </div>
                ))}
              </dl>
            </div>
            <div className="hl-review-page__group" role="group" aria-label="host policy">
              <h3>Host policy</h3>
              {typeof policy === "string" && <p className="hl-flow__note">The host&apos;s policy couldn&apos;t be read: {policy}</p>}
              <ul className="hl-review-page__policy">
                {relies.map((r, i) => (
                  <li key={i} className={r.ok ? undefined : "hl-review-page__policy--no"}>
                    <span aria-hidden="true">{r.ok ? "✓" : "✗"}</span> {r.text}
                  </li>
                ))}
              </ul>
            </div>
          </section>

          <section className="hl-review-page__body" aria-label="review">
            <div className="hl-flow__bar">
              <h2>Review</h2>
              <div className="hl-seg" role="group" aria-label="show">
                {(
                  [
                    ["changes", "Changes"],
                    ["whole", "Whole flow"],
                    ["canvas", "Canvas"],
                  ] as [View, string][]
                ).map(([v, label]) => (
                  <button key={v} className="hl-seg__opt" aria-pressed={view === v} onClick={() => setView(v)}>
                    {label}
                  </button>
                ))}
              </div>
            </div>
            {view === "changes" ? (
              <Review hunks={[codeHunk(data.deployedText, data.text), ...touchHunks(touchLines(data.preview.touches, data.preview.settings))]} label="changes" />
            ) : view === "whole" ? (
              <Whole lines={lines} line={line} />
            ) : (
              <Canvas text={data.text} deployed={data.deployedText} version={data.current?.number ?? null} onLine={show} />
            )}
          </section>

          <aside className="hl-review-page__decide" aria-label="decision">
            <div className="hl-review-page__card" role="group" aria-label="proposal">
              <h2>Proposal</h2>
              {data.proposal && (
                <>
                  <p className="hl-review-page__why">
                    <b>Why:</b> {data.proposal.why || "no why was given"}
                  </p>
                  {data.proposal.note && <blockquote className="hl-review-page__note">{data.proposal.note}</blockquote>}
                </>
              )}
              <dl className="hl-flow__facts">
                <div>
                  <dt>{data.proposal ? "proposed" : "saved"}</dt>
                  <dd>
                    {data.proposal ? when(Math.floor(data.proposal.at_ms / 1000)) : when(data.saved.saved_at_epoch_seconds)}
                    {data.saved.saved_by ? ` by ${data.saved.saved_by}` : ""}
                  </dd>
                </div>
                <div>
                  <dt>deployed now</dt>
                  <dd>{data.current ? `v${data.current.number}, ${when(data.current.deployed_at)}${data.current.by ? ` by ${data.current.by}` : ""}` : "nothing: its first deploy"}</dd>
                </div>
              </dl>
            </div>
            <div className="hl-review-page__card" role="group" aria-label="if approved">
              <h2>If approved</h2>
              <ul className="hl-review-page__if">
                <li>
                  v{data.version} goes live{entry?.port ? ` on :${entry.port}` : ""}
                  {data.current ? `; v${data.current.number} stays a version you can go back to.` : "."}
                </li>
                <li>{asked > 0 ? `You grant ${plural(asked, "new permission")}; the deploy is kept on its history with your name.` : "It asks for nothing new; the deploy is kept on its history with your name."}</li>
                {gone.length > 0 && <li>It no longer may {gone.map((r) => r.line.text).join(", ")}.</li>}
              </ul>
            </div>
            {entry && <ReplayRuns slug={slug} unit={data.hash} navigate={navigate} />}
            {data.preview.refused ? (
              <div className="hl-flow__banner">This host can't grant it: {data.preview.refused}</div>
            ) : (
              <>
                <label className="hl-review-page__note-field">
                  A note, kept with the grant
                  <textarea value={note} onChange={(e) => setNote(e.target.value)} rows={2} placeholder="why you approve it, what you checked" />
                </label>
                <label className="hl-flow__check">
                  <input type="checkbox" checked={read} onChange={(e) => setRead(e.target.checked)} />
                  {asked > 0 ? `I read the ${asked} new permission${asked === 1 ? "" : "s"} and the change.` : "I read the change."}
                </label>
                <button className="hl-flow__button hl-flow__button--go" {...can.gate(asked > 0 ? "flow.approve-deploy" : "flow.deploy", { disabled: !read || busy })} onClick={() => void approve()}>
                  {busy ? "Deploying…" : asked > 0 ? "Approve and deploy" : entry ? "Update" : "Deploy"}
                </button>
              </>
            )}
            <button className="hl-flow__button" onClick={keep} title="it leaves the queue in this browser until it is saved again">
              Keep it saved, don't deploy
            </button>
            {data.proposal && (
              <button className="hl-flow__button hl-flow__button--bad" {...can.gate("flow.approve-deploy")} onClick={() => void decline({ ...data.proposal!, version: data.version })}>
                Decline with a reason…
              </button>
            )}
            {entry && <p className="hl-flow__note">Its state and sessions are kept.</p>}
          </aside>
        </div>
      )}
    </div>
  );
}

/** The whole text, numbered; the line a permission points at is marked and scrolled to. */
function Whole({ lines, line }: { lines: string[]; line: number | null }) {
  const marked = useRef<HTMLDivElement>(null);
  useEffect(() => {
    marked.current?.scrollIntoView({ block: "center" });
  }, [line]);
  return (
    <div className="hl-review-page__text" role="region" aria-label="whole flow">
      {lines.map((l, i) => (
        <div key={i} ref={i + 1 === line ? marked : undefined} className={`hl-review-page__line${i + 1 === line ? " hl-review-page__line--at" : ""}`} aria-current={i + 1 === line ? "true" : undefined}>
          <span className="hl-review-page__n">{i + 1}</span>
          <span>{l || " "}</span>
        </div>
      ))}
    </div>
  );
}

/** Its statement graph, the rows new since the version deployed dashed; a row picked shows its
 *  line in the whole flow. */
function Canvas({ text, deployed, version, onLine }: { text: string; deployed: string | null; version: number | null; onLine: (n: number) => void }) {
  const drawn = useMemo((): { graph: FlowGraph; against: FlowGraph | null } | string => {
    try {
      const graph = holonGraph(text);
      let against: FlowGraph | null = null;
      try {
        against = deployed === null ? null : holonGraph(deployed);
      } catch {
        // The version deployed has no graph: nothing is marked new.
      }
      return { graph, against };
    } catch (e) {
      return e instanceof Error ? e.message : String(e);
    }
  }, [text, deployed]);
  if (typeof drawn === "string") return <p className="hl-flow__none">It has no statement graph: {drawn}</p>;
  return (
    <>
      <p className="hl-flow__note">
        {drawn.against ? `Dashed: new since v${version ?? "the one deployed"}.` : deployed === null ? "Its first deploy: all of it is new." : ""} Click a row for its line.
      </p>
      <StatementGraph graph={drawn.graph} against={drawn.against} onPick={(r) => onLine(r.line)} />
    </>
  );
}
