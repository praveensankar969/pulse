import { useEffect, useMemo, useRef } from "react";
import { sortServices, summary } from "../../lib/format";
import {
  addService,
  checkEvery,
  checkSelected,
  closePopover,
  openSelectedAction,
  openSelectedDetail,
  openSettings,
  quitApp,
  selectService,
  togglePauseSelected,
  usePulseStore,
} from "../../state/store";
import { EmptyState } from "./EmptyState";
import { Footer } from "./Footer";
import { ServiceRow } from "./ServiceRow";

export function Popover() {
  const { services, selectedId, pollerDead, checkingIds, now, ready } = usePulseStore();
  const listRef = useRef<HTMLUListElement>(null);
  const sorted = useMemo(() => sortServices(services, now), [services, now]);
  const head = useMemo(() => summary(services), [services]);
  const checking = useMemo(() => new Set(checkingIds), [checkingIds]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "n") {
        event.preventDefault();
        void addService();
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        void closePopover();
        return;
      }

      const idx = sorted.findIndex((view) => view.id === selectedId);
      if (event.key === "ArrowDown") {
        event.preventDefault();
        const next = sorted[Math.min(sorted.length - 1, Math.max(0, idx) + 1)];
        if (next) selectService(next.id);
        return;
      }
      if (event.key === "ArrowUp") {
        event.preventDefault();
        const next = sorted[Math.max(0, idx - 1)];
        if (next) selectService(next.id);
        return;
      }
      if (event.key.toLowerCase() === "r" && !event.metaKey && !event.ctrlKey) {
        event.preventDefault();
        void checkSelected();
        return;
      }
      if (event.key === "Enter") {
        const target = event.target;
        if (
          target instanceof HTMLElement &&
          target.closest("button") &&
          !target.closest(".service-row")
        ) {
          return;
        }
        event.preventDefault();
        if (event.shiftKey) void openSelectedDetail();
        else void openSelectedAction();
        return;
      }
      if (event.key.toLowerCase() === "p" && !event.metaKey && !event.ctrlKey) {
        event.preventDefault();
        void togglePauseSelected();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [sorted, selectedId]);

  useEffect(() => {
    const row = listRef.current?.querySelector(`[data-id="${selectedId}"]`);
    if (row instanceof HTMLElement) {
      row.scrollIntoView({ block: "nearest" });
    }
  }, [selectedId]);

  return (
    <main className="popover" role="dialog" aria-label="Pulse">
      <header className="popover-head">
        <div className="popover-title">
          <h1 className="wordmark">Pulse</h1>
          <span className="count">{ready ? head.countText : ""}</span>
        </div>
        <button
          type="button"
          className="icon-btn"
          title="Add service"
          aria-label="Add service"
          onClick={() => void addService()}
        >
          +
        </button>
      </header>
      {pollerDead ? (
        <div className="poller-dead" role="alert">
          Pulse's checker stopped — restart the app.
        </div>
      ) : null}
      <div className={`summary-strip is-${ready ? head.stripTone : "neutral"}`}>
        {ready ? head.stripText : ""}
      </div>
      {!ready ? null : sorted.length === 0 ? (
        <EmptyState onAdd={() => void addService()} />
      ) : (
        <ul className="service-list" ref={listRef}>
          {sorted.map((view) => (
            <ServiceRow
              key={view.id}
              view={view}
              selected={view.id === selectedId}
              checking={checking.has(view.id)}
              now={now}
              onSelect={selectService}
              onOpen={() => {
                selectService(view.id);
                void openSelectedDetail();
              }}
            />
          ))}
        </ul>
      )}
      <Footer
        onCheckAll={() => void checkEvery()}
        onSettings={() => void openSettings()}
        onQuit={() => void quitApp()}
      />
    </main>
  );
}
