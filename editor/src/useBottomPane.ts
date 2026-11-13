// The editor's bottom pane in Edit (BottomPane.tsx): the tab it shows, remembered in this browser;
// the text's mistakes, worked out as it is typed (once typing pauses, as the text's own lint
// does); and Check, which says how many there are now and shows them.

import { useCallback, useEffect, useState } from "react";
import { EDIT_TABS, problemsOf, type EditTab, type Problem } from "./BottomPane";

const TAB_KEY = "holon.bottomTab";
/** How long typing pauses before the mistakes are worked out again, as the lint waits. */
const PAUSE_MS = 250;

/** A pane's tab, remembered in this browser under `key`: one of `tabs`, else `fallback`. The
 *  bottom pane's in Edit is one, and in View runs another (ViewPanes.tsx's, Runs or Run). */
export function useRememberedTab<T extends string>(key: string, tabs: readonly T[], fallback: T): [T, (t: T) => void] {
  const [tab, setTabState] = useState<T>(() => {
    try {
      const t = localStorage.getItem(key);
      return tabs.includes(t as T) ? (t as T) : fallback;
    } catch {
      return fallback;
    }
  });
  const setTab = useCallback(
    (t: T) => {
      setTabState(t);
      try {
        localStorage.setItem(key, t);
      } catch {
        // Not remembered, then.
      }
    },
    [key],
  );
  return [tab, setTab];
}

const EDIT_TAB_NAMES = EDIT_TABS.map(([k]) => k);

export function useBottomPane({ text, shown, onShown }: {
  /** The flow's text as the pane holds it (null before it is first written). */
  text: string | null;
  /** Whether the bottom pane is shown, and showing it. */
  shown: boolean;
  onShown: (shown: boolean) => void;
}) {
  const [tab, setTab] = useRememberedTab<EditTab>(TAB_KEY, EDIT_TAB_NAMES, "try");
  const [problems, setProblems] = useState<Problem[]>(() => problemsOf(text));
  // What the last Check said, until the text changes.
  const [checked, setChecked] = useState("");
  useEffect(() => {
    setChecked("");
    const wait = setTimeout(() => setProblems(problemsOf(text)), PAUSE_MS);
    return () => clearTimeout(wait);
  }, [text]);

  /** Show tab `t`, the pane shown with it. */
  const open = useCallback(
    (t: EditTab) => {
      setTab(t);
      if (!shown) onShown(true);
    },
    [setTab, shown, onShown],
  );

  /** The text's mistakes now, without waiting for typing to pause: Problems shown, saying how
   *  many; the first, if any. */
  const check = useCallback((): Problem | null => {
    const found = problemsOf(text);
    setProblems(found);
    setChecked(found.length === 0 ? "Checked: no mistakes." : `Checked: ${found.length} mistake${found.length === 1 ? "" : "s"}.`);
    open("problems");
    return found[0] ?? null;
  }, [text, open]);

  return { tab, setTab, open, problems, checked, check };
}
