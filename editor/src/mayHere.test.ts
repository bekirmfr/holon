// The Inspector's field marks (mayHere.ts): a field a manifest item names is a permission's, a
// secret read's is a secret's.

import { describe, expect, it } from "vitest";
import { fieldMarks } from "./mayHere";

describe("fieldMarks", () => {
  const items = ['HttpRequest("api.example.com")', 'SecretRead("api_key")', "Clock"];
  it("marks a field an item names, quoted or not", () => {
    expect(fieldMarks("api.example.com", items)).toEqual({ permission: true, secret: false });
    expect(fieldMarks('"api.example.com"', items)).toEqual({ permission: true, secret: false });
    expect(fieldMarks("api_key", items)).toEqual({ permission: true, secret: true });
  });
  it("leaves the rest alone: a name inside another, and an empty field", () => {
    expect(fieldMarks("example.com", items)).toEqual({ permission: false, secret: false });
    expect(fieldMarks("key", items)).toEqual({ permission: false, secret: false });
    expect(fieldMarks("", items)).toEqual({ permission: false, secret: false });
  });
});
