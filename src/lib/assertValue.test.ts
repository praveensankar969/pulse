import { describe, expect, it } from "vitest";
import { formatAssertionValue, parseAssertionValue } from "./assertValue";

describe("parseAssertionValue", () => {
  it("parses JSON literals", () => {
    expect(parseAssertionValue("true")).toBe(true);
    expect(parseAssertionValue("false")).toBe(false);
    expect(parseAssertionValue("null")).toBeNull();
    expect(parseAssertionValue("0")).toBe(0);
    expect(parseAssertionValue("-1.5")).toBe(-1.5);
    expect(parseAssertionValue("[]")).toEqual([]);
    expect(parseAssertionValue('{"ok":true}')).toEqual({ ok: true });
    expect(parseAssertionValue('"ok"')).toBe("ok");
    expect(parseAssertionValue("ok")).toBe("ok");
  });
});

describe("formatAssertionValue", () => {
  it("round-trips non-strings as JSON", () => {
    expect(formatAssertionValue(0)).toBe("0");
    expect(formatAssertionValue([])).toBe("[]");
    expect(formatAssertionValue("ok")).toBe("ok");
  });
});
