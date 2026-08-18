import { isMaskLike, SECRET_MASK } from "../../lib/types";

export type HeaderRow = {
  key: string;
  value: string;
  secret: boolean;
  hadValue: boolean;
};

type Props = {
  rows: HeaderRow[];
  missingKey?: string;
  onChange: (rows: HeaderRow[]) => void;
};

export function HeadersField({ rows, missingKey, onChange }: Props) {
  const update = (index: number, patch: Partial<HeaderRow>) => {
    onChange(rows.map((row, i) => (i === index ? { ...row, ...patch } : row)));
  };

  return (
    <div className="headers-edit">
      <span className="field-label">Headers</span>
      {rows.map((row, index) => {
        const missing =
          row.secret && missingKey && row.key.toLowerCase() === missingKey.toLowerCase();
        const masked = isMaskLike(row.value);
        return (
          <div key={index} className="header-edit-row">
            <input
              className="mono"
              value={row.key}
              placeholder="Authorization"
              aria-label="Header name"
              onChange={(event) => update(index, { key: event.target.value })}
            />
            <input
              className="mono"
              value={row.value}
              placeholder={row.secret ? "secret value" : "value"}
              type={row.secret && masked ? "password" : "text"}
              autoComplete="off"
              aria-label="Header value"
              onFocus={(event) => {
                if (masked) event.currentTarget.select();
              }}
              onChange={(event) => update(index, { value: event.target.value })}
            />
            <label className="check-row tight">
              <input
                type="checkbox"
                checked={row.secret}
                onChange={(event) => {
                  const secret = event.target.checked;
                  if (!secret && masked) {
                    update(index, { secret: false, value: "" });
                    return;
                  }
                  if (secret && row.hadValue && !row.value) {
                    update(index, { secret: true, value: SECRET_MASK });
                    return;
                  }
                  update(index, { secret });
                }}
              />
              secret
            </label>
            <button
              type="button"
              className="text-btn"
              aria-label="Remove header"
              onClick={() => onChange(rows.filter((_, i) => i !== index))}
            >
              ✕
            </button>
            {missing ? (
              <p className="hint danger-text span-all">
                Secret header {row.key || missingKey} is not set
              </p>
            ) : null}
          </div>
        );
      })}
      <button
        type="button"
        className="text-btn"
        onClick={() =>
          onChange([...rows, { key: "", value: "", secret: false, hadValue: false }])
        }
      >
        + Header
      </button>
    </div>
  );
}

export function draftHeaders(rows: HeaderRow[]) {
  return rows
    .filter((row) => row.key.trim().length > 0)
    .map((row) => {
      const key = row.key.trim();
      const maskLike = isMaskLike(row.value);
      if (!row.secret) {
        // Uncheck-while-masked must not persist the bullets.
        return { key, secret: false, value: maskLike ? "" : row.value };
      }
      if (row.value === "" && row.hadValue) {
        return { key, secret: true, clear: true };
      }
      if (row.value && !maskLike) {
        return { key, secret: true, value: row.value };
      }
      return { key, secret: true };
    });
}
