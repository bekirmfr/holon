// holon-server's live deployment table, and what the deployed page does with it.

import { useCallback, useEffect, useState } from "react";
import { deleteDeployment, listDeployments, ServerError, triggerDeployment, type DeploymentEntry } from "./server";
import { useDialog } from "./Dialog";
import type { AppPage, TriggerOutcome } from "./Pages";

// `deployments` is the server's own table, re-fetched after every mutation rather than guessed
// at locally, so the editor can never drift from what the server actually has running. Errors
// go to `setError` (the canvas shows them as deploy errors).
export function useDeployments(page: AppPage, setError: (message: string) => void) {
  const dialog = useDialog();
  const [deployments, setDeployments] = useState<DeploymentEntry[]>([]);
  // Per-slug trigger body input and last result/error — a small map rather than one shared
  // field, since more than one deployed flow can be sitting in the list at once, each with its
  // own thing to try. Kept here, not in the page, so they survive leaving it.
  const [triggerInputs, setTriggerInputs] = useState<Record<string, string>>({});
  const [triggerResults, setTriggerResults] = useState<Record<string, TriggerOutcome>>({});

  const refresh = useCallback(() => {
    listDeployments()
      .then(setDeployments)
      .catch((e: unknown) => setError(e instanceof ServerError ? e.message : String(e)));
  }, [setError]);

  // Fetched at start and then every few seconds, so the header's live status (and the deployed
  // page) also follow changes made elsewhere: another tab, curl, a restart.
  useEffect(() => {
    refresh();
    const timer = setInterval(refresh, 4000);
    return () => clearInterval(timer);
  }, [refresh]);

  // Opening the Deployed page always shows the live table, not whatever the last fetch saw.
  useEffect(() => {
    if (page === "deployed") refresh();
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

  const onTriggerInputChange = useCallback((slug: string, value: string) => {
    setTriggerInputs((t) => ({ ...t, [slug]: value }));
  }, []);

  const onTrigger = useCallback(
    (slug: string) => {
      triggerDeployment(slug, triggerInputs[slug] ?? "")
        .then((result) => setTriggerResults((r) => ({ ...r, [slug]: { ok: true, result } })))
        .catch((e: unknown) =>
          setTriggerResults((r) => ({
            ...r,
            [slug]: { ok: false, message: e instanceof ServerError ? e.message : String(e) },
          })),
        )
        // The server bumps trigger_count/error_count on every attempt, success or failure
        // alike, so the table is refreshed either way.
        .finally(() => refresh());
    },
    [triggerInputs, refresh],
  );

  return { deployments, refresh, triggerInputs, onTriggerInputChange, triggerResults, onTrigger, onDelete };
}
