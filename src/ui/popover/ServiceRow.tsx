import {
  labelTone,
  markState,
  primaryLabel,
  relativeTime,
  snoozeRemaining,
} from "../../lib/format";
import type { ServiceView } from "../../lib/types";
import { StatusMark } from "../shared/StatusMark";
import { StatusPill } from "../shared/StatusPill";

type Props = {
  view: ServiceView;
  selected: boolean;
  checking: boolean;
  now: number;
  onSelect: (id: string) => void;
  onOpen: (id: string) => void;
};

export function ServiceRow({
  view,
  selected,
  checking,
  now,
  onSelect,
  onOpen,
}: Props) {
  const label = primaryLabel(view);
  const tone = labelTone(label);
  const paused = label === "Paused";
  const snooze = snoozeRemaining(view.snoozeUntil, now);
  const time = relativeTime(view, now, checking);

  return (
    <li>
      <button
        type="button"
        className={`service-row${selected ? " is-selected" : ""}${paused ? " is-paused" : ""}`}
        data-id={view.id}
        tabIndex={selected ? 0 : -1}
        aria-current={selected ? "true" : undefined}
        onClick={() => {
          onSelect(view.id);
          onOpen(view.id);
        }}
      >
        <StatusMark tone={markState(view)} dim={paused} />
        <span className="name">{view.name}</span>
        <StatusPill tone={tone}>{label}</StatusPill>
        <span className="meta">
          {time ? <span>{time}</span> : null}
          {snooze ? <StatusPill tone="snooze">{snooze}</StatusPill> : null}
        </span>
      </button>
    </li>
  );
}
