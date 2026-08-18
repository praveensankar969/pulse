/** UI mask. Never persist or send this string as a header value. */
export const SECRET_MASK = "••••••••";

export type HttpMethod = "GET" | "HEAD" | "POST";
/** Flap-damped machine state after on_result. Never produced by evaluate(). */
export type ServiceStatus = "healthy" | "degraded" | "down";
export type UiState = ServiceStatus | "paused" | "pending";
export type OutcomeClass = "ok" | "soft" | "hard";
export type Theme = "system" | "dark" | "light";
export type AssertOp =
  | "equals"
  | "not_equals"
  | "contains"
  | "exists"
  | "gt"
  | "lt";

export type ExpectedStatus = "2xx" | number | number[];

export interface Header {
  key: string;
  /** Always "" or masked "••••••••" on the wire to the UI. */
  value: string;
  secret: boolean;
  /** True when a keychain item exists. UI uses this to show the mask. */
  hasValue: boolean;
}

/** Persisted header. Secret values never sit in services.json. */
export interface HeaderSpec {
  key: string;
  secret: boolean;
  /** Plaintext only when secret is false. */
  value?: string;
}

export interface Assertion {
  path: string;
  op: AssertOp;
  /** JSON value. Omitted for `exists`. */
  value?: unknown;
}

/** Persisted config. No snooze, no last result, no consecutive fails. */
export interface Service {
  id: string; // ulid
  name: string;
  url: string;
  method: HttpMethod;
  headers: HeaderSpec[];
  body?: string; // POST only; plaintext on disk
  intervalSec: number; // UI offers 15|30|60|120|300|600; store the number
  timeoutMs: number;
  expectedStatus: ExpectedStatus;
  assertions: Assertion[];
  maxLatencyMs?: number;
  actionUrl?: string;
  notify: boolean;
  alwaysAlert: boolean;
  paused: boolean;
  followRedirects: boolean; // default true
  failThreshold?: number; // omit = inherit; never persist JSON null
  group?: string; // stored in v1, no filter UI
  createdAt: string;
  updatedAt: string;
}

export interface AssertionResult {
  path: string;
  op: AssertOp;
  ok: boolean;
  expected?: unknown;
  actual?: unknown;
  reason?: string; // "missing" | "not numeric" | "not containable" | "invalid_path"
}

export type ErrorKind =
  | "timeout"
  | "dns"
  | "tls_untrusted"
  | "tls_expired"
  | "tls_hostname"
  | "tls_other"
  | "refused"
  | "reset"
  | "unreachable"
  | "too_many_redirects"
  | "redirect_downgrade"
  | "unexpected_status"
  | "body_parse"
  | "assertion"
  | "slow"
  | "canceled"
  | "offline"
  | "invalid_url"
  | "missing_secret";

/** Evaluator output. No flap-damped status. */
export interface CheckEvidence {
  at: string;
  outcome: OutcomeClass;
  httpStatus?: number;
  latencyMs?: number;
  redirects?: number;
  headersStrippedOnRedirect?: boolean;
  assertionResults: AssertionResult[];
  assertionSkipped?: "head";
  errorKind?: ErrorKind;
  error?: string; // user-facing, only real failures
  bodyPreview?: string; // ≤ 2048 chars
}

/** Live check after on_result. test_draft returns CheckEvidence, not this. */
export interface CheckResult extends CheckEvidence {
  state: ServiceStatus;
}

export interface ServiceView extends Service {
  state: UiState;
  /** Runtime only. Never on Service / services.json / export. */
  snoozeUntil?: string;
  /** True when keychain read failed after a signing-identity change. */
  keychainIdentityChanged?: boolean;
  lastResult?: CheckResult;
  lastCheckAt?: string;
  downSince?: string;
  /** When the machine entered degraded. Independent of lastCheckAt. */
  degradedSince?: string;
  /** Subtracted from now - downSince for displayed down duration. */
  downClockAdjustMs?: number;
  consecutiveHardFails: number;
  /** Post-machine states; "gap" = canceled / offline-frozen / not-yet-checked. */
  sparkline24: Array<ServiceStatus | "gap">;
}

export interface CompactSample {
  at: string;
  /** Post-machine. Sparkline "red run" uses this. */
  state: ServiceStatus;
  outcome: OutcomeClass;
  httpStatus?: number;
  latencyMs?: number;
  errorKind?: ErrorKind;
}

export interface QuietHours {
  start: string; // "HH:MM" 24h local
  end: string;
  days: number[]; // 0=Sun .. 6=Sat
}

export interface AppSettings {
  launchAtLogin: boolean;
  hotkey?: string; // e.g. "CommandOrControl+Shift+U"
  theme: Theme;
  defaultInterval: number;
  defaultTimeoutMs: number;
  failThreshold: number; // default 3
  notifications: boolean;
  sound: boolean;
  quietHours?: QuietHours;
  lastExportAt?: string;
  askedLaunchAtLogin: boolean;
}

export interface ServiceDraft {
  /** Existing id for edit; omit for create. */
  id?: string;
  name: string;
  url: string;
  method: HttpMethod;
  headers: Array<{
    key: string;
    value?: string; // omit to keep existing secret; never the mask string
    secret: boolean;
    clear?: boolean; // drop keychain item
  }>;
  body?: string;
  intervalSec: number;
  timeoutMs: number;
  expectedStatus: ExpectedStatus;
  followRedirects?: boolean; // default true
  assertions: Assertion[];
  maxLatencyMs?: number;
  actionUrl?: string;
  notify: boolean;
  alwaysAlert: boolean;
  failThreshold?: number;
  group?: string;
}
