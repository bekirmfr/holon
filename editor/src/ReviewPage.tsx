// A version reviewed before it is deployed (gaps-web.md G9, C4, C15), at `#approvals.<slug>.v<n>`:
// three columns. Left, what it may do, grouped as a person decides (needs your approval /
// unchanged / no longer), each pointing at its line. Middle, the version itself: its changes
// beside the version deployed (hunks, as a deploy's consent shows them), the whole flow, or its
// canvas. Right, the decision: who proposed or saved it, and why (an agent's proposal, with its
// note), what approving it does, the gate ("I read …"), Approve, keep it saved without deploying,
// or decline a proposal with a reason; and the queue, `[` and `]` for the one before and after.
// A proposal is reviewed as it was proposed (its own document, GET /proposals/<id>), whatever was
// saved since. A version of a deployed flow is replayed on its recent runs (G11, ReplayRuns.tsx).

import { useEffect, useMemo, useRef, useState } from "react";
import type { Reviewable } from "./approvals";
import { useDecline } from "./ApprovalsPage";
import { codeHunk, touchHunks } from "./hunks";
import { ReplayRuns } from "./ReplayRuns";
import { Review } from "./Review";
import { mayGroups, textLines } from "./reviewMay";
import { touchLines } from "./touches";
import { follow, format, type Route } from "./routes";
import { deployUnit, getDeployedDocument, getFlow, getProposal, getVersions, previewDeploy, type DeployPreview, type DeploymentEntry, type FlowDocument, type FlowRecord, type Proposal, type VersionEntry } from "./server";
import { useKeys } from "./ui/Keys";

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
  useKeys({ "]": () => step(1), "[": () => step(-1) });

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
      await deployUnit(slug, data.hash, data.preview.needs_approval, data.document, data.saved.document.server?.requirements);
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

  return (
    <div className="hl-flow hl-review-page">
      <header className="hl-flow__head">
        <h1 className="hl-flow__name">
          Review {slug} v{data?.version ?? version}
        </h1>
        <span className="hl-flow__status">{entry ? "a new version" : "its first deploy"}</span>
        <span className="hl-flow__spacer" />
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
              <Canvas doc={data.saved.document} />
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
                <label className="hl-flow__check">
                  <input type="checkbox" checked={read} onChange={(e) => setRead(e.target.checked)} />
                  {asked > 0 ? `I read the ${asked} new permission${asked === 1 ? "" : "s"} and the change.` : "I read the change."}
                </label>
                <button className="hl-flow__button hl-flow__button--go" disabled={!read || busy} onClick={() => void approve()}>
                  {busy ? "Deploying…" : asked > 0 ? "Approve and deploy" : entry ? "Update" : "Deploy"}
                </button>
              </>
            )}
            <button className="hl-flow__button" onClick={keep} title="it leaves the queue in this browser until it is saved again">
              Keep it saved, don't deploy
            </button>
            {data.proposal && (
              <button className="hl-flow__button hl-flow__button--bad" onClick={() => void decline({ ...data.proposal!, version: data.version })}>
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

/** Its canvas as saved, drawn plain: each node where it was placed, each wire between them. */
function Canvas({ doc }: { doc: FlowDocument }) {
  type N = { id: string; position?: { x: number; y: number }; measured?: { width?: number; height?: number }; width?: number; height?: number; parentId?: string; data?: { name?: string; label?: string } };
  const nodes = (doc.nodes as N[]).filter((n) => n.position && !n.parentId);
  const edges = doc.edges as { id: string; source: string; target: string }[];
  if (nodes.length === 0) return <p className="hl-flow__none">No canvas was saved with it.</p>;
  const size = (n: N) => ({ w: n.measured?.width ?? n.width ?? 170, h: n.measured?.height ?? n.height ?? 48 });
  const box = new Map(nodes.map((n) => [n.id, { ...n.position!, ...size(n) }]));
  const xs = nodes.flatMap((n) => [box.get(n.id)!.x, box.get(n.id)!.x + box.get(n.id)!.w]);
  const ys = nodes.flatMap((n) => [box.get(n.id)!.y, box.get(n.id)!.y + box.get(n.id)!.h]);
  const [x0, y0] = [Math.min(...xs) - 20, Math.min(...ys) - 20];
  const [w, h] = [Math.max(...xs) - x0 + 20, Math.max(...ys) - y0 + 20];
  return (
    // Near the size it was drawn at (three quarters), scrolled when wider than the column.
    <div className="hl-review-page__canvas-box">
    <svg className="hl-review-page__canvas" viewBox={`${x0} ${y0} ${w} ${h}`} width={w * 0.75} height={h * 0.75} role="img" aria-label="canvas">
      {edges.map((e) => {
        const a = box.get(e.source);
        const b = box.get(e.target);
        if (!a || !b) return null;
        const [sx, sy, tx, ty] = [a.x + a.w, a.y + a.h / 2, b.x, b.y + b.h / 2];
        return <path key={e.id} d={`M${sx},${sy} C${sx + 60},${sy} ${tx - 60},${ty} ${tx},${ty}`} className="hl-review-page__wire" />;
      })}
      {nodes.map((n) => {
        const b = box.get(n.id)!;
        return (
          <g key={n.id}>
            <rect x={b.x} y={b.y} width={b.w} height={b.h} rx={5} className="hl-review-page__node" />
            <text x={b.x + 10} y={b.y + 20} className="hl-review-page__label">
              {n.data?.label ?? n.data?.name ?? n.id}
            </text>
          </g>
        );
      })}
    </svg>
    </div>
  );
}
