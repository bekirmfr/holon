// A replace says where the person already is, as a render saw it (useHashRoute.ts); when they have
// moved since, it yields. The e2e's slow runner: a reload reopening `#editor.hello-api`, the
// person clicking the Library while the answer was on its way; the answer's effect, run from the
// render before the click, put the editor's own address over the Library, and the Library never
// showed. Each case here: what moved the address before the replace came, and where it ends.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { format, type Route } from "./routes";
import { useHashRoute, type Navigate } from "./useHashRoute";

const LIBRARY: Route = { kind: "library", tab: "samples" };
const EDITOR_HOME: Route = { kind: "editor-home" };
const FLOWS: Route = { kind: "flows" };
const OVERVIEW: Route = { kind: "overview" };

let root: Root | null = null;
let container: HTMLDivElement | null = null;
/** The hook as the last render gave it. */
let now: { route: Route; navigate: Navigate } = { route: OVERVIEW, navigate: () => undefined };

function Shows() {
  const { route, navigate } = useHashRoute();
  now = { route, navigate };
  return null;
}

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

beforeEach(() => {
  window.history.replaceState(null, "", "#editor.hello-api");
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(<Shows />));
});

afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  root = null;
  container = null;
});

/** The address, and the place the editor shows, once everything asked has rendered. */
const where = () => ({ address: window.location.hash, shown: format(now.route) });

describe("a replace decided before the person moved", () => {
  it("yields to a place they pushed since, not yet rendered", () => {
    const stale = now.navigate;
    act(() => {
      now.navigate(LIBRARY);
      stale(EDITOR_HOME, { replace: true });
    });
    expect(where()).toEqual({ address: format(LIBRARY), shown: format(LIBRARY) });
  });

  it("yields to a link the browser followed, its hashchange not yet read", async () => {
    window.location.hash = format(LIBRARY);
    act(() => now.navigate(EDITOR_HOME, { replace: true }));
    await act(() => new Promise((r) => setTimeout(r, 0)));
    expect(where()).toEqual({ address: format(LIBRARY), shown: format(LIBRARY) });
  });

  it("applies with nothing moved: the editor's own address, and another replace after it", () => {
    act(() => {
      now.navigate(EDITOR_HOME, { replace: true });
      now.navigate(FLOWS, { replace: true });
    });
    expect(where()).toEqual({ address: format(FLOWS), shown: format(FLOWS) });
  });

  it("applies once the place moved to has rendered: decided there, it says where they are", () => {
    act(() => now.navigate(LIBRARY));
    act(() => now.navigate(OVERVIEW, { replace: true }));
    expect(where()).toEqual({ address: format(OVERVIEW), shown: format(OVERVIEW) });
  });
});
