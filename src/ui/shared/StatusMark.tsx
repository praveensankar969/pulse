import type { LabelTone } from "../../lib/format";

type Props = {
  tone: LabelTone;
  dim?: boolean;
};

export function StatusMark({ tone, dim }: Props) {
  return (
    <span
      className={`dot ${tone}${dim ? " is-dim" : ""}`}
      aria-hidden="true"
    />
  );
}
