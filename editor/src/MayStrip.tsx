// The It may strip, above the canvas and the text while a flow is edited: what it may do, in
// words, each new item (to approve when it is deployed) marked, and what the deployed version may
// that this one no longer would. From the draft's manifest (wasm) and the deployed one's
// (`GET /deployments/<slug>/manifest`), compared in mayItems.ts. A secret's item is marked with
// the key, in the secret's colour, and says where the draft sends it and what is made with it
// (`sent`: core's `sent_secrets` of the draft's unit), as its approval will.

import { describeItemSending } from "./holon";
import type { MayItem } from "./mayItems";
import { isSecretItem, Said } from "./SecretSaid";

const SAID: Record<MayItem["change"], string> = { new: "new", same: "", gone: "no longer" };

export function MayStrip({ items, deployed, sent }: { items: MayItem[] | null; deployed: boolean; sent?: Record<string, string[]> }) {
  if (items === null) return null;
  return (
    <div className="hl-may" role="region" aria-label="it may">
      <span className="hl-may__head">{deployed ? "It may" : "Deployed, it may"}</span>
      {items.length === 0 ? (
        <span className="hl-may__none">do nothing yet: add a route, a clock, a file handler or an action</span>
      ) : (
        items.map(({ item, change }) => (
          <span key={`${change} ${item}`} className={`hl-may__chip hl-may__chip--${change}${isSecretItem(item) ? " hl-may__chip--secret" : ""}`} title={item}>
            {/* What the deployed version sent a secret to isn't the draft's to say: a gone item is its words alone. */}
            <Said item={item} said={describeItemSending(item, change === "gone" ? undefined : sent)} />
            {SAID[change] && <span className="hl-may__tag">{SAID[change]}</span>}
          </span>
        ))
      )}
    </div>
  );
}
