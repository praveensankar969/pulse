import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import {
  expectedHas3xx,
  expectedIs204,
  formatExpectedStatus,
  parseExpectedStatus,
  REDIRECT_HELPER,
} from "../../lib/expectedStatus";
import * as ipc from "../../lib/ipc";
import type {
  AppSettings,
  CheckEvidence,
  HttpMethod,
  ServiceDraft,
} from "../../lib/types";
import { SECRET_MASK } from "../../lib/types";
import {
  AssertionsField,
  assertionsFromRows,
  rowsFromAssertions,
  type AssertionRow,
} from "./AssertionsField";
import { draftHeaders, HeadersField, type HeaderRow } from "./HeadersField";
import { TestNowPanel } from "./TestNowPanel";

const INTERVALS = [15, 30, 60, 120, 300, 600];

type FormState = {
  id?: string;
  name: string;
  url: string;
  method: HttpMethod;
  intervalSec: number;
  timeoutMs: number;
  expectedRaw: string;
  followRedirects: boolean;
  maxLatencyRaw: string;
  actionUrl: string;
  notify: boolean;
  alwaysAlert: boolean;
  body: string;
};

function defaults(settings: AppSettings): FormState {
  return {
    name: "",
    url: "",
    method: "GET",
    intervalSec: settings.defaultInterval,
    timeoutMs: settings.defaultTimeoutMs,
    expectedRaw: "2xx",
    followRedirects: true,
    maxLatencyRaw: "",
    actionUrl: "",
    notify: true,
    alwaysAlert: false,
    body: "",
  };
}

function parseIdFromLocation(): string | undefined {
  const id = new URLSearchParams(window.location.search).get("id");
  return id || undefined;
}

