// The It may strip, above the canvas and the text while a flow is edited: what it may do, each
// thing a short pill (mayPill.tsx): an icon for what it reaches, a small one for what is done to
// it, and its name. The whole sentence shows at once in a tip while the pointer is on a pill, or
// the keyboard's focus, and is what a screen reader hears. Each new item (to approve when it is
// deployed) is marked, and what the deployed version may that this one no longer would. From the
// draft's manifest (wasm) and the deployed one's (`GET /deployments/<slug>/manifest`), compared
// in mayItems.ts. A secret's pill is in the secret's colour, and its sentence says where the
// draft sends it and what is made with it (`sent`: core's `sent_secrets` of the draft's unit), as
// its approval will.

import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { itemParts } from "./holdings";
import { describeItemSending } from "./holon";
import type { MayItem } from "./mayItems";
import { pillOf, PillIcons, WRITES, type Pill } from "./mayPill";
import { isSecretItem } from "./SecretSaid";

const SAID: Record<MayItem["change"], string> = { new: "new", same: "", gone: "no longer" };
/** What a pill's mark means, in its tip. */
const MEANS: Record<MayItem["change"], string> = {
  new: "new: a person approves it when this is deployed",
  same: "",
  gone: "no longer: the deployed version may, this draft doesn't",
};

/** One pill of the strip: the items it stands for (one, or a thing's read and its write), how it
 *  is drawn, its sentence, and the sentence in the lines its tip shows it in. */
export type StripPill = { items: string[]; change: MayItem["change"]; pill: Pill; said: string; lines: string[]; secret: boolean };

/** The strip's pills, in the items' order: each item one pill, but a thing both read and written
 *  (`StateRead("x")` and `StateWrite("x")`, changed alike), which is one, at the place of the
 *  first of the two, saying both. A sentence's parts (where a secret is sent; where what is
 *  answered to it goes; where what is made with it goes) are a line each in its tip. */
export function stripPills(items: MayItem[], sent?: Record<string, string[]>): StripPill[] {
  // What the deployed version sent a secret to isn't the draft's to say: a gone item is its words alone.
  const said = ({ item, change }: MayItem) => describeItemSending(item, change === "gone" ? undefined : sent);
  const leaves = ({ item, change }: MayItem) => {
    const { kind, arg } = itemParts(item);
    return change !== "gone" && kind === "SecretRead" && arg !== null && (sent?.[arg]?.length ?? 0) > 0;
  };
  const at = (kind: string, arg: string | null, change: MayItem["change"]) => items.findIndex((m) => m.change === change && itemParts(m.item).kind === kind && itemParts(m.item).arg === arg);
  const pills: (StripPill | null)[] = items.map((m) => {
    const sentence = said(m);
    return { items: [m.item], change: m.change, pill: pillOf(m.item, leaves(m)), said: sentence, lines: sentence.split("; "), secret: isSecretItem(m.item) };
  });
  items.forEach((m, w) => {
    const { kind, arg } = itemParts(m.item);
    const r = kind in WRITES ? at(WRITES[kind], arg, m.change) : -1;
    if (r < 0) return;
    const [first, second] = r < w ? [r, w] : [w, r];
    const [read, write] = [pills[r]!, pills[w]!];
    pills[first] = { items: [items[r].item, m.item], change: m.change, pill: { ...read.pill, does: "both" }, said: `${read.said}; ${write.said}`, lines: [read.said, write.said], secret: false };
    pills[second] = null;
  });
  return pills.filter((p): p is StripPill => p !== null);
}

/** The widest a tip is drawn. */
const TIP_WIDTH = 380;

export function MayStrip({ items, deployed, sent }: { items: MayItem[] | null; deployed: boolean; sent?: Record<string, string[]> }) {
  // The pill whose tip shows, and where: under it, kept inside the window. Drawn on the page
  // itself (a portal), since the strip scrolls and would clip it.
  const [tip, setTip] = useState<{ key: string; x: number; y: number } | null>(null);
  // Esc puts it away, as it does every other thing that opens over the page.
  useEffect(() => {
    if (!tip) return;
    const away = (e: KeyboardEvent) => e.key === "Escape" && setTip(null);
    window.addEventListener("keydown", away);
    return () => window.removeEventListener("keydown", away);
  }, [tip]);
  if (items === null) return null;
  const pills = stripPills(items, sent);
  const keyOf = (p: StripPill) => `${p.change} ${p.items.join(" ")}`;
  const show = (key: string, el: HTMLElement) => {
    const r = el.getBoundingClientRect();
    setTip({ key, x: Math.max(8, Math.min(r.left, window.innerWidth - TIP_WIDTH - 8)), y: r.bottom + 6 });
  };
  const shown = tip && pills.find((p) => keyOf(p) === tip.key);
  return (
    <div className="hl-may" role="region" aria-label="it may" onScroll={() => setTip(null)}>
      <span className="hl-may__head">{deployed ? "It may" : "Deployed, it may"}</span>
      {items.length === 0 ? (
        <span className="hl-may__none">do nothing yet: add a route, a clock, a file handler or an action</span>
      ) : (
        pills.map((p) => (
          <span
            key={keyOf(p)}
            className={`hl-may__chip hl-may__chip--${p.change}${p.secret ? " hl-may__chip--secret" : ""}`}
            data-item={p.items.join(" ")}
            // Reached by Tab too: the tip is for whoever doesn't use a pointer as well.
            tabIndex={0}
            onMouseEnter={(e) => show(keyOf(p), e.currentTarget)}
            onMouseLeave={() => setTip(null)}
            onFocus={(e) => show(keyOf(p), e.currentTarget)}
            onBlur={() => setTip(null)}
          >
            <PillIcons pill={p.pill} />
            <span className="hl-may__label" aria-hidden="true">
              {p.pill.label}
            </span>
            <span className="hl-may__said">{p.said}</span>
            {SAID[p.change] && <span className="hl-may__tag">{SAID[p.change]}</span>}
          </span>
        ))
      )}
      {tip &&
        shown &&
        createPortal(
          // What a screen reader hears is in the pill already: the tip is for the eye.
          <div className={`hl-may__tip${shown.secret ? " hl-may__tip--secret" : ""}`} style={{ left: tip.x, top: tip.y, maxWidth: TIP_WIDTH }} role="tooltip" aria-hidden="true">
            <div className="hl-may__tip-head">
              <PillIcons pill={shown.pill} />
              <span>{shown.pill.label}</span>
            </div>
            {shown.lines.map((line) => (
              <div key={line} className="hl-may__tip-line">
                {line}
              </div>
            ))}
            {MEANS[shown.change] && <div className={`hl-may__tip-mark hl-may__tip-mark--${shown.change}`}>{MEANS[shown.change]}</div>}
            <div className="hl-may__tip-item">{shown.items.join(" · ")}</div>
          </div>,
          document.body,
        )}
    </div>
  );
}
