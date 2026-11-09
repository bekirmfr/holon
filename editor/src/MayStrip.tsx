// The It may strip, above the canvas and the text while a flow is edited: what it may do, in
// words, each new item (to approve when it is deployed) marked, and what the deployed version may
// that this one no longer would. From the draft's manifest (wasm) and the deployed one's
// (`GET /deployments/<slug>/manifest`), compared in mayItems.ts.

import { describeItem } from "./holon";
import type { MayItem } from "./mayItems";

const SAID: Record<MayItem["change"], string> = { new: "new", same: "", gone: "no longer" };

export function MayStrip({ items, deployed }: { items: MayItem[] | null; deployed: boolean }) {
  if (items === null) return null;
  return (
    <div className="hl-may" role="region" aria-label="it may">
      <span className="hl-may__head">{deployed ? "It may" : "Deployed, it may"}</span>
      {items.length === 0 ? (
        <span className="hl-may__none">do nothing yet: add a route, a clock, a file handler or an action</span>
      ) : (
        items.map(({ item, change }) => (
          <span key={`${change} ${item}`} className={`hl-may__chip hl-may__chip--${change}`} title={item}>
            {describeItem(item)}
            {SAID[change] && <span className="hl-may__tag">{SAID[change]}</span>}
          </span>
        ))
      )}
    </div>
  );
}
