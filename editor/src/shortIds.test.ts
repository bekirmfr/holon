import { describe, expect, it } from "vitest";
import { shortIds } from "./shortIds";

describe("shortIds", () => {
  it("cuts a run's hash to its first eight, and leaves the rest of the line", () => {
    const run = "c8bc0271aab27e5b16fa27ed2833f48d49c639d0713a0dc1140c0396c951a862";
    expect(shortIds(`step4-wait run ${run} at "sign-off"`)).toBe('step4-wait run c8bc0271… at "sign-off"');
  });

  it("leaves short hex words, numbers and names alone", () => {
    expect(shortIds("hello-api v5 · run 14 · cafe · deadbeef")).toBe("hello-api v5 · run 14 · cafe · deadbeef");
  });
});
