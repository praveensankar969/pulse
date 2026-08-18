import { formatAssertionValue, parseAssertionValue } from "../../lib/assertValue";
import type { Assertion, AssertOp } from "../../lib/types";

const OPS: AssertOp[] = ["equals", "not_equals", "contains", "exists", "gt", "lt"];

export type AssertionRow = {
  path: string;
  op: AssertOp;
  valueText: string;
};

type Props = {
  rows: AssertionRow[];
  disabled: boolean;
  onChange: (rows: AssertionRow[]) => void;
};

export function AssertionsField({ rows, disabled, onChange }: Props) {
  if (disabled) {
    return (
      <div>
        <span className="field-label">JSON assertions</span>
        <p className="hint">HEAD responses have no body. Use GET to assert JSON.</p>
      </div>
    );
  }

  const update = (index: number, patch: Partial<AssertionRow>) => {
    onChange(rows.map((row, i) => (i === index ? { ...row, ...patch } : row)));
  };

  return (
    <div>
      <span className="field-label">JSON assertions — all must pass</span>
      {rows.map((row, index) => (
        <div key={index} className="assert-edit-row">
          <input
            className="mono"
            value={row.path}
            placeholder="status"
            aria-label="Assertion path"
            onChange={(event) => update(index, { path: event.target.value })}
          />
          <select
            value={row.op}
            aria-label="Assertion operator"
            onChange={(event) => update(index, { op: event.target.value as AssertOp })}
          >
            {OPS.map((op) => (
              <option key={op} value={op}>
                {op}
              </option>
            ))}
          </select>
          <input
            className="mono"
            value={row.op === "exists" ? "" : row.valueText}
            placeholder={row.op === "exists" ? "—" : "ok"}
            disabled={row.op === "exists"}
            aria-label="Assertion value"
            onChange={(event) => update(index, { valueText: event.target.value })}
            onBlur={(event) => {
              if (row.op === "exists") return;
              const parsed = parseAssertionValue(event.target.value);
              update(index, { valueText: formatAssertionValue(parsed) });
            }}
          />
          <button
            type="button"
            className="text-btn"
            aria-label="Remove assertion"
            onClick={() => onChange(rows.filter((_, i) => i !== index))}
          >
            ✕
          </button>
        </div>
      ))}
      <button
        type="button"
        className="text-btn"
        onClick={() =>
          onChange([...rows, { path: "", op: "equals", valueText: "" }])
        }
      >
        + Assertion
      </button>
      <p className="hint">
        Paths are dot notation from the JSON root. <span className="mono">$</span> is
        optional. <span className="mono">$</span> alone is the root.
        <br />
        <span className="mono">status</span> · <span className="mono">$.status</span> ·{" "}
        <span className="mono">$.data.healthy</span> ·{" "}
        <span className="mono">items.0.id</span> ·{" "}
        <span className="mono">items[0].id</span> ·{" "}
        <span className="mono">errors.length</span>
        <br />
        Hyphenated keys need brackets: <span className="mono">["error-code"]</span> or{" "}
        <span className="mono">$["error-code"]</span>.
        <br />
        <span className="mono">length</span> is the array or string length. To read a
        field named <span className="mono">length</span>, use{" "}
        <span className="mono">obj["length"]</span>.
      </p>
    </div>
  );
}

export function rowsFromAssertions(assertions: Assertion[]): AssertionRow[] {
  return assertions.map((assertion) => ({
    path: assertion.path,
    op: assertion.op,
    valueText: formatAssertionValue(assertion.value),
  }));
}

export function assertionsFromRows(rows: AssertionRow[]): Assertion[] {
  return rows
    .filter((row) => row.path.trim().length > 0)
    .map((row) => {
      const assertion: Assertion = { path: row.path.trim(), op: row.op };
      if (row.op !== "exists") {
        assertion.value = parseAssertionValue(row.valueText);
      }
      return assertion;
    });
}
