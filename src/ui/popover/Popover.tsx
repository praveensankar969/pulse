import { useEffect, useMemo, useRef, useState } from "react";
import { sortServices, summary } from "../../lib/format";
import { onPopoverShown } from "../../lib/ipc";
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
import { BrandMark } from "../shared/BrandMark";
import { EmptyState } from "./EmptyState";
import { Footer } from "./Footer";
import { ServiceRow } from "./ServiceRow";

export function Popover() {
  const { services, selectedId, pollerDead, checkingIds, now, ready } = usePulseStore();
  const listRef = useRef<HTMLUListElement>(null);
  const keyboardNav = useRef(false);
  const [entering, setEntering] = useState(false);
  const sorted = useMemo(() => sortServices(services, now), [services, now]);
  const head = useMemo(() => summary(services), [services]);
  const checking = useMemo(() => new Set(checkingIds), [checkingIds]);
  const pillText = head.stripText.split(" · ")[0];

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
        if (next) {
          keyboardNav.current = true;
          selectService(next.id);
        }
        return;
      }
      if (event.key === "ArrowUp") {
        event.preventDefault();
        const next = sorted[Math.max(0, idx - 1)];
        if (next) {
          keyboardNav.current = true;
          selectService(next.id);
        }
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
    if (!selectedId) return;
    const row = listRef.current?.querySelector(
      `[data-id="${CSS.escape(selectedId)}"]`,
    );
    if (row instanceof HTMLElement) {
      row.scrollIntoView({ block: "nearest" });
      if (keyboardNav.current) {
        keyboardNav.current = false;
        if (document.activeElement !== row) row.focus();
      }
    }
  }, [selectedId]);

  useEffect(() => {
    let stop: (() => void) | undefined;
    void onPopoverShown(() => {
      setEntering(false);
      requestAnimationFrame(() => {
        requestAnimationFrame(() => setEntering(true));
      });
    }).then((unlisten) => {
      stop = unlisten;
    });
    return () => stop?.();
  }, []);

  return (
    <main
      className={`popover${entering ? " is-entering" : ""}`}
      role="dialog"
      aria-label="Pulse"
      onAnimationEnd={() => setEntering(false)}
    >
      <header className="popover-head">
        <div className="pop-brand">
          <BrandMark />
          <div>
            <h1 className="wordmark">Pulse</h1>
            <p className="count">{ready ? head.countText : ""}</p>
          </div>
        </div>
        {ready ? (
          <span className={`pill ${head.stripTone}`}>{pillText}</span>
        ) : null}
      </header>
      {pollerDead ? (
        <div className="poller-dead" role="alert">
          Pulse's checker stopped — restart the app.
        </div>
      ) : null}
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
        onAdd={() => void addService()}
        onSettings={() => void openSettings()}
        onQuit={() => void quitApp()}
      />
    </main>
  );
}
