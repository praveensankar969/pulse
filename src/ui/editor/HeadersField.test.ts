import { describe, expect, it } from "vitest";
import { SECRET_MASK } from "../../lib/types";
import { draftHeaders, type HeaderRow } from "./HeadersField";

function row(patch: Partial<HeaderRow> & { key: string }): HeaderRow {
  return {
    value: "",
    secret: false,
    hadValue: false,
    ...patch,
  };
}

describe("draftHeaders", () => {
  it("does not send the mask when unchecking secret", () => {
    expect(
      draftHeaders([
        row({
          key: "Authorization",
          value: SECRET_MASK,
          secret: false,
          hadValue: true,
        }),
      ]),
    ).toEqual([{ key: "Authorization", secret: false, value: "" }]);
  });

  it("treats edited mask leftovers as unchanged", () => {
    expect(
      draftHeaders([
        row({
          key: "Authorization",
          value: `${SECRET_MASK}x`,
          secret: true,
          hadValue: true,
        }),
      ]),
    ).toEqual([{ key: "Authorization", secret: true }]);
    expect(
      draftHeaders([
        row({
          key: "Authorization",
          value: "•••••••",
          secret: true,
          hadValue: true,
        }),
      ]),
    ).toEqual([{ key: "Authorization", secret: true }]);
  });

  it("clears a stored secret when the field is emptied", () => {
    expect(
      draftHeaders([row({ key: "Authorization", value: "", secret: true, hadValue: true })]),
    ).toEqual([{ key: "Authorization", secret: true, clear: true }]);
  });

  it("sends a replacement secret value", () => {
    expect(
      draftHeaders([
        row({
          key: "Authorization",
          value: "Bearer new",
          secret: true,
          hadValue: true,
        }),
      ]),
    ).toEqual([{ key: "Authorization", secret: true, value: "Bearer new" }]);
  });
});
