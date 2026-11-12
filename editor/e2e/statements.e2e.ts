// The canvas's two levels (web-console-L4.md §5.2, §5.3), in the real editor: a flow opens at its
// Statements, the statement graph of its text, with no palette; `l` goes to its Nodes and back. A
// row picked puts the text's cursor on its mark, opening the text if it is shut, and the
// Inspector says the statement; Enter on it opens the Nodes it stands for, picked.

import type { Page } from "@playwright/test";
import { expect, test } from "./fixtures";
import { openSample } from "./helpers";

const level = (page: Page, name: "Statements" | "Nodes") =>
  page.locator(".hl-canvas").getByRole("group", { name: "canvas level" }).getByRole("button", { name, exact: true });

test("a sample opens at the Statements level", async ({ page }) => {
  await openSample(page, "Hello API");
  await expect(level(page, "Statements")).toHaveAttribute("aria-pressed", "true");
  const graph = page.getByRole("region", { name: "statement graph" });
  await expect(graph.locator(".hl-sg__row").first()).toBeVisible();
  await expect(graph.locator(".hl-sg__row", { hasText: "return" })).toHaveCount(1);
  await expect(page.locator(".hl-palette")).toHaveCount(0);
  // l: the nodes, with the palette to place more; l again: the statements.
  await page.keyboard.press("l");
  await expect(level(page, "Nodes")).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator(".hl-palette")).toBeVisible();
  await expect(page.locator(".hl-canvas__flow > .react-flow .react-flow__node").first()).toBeVisible();
  await expect(graph).toHaveCount(0);
  await page.keyboard.press("l");
  await expect(level(page, "Statements")).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator(".hl-palette")).toHaveCount(0);
  await expect(graph).toBeVisible();
});

test("clicking a row moves the cursor", async ({ page }) => {
  await openSample(page, "Orders API (a database on the host)");
  // Below 1600px wide (the tests' 1280) the text is shut: picking a row opens it.
  const text = page.getByRole("region", { name: "Text" });
  await expect(text).toHaveCount(0);
  const row = page.getByRole("region", { name: "statement graph" }).locator(".hl-sg__row", { hasText: "shop.exec" }).first();
  await row.click();
  await expect(row).toHaveAttribute("aria-current", "true");
  await expect(text).toBeVisible();
  await expect(text.locator(".cm-activeLine")).toContainText("shop.exec(");
  const inspector = page.getByRole("complementary", { name: "Inspector" });
  await expect(inspector.getByRole("region", { name: "the statement on line 33" })).toBeVisible();
  await expect(inspector.getByRole("region", { name: "this flow" })).toContainText("8150");
  // Enter on it: its nodes, picked, at the Nodes level.
  await row.press("Enter");
  await expect(level(page, "Nodes")).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator(".react-flow__node.selected")).not.toHaveCount(0);
});
