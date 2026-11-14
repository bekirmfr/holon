// Policy's views (web-console-L4.md §9.3), beside HostPages.tsx's PolicyPage: the policy file as
// the server read it, with line numbers; and Check flows against it, which says which deployments
// run within it and checks each version waiting for a person, item by item (policyMeans.ts).

import type { DeploymentEntry, HostPolicy, Proposal } from "./server";
import { checkWaiting, versionName, type Verdict } from "./policyMeans";
import { describeItem } from "./holon";
import { format } from "./routes";
import { quiet } from "./hostStyles";

/** The policy file's text as read, a line number beside each line; a comment quiet, a section's
 *  heading strong. Null: there is no file, and the defaults hold. */
export function PolicyText({ text }: { text: string | null | undefined }) {
  if (text === undefined) return null;
  if (text === null) return <div style={quiet}>no holon.toml: this host runs on the defaults above</div>;
  const lines = text.replace(/\r\n/g, "\n").replace(/\n$/, "").split("\n");
  return (
    <pre className="hl-policy-text" aria-label="the policy file">
      {lines.map((line, i) => {
        const t = line.trimStart();
        const kind = t.startsWith("#") ? "hl-policy-text__comment" : t.startsWith("[") ? "hl-policy-text__section" : undefined;
        return (
          <div key={i} className="hl-policy-text__line">
            <span className="hl-policy-text__n" aria-hidden="true">
              {i + 1}
            </span>
            <span className={kind}>{line || " "}</span>
          </div>
        );
      })}
    </pre>
  );
}

/** One item of a waiting version's, as the check says it: what it asks for, and what the policy
 *  says to it. */
const said = (v: Verdict) => {
  try {
    return describeItem(v.item);
  } catch {
    return v.item;
  }
};

/** What checking every flow against the policy found: the deployments within it and those it
 *  holds off, then each waiting version's items against it as it is now. */
export function PolicyCheck({ policy, deployments, proposals }: { policy: HostPolicy; deployments: DeploymentEntry[]; proposals: Proposal[] }) {
  const held = deployments.filter((d) => d.attention);
  const waiting = checkWaiting(proposals, policy);
  return (
    <div className="hl-policy-check" aria-label="flows against the policy" role="region">
      <div>
        <span style={{ color: "var(--ok)" }}>
          {deployments.length - held.length} of {deployments.length} deployment{deployments.length === 1 ? "" : "s"}
        </span>{" "}
        run within it
        {held.length > 0 && (
          <>
            ; <span style={{ color: "var(--bad)" }}>{held.map((d) => d.slug).join(", ")}</span> can&apos;t run as granted (held off, below)
          </>
        )}
        .
      </div>
      {waiting.length === 0 ? (
        <div style={quiet}>no version waits for a person</div>
      ) : (
        waiting.map((w) => (
          <div key={w.id} className="hl-policy-check__version" aria-label={`check of ${versionName(w)}`}>
            <div>
              <a href={format({ kind: "approvals", tab: "deploys" })}>{versionName(w)}</a>, waiting for a person
              {w.verdicts.length === 0 && <span style={quiet}>: it asks for nothing new</span>}
            </div>
            {w.verdicts.map((v) => (
              <div key={v.item} className={`hl-policy-check__item hl-policy-check__item--${v.says}`}>
                <span className="hl-policy-check__sign">{v.says === "refused" ? "✕" : v.says === "auto" ? "✓" : "?"}</span>
                <span>
                  {versionName(w)} would {said(v)}, which {v.why}
                </span>
              </div>
            ))}
          </div>
        ))
      )}
    </div>
  );
}
