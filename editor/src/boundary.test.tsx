// Guards: a part of the console that throws while it draws says so in its place, and the rest goes
// on: each page (the page box) and the canvas are held by a PageBoundary, the whole console by a
// RootBoundary; what failed says its error, is tried again on asking, and once another page opens.
// Needed because there was no error boundary: one page that threw blanked the whole editor, and
// the open flows' unsaved changes went with it (R44). If it fails: the sweep names the part not
// held (wrap it in a PageBoundary); a row names what the boundary no longer does.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import ts from "typescript";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { PageBoundary, RootBoundary } from "./ui/Boundary";

const dir = join(process.cwd(), "src");
const parse = (file: string) => ts.createSourceFile(file, readFileSync(join(dir, file), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);

/** The parts of App's editor that a boundary holds, by the class of the element around them. */
const PARTS = ["hl-pagebox", "hl-canvas__flow"];

/** A JSX element's class, as written: a string, or each string in the expression that makes it. */
function classOf(el: ts.JsxOpeningElement): string {
  const attr = el.attributes.properties.find((p): p is ts.JsxAttribute => ts.isJsxAttribute(p) && p.name.getText() === "className");
  return attr?.initializer?.getText() ?? "";
}

const tag = (n: ts.Node) => (ts.isJsxElement(n) ? n.openingElement.tagName.getText() : ts.isJsxSelfClosingElement(n) ? n.tagName.getText() : "");

describe("the console's parts, in its source", () => {
  it("each page and the canvas are held by a PageBoundary, and all of it by a RootBoundary", () => {
    const app = parse("App.tsx");
    const held: Record<string, boolean> = {};
    const visit = (node: ts.Node) => {
      if (ts.isJsxElement(node)) {
        const part = PARTS.find((p) => classOf(node.openingElement).includes(p));
        if (part) {
          // What it holds is one PageBoundary, and nothing beside it.
          const inside = node.children.filter((c) => !(ts.isJsxText(c) && c.containsOnlyTriviaWhiteSpaces));
          held[part] = inside.length === 1 && tag(inside[0]) === "PageBoundary";
        }
      }
      ts.forEachChild(node, visit);
    };
    visit(app);
    expect(held).toEqual(Object.fromEntries(PARTS.map((p) => [p, true])));

    const main = parse("main.tsx");
    let rooted = false;
    const find = (node: ts.Node) => {
      if (tag(node) === "App") for (let at = node.parent; at; at = at.parent) if (tag(at) === "RootBoundary") rooted = true;
      ts.forEachChild(node, find);
    };
    find(main);
    expect(rooted, "main.tsx renders App inside a RootBoundary").toBe(true);
  });
});

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});
let container: HTMLDivElement | null = null;
let root: Root | null = null;
beforeEach(() => {
  // React says each error it catches on the console; the boundary says it too.
  vi.spyOn(console, "error").mockImplementation(() => {});
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});
afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  container = null;
  vi.restoreAllMocks();
});

let failing = true;
function Page({ name }: { name: string }) {
  if (failing) throw new Error(`${name} can't be drawn`);
  return <p>{name} drawn</p>;
}

/** The console around a page: a count kept beside it, as the open flows are kept around a page. */
function Console({ page }: { page: string }) {
  const [kept, setKept] = useState(0);
  return (
    <>
      <button onClick={() => setKept((k) => k + 1)}>kept {kept}</button>
      <PageBoundary what="This page" resetKey={page}>
        <Page name={page} />
      </PageBoundary>
    </>
  );
}

const alert = () => container!.querySelector('[role="alert"]');
const button = (name: string) => [...container!.querySelectorAll("button")].find((b) => b.textContent?.startsWith(name))!;

describe("a page that throws while it draws", () => {
  beforeEach(() => {
    failing = true;
  });

  it("says so in its place, with its error, and what's around it goes on", () => {
    act(() => root!.render(<Console page="flows" />));
    act(() => button("kept").click());
    expect(alert()?.textContent).toContain("This page failed");
    expect(alert()?.textContent).toContain("flows can't be drawn");
    expect(button("kept").textContent, "the console around it keeps its state").toBe("kept 1");
  });

  it("is drawn again on Try again, and once another page opens", () => {
    act(() => root!.render(<Console page="flows" />));
    failing = false;
    act(() => button("Try again").click());
    expect(alert()).toBeNull();
    expect(container!.textContent).toContain("flows drawn");

    failing = true;
    act(() => root!.render(<Console page="approvals" />));
    expect(alert()?.textContent).toContain("approvals can't be drawn");
    failing = false;
    act(() => root!.render(<Console page="people" />));
    expect(alert()).toBeNull();
    expect(container!.textContent).toContain("people drawn");
  });

  it("not held by a page's boundary, says the console failed, and that a reload goes on", () => {
    act(() =>
      root!.render(
        <RootBoundary>
          <Page name="the sign-in" />
        </RootBoundary>,
      ),
    );
    expect(alert()?.textContent).toContain("The console failed");
    expect(alert()?.textContent).toContain("the sign-in can't be drawn");
    expect(button("Reload")).toBeDefined();
  });
});
