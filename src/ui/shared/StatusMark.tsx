import type { LabelTone } from "../../lib/format";

type Props = {
  tone: LabelTone;
  dim?: boolean;
  checking?: boolean;
};

export function StatusMark({ tone, dim, checking }: Props) {
  return (
    <span
      className={`dot ${tone}${dim ? " is-dim" : ""}${checking ? " is-checking" : ""}`}
      aria-hidden="true"
    />
  );
}
