import { formatAbsolute, formatRelative } from "../../lib/format";

type Props = {
  at?: string;
  now: number;
};

export function TimeAgo({ at, now }: Props) {
  if (!at) return <span>—</span>;
  const relative = formatRelative(at, now);
  return (
    <span title={formatAbsolute(at)}>
      {formatAbsolute(at)}
      {relative ? ` · ${relative}` : ""}
    </span>
  );
}
