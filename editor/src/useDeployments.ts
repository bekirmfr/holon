// holon-server's live deployment table, and what the deployed page does with it.

import { useCallback, useEffect, useState } from "react";
import { deleteDeployment, listDeployments, ServerError, type DeploymentEntry } from "./server";
import { useDialog } from "./Dialog";
import type { AppPage } from "./Pages";

// `deployments` is the server's own table, re-fetched after every mutation rather than guessed
// at locally, so the editor can never drift from what the server actually has running. A failed
// fetch is `listError`, cleared by the next one that succeeds (the server was restarting, say);
// a failed action (undeploy) goes to `setError`, with the deploy errors.
export function useDeployments(page: AppPage, setError: (message: string) => void) {
  const dialog = useDialog();
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

  // Fetched at start and then every few seconds, so the header's live status (and the deployed
  // page) also follow changes made elsewhere: another tab, curl, a restart.
  useEffect(() => {
    refresh();
    const timer = setInterval(refresh, 4000);
    return () => clearInterval(timer);
  }, [refresh]);

  // Opening the Deployed page or a flow's always shows the live table, not whatever the last fetch saw.
  useEffect(() => {
    if (page === "deployed" || page === "flow") refresh();
  }, [page, refresh]);

  const onDelete = useCallback(
    async (slug: string) => {
      // Undeploying is also the one way a deployment's state is discarded (a redeploy keeps it).
      const question = `Undeploy "${slug}"? Its stored state is deleted too. This can't be undone.`;
      if (!(await dialog.confirm(question, "Undeploy"))) return;
      deleteDeployment(slug)
        .then(() => refresh())
        .catch((e: unknown) => setError(e instanceof ServerError ? e.message : String(e)));
    },
    [refresh, dialog, setError],
  );

  return { deployments, listError, refresh, onDelete };
}
