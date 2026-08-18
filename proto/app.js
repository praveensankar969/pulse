(() => {
  const $ = (sel, root = document) => root.querySelector(sel);
  const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];

  const now = new Date("2026-08-18T09:41:00");

  const spark = (tail) => {
    const base = Array(24 - tail.length).fill("healthy");
    return base.concat(tail);
  };

  const harborHealthy = () => [
    svc("api", "API", "https://api.harbor.dev/health", "healthy", {
      ageSec: 12,
      http: 200,
      latency: 48,
      body: '{"status":"ok","version":"1.4.2"}',
      asserts: [ok("status", "equals", "ok", "ok")],
      spark: spark(["healthy", "healthy", "healthy"]),
    }),
    svc("web", "Web", "https://app.harbor.dev/api/healthz", "healthy", {
      ageSec: 18,
      http: 200,
      latency: 71,
      body: '{"ok":true}',
      asserts: [ok("ok", "equals", true, true)],
      spark: spark(["healthy"]),
    }),
    svc("worker", "Worker", "https://worker.harbor.dev/health", "healthy", {
      ageSec: 41,
      http: 200,
      latency: 63,
      body: '{"status":"ok","queue":12}',
      asserts: [ok("status", "equals", "ok", "ok")],
      action: "https://grafana.harbor.dev/d/worker",
      spark: spark(["healthy", "degraded", "healthy"]),
    }),
    svc("auth", "Auth", "https://auth.harbor.dev/health", "healthy", {
      ageSec: 9,
      http: 200,
      latency: 112,
      body: '{"status":"ok"}',
      asserts: [ok("status", "equals", "ok", "ok")],
      spark: spark(["healthy"]),
    }),
    svc("pay", "Payments API", "https://pay.harbor.dev/health", "healthy", {
      ageSec: 27,
      http: 200,
      latency: 94,
      body: '{"status":"ok","errors":[]}',
      asserts: [ok("status", "equals", "ok", "ok"), ok("errors.length", "equals", 0, 0)],
      action: "https://grafana.harbor.dev/d/pay",
      alwaysAlert: true,
      maxLatency: 800,
      spark: spark(["healthy"]),
    }),
    svc("docs", "Docs", "https://docs.harbor.dev/health", "healthy", {
      ageSec: 240,
      http: 200,
      latency: 38,
      body: '{"ok":true}',
      asserts: [ok("ok", "equals", true, true)],
      spark: spark(["healthy"]),
    }),
    svc("nas", "NAS", "https://nas.home.arpa/api/v2.0/system/info", "healthy", {
      ageSec: 55,
      http: 200,
      latency: 21,
      body: '{"healthy":true,"uptime":803520}',
      asserts: [ok("healthy", "equals", true, true)],
      spark: spark(["healthy"]),
    }),
  ];

  const harborIncident = () => {
    const list = harborHealthy();
    const pay = list.find((s) => s.id === "pay");
    const worker = list.find((s) => s.id === "worker");
    const auth = list.find((s) => s.id === "auth");
    Object.assign(pay, {
      state: "down",
      downSec: 360,
      ageSec: 14,
      http: 502,
      latency: 1420,
      outcome: "hard",
      errorKind: "unexpected_status",
      error: "HTTP 502",
      body: '{"status":"degraded","errors":["stripe_timeout"]}',
      asserts: [
        fail("status", "equals", "ok", "degraded"),
        fail("errors.length", "equals", 0, 1),
      ],
      spark: spark(["healthy", "healthy", "degraded", "degraded", "down", "down", "down"]),
    });
    Object.assign(worker, {
      state: "down",
      downSec: 120,
      ageSec: 22,
      http: null,
      latency: 10000,
      outcome: "hard",
      errorKind: "timeout",
      error: "Timed out after 10s",
      body: "",
      asserts: [fail("status", "equals", "ok", "<missing>")],
      spark: spark(["healthy", "degraded", "down", "down"]),
    });
    Object.assign(auth, {
      state: "degraded",
      outcome: "soft",
      errorKind: "slow",
      error: "910ms (limit 800ms)",
      degradedSec: 180,
      ageSec: 9,
      http: 200,
      latency: 910,
      maxLatency: 800,
      spark: spark(["healthy", "healthy", "degraded", "degraded"]),
    });
    return list;
  };

  function ok(path, op, expected, actual) {
    return { path, op, expected, actual, ok: true };
  }
  function fail(path, op, expected, actual) {
    return { path, op, expected, actual, ok: false };
  }

  function svc(id, name, url, state, extra = {}) {
    return {
      id,
      name,
      url,
      method: "GET",
      state,
      outcome: extra.outcome || "ok",
      intervalSec: 60,
      timeoutMs: 10000,
      expectedStatus: "2xx",
      notify: true,
      alwaysAlert: false,
      paused: false,
      headers: [
        { key: "Authorization", secret: true, value: "Bearer 0xharbor_live_4f91" },
        { key: "Accept", secret: false, value: "application/json" },
      ],
      assertions: [
        { path: "status", op: "equals", value: "ok" },
      ],
      ...extra,
    };
  }

  const scenes = {
    empty: { services: () => [], toasts: [] },
    healthy: { services: harborHealthy, toasts: [] },
    incident: {
      services: harborIncident,
      toasts: [
        { id: "pay", title: "Payments API", body: "HTTP 502 · 1.4s", kind: "down" },
        { id: "worker", title: "Worker", body: "Timed out after 10s", kind: "down" },
      ],
    },
  };

  const state = {
    scene: "incident",
    os: "mac",
    services: harborIncident(),
    selected: "pay",
    popoverOpen: true,
    suppressBlurUntil: 0,
    editingId: null,
    lastTest: null,
    qhDays: [1, 2, 3, 4, 5],
    checking: new Set(),
  };

  const desktop = $("#desktop");
  const popover = $("#popover");
  const listEl = $("#serviceList");
  const emptyEl = $("#emptyState");
  const summaryCount = $("#summaryCount");
  const summaryStrip = $("#summaryStrip");
  const notifyStack = $("#notifyStack");

  function band(s) {
    if (s.state === "down") return 0;
    if (s.state === "degraded") return 1;
    if (s.state === "pending") return 2;
    if (s.paused) return 3;
    return 4;
  }

  function timeInState(s) {
    if (s.state === "down") return s.downSec || 0;
    if (s.state === "degraded") return s.degradedSec || 0;
    return s.ageSec || 0;
  }

  function sortedServices() {
    return [...state.services].sort((a, b) => {
      const ba = band(a);
      const bb = band(b);
      if (ba !== bb) return ba - bb;
      const ta = timeInState(a);
      const tb = timeInState(b);
      if (ta !== tb) return tb - ta;
      return a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
    });
  }

  function primaryLabel(s) {
    if (s.paused) return "Paused";
    if (s.state === "pending") return "Pending";
    if (s.state === "down") return "Down";
    if (s.state === "degraded" && (s.outcome === "soft" || s.errorKind === "slow")) return "Slow";
    if (s.state === "degraded") return "Degraded";
    return "Healthy";
  }

  function rel(sec, prefix) {
    if (sec == null) return "";
    const n = Math.max(0, sec);
    let text;
    if (n < 60) text = `${n}s`;
    else if (n < 3600) text = `${Math.round(n / 60)}m`;
    else text = `${Math.round(n / 3600)}h`;
    return prefix ? `${prefix} ${text}` : `${text} ago`;
  }

  function worstOf() {
    const active = state.services.filter((s) => !s.paused);
    if (!active.length) return { state: "hollow", badge: 0 };
    if (active.every((s) => s.state === "pending")) return { state: "hollow", badge: 0 };
    const downs = active.filter((s) => s.state === "down");
    if (downs.length) return { state: "down", badge: downs.length };
    if (active.some((s) => s.state === "degraded")) return { state: "degraded", badge: 0 };
    return { state: "healthy", badge: 0 };
  }

  function paintTray() {
    const w = worstOf();
    for (const el of [("#trayIcon"), ("#winTrayIcon")].map((id) => $(id))) {
      el.dataset.state = w.state;
      el.dataset.badge = String(w.badge);
      $(".tray-badge", el).textContent = w.badge || "";
    }
    $("#trayHit").setAttribute("aria-label", `Pulse tray, ${w.state}${w.badge ? `, ${w.badge} down` : ""}`);
  }

  function renderList() {
    const items = sortedServices();
    const downs = state.services.filter((s) => s.state === "down" && !s.paused);
    const slows = state.services.filter((s) => s.state === "degraded" && !s.paused);
    emptyEl.hidden = items.length > 0;
    listEl.hidden = items.length === 0;
    summaryCount.textContent = items.length
      ? `${items.length} service${items.length === 1 ? "" : "s"}${downs.length ? ` · ${downs.length} down` : ""}`
      : "No services";
    if (!items.length) {
      summaryStrip.textContent = "Add a check to start watching.";
      summaryStrip.className = "summary-strip";
    } else if (downs.length) {
      summaryStrip.textContent = `${downs.length} down · ${downs.map((s) => s.name).join(", ")}`;
      summaryStrip.className = "summary-strip is-down";
    } else if (slows.length) {
      summaryStrip.textContent = `${slows.length} slow · ${slows.map((s) => s.name).join(", ")}`;
      summaryStrip.className = "summary-strip is-warn";
    } else {
      summaryStrip.textContent = "All healthy";
      summaryStrip.className = "summary-strip is-ok";
    }

    listEl.innerHTML = "";
    for (const s of items) {
      const li = document.createElement("li");
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "service-row"
        + (state.selected === s.id ? " is-selected" : "")
        + (s.paused ? " is-paused" : "");
      btn.dataset.id = s.id;
      const label = primaryLabel(s);
      const cls = label.toLowerCase();
      const time = s.state === "pending"
        ? "Checking…"
        : s.state === "down"
          ? rel(s.downSec, "down")
          : s.state === "degraded"
            ? rel(s.degradedSec || s.ageSec, "degraded")
            : rel(s.ageSec);
      btn.innerHTML = `
        <span class="dot ${cls}"></span>
        <span class="name">${escapeHtml(s.name)}</span>
        <span class="pill ${cls}">${label}</span>
        <span class="meta">
          <span>${time}</span>
          ${s.snoozed ? `<span class="pill snooze">Snoozed · ${s.snoozed}</span>` : ""}
        </span>
      `;
      btn.addEventListener("click", () => openDetail(s.id));
      li.appendChild(btn);
      listEl.appendChild(li);
    }
    paintTray();
  }

  function renderToasts(toasts) {
    notifyStack.innerHTML = "";
    for (const t of toasts) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "toast" + (t.kind === "recovered" ? " recovered" : "");
      b.innerHTML = `
        <span class="toast-mark"></span>
        <span>
          <div class="toast-title">${escapeHtml(t.title)}</div>
          <div class="toast-body">${escapeHtml(t.body)}</div>
        </span>
        <span class="toast-app">Pulse</span>
      `;
      b.addEventListener("click", () => {
        showPopover();
        state.selected = t.id;
        renderList();
        const row = listEl.querySelector(`[data-id="${t.id}"]`);
        row?.focus();
        row?.scrollIntoView({ block: "nearest" });
      });
      notifyStack.appendChild(b);
    }
  }

  function loadScene(name) {
    state.scene = name;
    desktop.dataset.scene = name;
    state.services = scenes[name].services().map((s) => ({ ...s }));
    state.selected = state.services[0]?.id || null;
    closeWin("detailWin");
    closeWin("editorWin");
    closeWin("settingsWin");
    showPopover();
    renderList();
    renderToasts(scenes[name].toasts);
    $$(".scene-nav button, .win-scenes button[data-scene]").forEach((b) => {
      b.classList.toggle("is-active", b.dataset.scene === name);
    });
  }

  function showPopover() {
    popover.hidden = false;
    state.popoverOpen = true;
    $("#trayHit").setAttribute("aria-expanded", "true");
  }
  function hidePopover() {
    popover.hidden = true;
    state.popoverOpen = false;
    $("#trayHit").setAttribute("aria-expanded", "false");
  }
  function togglePopover() {
    if (state.popoverOpen) hidePopover();
    else showPopover();
  }

  function openDetail(id) {
    const s = state.services.find((x) => x.id === id);
    if (!s) return;
    state.selected = id;
    hidePopover();
    closeWin("editorWin");
    const win = $("#detailWin");
    $("#detailTitle").textContent = s.name;
    const label = primaryLabel(s);
    $("#detailBody").innerHTML = `
      <div class="detail-head">
        <div>
          <h3>${escapeHtml(s.name)}</h3>
          <p class="reason">${escapeHtml(s.error || (s.http ? `HTTP ${s.http} · ${s.latency}ms` : "Last check passed"))}</p>
        </div>
        <span class="pill ${label.toLowerCase()}">${label}</span>
      </div>
      <dl class="kv">
        <dt>HTTP</dt><dd>${s.http ?? "—"}</dd>
        <dt>Latency</dt><dd>${s.latency != null ? s.latency.toLocaleString() + "ms" : "—"}</dd>
        <dt>Expected</dt><dd>${escapeHtml(String(s.expectedStatus || "2xx"))}</dd>
        <dt>Checked</dt><dd>${rel(s.ageSec) || "—"}</dd>
        <dt>URL</dt><dd>${escapeHtml(s.url)}</dd>
      </dl>
      <table class="assert-table">
        <thead><tr><th>Path</th><th>Op</th><th>Expected</th><th>Actual</th><th></th></tr></thead>
        <tbody>
          ${(s.asserts || []).map((a) => `
            <tr>
              <td>${escapeHtml(a.path)}</td>
              <td>${escapeHtml(a.op)}</td>
              <td>${fmtVal(a.expected)}</td>
              <td>${fmtVal(a.actual)}</td>
              <td class="${a.ok ? "pass" : "fail"}">${a.ok ? "pass" : "fail"}</td>
            </tr>`).join("")}
        </tbody>
      </table>
      <pre class="preview">${escapeHtml(s.body || "(empty)")}</pre>
      <button type="button" class="btn" id="copyBody">Copy response</button>
      <div class="spark" aria-label="Last 24 checks">${(s.spark || []).map((st) => `<i class="${st}"></i>`).join("")}</div>
      <div class="header-list">
        ${(s.headers || []).map((h) => `
          <div class="header-row">
            <span class="k">${escapeHtml(h.key)}</span>
            ${h.secret
              ? `<button type="button" class="secret-mask" data-secret="${escapeAttr(h.value)}" aria-label="Hold to reveal">••••••••</button>`
              : `<span>${escapeHtml(h.value)}</span>`}
          </div>`).join("")}
      </div>
      <div class="actions">
        <button type="button" class="btn primary" id="openUrl">Open</button>
        <button type="button" class="btn" id="checkNow">Check now</button>
        <button type="button" class="btn" id="pauseSvc">${s.paused ? "Resume" : "Pause"}</button>
        <button type="button" class="btn" id="snoozeBtn">Snooze ▾</button>
        <button type="button" class="btn" id="editSvc">Edit</button>
      </div>
    `;
    win.hidden = false;
    $("#copyBody").onclick = () => navigator.clipboard?.writeText(s.body || "");
    $("#openUrl").onclick = () => window.open(s.action || s.url, "_blank", "noopener");
    $("#checkNow").onclick = () => checkNow(s.id);
    $("#pauseSvc").onclick = () => {
      s.paused = !s.paused;
      if (s.paused) s.stateWas = s.state;
      renderList();
      openDetail(s.id);
    };
    $("#snoozeBtn").onclick = (e) => openSnooze(e.currentTarget, s);
    $("#editSvc").onclick = () => openEditor(s.id);
    $$(".secret-mask", win).forEach(bindSecret);
    renderList();
  }

  function bindSecret(btn) {
    const mask = "••••••••";
    const reveal = () => {
      btn.textContent = btn.dataset.secret;
      btn.classList.add("revealed");
    };
    const hide = () => {
      btn.textContent = mask;
      btn.classList.remove("revealed");
    };
    btn.addEventListener("pointerdown", (e) => { e.preventDefault(); reveal(); });
    ["pointerup", "pointerleave", "blur"].forEach((ev) => btn.addEventListener(ev, hide));
  }

  function openSnooze(anchor, s) {
    $$(".snooze-menu").forEach((n) => n.remove());
    const menu = document.createElement("div");
    menu.className = "snooze-menu";
    menu.innerHTML = `
      <button type="button" data-for="15m">15 minutes</button>
      <button type="button" data-for="60m">60 minutes</button>
      <button type="button" data-for="tomorrow">Until tomorrow 08:00</button>
      ${s.snoozed ? `<button type="button" data-for="clear">Clear snooze</button>` : ""}
    `;
    document.body.appendChild(menu);
    const r = anchor.getBoundingClientRect();
    menu.style.left = `${r.left}px`;
    menu.style.top = `${r.bottom + 4}px`;
    menu.addEventListener("click", (e) => {
      const v = e.target.dataset.for;
      if (!v) return;
      s.snoozed = v === "clear" ? null : v === "tomorrow" ? "until 08:00" : v;
      menu.remove();
      renderList();
      openDetail(s.id);
    });
    setTimeout(() => document.addEventListener("click", () => menu.remove(), { once: true }), 0);
  }

  function checkNow(id) {
    const s = state.services.find((x) => x.id === id);
    if (!s) return;
    s.ageSec = 0;
    if (state.scene === "incident" && (s.id === "pay" || s.id === "worker")) {
      /* stay down — operator is looking at a live failure */
    } else if (s.state !== "down") {
      s.state = "healthy";
      s.outcome = "ok";
      s.latency = 40 + Math.round(Math.random() * 80);
    }
    renderList();
    if (!$("#detailWin").hidden) openDetail(id);
  }

  function openEditor(id) {
    const s = id ? state.services.find((x) => x.id === id) : null;
    state.editingId = id;
    state.lastTest = null;
    $("#editorTitle").textContent = s ? "Edit service" : "Add service";
    const form = $("#editorForm");
    form.innerHTML = `
      <label class="field"><span>Name</span><input name="name" required value="${escapeAttr(s?.name || "")}" /></label>
      <label class="field"><span>Health URL</span><input name="url" class="mono" required placeholder="https://api.example/health" value="${escapeAttr(s?.url || "")}" /></label>
      <div class="row-3">
        <label class="field"><span>Method</span>
          <select name="method">
            ${["GET", "HEAD", "POST"].map((m) => `<option ${((s?.method || "GET") === m) ? "selected" : ""}>${m}</option>`).join("")}
          </select>
        </label>
        <label class="field"><span>Interval</span>
          <select name="interval">${[15, 30, 60, 120, 300, 600].map((n) => `<option ${(s?.intervalSec || 60) === n ? "selected" : ""}>${n}</option>`).join("")}</select>
        </label>
        <label class="field"><span>Timeout (ms)</span><input name="timeout" type="number" value="${s?.timeoutMs || 10000}" /></label>
      </div>
      <div class="headers-edit">
        <span class="field"><span>Headers</span></span>
        <div id="headerRows"></div>
        <button type="button" class="text-btn" id="addHeader">+ Header</button>
      </div>
      <label class="field" id="postBodyField" ${s?.method === "POST" ? "" : "hidden"}>
        <span>POST body</span>
        <textarea name="body" rows="3" class="mono">${escapeHtml(s?.bodyDraft || "")}</textarea>
        <p class="hint">Pulse will POST this body on every poll. Only use an idempotent endpoint.</p>
      </label>
      <label class="field"><span>Expected status</span>
        <input name="expected" class="mono" value="${escapeAttr(String(s?.expectedStatus || "2xx"))}" />
      </label>
      <label class="check-row"><input type="checkbox" name="follow" ${s?.followRedirects === false ? "" : "checked"} /><span>Follow redirects (≤3)</span></label>
      <p class="hint">We follow up to 3 redirects and evaluate the final status. Uncheck Follow redirects to treat the first response as final — required if you expect 3xx.</p>
      <div>
        <span class="field"><span>JSON assertions — all must pass</span></span>
        <div id="assertRows"></div>
        <button type="button" class="text-btn" id="addAssert">+ Assertion</button>
        <p class="hint">Paths are dot notation from the JSON root. <span class="mono">$</span> is optional. <span class="mono">status</span> · <span class="mono">$.data.healthy</span> · <span class="mono">items.0.id</span> · <span class="mono">errors.length</span></p>
      </div>
      <label class="field"><span>Latency SLO (ms, optional)</span><input name="slo" type="number" value="${s?.maxLatency || ""}" placeholder="800" /></label>
      <label class="field"><span>Action URL</span><input name="action" class="mono" value="${escapeAttr(s?.action || "")}" placeholder="https://grafana.example/d/pay" /></label>
      <label class="check-row"><input type="checkbox" name="notify" ${s?.notify === false ? "" : "checked"} /><span>Notify when this service goes down</span></label>
      <label class="check-row"><input type="checkbox" name="always" ${s?.alwaysAlert ? "checked" : ""} /><span>Always alert (bypass quiet hours)</span></label>
      <div class="test-panel" id="testPanel">Test now runs one request and does not save.</div>
      <div class="editor-actions">
        <button type="button" class="btn" id="testNow">Test now</button>
        <button type="submit" class="btn primary">Save</button>
      </div>
    `;
    const headers = s?.headers?.length ? s.headers.map((h) => ({ ...h })) : [{ key: "Authorization", secret: true, value: "" }];
    const asserts = s?.assertions?.length ? s.assertions.map((a) => ({ ...a })) : [
      { path: "status", op: "equals", value: "ok" },
    ];
    const headerBox = $("#headerRows", form);
    const assertBox = $("#assertRows", form);
    function paintHeaders() {
      headerBox.innerHTML = headers.map((h, i) => `
        <div class="row-2" style="margin-bottom:6px">
          <input class="mono" data-h="key" data-i="${i}" value="${escapeAttr(h.key)}" placeholder="Authorization" />
          <div style="display:flex;gap:6px">
            <input class="mono" data-h="value" data-i="${i}" value="${h.secret && h.value ? "••••••••" : escapeAttr(h.value || "")}" placeholder="value" />
            <label class="check-row" style="white-space:nowrap"><input type="checkbox" data-h="secret" data-i="${i}" ${h.secret ? "checked" : ""} />secret</label>
          </div>
        </div>`).join("");
      headerBox.oninput = headerBox.onchange = (e) => {
        const el = e.target;
        const i = Number(el.dataset.i);
        if (Number.isNaN(i) || !headers[i]) return;
        if (el.dataset.h === "key") headers[i].key = el.value;
        if (el.dataset.h === "value" && el.value !== "••••••••") headers[i].value = el.value;
        if (el.dataset.h === "secret") headers[i].secret = el.checked;
      };
    }
    function paintAsserts() {
      assertBox.innerHTML = asserts.map((a, i) => `
        <div class="row-3" style="margin-bottom:6px">
          <input class="mono" data-a="path" data-i="${i}" value="${escapeAttr(a.path)}" placeholder="status" />
          <select data-a="op" data-i="${i}">
            ${["equals", "not_equals", "contains", "exists", "gt", "lt"].map((op) => `<option ${a.op === op ? "selected" : ""}>${op}</option>`).join("")}
          </select>
          <input class="mono" data-a="value" data-i="${i}" value="${escapeAttr(a.value == null ? "" : String(a.value))}" placeholder="ok" />
        </div>`).join("");
      assertBox.oninput = assertBox.onchange = (e) => {
        const el = e.target;
        const i = Number(el.dataset.i);
        if (Number.isNaN(i) || !asserts[i]) return;
        if (el.dataset.a === "path") asserts[i].path = el.value;
        if (el.dataset.a === "op") asserts[i].op = el.value;
        if (el.dataset.a === "value") asserts[i].value = el.value;
      };
    }
    paintHeaders();
    paintAsserts();
    const field = (name) => form.elements.namedItem(name);
    $("#addHeader", form).onclick = () => { headers.push({ key: "", secret: false, value: "" }); paintHeaders(); };
    $("#addAssert", form).onclick = () => { asserts.push({ path: "", op: "equals", value: "" }); paintAsserts(); };
    field("method").addEventListener("change", () => {
      $("#postBodyField", form).hidden = field("method").value !== "POST";
    });
    $("#testNow", form).onclick = () => {
      const panel = $("#testPanel", form);
      const url = field("url").value.trim();
      if (!url) {
        panel.textContent = "URL is required.";
        panel.className = "test-panel fail";
        state.lastTest = "fail";
        return;
      }
      const failScene = /pay\.harbor|worker\.harbor/.test(url) && state.scene === "incident";
      if (failScene) {
        panel.innerHTML = `<strong>Failed</strong> · HTTP 502 · 1.42s<br><span class="mono">status expected ok, got degraded</span>`;
        panel.className = "test-panel fail";
        state.lastTest = "fail";
      } else {
        panel.innerHTML = `<strong>Passed</strong> · HTTP 200 · 64ms<br><span class="mono">status equals ok · errors.length equals 0</span>`;
        panel.className = "test-panel pass";
        state.lastTest = "pass";
      }
    };
    form.onsubmit = (e) => {
      e.preventDefault();
      if (state.lastTest === "fail" && !confirm("Last test failed. Save anyway?")) return;
      const draft = {
        id: s?.id || slug(field("name").value),
        name: field("name").value.trim(),
        url: field("url").value.trim(),
        method: field("method").value,
        state: "pending",
        ageSec: null,
        intervalSec: Number(field("interval").value),
        timeoutMs: Number(field("timeout").value),
        expectedStatus: field("expected").value,
        notify: field("notify").checked,
        alwaysAlert: field("always").checked,
        paused: false,
        headers,
        assertions: asserts,
        maxLatency: field("slo").value ? Number(field("slo").value) : undefined,
        action: field("action").value || undefined,
        http: null,
        latency: null,
        asserts: [],
        spark: Array(24).fill("gap"),
      };
      const idx = state.services.findIndex((x) => x.id === draft.id);
      if (idx >= 0) state.services[idx] = { ...state.services[idx], ...draft };
      else state.services.push(draft);
      closeWin("editorWin");
      showPopover();
      renderList();
      setTimeout(() => {
        const live = state.services.find((x) => x.id === draft.id);
        if (!live || live.state !== "pending") return;
        live.state = "healthy";
        live.ageSec = 0;
        live.http = 200;
        live.latency = 58;
        live.asserts = (live.assertions || []).map((a) => ok(a.path, a.op, a.value, a.value));
        live.body = '{"status":"ok"}';
        renderList();
      }, 700);
    };
    closeWin("detailWin");
    hidePopover();
    $("#editorWin").hidden = false;
    field("name").focus();
  }

  function slug(name) {
    return (name || "svc").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") + "-" + Math.random().toString(36).slice(2, 6);
  }

  function closeWin(id) {
    const el = document.getElementById(id);
    if (el) el.hidden = true;
  }

  function escapeHtml(v) {
    return String(v ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  }
  function escapeAttr(v) { return escapeHtml(v); }
  function fmtVal(v) {
    if (typeof v === "string") return escapeHtml(v);
    return escapeHtml(JSON.stringify(v));
  }

  /* Scene + OS */
  $$(".scene-nav button, .win-scenes button[data-scene]").forEach((b) => {
    b.addEventListener("click", () => loadScene(b.dataset.scene));
  });
  function setOs(os) {
    state.os = os;
    desktop.dataset.os = os;
    $("#osToggle").textContent = os === "mac" ? "macOS" : "Windows";
    $("#osToggleWin").textContent = os === "mac" ? "macOS" : "Windows";
  }
  $("#osToggle").addEventListener("click", () => setOs(state.os === "mac" ? "win" : "mac"));
  $("#osToggleWin").addEventListener("click", () => setOs(state.os === "mac" ? "win" : "mac"));

  /* Tray click protocol: mouse-down suppress blur, toggle on mouse-up */
  function bindTray(el) {
    el.addEventListener("pointerdown", () => {
      state.suppressBlurUntil = Date.now() + 250;
    });
    el.addEventListener("pointerup", () => togglePopover());
  }
  bindTray($("#trayHit"));
  bindTray($("#winTrayHit"));

  document.addEventListener("pointerdown", (e) => {
    if (!state.popoverOpen) return;
    if (Date.now() < state.suppressBlurUntil) return;
    if (popover.contains(e.target)) return;
    if (e.target.closest(".tray-hit")) return;
    if (e.target.closest(".util-window")) return;
    if (e.target.closest(".toast")) return;
    hidePopover();
  });

  /* Footer */
  $("#addBtn").onclick = () => openEditor(null);
  $("#emptyAdd").onclick = () => openEditor(null);
  $("#checkAll").onclick = () => {
    state.services.forEach((s) => { if (!s.paused) s.ageSec = 0; });
    renderList();
  };
  $("#openSettings").onclick = () => {
    hidePopover();
    $("#settingsWin").hidden = false;
  };
  $("#quitBtn").onclick = () => {
    hidePopover();
    closeWin("detailWin");
    closeWin("editorWin");
    closeWin("settingsWin");
    notifyStack.innerHTML = "";
    const n = document.createElement("div");
    n.className = "quit-note";
    n.textContent = "Pulse would quit. This is a prototype — use Empty / Healthy / Incident to continue.";
    desktop.appendChild(n);
    setTimeout(() => n.remove(), 2600);
  };

  $$("[data-close]").forEach((b) => {
    b.addEventListener("click", () => closeWin(b.dataset.close));
  });

  /* Settings */
  $$(".settings-nav button").forEach((b) => {
    b.addEventListener("click", () => {
      $$(".settings-nav button").forEach((x) => x.classList.remove("is-active"));
      b.classList.add("is-active");
      $$(".settings-pane").forEach((p) => p.classList.toggle("is-active", p.dataset.pane === b.dataset.pane));
    });
  });
  const days = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
  const dayBox = $("#qhDays");
  days.forEach((d, i) => {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.textContent = d;
    btn.className = state.qhDays.includes(i) ? "is-on" : "";
    btn.addEventListener("click", () => {
      if (state.qhDays.includes(i)) state.qhDays = state.qhDays.filter((x) => x !== i);
      else state.qhDays.push(i);
      btn.classList.toggle("is-on");
    });
    dayBox.appendChild(btn);
  });
  $("#exportSecrets").addEventListener("change", (e) => {
    $("#secretWarn").hidden = !e.target.checked;
  });
  $("#resetInput").addEventListener("input", (e) => {
    $("#resetBtn").disabled = e.target.value !== "RESET";
  });
  $("#exportBtn").onclick = () => {
    const payload = {
      schemaVersion: 1,
      includeSecrets: $("#exportSecrets").checked,
      services: state.services.map((s) => ({
        name: s.name,
        url: s.url,
        method: s.method,
        headers: (s.headers || []).map((h) => ({
          key: h.key,
          secret: h.secret,
          value: $("#exportSecrets").checked || !h.secret ? h.value : undefined,
        })),
        intervalSec: s.intervalSec,
        assertions: s.assertions,
      })),
    };
    const blob = new Blob([JSON.stringify(payload, null, 2)], { type: "application/json" });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = $("#exportSecrets").checked ? "pulse-services.SECRETS.json" : "pulse-services.json";
    a.click();
  };
  $("#importBtn").onclick = () => {
    alert("Import opens a native file dialog in the real app (Rust-side). Hosts are listed before anything is written.");
  };
  $("#resetBtn").onclick = () => {
    loadScene("empty");
    closeWin("settingsWin");
  };
  $("#setTheme").addEventListener("change", (e) => {
    desktop.dataset.theme = e.target.value === "system" ? "dark" : e.target.value;
  });

  /* Keyboard */
  document.addEventListener("keydown", (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "n") {
      e.preventDefault();
      openEditor(null);
      return;
    }
    if (e.key === "Escape") {
      if (!$("#editorWin").hidden) return closeWin("editorWin");
      if (!$("#detailWin").hidden) return closeWin("detailWin");
      if (!$("#settingsWin").hidden) return closeWin("settingsWin");
      hidePopover();
      return;
    }
    if (popover.hidden) return;
    const items = sortedServices();
    const idx = items.findIndex((s) => s.id === state.selected);
    if (e.key === "ArrowDown") {
      e.preventDefault();
      state.selected = items[Math.min(items.length - 1, idx + 1)]?.id;
      renderList();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      state.selected = items[Math.max(0, idx - 1)]?.id;
      renderList();
    } else if (e.key.toLowerCase() === "r") {
      if (state.selected) checkNow(state.selected);
    } else if (e.key === "Enter" && e.shiftKey) {
      if (state.selected) openDetail(state.selected);
    } else if (e.key === "Enter") {
      const s = items.find((x) => x.id === state.selected);
      if (s) window.open(s.action || s.url, "_blank", "noopener");
    } else if (e.key.toLowerCase() === "p") {
      const s = items.find((x) => x.id === state.selected);
      if (s) { s.paused = !s.paused; renderList(); }
    }
  });

  function tickClock() {
    const d = new Date();
    const fmt = d.toLocaleString("en-US", { weekday: "short", month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
    $("#clock").textContent = fmt.replace(",", "  ");
    $("#winClock").textContent = d.toLocaleTimeString("en-US", { hour: "numeric", minute: "2-digit" });
  }
  tickClock();
  setInterval(tickClock, 15000);

  loadScene("incident");
})();
