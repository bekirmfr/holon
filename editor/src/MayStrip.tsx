// The It may strip, above the canvas and the text while a flow is edited: what it may do, each
// thing a short pill (mayPill.tsx): an icon for what it reaches, a small one for what is done to
// it, and its name. The whole sentence is the pill's title, shown on hover, and what a screen
// reader hears. Each new item (to approve when it is deployed) is marked, and what the deployed
// version may that this one no longer would. From the draft's manifest (wasm) and the deployed
// one's (`GET /deployments/<slug>/manifest`), compared in mayItems.ts. A secret's pill is in the
// secret's colour, and its sentence says where the draft sends it and what is made with it
// (`sent`: core's `sent_secrets` of the draft's unit), as its approval will.

import { itemParts } from "./holdings";
import { describeItemSending } from "./holon";
import type { MayItem } from "./mayItems";
import { pillOf, PillIcons, WRITES, type Pill } from "./mayPill";
import { isSecretItem } from "./SecretSaid";

const SAID: Record<MayItem["change"], string> = { new: "new", same: "", gone: "no longer" };

/** One pill of the strip: the items it stands for (one, or a thing's read and its write), how it
 *  is drawn, and its sentence. */
export type StripPill = { items: string[]; change: MayItem["change"]; pill: Pill; said: string; secret: boolean };

/** The strip's pills, in the items' order: each item one pill, but a thing both read and written
 *  (`StateRead("x")` and `StateWrite("x")`, changed alike), which is one, at the place of the
 *  first of the two, saying both. */
export function stripPills(items: MayItem[], sent?: Record<string, string[]>): StripPill[] {
  // What the deployed version sent a secret to isn't the draft's to say: a gone item is its words alone.
  const said = ({ item, change }: MayItem) => describeItemSending(item, change === "gone" ? undefined : sent);
  const leaves = ({ item, change }: MayItem) => {
    const { kind, arg } = itemParts(item);
    return change !== "gone" && kind === "SecretRead" && arg !== null && (sent?.[arg]?.length ?? 0) > 0;
  };
  const at = (kind: string, arg: string | null, change: MayItem["change"]) => items.findIndex((m) => m.change === change && itemParts(m.item).kind === kind && itemParts(m.item).arg === arg);
  const pills: (StripPill | null)[] = items.map((m) => ({ items: [m.item], change: m.change, pill: pillOf(m.item, leaves(m)), said: said(m), secret: isSecretItem(m.item) }));
  items.forEach((m, w) => {
    const { kind, arg } = itemParts(m.item);
    const r = kind in WRITES ? at(WRITES[kind], arg, m.change) : -1;
    if (r < 0) return;
    const [first, second] = r < w ? [r, w] : [w, r];
    const read = pills[r]!;
    pills[first] = { items: [items[r].item, m.item], change: m.change, pill: { ...read.pill, does: "both" }, said: `${read.said}; ${pills[w]!.said}`, secret: false };
    pills[second] = null;
  });
  return pills.filter((p): p is StripPill => p !== null);
}

export function MayStrip({ items, deployed, sent }: { items: MayItem[] | null; deployed: boolean; sent?: Record<string, string[]> }) {
  if (items === null) return null;
  return (
    <div className="hl-may" role="region" aria-label="it may">
      <span className="hl-may__head">{deployed ? "It may" : "Deployed, it may"}</span>
      {items.length === 0 ? (
        <span className="hl-may__none">do nothing yet: add a route, a clock, a file handler or an action</span>
      ) : (
        stripPills(items, sent).map(({ items: stood, change, pill, said, secret }) => (
          <span
            key={`${change} ${stood.join(" ")}`}
            className={`hl-may__chip hl-may__chip--${change}${secret ? " hl-may__chip--secret" : ""}`}
            title={SAID[change] ? `${said} (${SAID[change]})` : said}
            data-item={stood.join(" ")}
          >
            <PillIcons pill={pill} />
            <span className="hl-may__label" aria-hidden="true">
              {pill.label}
            </span>
            <span className="hl-may__said">{said}</span>
            {SAID[change] && <span className="hl-may__tag">{SAID[change]}</span>}
          </span>
        ))
      )}
    </div>
  );
}
