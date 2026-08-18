import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  answerLaunchPrompt,
  getSettings,
  onAskLaunchAtLogin,
  onSettings,
  pendingLaunchPrompt,
  updateSettings,
} from "../../lib/ipc";
import {
  commitFailThreshold as clampFailThreshold,
  DEFAULT_QUIET_HOURS,
  INTERVAL_OPTIONS,
  MIXED_REACHABILITY_HELP,
  resolvedHotkey,
  TIMEOUT_OPTIONS,
  WEEKDAYS,
} from "../../lib/settings";
import { applyTheme } from "../../lib/theme";
import type { AppSettings, QuietHours, Theme } from "../../lib/types";

type Pane = "general" | "notifications" | "defaults" | "data";

const PANES: Array<{ id: Pane; label: string }> = [
  { id: "general", label: "General" },
  { id: "notifications", label: "Notifications" },
  { id: "defaults", label: "Defaults" },
  { id: "data", label: "Data" },
];
  DEFAULT_QUIET_HOURS,
  inQuietWindow,
  WEEKDAYS,
} from "../../lib/format";
import { getSettings, onSettings, updateSettings } from "../../lib/ipc";
import type { AppSettings, QuietHours } from "../../lib/types";

function cloneSettings(settings: AppSettings): AppSettings {
  return {
    ...settings,
    quietHours: settings.quietHours
      ? { ...settings.quietHours, days: [...settings.quietHours.days] }
      : undefined,
  };
}

