import { useEffect, useState } from "react";
import {
  labelTone,
  markState,
  primaryLabel,
  snoozeRemaining,
} from "../../lib/format";
import { listServices, onServices, openDetail, openSettings } from "../../lib/ipc";
import type { ServiceView } from "../../lib/types";
import { StatusMark } from "../shared/StatusMark";
import { StatusPill } from "../shared/StatusPill";

export function PopoverWindow() {
  const [views, setViews] = useState<ServiceView[]>([]);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    void listServices()
      .then(setViews)
      .catch(() => setViews([]));
    let stop: (() => void) | undefined;
    void onServices(setViews).then((unlisten) => {
      stop = unlisten;
    });
    return () => stop?.();
  }, []);

  useEffect(() => {
    const tick = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(tick);
  }, []);

  return (
    <main className="popover">
      <header className="popover-head">
        <h1 className="wordmark">Pulse</h1>
      </header>
      {views.length === 0 ? (
        <div className="empty-state">
          <p>
            Add the HTTP endpoints you own. Pulse will watch them from the tray.
          </p>
        </div>
      ) : (
        <ul className="service-list">
          {views.map((view) => {
            const label = primaryLabel(view);
            const snooze = snoozeRemaining(view.snoozeUntil, now);
            return (
              <li key={view.id}>
                <button
                  type="button"
                  className={`service-row${view.paused ? " is-paused" : ""}`}
                  onClick={() => void openDetail(view.id)}
                >
                  <StatusMark tone={markState(view)} dim={label === "Paused"} />
                  <span className="name">{view.name}</span>
                  <StatusPill tone={labelTone(label)}>{label}</StatusPill>
                  <span className="meta">
                    {snooze ? <StatusPill tone="snooze">{snooze}</StatusPill> : null}
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      )}
      <footer className="popover-foot">
        <span className="foot-spacer" />
        <button type="button" className="text-btn" onClick={() => void openSettings()}>
          Settings
        </button>
      </footer>
    </main>
  );
}
