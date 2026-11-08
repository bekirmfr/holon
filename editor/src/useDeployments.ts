// holon-server's live deployment table, kept fresh.

import { useCallback, useEffect, useState } from "react";
import { listDeployments, ServerError, type DeploymentEntry } from "./server";
import type { AppPage } from "./Pages";

// `deployments` is the server's own table, re-fetched after every mutation rather than guessed
// at locally, so the editor can never drift from what the server actually has running. A failed
// fetch is `listError`, cleared by the next one that succeeds (the server was restarting, say).
export function useDeployments(page: AppPage) {
  const [deployments, setDeployments] = useState<DeploymentEntry[]>([]);
  const [listError, setListError] = useState("");

  const refresh = useCallback(() => {
    listDeployments()
      .then((d) => {
        setDeployments(d);
        setListError("");
      })
      .catch((e: unknown) =>
        setListError(e instanceof ServerError ? e.message : `the server isn't answering (${String(e)})`),
      );
  }, []);

  // Fetched at start and then every few seconds, so the header's live status (and the flows
  // page) also follow changes made elsewhere: another tab, curl, a restart.
  useEffect(() => {
    refresh();
    const timer = setInterval(refresh, 4000);
    return () => clearInterval(timer);
  }, [refresh]);

  // Opening Flows or a flow's page always shows the live table, not whatever the last fetch saw.
  useEffect(() => {
    if (page === "flows" || page === "flow") refresh();
  }, [page, refresh]);

  return { deployments, listError, refresh };
}
