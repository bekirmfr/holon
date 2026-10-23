import { describe, expect, it } from "vitest";
import { tokenFor } from "./server";

describe("the token a request carries", () => {
  const both = { admin: "A", secrets: "S" };
  it("is the secrets token for writing a secret, when there is one", () => {
    expect(tokenFor("http://h/deployments/s/secrets/k", "POST", both)).toBe("S");
    expect(tokenFor("http://h/deployments/s/secrets/k/rollback", "POST", both)).toBe("S");
    expect(tokenFor("http://h/deployments/s/secrets/k", "DELETE", both)).toBe("S");
  });
  it("is the admin token for everything else, and for secrets without a secrets token", () => {
    expect(tokenFor("http://h/deployments/s/secrets", "GET", both)).toBe("A");
    expect(tokenFor("http://h/deployments/s/secrets/k", "GET", both)).toBe("A");
    expect(tokenFor("http://h/deployments", "GET", both)).toBe("A");
    expect(tokenFor("http://h/deployments/s/secrets/k", "POST", { admin: "A", secrets: "" })).toBe("A");
    expect(tokenFor("http://h/deployments", "GET", { admin: "", secrets: "" })).toBe("");
  });
});
