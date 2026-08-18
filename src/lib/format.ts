import type { ServiceView, UiState } from "./types";

export type PrimaryLabel =
  | "Down"
  | "Slow"
  | "Degraded"
  | "Pending"
  | "Paused"
  | "Healthy";

export type LabelTone =
  | "down"
  | "slow"
  | "degraded"
  | "pending"
  | "paused"
  | "healthy";

export type StripTone = "down" | "warn" | "ok" | "neutral";

const BAND: Record<UiState, number> = {
  down: 0,
  degraded: 1,
  pending: 2,
  paused: 3,
  healthy: 4,
};

export function isSlow(view: ServiceView): boolean {
  if (view.state !== "degraded") return false;
  const result = view.lastResult;
  return result?.outcome === "soft" || result?.errorKind === "slow";
}

export function primaryLabel(view: ServiceView): PrimaryLabel {
  if (view.state === "paused" || view.paused) return "Paused";
  if (view.state === "pending") return "Pending";
  if (view.state === "down") return "Down";
  if (view.state === "degraded") return isSlow(view) ? "Slow" : "Degraded";
  return "Healthy";
}

export function labelTone(label: PrimaryLabel): LabelTone {
  switch (label) {
    case "Down":
      return "down";
    case "Slow":
      return "slow";
    case "Degraded":
      return "degraded";
    case "Pending":
      return "pending";
    case "Paused":
      return "paused";
    case "Healthy":
      return "healthy";
  }
}

/** Last known machine mark; paused rows keep this color at 40% opacity. */
export function markState(view: ServiceView): LabelTone {
  const label = primaryLabel(view);
  if (label !== "Paused") return labelTone(label);
  const last = view.lastResult?.state;
  if (last === "down" || last === "degraded" || last === "healthy") {
    return last === "degraded" && isSlow(view) ? "slow" : last;
  }
  return "paused";
}

export function formatCompactDuration(ms: number): string {
  const n = Math.max(0, ms);
  if (n < 60_000) return `${Math.floor(n / 1000)}s`;
  if (n < 3_600_000) return `${Math.round(n / 60_000)}m`;
  return `${Math.round(n / 3_600_000)}h`;
}

export function downDurationMs(view: ServiceView, now: number): number | null {
  if (view.state !== "down" || !view.downSince) return null;
  const since = Date.parse(view.downSince);
  if (Number.isNaN(since)) return null;
  const adjust = view.downClockAdjustMs ?? 0;
  return Math.max(0, now - since - adjust);
}

export function degradedDurationMs(view: ServiceView, now: number): number | null {
  if (view.state !== "degraded") return null;
  const stamp = view.degradedSince ?? view.lastCheckAt;
  if (!stamp) return null;
  const at = Date.parse(stamp);
  if (Number.isNaN(at)) return null;
  return Math.max(0, now - at);
}

export function timeInStateMs(view: ServiceView, now: number): number {
  const down = downDurationMs(view, now);
  if (down != null) return down;
  const degraded = degradedDurationMs(view, now);
  if (degraded != null) return degraded;
  const stamp = view.lastCheckAt ?? view.createdAt;
  const at = Date.parse(stamp);
  if (Number.isNaN(at)) return 0;
  return Math.max(0, now - at);
}

export function sortServices(views: ServiceView[], now: number): ServiceView[] {
  return [...views].sort((a, b) => {
    const band = BAND[a.state] - BAND[b.state];
    if (band !== 0) return band;
    const time = timeInStateMs(b, now) - timeInStateMs(a, now);
    if (time !== 0) return time;
    return a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
  });
}

export function relativeTime(
  view: ServiceView,
  now: number,
  checking: boolean,
): string {
  if (view.state === "pending" || checking) return "Checking…";
  if (view.state === "down") {
    const down = downDurationMs(view, now);
    if (down == null) return "";
    return `down ${formatCompactDuration(down)}`;
  }
  if (view.state === "degraded") {
    const age = degradedDurationMs(view, now) ?? timeInStateMs(view, now);
    return `degraded ${formatCompactDuration(age)}`;
  }
  if (!view.lastCheckAt) return "";
  const at = Date.parse(view.lastCheckAt);
  if (Number.isNaN(at)) return "";
  return `${formatCompactDuration(Math.max(0, now - at))} ago`;
}

export function snoozeRemaining(
  until: string | undefined,
  now: number,
): string | null {
  if (!until) return null;
  const at = Date.parse(until);
  if (Number.isNaN(at) || at <= now) return null;
  return `Snoozed · ${formatCompactDuration(at - now)}`;
}

export function summary(views: ServiceView[]): {
  countText: string;
  stripText: string;
  stripTone: StripTone;
} {
  if (views.length === 0) {
    return {
      countText: "No services",
      stripText: "Add a check to start watching.",
      stripTone: "neutral",
    };
  }

  const noun = views.length === 1 ? "service" : "services";
  const active = views.filter((view) => !view.paused && view.state !== "paused");
  const downs = active.filter((view) => view.state === "down");
  const degraded = active.filter((view) => view.state === "degraded");

  const countText = downs.length
    ? `${views.length} ${noun} · ${downs.length} down`
    : `${views.length} ${noun}`;

  if (downs.length) {
    return {
      countText,
      stripText: `${downs.length} down · ${downs.map((view) => view.name).join(", ")}`,
      stripTone: "down",
    };
  }

  if (degraded.length) {
    const slow = degraded.filter(isSlow);
    const kind = slow.length === degraded.length ? "slow" : "degraded";
    const named = kind === "slow" ? slow : degraded;
    return {
      countText,
      stripText: `${named.length} ${kind} · ${named.map((view) => view.name).join(", ")}`,
      stripTone: "warn",
    };
  }

  if (active.length === 0) {
    return { countText, stripText: "All paused", stripTone: "neutral" };
  }
  if (active.every((view) => view.state === "pending")) {
    return { countText, stripText: "Checking…", stripTone: "neutral" };
  }
  return { countText, stripText: "All healthy", stripTone: "ok" };
}
