import type {
  AssertOp,
  CheckResult,
  CompactSample,
  ErrorKind,
  ExpectedStatus,
  QuietHours,
  ServiceStatus,
  ServiceView,
  UiState,
} from "./types";

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

export type SparkPoint = ServiceStatus | "gap";

const HIDDEN_ERRORS: ErrorKind[] = ["canceled", "offline"];
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

export function isShownError(kind?: ErrorKind): boolean {
  return kind != null && !HIDDEN_ERRORS.includes(kind);
}

export function formatLatency(ms: number): string {
  if (ms >= 1000) {
    const secs = ms / 1000;
    return Number.isInteger(secs) ? `${secs}s` : `${secs.toFixed(2)}s`;
  }
  return `${ms}ms`;
}

export function formatAbsolute(iso: string): string {
  const at = Date.parse(iso);
  if (Number.isNaN(at)) return iso;
  return new Date(at).toLocaleString(undefined, {
    day: "numeric",
    month: "short",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  });
}

export function formatCompactDuration(ms: number): string {
  const n = Math.max(0, ms);
  if (n < 60_000) return `${String(Math.floor(n / 1000)).padStart(2, "0")}s`;
  if (n < 3_600_000) return `${String(Math.round(n / 60_000)).padStart(2, "0")}m`;
  return `${String(Math.round(n / 3_600_000)).padStart(2, "0")}h`;
}

export function formatRelative(iso: string | undefined, now: number): string {
  if (!iso) return "";
  const at = Date.parse(iso);
  if (Number.isNaN(at)) return "";
  return `${formatCompactDuration(Math.max(0, now - at))} ago`;
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

export function tomorrowEightLocal(now = new Date()): string {
  const next = new Date(now);
  next.setDate(next.getDate() + 1);
  next.setHours(8, 0, 0, 0);
  return next.toISOString();
}

export const DEFAULT_QUIET_HOURS: QuietHours = {
  start: "22:00",
  end: "08:00",
  days: [1, 2, 3, 4, 5],
};

export const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"] as const;

function parseHhmm(value: string): [number, number] | null {
  if (!/^\d{2}:\d{2}$/.test(value)) return null;
  const hour = Number(value.slice(0, 2));
  const minute = Number(value.slice(3, 5));
  if (hour > 23 || minute > 59) return null;
  return [hour, minute];
}

/** `days` names the day the window starts. Overnight continues into D+1. */
export function inQuietWindow(hours: QuietHours, local: Date): boolean {
  const start = parseHhmm(hours.start);
  const end = parseHhmm(hours.end);
  if (!start || !end || hours.days.length === 0) return false;
  const weekday = local.getDay();
  const yesterday = (weekday + 6) % 7;
  const minutes = local.getHours() * 60 + local.getMinutes();
  const startMin = start[0] * 60 + start[1];
  const endMin = end[0] * 60 + end[1];
  const selected = (day: number) => hours.days.includes(day);
  if (startMin < endMin) {
    return selected(weekday) && minutes >= startMin && minutes < endMin;
  }
  return (
    (selected(weekday) && minutes >= startMin) ||
    (selected(yesterday) && minutes < endMin)
  );
}

/** Fixture: Mon–Fri 22:00–08:00 covers Friday night into Saturday morning. */
export function overnightFridaySaturdayHolds(hours: QuietHours): boolean {
  const cases: Array<[number, number, number, number, number, boolean]> = [
    [2026, 7, 21, 21, 59, false],
    [2026, 7, 21, 22, 0, true],
    [2026, 7, 21, 23, 30, true],
    [2026, 7, 22, 0, 0, true],
    [2026, 7, 22, 7, 59, true],
    [2026, 7, 22, 8, 0, false],
    [2026, 7, 22, 22, 0, false],
    [2026, 7, 23, 7, 59, false],
  ];
  return cases.every(([year, month, day, hour, minute, want]) => {
    const local = new Date(year, month, day, hour, minute, 0, 0);
    return inQuietWindow(hours, local) === want;
  });
}

export function formatExpectedStatus(status: ExpectedStatus): string {
  if (status === "2xx") return "2xx";
  if (Array.isArray(status)) return status.join(", ");
  return String(status);
}

export function formatOp(op: AssertOp): string {
  switch (op) {
    case "not_equals":
      return "not equals";
    case "equals":
    case "contains":
    case "exists":
    case "gt":
    case "lt":
      return op;
  }
}

export function formatJson(value: unknown): string {
  if (value === undefined) return "—";
  if (typeof value === "string") return JSON.stringify(value);
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

/** Omitted assertion expected/actual (serde skip). Never-evaluated rows stay `—`. */
export function formatAssertionValue(value: unknown): string {
  if (value === undefined) return "<missing>";
  return formatJson(value);
}

export function reasonLine(
  view: ServiceView,
  last: CheckResult | null,
): string {
  if (view.keychainIdentityChanged) {
    return "Keychain identity changed; re-enter secret headers";
  }
  if (last && isShownError(last.errorKind) && last.error) return last.error;
  if (last?.httpStatus != null && last.latencyMs != null) {
    return `HTTP ${last.httpStatus} · ${formatLatency(last.latencyMs)}`;
  }
  if (last?.httpStatus != null) return `HTTP ${last.httpStatus}`;
  if (view.state === "pending") return "Waiting for first check";
  if (view.state === "paused") return "Paused";
  if (view.state === "healthy") return "Last check passed";
  return "";
}

const RANK: Record<ServiceStatus, number> = {
  healthy: 1,
  degraded: 2,
  down: 3,
};

export function bucket24h(
  samples: CompactSample[],
  now: number,
): SparkPoint[] {
  const bucketMs = 5 * 60 * 1000;
  const count = (24 * 60) / 5;
  const start = now - 24 * 60 * 60 * 1000;
  const buckets: SparkPoint[] = Array.from({ length: count }, () => "gap");
  for (const sample of samples) {
    const at = Date.parse(sample.at);
    if (Number.isNaN(at)) continue;
    const index = Math.floor((at - start) / bucketMs);
    if (index < 0 || index >= count) continue;
    const prev = buckets[index];
    if (prev === "gap" || RANK[sample.state] > RANK[prev]) {
      buckets[index] = sample.state;
    }
  }
  return buckets;
}

export function padSparkline(points: SparkPoint[]): SparkPoint[] {
  if (points.length >= 24) return points.slice(-24);
  return [...Array<SparkPoint>(24 - points.length).fill("gap"), ...points];
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
