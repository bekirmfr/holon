// What the flow being edited may do, beside what its deployed version may (MayStrip.tsx): each
// item its draft's manifest asks for, new or as deployed, then each the deployed version asks for
// and the draft no longer does.

export type MayChange = "new" | "same" | "gone";
export type MayItem = { item: string; change: MayChange };

/** `draft`'s items beside `deployed`'s (null: not deployed, so nothing is new against it): new
 *  first, then the same, then those gone, each in its list's order. */
export function mayDiff(draft: string[], deployed: string[] | null): MayItem[] {
  if (deployed === null) return draft.map((item) => ({ item, change: "same" }));
  const was = new Set(deployed);
  const is = new Set(draft);
  return [
    ...draft.filter((i) => !was.has(i)).map((item): MayItem => ({ item, change: "new" })),
    ...draft.filter((i) => was.has(i)).map((item): MayItem => ({ item, change: "same" })),
    ...deployed.filter((i) => !is.has(i)).map((item): MayItem => ({ item, change: "gone" })),
  ];
}
