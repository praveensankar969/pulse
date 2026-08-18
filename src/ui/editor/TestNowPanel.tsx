import type { CheckEvidence } from "../../lib/types";

type Props = {
  evidence: CheckEvidence | null;
  busy: boolean;
  message?: string;
};

export function TestNowPanel({ evidence, busy, message }: Props) {
  if (busy) {
    return <div className="test-panel">Checking…</div>;
  }
  if (message) {
    return <div className="test-panel fail">{message}</div>;
  }
  if (!evidence) {
    return (
      <div className="test-panel">Test now runs one request and does not save.</div>
    );
  }

  const pass = evidence.outcome === "ok";
  const status = evidence.httpStatus != null ? `HTTP ${evidence.httpStatus}` : null;
  const latency = evidence.latencyMs != null ? `${evidence.latencyMs}ms` : null;
  const summary = [pass ? "Passed" : "Failed", status, latency].filter(Boolean).join(" · ");
  const asserts = evidence.assertionResults
    .map((result) => {
      const mark = result.ok ? "equals" : "expected";
      const extra =
        result.ok || result.actual === undefined
          ? ""
          : ` ${mark} ${fmt(result.expected)}, got ${fmt(result.actual)}`;
      return `${result.path} ${result.ok ? `${result.op} ${fmt(result.expected)}` : extra || result.reason || "failed"}`;
    })
    .join(" · ");

  return (
    <div className={`test-panel ${pass ? "pass" : "fail"}`}>
      <strong>{summary}</strong>
      {evidence.error ? (
        <>
          <br />
          <span className="mono">{evidence.error}</span>
        </>
      ) : asserts ? (
        <>
          <br />
          <span className="mono">{asserts}</span>
        </>
      ) : null}
    </div>
  );
}

function fmt(value: unknown): string {
  if (value === undefined) return "<missing>";
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}
