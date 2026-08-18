import type { LabelTone } from "../../lib/format";

type Props = {
  tone: LabelTone | "snooze";
  children: string;
};

export function StatusPill({ tone, children }: Props) {
  return <span className={`pill ${tone}`}>{children}</span>;
}
