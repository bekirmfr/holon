// The tests against the main test server (7520) start signed in as its admin: e2e/server.mjs
// sets that server up with a known setup code and this admin, and every page here begins with
// that admin's session in the browser, as signing in leaves it (server.ts's holon.session).

import { test as base, expect } from "@playwright/test";

export const SERVER = "http://127.0.0.1:7520";
export const ADMIN = { name: "e2e", password: "e2e-password-0123" };

export const test = base.extend<{ adminToken: string }>({
  /** A session of the test server's admin, for calls to its control plane. */
  adminToken: async ({ request }, use) => {
    // The server answers /health a moment before server.mjs has set it up: wait for that (not
    // by trying to sign in, which a few failures would lock).
    for (let i = 0; i < 60; i++) {
      const state = (await (await request.get(`${SERVER}/auth/state`)).json()) as { setup: boolean };
      if (!state.setup) break;
      await new Promise((r) => setTimeout(r, 500));
    }
    const res = await request.post(`${SERVER}/auth/sign-in`, { data: ADMIN });
    expect(res.ok(), await res.text()).toBe(true);
    await use(((await res.json()) as { token: string }).token);
  },
  page: async ({ page, adminToken }, use) => {
    await page.addInitScript((session) => localStorage.setItem("holon.session", session), JSON.stringify({ token: adminToken, name: ADMIN.name, role: "admin" }));
    await use(page);
  },
});

export { expect };
