import { describe, expect, it } from "vitest";
import {
  isSlow,
  primaryLabel,
  relativeTime,
  snoozeRemaining,
  sortServices,
} from "./format";
import type { CheckResult, ServiceView, UiState } from "./types";

const NOW = Date.parse("2026-08-18T14:10:00Z");

function view(
  patch: Partial<ServiceView> & { name: string; state: UiState },
): ServiceView {
  return {
    id: patch.id ?? patch.name,
    url: "https://example.test/health",
    method: "GET",
    headers: [],
    intervalSec: 60,
    timeoutMs: 10_000,
    expectedStatus: "2xx",
    assertions: [],
    notify: true,
    alwaysAlert: false,
    paused: patch.state === "paused",
    followRedirects: true,
    createdAt: "2026-08-18T14:00:00Z",
    updatedAt: "2026-08-18T14:00:00Z",
    consecutiveHardFails: 0,
    sparkline24: [],
    ...patch,
  };
}

function result(patch: Partial<CheckResult>): CheckResult {
  return {
    at: "2026-08-18T14:09:00Z",
    outcome: "ok",
    assertionResults: [],
    state: "healthy",
    ...patch,
  };
}

describe("primaryLabel / isSlow", () => {
  it("uses machine / last-outcome labels only", () => {
    expect(primaryLabel(view({ name: "A", state: "paused" }))).toBe("Paused");
    expect(primaryLabel(view({ name: "A", state: "pending" }))).toBe("Pending");
    expect(primaryLabel(view({ name: "A", state: "down" }))).toBe("Down");
    expect(primaryLabel(view({ name: "A", state: "healthy" }))).toBe("Healthy");
  });

  it("is Slow only when degraded and last outcome is soft or errorKind slow", () => {
    const soft = view({
      name: "Auth",
      state: "degraded",
      lastResult: result({ outcome: "soft", errorKind: "slow", state: "degraded" }),
    });
    const kind = view({
      name: "Auth",
      state: "degraded",
      lastResult: result({ outcome: "hard", errorKind: "slow", state: "degraded" }),
    });
    const hard = view({
      name: "API",
      state: "degraded",
      lastResult: result({ outcome: "hard", errorKind: "timeout", state: "degraded" }),
    });
    expect(isSlow(soft)).toBe(true);
    expect(primaryLabel(soft)).toBe("Slow");
    expect(isSlow(kind)).toBe(true);
    expect(primaryLabel(kind)).toBe("Slow");
    expect(isSlow(hard)).toBe(false);
    expect(primaryLabel(hard)).toBe("Degraded");
  });
});

describe("relativeTime / snoozeRemaining", () => {
  it("shows Checking… for pending and in-flight checks", () => {
    const pending = view({ name: "New", state: "pending" });
    expect(relativeTime(pending, NOW, false)).toBe("Checking…");
    const healthy = view({
      name: "API",
      state: "healthy",
      lastCheckAt: "2026-08-18T14:09:48Z",
    });
    expect(relativeTime(healthy, NOW, true)).toBe("Checking…");
  });

  it("subtracts downClockAdjustMs from down duration", () => {
    const down = view({
      name: "Pay",
      state: "down",
      downSince: "2026-08-18T14:00:00Z",
      downClockAdjustMs: 4 * 60_000,
    });
    expect(relativeTime(down, NOW, false)).toBe("down 06m");
  });

  it("uses degradedSince, not lastCheckAt, for degraded 3m", () => {
    const degraded = view({
      name: "Auth",
      state: "degraded",
      lastCheckAt: "2026-08-18T14:09:51Z",
      degradedSince: "2026-08-18T14:07:00Z",
      lastResult: result({ outcome: "soft", errorKind: "slow", state: "degraded" }),
    });
    expect(relativeTime(degraded, NOW, false)).toBe("degraded 03m");
  });

  it("formats healthy last-check age", () => {
    const healthy = view({
      name: "API",
      state: "healthy",
      lastCheckAt: "2026-08-18T14:09:48Z",
    });
    expect(relativeTime(healthy, NOW, false)).toBe("12s ago");
  });

  it("renders remaining snooze and hides expired", () => {
    expect(snoozeRemaining("2026-08-18T15:09:00Z", NOW)).toBe("Snoozed · 59m");
    expect(snoozeRemaining("2026-08-18T14:00:00Z", NOW)).toBeNull();
    expect(snoozeRemaining(undefined, NOW)).toBeNull();
  });
});

describe("sortServices", () => {
  it("orders bands then longest in-state then name", () => {
    const rows = [
      view({
        name: "Web",
        state: "healthy",
        lastCheckAt: "2026-08-18T14:09:00Z",
      }),
      view({
        name: "Auth",
        state: "degraded",
        degradedSince: "2026-08-18T14:07:00Z",
        lastCheckAt: "2026-08-18T14:09:51Z",
      }),
      view({
        name: "Worker",
        state: "down",
        downSince: "2026-08-18T14:08:00Z",
      }),
      view({
        name: "Payments API",
        state: "down",
        downSince: "2026-08-18T14:04:00Z",
      }),
      view({ name: "Draft", state: "pending" }),
      view({ name: "Old NAS", state: "paused", lastCheckAt: "2026-08-18T13:00:00Z" }),
      view({
        name: "api",
        state: "healthy",
        lastCheckAt: "2026-08-18T14:00:00Z",
      }),
    ];
    expect(sortServices(rows, NOW).map((row) => row.name)).toEqual([
      "Payments API",
      "Worker",
      "Auth",
      "Draft",
      "Old NAS",
      "api",
      "Web",
    ]);
  });
});
