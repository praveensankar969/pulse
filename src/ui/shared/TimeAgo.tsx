import { formatAbsolute, formatRelative } from "../../lib/format";

type Props = {
  at?: string;
  now: number;
};

export function TimeAgo({ at, now }: Props) {
  if (!at) return <span>—</span>;
  const relative = formatRelative(at, now);
  return (
    <span className="time-ago" title={formatAbsolute(at)}>
      <span className="time-ago-abs">{formatAbsolute(at)}</span>
      {relative ? <span className="time-ago-rel">{relative}</span> : null}
    </span>
  );
}
