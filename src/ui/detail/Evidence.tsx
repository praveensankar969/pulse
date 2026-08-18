import {
  formatExpectedStatus,
  formatJson,
  formatLatency,
  formatOp,
  isShownError,
} from "../../lib/format";
import type { CheckResult, HeaderSpec, ServiceView } from "../../lib/types";
import { SecretValue } from "../shared/SecretValue";
import { TimeAgo } from "../shared/TimeAgo";

type Props = {
  view: ServiceView;
  last: CheckResult | null;
  now: number;
  onCopy: () => void;
  copied: boolean;
};

export function Evidence({ view, last, now, onCopy, copied }: Props) {
  const headers = view.headers;
  const skipped = last?.assertionSkipped === "head";
  const showError = last != null && isShownError(last.errorKind) && last.error;
  const stripped = last?.headersStrippedOnRedirect === true;

  return (
    <section className="evidence" aria-label="Last check">
      <dl className="kv">
        <dt>HTTP</dt>
        <dd>{last?.httpStatus ?? "—"}</dd>
        <dt>Latency</dt>
        <dd>{last?.latencyMs != null ? formatLatency(last.latencyMs) : "—"}</dd>
        <dt>Expected</dt>
        <dd>{formatExpectedStatus(view.expectedStatus)}</dd>
        <dt>Checked</dt>
        <dd>
          <TimeAgo at={last?.at ?? view.lastCheckAt} now={now} />
        </dd>
        <dt>URL</dt>
        <dd className="url">{view.url}</dd>
      </dl>

      {last && last.assertionResults.length > 0 ? (
        <table className="assert-table">
          <thead>
            <tr>
              <th>Path</th>
              <th>Op</th>
              <th>Expected</th>
              <th>Actual</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {last.assertionResults.map((row, index) => (
              <tr key={`${row.path}-${row.op}-${index}`}>
                <td>{row.path}</td>
                <td>{formatOp(row.op)}</td>
                <td>{formatJson(row.expected)}</td>
                <td>{formatJson(row.actual)}</td>
                <td className={row.ok ? "pass" : "fail"}>
                  {row.ok ? "pass" : "fail"}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : null}

      {showError ? (
        <p className="error-line" role="status">
          {last.error}
        </p>
      ) : null}
      {skipped ? (
        <p className="muted-note">
          HEAD responses have no body. Assertions were skipped.
        </p>
      ) : null}
      {stripped ? (
        <p className="warn-note">
          Secret headers were dropped after a cross-host or scheme-changing
          redirect.
        </p>
      ) : null}

      {last?.bodyPreview ? (
        <>
          <pre className="preview">{last.bodyPreview}</pre>
          <button type="button" className="btn" onClick={onCopy}>
            {copied ? "Copied" : "Copy response"}
          </button>
        </>
      ) : last ? (
        <pre className="preview">(empty)</pre>
      ) : null}

      {headers.length > 0 ? (
        <div className="header-list" aria-label="Request headers">
          {headers.map((header) => (
            <HeaderRow
              key={header.key}
              serviceId={view.id}
              header={header}
            />
          ))}
        </div>
      ) : null}
    </section>
  );
}

function HeaderRow({
  serviceId,
  header,
}: {
  serviceId: string;
  header: HeaderSpec;
}) {
  return (
    <div className="header-row">
      <span className="k">{header.key}</span>
      {header.secret ? (
        <SecretValue serviceId={serviceId} headerKey={header.key} />
      ) : (
        <span>{header.value ?? ""}</span>
      )}
    </div>
  );
}
