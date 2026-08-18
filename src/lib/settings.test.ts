import { describe, expect, it } from "vitest";
import {
  DEFAULT_HOTKEY,
  DEFAULT_QUIET_HOURS,
  MIXED_REACHABILITY_HELP,
  resolvedHotkey,
} from "./settings";

describe("settings constants", () => {
  it("keeps mixed-reachability help verbatim", () => {
    expect(MIXED_REACHABILITY_HELP).toBe(
      "If any check succeeds, Pulse assumes the network is up. A homelab box that still answers will keep Pulse online even if the public internet is gone.",
    );
  });

  it("defaults the hotkey to CommandOrControl+Shift+U", () => {
    expect(DEFAULT_HOTKEY).toBe("CommandOrControl+Shift+U");
    expect(resolvedHotkey(undefined)).toBe(DEFAULT_HOTKEY);
    expect(resolvedHotkey("")).toBe(DEFAULT_HOTKEY);
    expect(resolvedHotkey("  ")).toBe(DEFAULT_HOTKEY);
    expect(resolvedHotkey("CommandOrControl+Shift+P")).toBe(
      "CommandOrControl+Shift+P",
    );
  });

  it("defaults quiet hours to weekdays overnight", () => {
    expect(DEFAULT_QUIET_HOURS).toEqual({
      start: "22:00",
      end: "08:00",
      days: [1, 2, 3, 4, 5],
    });
  });
});
