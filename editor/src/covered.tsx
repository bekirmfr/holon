// What another page covers does no work. The editor's canvas and its panes stay drawn under the
// pages (Flows, a flow's page…) so nothing typed into them is lost (App.tsx), and while covered
// they keep what they show and their state: they read the server no more, each poller waiting
// until they are shown again (useCovered), and the editor's renders don't reach them (Coverable).
// Needed because view mode, left open under a flow's page, read its log every 1.5 s and drew its
// up to 500 runs again each time a run came, and with every render of the editor: on a slow
// machine the page in front had too little time left to answer a click (the e2e's ticker test ran
// out of its time on the hosted runner).

import { createContext, memo, useContext, type ReactNode } from "react";

const CoveredContext = createContext(false);

/** Whether another page covers what is drawn here: a poller waits while it does. */
export const useCovered = (): boolean => useContext(CoveredContext);

/** `children`, which another page covers while `covered`: drawn as they were when it came over
 *  them, until it goes. */
export const Coverable = memo(
  function Coverable({ children, covered }: { children: ReactNode; covered: boolean }) {
    return <CoveredContext.Provider value={covered}>{children}</CoveredContext.Provider>;
  },
  // Covered before and still: what the editor draws them with now waits until they show.
  (was, now) => was.covered && now.covered,
);