export function SettingsWindow() {
  const [pane, setPane] = useState<Pane>("general");
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [askLaunch, setAskLaunch] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [includeSecrets, setIncludeSecrets] = useState(false);
  const [resetTyped, setResetTyped] = useState("");
  const [failDraft, setFailDraft] = useState("3");
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const tick = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(tick);
  }, []);

  useEffect(() => {
    let stop: (() => void) | undefined;
    void (async () => {
      try {
        const loaded = await getSettings();
        setSettings(loaded);
        setFailDraft(String(loaded.failThreshold));
        applyTheme(loaded.theme);
        if (await pendingLaunchPrompt()) setAskLaunch(true);
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
      }
      const unlisten: Array<() => void> = [];
      try {
        unlisten.push(
          await onSettings((next) => {
            setSettings(next);
            setFailDraft(String(next.failThreshold));
            applyTheme(next.theme);
          }),
        );
        unlisten.push(
          await onAskLaunchAtLogin(() => {
            setAskLaunch(true);
          }),
        );
      } catch {
        // Browser/vite-only.
      }
      stop = () => {
        for (const fn of unlisten) fn();
      };
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
      }
      try {
        stop = await onSettings(setSettings);
      } catch {
        // Vite-only preview has no Tauri events.
      }
    })();
    return () => stop?.();
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      if (askLaunch) {
        void answerLaunch(false);
        return;
      }
      try {
        void getCurrentWindow().close();
      } catch {
        // Browser/vite-only.
      try {
        void getCurrentWindow().close();
      } catch {
        // Vite-only.
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [askLaunch]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    try {
      void getCurrentWindow()
        .onCloseRequested(() => {
          if (askLaunch) void answerLaunch(false);
        })
        .then((stop) => {
          if (cancelled) stop();
          else unlisten = stop;
        });
    } catch {
      // Browser/vite-only.
    }
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [askLaunch]);
  }, []);

  async function persist(next: AppSettings): Promise<void> {
    setError(null);
    try {
      const saved = await updateSettings(next);
      setSettings(saved);
      setFailDraft(String(saved.failThreshold));
      applyTheme(saved.theme);
      setSettings(await updateSettings(next));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function patch(partial: Partial<AppSettings>): Promise<void> {
    if (!settings) return;
    await persist({ ...cloneSettings(settings), ...partial });
  }

  async function answerLaunch(enable: boolean): Promise<void> {
    setAskLaunch(false);
    try {
      const saved = await answerLaunchPrompt(enable);
      setSettings(saved);
      setFailDraft(String(saved.failThreshold));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  function commitFailThreshold(): void {
    if (!settings) return;
    const clamped = clampFailThreshold(failDraft);
    if (clamped === null) {
      setFailDraft(String(settings.failThreshold));
      return;
    }
    setFailDraft(String(clamped));
    if (clamped !== settings.failThreshold) {
      void patch({ failThreshold: clamped });
    }
  }

  function toggleDay(day: number): void {
    if (!settings) return;
    const hours: QuietHours = settings.quietHours
      ? { ...settings.quietHours, days: [...settings.quietHours.days] }
      : { ...DEFAULT_QUIET_HOURS, days: [...DEFAULT_QUIET_HOURS.days] };
    hours.days = hours.days.includes(day)
      ? hours.days.filter((item) => item !== day)
      : [...hours.days, day].sort((a, b) => a - b);
    void persist({ ...settings, quietHours: hours });
  }

  if (!settings) {
    return (
      <main className="settings" aria-label="Settings">
        <p className="hint">{error ?? "Loading…"}</p>
      </main>
    );
  }

  const quiet = settings.quietHours;
  const quietOn = quiet !== undefined;

  return (
    <main className="settings" aria-label="Settings">
      {askLaunch ? (
        <div className="launch-prompt" role="dialog" aria-label="Launch at login">
          <p>Open Pulse automatically when you log in?</p>
          <div className="launch-prompt-actions">
            <button
              type="button"
              className="btn"
              onClick={() => void answerLaunch(false)}
            >
              Not now
            </button>
            <button
              type="button"
              className="btn primary"
              onClick={() => void answerLaunch(true)}
            >
              Enable
            </button>
          </div>
        </div>
      ) : null}
      <nav className="settings-nav" aria-label="Settings sections">
        {PANES.map((item) => (
          <button
            key={item.id}
            type="button"
            className={pane === item.id ? "is-active" : undefined}
            onClick={() => setPane(item.id)}
          >
            {item.label}
          </button>
        ))}
      </nav>
      <div className="settings-panes">
        {error ? (
          <p className="hint danger-text" role="alert">
            {error}
          </p>
        ) : null}
        {pane === "general" ? (
          <section className="settings-pane" data-pane="general">
            <label className="check-row">
              <input
                type="checkbox"
                checked={settings.launchAtLogin}
                onChange={(event) =>
                  void patch({ launchAtLogin: event.target.checked })
                }
              />
              <span>Launch at login</span>
            </label>
            <p className="hint">
              Off until you add a service. Pulse will ask once after the first
              save.
            </p>
            <label className="field">
              <span>Global hotkey</span>
              <input
                type="text"
                className="mono"
                value={resolvedHotkey(settings.hotkey)}
                maxLength={64}
                onChange={(event) => {
                  setSettings({ ...settings, hotkey: event.target.value });
                }}
                onBlur={(event) => {
                  const value = event.target.value.trim();
                  void patch({ hotkey: value || undefined });
                }}
              />
            </label>
            <label className="field">
              <span>Theme</span>
              <select
                value={settings.theme}
                onChange={(event) =>
                  void patch({ theme: event.target.value as Theme })
                }
              >
                <option value="system">System</option>
                <option value="dark">Dark</option>
                <option value="light">Light</option>
              </select>
            </label>
          </section>
        ) : null}
        {pane === "notifications" ? (
          <section className="settings-pane" data-pane="notifications">
            <label className="check-row">
              <input
                type="checkbox"
                checked={settings.notifications}
                onChange={(event) =>
                  void patch({ notifications: event.target.checked })
                }
              />
              <span>Notifications</span>
            </label>
            <label className="check-row">
              <input
                type="checkbox"
                checked={settings.sound}
                onChange={(event) => void patch({ sound: event.target.checked })}
              />
              <span>Play sound</span>
            </label>
            <label className="check-row">
              <input
                type="checkbox"
                checked={quietOn}
                onChange={(event) =>
                  void patch({
                    quietHours: event.target.checked
                      ? { ...DEFAULT_QUIET_HOURS, days: [...DEFAULT_QUIET_HOURS.days] }
                      : undefined,
                  })
                }
              />
              <span>Quiet hours</span>
            </label>
            {quietOn && quiet ? (
              <fieldset className="quiet-hours">
                <legend>Quiet hours</legend>
                <div className="row-2">
                  <label className="field">
                    <span>Start</span>
                    <input
                      type="time"
                      value={quiet.start}
                      onChange={(event) =>
                        void persist({
                          ...settings,
                          quietHours: {
                            ...quiet,
                            start: event.target.value.slice(0, 5),
                          },
                        })
                      }
                    />
                  </label>
                  <label className="field">
                    <span>End</span>
                    <input
                      type="time"
                      value={quiet.end}
                      onChange={(event) =>
                        void persist({
                          ...settings,
                          quietHours: {
                            ...quiet,
                            end: event.target.value.slice(0, 5),
                          },
                        })
                      }
                    />
                  </label>
                </div>
                <div className="day-pills">
                  {WEEKDAYS.map((label, day) => (
                    <button
                      key={label}
                      type="button"
                      className={quiet.days.includes(day) ? "is-on" : undefined}
                      onClick={() => toggleDay(day)}
                    >
                      {label}
                    </button>
                  ))}
                </div>
                <p className="hint">
                  Overnight ranges are valid. Friday 22:00 runs through Saturday
                  08:00 even if Saturday is unchecked. Tray still turns red;
                  toasts wait for a digest unless Always alert is on.
                </p>
              </fieldset>
            ) : null}
          </section>
        ) : null}
        {pane === "defaults" ? (
          <section className="settings-pane" data-pane="defaults">
            <label className="field">
              <span>Default interval</span>
              <select
                value={settings.defaultInterval}
                onChange={(event) =>
                  void patch({ defaultInterval: Number(event.target.value) })
                }
              >
                {INTERVAL_OPTIONS.map((sec) => (
                  <option key={sec} value={sec}>
                    {sec}s
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              <span>Default timeout</span>
              <select
                value={settings.defaultTimeoutMs}
                onChange={(event) =>
                  void patch({ defaultTimeoutMs: Number(event.target.value) })
                }
              >
                {TIMEOUT_OPTIONS.map((ms) => (
                  <option key={ms} value={ms}>
                    {ms / 1000}s
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              <span>Fail threshold</span>
              <input
                type="number"
                min={1}
                max={10}
                step={1}
                value={failDraft}
                onChange={(event) => setFailDraft(event.target.value)}
                onBlur={() => commitFailThreshold()}
              />
            </label>
            <p className="hint">
              A service is Down only after this many consecutive hard fails. The
              first fail is Degraded (amber). Default is 3.
            </p>
            <p className="hint">{MIXED_REACHABILITY_HELP}</p>
          </section>
        ) : null}
        {pane === "data" ? (
          <section className="settings-pane" data-pane="data">
            <div className="data-actions">
              <button type="button" className="btn" disabled>
                Export JSON
              </button>
              <button type="button" className="btn" disabled>
                Import…
              </button>
            </div>
            <label className="check-row warn-row">
              <input
                type="checkbox"
                checked={includeSecrets}
                onChange={(event) => setIncludeSecrets(event.target.checked)}
              />
              <span>Include secret values</span>
            </label>
            {includeSecrets ? (
              <p className="hint danger-text">
                Anyone with this file can call your endpoints as you. Do not
                commit it. Do not mail it.
              </p>
            ) : null}
            <p className="hint">
              Export, import, and reset land in a later update.
            </p>
            <hr />
            <label className="field">
              <span>
                Type RESET to wipe local config, history, and keychain items
              </span>
              <input
                type="text"
                className="mono"
                placeholder="RESET"
                value={resetTyped}
                onChange={(event) => setResetTyped(event.target.value)}
              />
            </label>
            <button
              type="button"
              className="btn danger"
              disabled
              title="Reset lands in a later update."
            >
              Reset Pulse
            </button>
          </section>
        ) : null}
  const active = quietOn && quiet ? inQuietWindow(quiet, new Date(now)) : false;

  return (
    <main className="settings" aria-label="Settings">
      <div className="settings-panes">
        <section className="settings-pane" data-pane="notifications">
          {error ? (
            <p className="hint danger-text" role="alert">
              {error}
            </p>
          ) : null}
          <label className="check-row">
            <input
              type="checkbox"
              checked={settings.notifications}
              onChange={(event) =>
                void patch({ notifications: event.target.checked })
              }
            />
            <span>Notifications</span>
          </label>
          <label className="check-row">
            <input
              type="checkbox"
              checked={settings.sound}
              onChange={(event) => void patch({ sound: event.target.checked })}
            />
            <span>Play sound</span>
          </label>
          <label className="check-row">
            <input
              type="checkbox"
              checked={quietOn}
              onChange={(event) =>
                void patch({
                  quietHours: event.target.checked
                    ? { ...DEFAULT_QUIET_HOURS, days: [...DEFAULT_QUIET_HOURS.days] }
                    : undefined,
                })
              }
            />
            <span>Quiet hours</span>
          </label>
          {quietOn && quiet ? (
            <fieldset className="quiet-hours">
              <legend>Quiet hours</legend>
              <div className="row-2">
                <label className="field">
                  <span>Start</span>
                  <input
                    type="time"
                    value={quiet.start}
                    onChange={(event) =>
                      void persist({
                        ...settings,
                        quietHours: {
                          ...quiet,
                          start: event.target.value.slice(0, 5),
                        },
                      })
                    }
                  />
                </label>
                <label className="field">
                  <span>End</span>
                  <input
                    type="time"
                    value={quiet.end}
                    onChange={(event) =>
                      void persist({
                        ...settings,
                        quietHours: {
                          ...quiet,
                          end: event.target.value.slice(0, 5),
                        },
                      })
                    }
                  />
                </label>
              </div>
              <div className="day-pills">
                {WEEKDAYS.map((label, day) => (
                  <button
                    key={label}
                    type="button"
                    className={quiet.days.includes(day) ? "is-on" : undefined}
                    onClick={() => toggleDay(day)}
                  >
                    {label}
                  </button>
                ))}
              </div>
              <p className="hint">
                Overnight ranges are valid. Friday 22:00 runs through Saturday
                08:00 even if Saturday is unchecked. Tray still turns red; toasts
                wait for a digest unless Always alert is on.
              </p>
              {active ? (
                <p className="hint">Quiet hours are active now.</p>
              ) : null}
            </fieldset>
          ) : null}
        </section>
      </div>
    </main>
  );
}
