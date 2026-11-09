// The place the address names (routes.ts), as state: read when the hash changes (Back, Forward,
// a link, a typed address), and written by `navigate`. A navigation the person makes adds to the
// history; one that only says where they already are (the editor's tab or run changing under
// them) replaces it, so Back doesn't step through every click on a run.

import { useCallback, useEffect, useState } from "react";
import { format, parse, type Route } from "./routes";

export type Navigate = (to: Route, how?: { replace?: boolean }) => void;

export function useHashRoute(): { route: Route; navigate: Navigate; arrivedAt: string } {
  const [route, setRoute] = useState<Route>(() => parse(window.location.hash));
  // The address the editor was opened at ("" when none): a place asked for, not a default.
  const [arrivedAt] = useState(() => window.location.hash.replace(/^#/, ""));

  useEffect(() => {
    // Back over entries pushed below fires popstate, and may fire hashchange too: the same place
    // read twice is kept as it was.
    const onHash = () => setRoute((r) => (format(r) === format(parse(window.location.hash)) ? r : parse(window.location.hash)));
    window.addEventListener("hashchange", onHash);
    window.addEventListener("popstate", onHash);
    return () => {
      window.removeEventListener("hashchange", onHash);
      window.removeEventListener("popstate", onHash);
    };
  }, []);

  const navigate = useCallback<Navigate>((to, how) => {
    const hash = format(to);
    if (hash !== window.location.hash) {
      // Neither way fires hashchange: the state is set here, the same for both.
      if (how?.replace) window.history.replaceState(window.history.state, "", hash);
      else window.history.pushState(window.history.state, "", hash);
    }
    setRoute((r) => (format(r) === hash ? r : to));
  }, []);

  return { route, navigate, arrivedAt };
}
