// The host's pages read their lists again when their tab changes: a tab picked by the address
// leaves the page mounted, and what was bound elsewhere since shows without a reload.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { DialogProvider } from "./Dialog";
import { ResourcesPage } from "./HostPages";
import { listDatabases, type DatabaseEntry } from "./server";

// Every read but the databases' waits for ever: the page shows what it has.
vi.mock("./server", async (original) => {
  const real = await original<typeof import("./server")>();
  const never = () => new Promise<never>(() => {});
  const reads = ["listPrincipals", "listRoles", "listFolders", "listMails", "listFeeds", "listQueues", "listServerSecrets", "listGrants", "listProposals", "getPermissions", "getHost", "getOidcSettings", "listTables", "mayI", "getActivity"];
  return { ...real, ...Object.fromEntries(reads.map((name) => [name, vi.fn(never)])), listDatabases: vi.fn() };
});

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

let container: HTMLDivElement | null = null;
let root: Root | null = null;
afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  container = null;
});

const shop: DatabaseEntry = { name: "shop", engine: "sqlite", path: "/data/shop.db", access: "read-write", deployment: null };

describe("Resources", () => {
  it("reads the databases again when its tab is picked by the address", async () => {
    vi.mocked(listDatabases).mockResolvedValue([]);
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    const show = (tab: "databases" | "folders") =>
      act(async () =>
        root!.render(
          <DialogProvider>
            <ResourcesPage tab={tab} deployments={[]} onOpenDeployment={() => {}} />
          </DialogProvider>,
        ),
      );
    await show("databases");
    expect(container.textContent).toContain("no databases bound");
    // Bound elsewhere while the folders were shown.
    await show("folders");
    vi.mocked(listDatabases).mockResolvedValue([shop]);
    await show("databases");
    expect(container.textContent).not.toContain("no databases bound");
    expect(container.textContent).toContain("database shop");
  });
});
