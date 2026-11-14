// The editor's bar (CanvasPanels.tsx's FlowToolbar): deploy says "update •" while the flow differs
// from what is deployed, unless the flow as it is now is a version proposed already and waiting for
// a person: then it says that version waits, linked to its review, and has no change to mark.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { FlowToolbar } from "./CanvasPanels";
import type { DeploymentEntry } from "./server";

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

const entry = { slug: "hello-api", unit: "aa", status: "running", session: 1, port: 8101, version: 4 } as unknown as DeploymentEntry;

function bar(outOfDate: boolean, waiting: number | null): HTMLDivElement {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  const none = () => {};
  act(() =>
    root!.render(
      <FlowToolbar
        mode="edit"
        canView
        onMode={none}
        panes={[]}
        flowName="hello-api"
        saved
        dirty={false}
        entry={entry}
        navigate={none}
        edit={{ onFormat: none, onCheck: none, onSave: none, onDeploy: none, busy: false, outOfDate, waiting, startsNothing: false }}
        error=""
      />,
    ),
  );
  return container;
}

const update = (el: HTMLElement) => [...el.querySelectorAll("button")].find((b) => b.textContent?.startsWith("update"))!;

describe("the bar's update", () => {
  it("marks a flow that differs from what is deployed", () => {
    const el = bar(true, null);
    expect(update(el).textContent).toBe("update •");
    expect(el.textContent).not.toContain("waits");
  });

  it("says the version waits, linked to its review, when the flow as it is is proposed already", () => {
    const el = bar(true, 5);
    expect(update(el).textContent).toBe("update");
    const link = [...el.querySelectorAll("a")].find((a) => a.textContent === "v5 waits");
    expect(link?.getAttribute("href")).toBe("#approvals.hello-api.v5");
  });

  it("marks nothing when it is what is deployed", () => {
    expect(update(bar(false, null)).textContent).toBe("update");
  });
});
