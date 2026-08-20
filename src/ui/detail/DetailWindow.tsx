import { useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  labelTone,
  markState,
  primaryLabel,
  reasonLine,
  snoozeRemaining,
  tomorrowEightLocal,
} from "../../lib/format";
import {
  checkNow,
  getDetail,
  onDetailService,
  onServices,
  openAction,
  openEditor,
  setPaused,
  snooze,
} from "../../lib/ipc";
import type { DetailPayload } from "../../lib/types";
import { StatusMark } from "../shared/StatusMark";
import { StatusPill } from "../shared/StatusPill";
import { Evidence } from "./Evidence";
import { Sparkline } from "./Sparkline";

function initialId(): string {
  if (typeof window !== "undefined" && window.__PULSE_DETAIL_ID__) {
    return window.__PULSE_DETAIL_ID__;
  }
  return new URLSearchParams(window.location.search).get("id") ?? "";
}

export function DetailWindow() {
  const [id, setId] = useState(initialId);
  const [detail, setDetail] = useState<DetailPayload | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const [checking, setChecking] = useState(false);
  const [copied, setCopied] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  const load = async (serviceId: string) => {
    if (!serviceId) {
      setError("No service selected.");
      return;
    }
    try {
      const next = await getDetail(serviceId);
      setDetail(next);
      setError(null);
      try {
        await getCurrentWindow().setTitle(next.view.name);
      } catch {
        document.title = next.view.name;
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  };

  useEffect(() => {
    void load(id);
  }, [id]);

  useEffect(() => {
    const tick = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(tick);
  }, []);

  useEffect(() => {
    const stops: Array<() => void> = [];
    void onDetailService((next) => setId(next)).then((stop) => stops.push(stop));
    void onServices((views) => {
      if (!id) return;
      if (views.some((view) => view.id === id)) void load(id);
    }).then((stop) => stops.push(stop));
    return () => {
      for (const stop of stops) stop();
    };
  }, [id]);

  useEffect(() => {
    if (!menuOpen) return;
    const onDoc = (event: MouseEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) {
        setMenuOpen(false);
      }
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [menuOpen]);

  const view = detail?.view;
  const last = detail?.last ?? view?.lastResult ?? null;
  const label = view ? primaryLabel(view) : null;
  const snoozePill = view ? snoozeRemaining(view.snoozeUntil, now) : null;
  const reason = view ? reasonLine(view, last) : "";

  const presets = useMemo(
    () => [
      { key: "15m", label: "15 minutes", until: () => new Date(Date.now() + 15 * 60_000).toISOString() },
      { key: "60m", label: "60 minutes", until: () => new Date(Date.now() + 60 * 60_000).toISOString() },
      { key: "tomorrow", label: "Until tomorrow 08:00", until: () => tomorrowEightLocal() },
    ],
    [],
  );

  const applySnooze = async (until: string | null) => {
    if (!id) return;
    setMenuOpen(false);
    try {
      await snooze(id, until);
      await load(id);
    } catch {
      // Next pulse://services snapshot is source of truth.
    }
  };

  if (error && !view) {
    return (
      <main className="detail">
        <p className="error-line">{error}</p>
      </main>
    );
  }

  if (!view || !label) {
    return (
      <main className="detail">
        <p className="muted-note">Loading…</p>
      </main>
    );
  }

  return (
    <main className="detail">
      <header className="detail-head">
        <div>
          <h1>{view.name}</h1>
          {reason ? <p className="reason">{reason}</p> : null}
        </div>
        <div className="detail-status">
          <StatusMark tone={markState(view)} dim={label === "Paused"} />
          <StatusPill tone={labelTone(label)}>{label}</StatusPill>
          {snoozePill ? <StatusPill tone="snooze">{snoozePill}</StatusPill> : null}
        </div>
      </header>

      <Evidence
        view={view}
        last={last}
        now={now}
        copied={copied}
        onCopy={() => {
          const body = last?.bodyPreview ?? "";
          void navigator.clipboard?.writeText(body)?.then(() => {
            setCopied(true);
            window.setTimeout(() => setCopied(false), 1500);
          });
        }}
      />

      <Sparkline samples24h={detail?.samples24h ?? []} now={now} />

      <div className="actions">
        <button
          type="button"
          className="btn primary"
          onClick={() => void openAction(view.id)}
        >
          Open
        </button>
        <button
          type="button"
          className="btn"
          disabled={checking}
          onClick={() => {
            setChecking(true);
            void checkNow(view.id)
              .then(() => load(view.id))
              .finally(() => setChecking(false));
          }}
        >
          {checking ? "Checking…" : "Check now"}
        </button>
        <button
          type="button"
          className="btn"
          onClick={() => {
            void setPaused(view.id, !view.paused).then(() => load(view.id));
          }}
        >
          {view.paused ? "Resume" : "Pause"}
        </button>
        <div className="snooze-wrap" ref={menuRef}>
          <button
            type="button"
            className="btn"
            aria-haspopup="menu"
            aria-expanded={menuOpen}
            onClick={() => setMenuOpen((open) => !open)}
          >
            Snooze ▾
          </button>
          {menuOpen ? (
            <div className="snooze-menu" role="menu">
              {presets.map((preset) => (
                <button
                  key={preset.key}
                  type="button"
                  role="menuitem"
                  onClick={() => void applySnooze(preset.until())}
                >
                  {preset.label}
                </button>
              ))}
              {view.snoozeUntil ? (
                <button
                  type="button"
                  role="menuitem"
                  onClick={() => void applySnooze(null)}
                >
                  Clear snooze
                </button>
              ) : null}
            </div>
          ) : null}
        </div>
        <button
          type="button"
          className="btn"
          onClick={() => void openEditor(view.id)}
        >
          Edit
        </button>
      </div>
    </main>
  );
}
