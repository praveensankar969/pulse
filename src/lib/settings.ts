import type { QuietHours } from "./types";

export const MIXED_REACHABILITY_HELP =
  "If any check succeeds, Pulse assumes the network is up. A homelab box that still answers will keep Pulse online even if the public internet is gone.";

export const DEFAULT_HOTKEY = "CommandOrControl+Shift+U";

export const DEFAULT_QUIET_HOURS: QuietHours = {
  start: "22:00",
  end: "08:00",
  days: [1, 2, 3, 4, 5],
};

export const INTERVAL_OPTIONS = [15, 30, 60, 120, 300, 600] as const;
export const TIMEOUT_OPTIONS = [5000, 10000, 15000, 30000] as const;

export const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"] as const;

export function resolvedHotkey(hotkey?: string): string {
  const trimmed = hotkey?.trim();
  return trimmed ? trimmed : DEFAULT_HOTKEY;
}

/** Commit a fail-threshold draft. Returns null when the field is not an integer. */
export function commitFailThreshold(raw: string): number | null {
  const trimmed = raw.trim();
  if (!/^-?\d+$/.test(trimmed)) return null;
  const parsed = Number(trimmed);
  return Math.min(10, Math.max(1, parsed));
}