export function EditorWindow() {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [form, setForm] = useState<FormState | null>(null);
  const [headers, setHeaders] = useState<HeaderRow[]>([]);
  const [assertions, setAssertions] = useState<AssertionRow[]>([]);
  const [evidence, setEvidence] = useState<CheckEvidence | null>(null);
  const [testError, setTestError] = useState<string | undefined>();
  const [testing, setTesting] = useState(false);
  const [saving, setSaving] = useState(false);
  const [formError, setFormError] = useState<string | undefined>();
  const lastTestFailed = useRef(false);
  const nameRef = useRef<HTMLInputElement>(null);

  const patch = (partial: Partial<FormState>) => {
    setForm((current) => (current ? { ...current, ...partial } : current));
  };

  const load = useCallback(async (id?: string) => {
    const nextSettings = await ipc.getSettings();
    setSettings(nextSettings);
    lastTestFailed.current = false;
    setEvidence(null);
    setTestError(undefined);
    setFormError(undefined);

    if (!id) {
      setForm(defaults(nextSettings));
      setHeaders([]);
      setAssertions([]);
      return;
    }

    const views = await ipc.listServices();
    const view = views.find((item) => item.id === id);
    if (!view) {
      setForm(defaults(nextSettings));
      setHeaders([]);
      setAssertions([]);
      return;
    }

    setForm({
      id: view.id,
      name: view.name,
      url: view.url,
      method: view.method,
      intervalSec: view.intervalSec,
      timeoutMs: view.timeoutMs,
      expectedRaw: formatExpectedStatus(view.expectedStatus),
      followRedirects: view.followRedirects,
      maxLatencyRaw: view.maxLatencyMs != null ? String(view.maxLatencyMs) : "",
      actionUrl: view.actionUrl ?? "",
      notify: view.notify,
      alwaysAlert: view.alwaysAlert,
      body: view.body ?? "",
    });
    setHeaders(
      view.headers.map((header) => ({
        key: header.key,
        value: header.secret ? SECRET_MASK : (header.value ?? ""),
        secret: header.secret,
        hadValue: header.secret,
      })),
    );
    setAssertions(rowsFromAssertions(view.assertions));
  }, []);

  useEffect(() => {
    void load(parseIdFromLocation()).then(() => {
      nameRef.current?.focus();
    });
    let stop: (() => void) | undefined;
    void ipc
      .onEditorTarget((payload) => {
        void load(payload.id);
      })
      .then((unlisten) => {
        stop = unlisten;
      });
    return () => stop?.();
  }, [load]);

  const expected = form ? parseExpectedStatus(form.expectedRaw) : null;
  const head = form?.method === "HEAD";
  const post = form?.method === "POST";
  const redirectError =
    form?.followRedirects && expected != null && expectedHas3xx(expected)
      ? REDIRECT_HELPER
      : undefined;
  const status204Hint =
    expected != null && expectedIs204(expected) && assertions.length > 0
      ? "204 has no body; drop assertions or expect 200."
      : undefined;

  const missingKey = useMemo(() => {
    if (evidence?.errorKind !== "missing_secret" || !evidence.error) return undefined;
    const match = evidence.error.match(/^Secret header (.+) is not set$/);
    return match?.[1];
  }, [evidence]);

  function buildDraft(): ServiceDraft | string {
    if (!form) return "Still loading.";
    const name = form.name.trim();
    const url = form.url.trim();
    if (!name) return "Name is required.";
    if (!url) return "URL is required.";
    if (!expected) return "expectedStatus must be 2xx, a status code, or a list of codes.";
    if (redirectError) return redirectError;
    const maxLatencyMs = form.maxLatencyRaw.trim()
      ? Number(form.maxLatencyRaw)
      : undefined;
    if (maxLatencyMs != null && (!Number.isFinite(maxLatencyMs) || maxLatencyMs < 1)) {
      return "maxLatencyMs must be between 1 and 60000.";
    }
    const actionUrl = form.actionUrl.trim();
    const draft: ServiceDraft = {
      name,
      url,
      method: form.method,
      headers: draftHeaders(headers),
      intervalSec: form.intervalSec,
      timeoutMs: form.timeoutMs,
      expectedStatus: expected,
      followRedirects: form.followRedirects,
      assertions: assertionsFromRows(assertions),
      notify: form.notify,
      alwaysAlert: form.alwaysAlert,
    };
    if (form.id) draft.id = form.id;
    if (post && form.body) draft.body = form.body;
    if (maxLatencyMs != null) draft.maxLatencyMs = maxLatencyMs;
    if (actionUrl) draft.actionUrl = actionUrl;
    return draft;
  }

  function buildTestDraft(): ServiceDraft | string {
    if (!form) return "Still loading.";
    const url = form.url.trim();
    if (!url) return "URL is required.";
    const status = parseExpectedStatus(form.expectedRaw) ?? "2xx";
    const maxLatencyMs = form.maxLatencyRaw.trim()
      ? Number(form.maxLatencyRaw)
      : undefined;
    const draft: ServiceDraft = {
      name: form.name.trim() || "draft",
      url,
      method: form.method,
      headers: draftHeaders(headers),
      intervalSec: form.intervalSec,
      timeoutMs: form.timeoutMs,
      expectedStatus: status,
      followRedirects: form.followRedirects,
      assertions: assertionsFromRows(assertions),
      notify: form.notify,
      alwaysAlert: form.alwaysAlert,
    };
    if (form.id) draft.id = form.id;
    if (post && form.body) draft.body = form.body;
    if (maxLatencyMs != null && Number.isFinite(maxLatencyMs) && maxLatencyMs >= 1) {
      draft.maxLatencyMs = maxLatencyMs;
    }
    return draft;
  }

  async function onTest() {
    const draft = buildTestDraft();
    if (typeof draft === "string") {
      setTestError(draft);
      lastTestFailed.current = true;
      return;
    }
    setTesting(true);
    setTestError(undefined);
    try {
      const result = await ipc.testDraft(draft);
      setEvidence(result);
      lastTestFailed.current = result.outcome !== "ok";
    } catch (error) {
      setEvidence(null);
      setTestError(error instanceof Error ? error.message : String(error));
      lastTestFailed.current = true;
    } finally {
      setTesting(false);
    }
  }

  async function onSave(event: FormEvent) {
    event.preventDefault();
    const draft = buildDraft();
    if (typeof draft === "string") {
      setFormError(draft);
      return;
    }
    if (lastTestFailed.current && !window.confirm("Last test failed. Save anyway?")) {
      return;
    }
    setSaving(true);
    setFormError(undefined);
    try {
      await ipc.saveService(draft);
      await ipc.closeEditor();
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setFormError(
        message.includes("followRedirects") || message.includes("3xx")
          ? REDIRECT_HELPER
          : message,
      );
    } finally {
      setSaving(false);
    }
  }

  if (!form || !settings) {
    return <main className="editor">Loading…</main>;
  }

  return (
    <main className="editor">
      <form className="editor-form" onSubmit={(event) => void onSave(event)} autoComplete="off">
        <h1 className="editor-title">{form.id ? "Edit service" : "Add service"}</h1>
        <label className="field">
          <span>Name</span>
          <input
            ref={nameRef}
            required
            value={form.name}
            onChange={(event) => patch({ name: event.target.value })}
          />
        </label>
        <label className="field">
          <span>Health URL</span>
          <input
            className="mono"
            required
            placeholder="https://api.example/health"
            value={form.url}
            onChange={(event) => patch({ url: event.target.value })}
          />
        </label>
        <div className="row-3">
          <label className="field">
            <span>Method</span>
            <select
              value={form.method}
              onChange={(event) => patch({ method: event.target.value as HttpMethod })}
            >
              <option value="GET">GET</option>
              <option value="HEAD">HEAD</option>
              <option value="POST">POST</option>
            </select>
          </label>
          <label className="field">
            <span>Interval</span>
            <select
              value={form.intervalSec}
              onChange={(event) => patch({ intervalSec: Number(event.target.value) })}
            >
              {INTERVALS.map((sec) => (
                <option key={sec} value={sec}>
                  {sec}
                </option>
              ))}
            </select>
          </label>
          <label className="field">
            <span>Timeout (ms)</span>
            <input
              type="number"
              min={500}
              max={60000}
              value={form.timeoutMs}
              onChange={(event) => patch({ timeoutMs: Number(event.target.value) })}
            />
          </label>
        </div>
        <HeadersField rows={headers} missingKey={missingKey} onChange={setHeaders} />
        {post ? (
          <label className="field">
            <span>POST body</span>
            <textarea
              className="mono"
              rows={3}
              value={form.body}
              onChange={(event) => patch({ body: event.target.value })}
            />
            <p className="hint">
              Pulse will POST this body on every poll. Only use an idempotent endpoint.
            </p>
          </label>
        ) : null}
        <label className="field">
          <span>Expected status</span>
          <input
            className="mono"
            value={form.expectedRaw}
            onChange={(event) => patch({ expectedRaw: event.target.value })}
          />
          {status204Hint ? <p className="hint">{status204Hint}</p> : null}
        </label>
        <label className="check-row">
          <input
            type="checkbox"
            checked={form.followRedirects}
            onChange={(event) => patch({ followRedirects: event.target.checked })}
          />
          <span>Follow redirects (≤3)</span>
        </label>
        <p className={`hint${redirectError ? " danger-text" : ""}`}>{REDIRECT_HELPER}</p>
        <AssertionsField rows={assertions} disabled={head} onChange={setAssertions} />
        <label className="field">
          <span>Latency SLO (ms, optional)</span>
          <input
            type="number"
            min={1}
            max={60000}
            placeholder="800"
            value={form.maxLatencyRaw}
            onChange={(event) => patch({ maxLatencyRaw: event.target.value })}
          />
        </label>
        <label className="field">
          <span>Action URL</span>
          <input
            className="mono"
            placeholder="https://grafana.example/d/pay"
            value={form.actionUrl}
            onChange={(event) => patch({ actionUrl: event.target.value })}
          />
        </label>
        <label className="check-row">
          <input
            type="checkbox"
            checked={form.notify}
            onChange={(event) => patch({ notify: event.target.checked })}
          />
          <span>Notify when this service goes down</span>
        </label>
        <label className="check-row">
          <input
            type="checkbox"
            checked={form.alwaysAlert}
            onChange={(event) => patch({ alwaysAlert: event.target.checked })}
          />
          <span>Always alert (bypass quiet hours)</span>
        </label>
        <TestNowPanel evidence={evidence} busy={testing} message={testError} />
        {formError ? <p className="hint danger-text">{formError}</p> : null}
        <div className="editor-actions">
          <button type="button" className="btn" onClick={() => void onTest()} disabled={testing}>
            Test now
          </button>
          <button type="submit" className="btn primary" disabled={saving}>
            Save
          </button>
        </div>
      </form>
    </main>
  );
}
