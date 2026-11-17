// The place the address names (routes.ts), as state: read when the hash changes (Back, Forward,
// a link, a typed address), and written by `navigate`. A navigation the person makes adds to the
// history; one that only says where they already are (the editor's tab or run changing under
// them) replaces it, so Back doesn't step through every click on a run.
//
// A replace is decided from a render (an effect, or an answer that arrives later), and the person
// may have moved since: a click pushed the Library, not yet rendered, when an answer to where the
// editor was opened came back and its effect, run from the render before, put the editor's own
// address over it (the e2e's slow runner). So a replace applies only while the address is the
// editor's own: the one its last render showed, or one a replace wrote since. Moved past both, the
// address is the person's, and newer: the replace yields to it, and the render for it decides again.
//
// The other way round, an address a replace wrote is never a place to go to: it said where the
// editor was as a render saw it, and the editor may have moved on before it rendered (back in
// edit while it said a run of view mode, the e2e's slow runner), so going there would undo the
// person's move. The route says whether a replace wrote it (`fromReplace`), for the editor to
// leave it be.

import { useCallback, useEffect, useRef, useState } from "react";
import { format, parse, type Route } from "./routes";

export type Navigate = (to: Route, how?: { replace?: boolean }) => void;

/** The address as the editor reads it: `#…` as routes.ts formats what it names. */
const addressNow = () => format(parse(window.location.hash));

export function useHashRoute(): { route: Route; navigate: Navigate; arrivedAt: string; fromReplace: boolean } {
  const [route, setRoute] = useState<Route>(() => parse(window.location.hash));
  // Whether the route was written by a replace (the editor saying where it is), not asked for:
  // set with the route, so the render that shows one says which it is.
  const [fromReplace, setFromReplace] = useState(false);
  // The address the editor was opened at ("" when none): a place asked for, not a default.
  const [arrivedAt] = useState(() => window.location.hash.replace(/^#/, ""));
  // What the last render showed, and what a replace wrote since the person last moved.
  const shown = useRef(format(route));
  shown.current = format(route);
  const replaced = useRef<string | null>(null);

  useEffect(() => {
    // Back over entries pushed below fires popstate, and may fire hashchange too: the same place
    // read twice is kept as it was.
    const onHash = () => {
      replaced.current = null;
      setRoute((r) => (format(r) === addressNow() ? r : parse(window.location.hash)));
      setFromReplace(false);
    };
    window.addEventListener("hashchange", onHash);
    window.addEventListener("popstate", onHash);
    return () => {
      window.removeEventListener("hashchange", onHash);
      window.removeEventListener("popstate", onHash);
    };
  }, []);

  const navigate = useCallback<Navigate>((to, how) => {
    if (how?.replace && addressNow() !== shown.current && addressNow() !== replaced.current) return;
    const hash = format(to);
    if (hash !== window.location.hash) {
      // Neither way fires hashchange: the state is set here, the same for both.
      if (how?.replace) window.history.replaceState(window.history.state, "", hash);
      else window.history.pushState(window.history.state, "", hash);
    }
    replaced.current = how?.replace ? addressNow() : null;
    setRoute((r) => (format(r) === hash ? r : to));
    setFromReplace(how?.replace === true);
  }, []);

  return { route, navigate, arrivedAt, fromReplace };
}
