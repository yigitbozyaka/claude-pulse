const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

const MODEL_ORDER = ["opus", "sonnet", "haiku", "other"];
const SESSION_MS = 5 * 3600 * 1000; // the API's `five_hour` window
const MODEL_NAME = { opus: "Opus", sonnet: "Sonnet", haiku: "Haiku", other: "Other" };
const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

const $ = (s) => document.querySelector(s);
const view = $("#view");
const tabsNav = $("#tabs");
const tabTrack = $("#tabTrack");
const tabHi = $("#tabHi");

let accounts = [];
let active = 0;
let busy = false;

// ── helpers ─────────────────────────────────────────────────────────

function fmtTokens(n) {
  if (n >= 1e6) return (n / 1e6).toFixed(1) + "M";
  if (n >= 1e3) return (n / 1e3).toFixed(1) + "K";
  return "" + n;
}
function esc(s) {
  return String(s).replace(/[&<>"]/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c])
  );
}
function sumModels(bm) {
  return MODEL_ORDER.reduce((a, m) => a + (bm[m] || 0), 0);
}
// Window start + burn rate, derived from the session reset timestamp.
function sessionExtras(u) {
  const iso = u.session.resets_at;
  const reset = iso ? new Date(iso) : null;
  if (!reset || isNaN(reset)) return { started: null, burn: null };
  const start = new Date(reset.getTime() - SESSION_MS);
  const started = start.toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
  const elapsedH = (Date.now() - start.getTime()) / 3600000;
  let burn = null;
  if (elapsedH > 0.05) {
    const rate = u.session.utilization / elapsedH;
    burn = "~" + (rate >= 10 ? Math.round(rate) : rate.toFixed(1)) + "%/h";
  }
  return { started, burn };
}

// ── tabs ────────────────────────────────────────────────────────────

function buildTabs() {
  if (accounts.length < 2) {
    tabsNav.hidden = true;
    return;
  }
  tabsNav.hidden = false;
  [...tabTrack.querySelectorAll(".tab, .tab-edit")].forEach((b) => b.remove());
  accounts.forEach((name, i) => {
    const b = document.createElement("button");
    b.className = "tab" + (i === active ? " active" : "");
    b.textContent = name;
    b.title = "Double-click to rename";
    b.onclick = () => selectTab(i);
    b.ondblclick = () => startRename(i);
    tabTrack.appendChild(b);
  });
  tabHi.style.left = "0px";
  requestAnimationFrame(moveHi);
}

function startRename(idx) {
  const btn = tabTrack.querySelectorAll(".tab")[idx];
  if (!btn) return;
  const input = document.createElement("input");
  input.className = "tab-edit";
  input.value = accounts[idx];
  input.maxLength = 24;
  input.spellcheck = false;
  btn.replaceWith(input);
  input.focus();
  input.select();
  let done = false;
  const commit = async () => {
    if (done) return;
    done = true;
    const name = input.value.trim();
    if (name && name !== accounts[idx]) {
      accounts = await invoke("rename_account", { idx, name });
    }
    buildTabs();
  };
  input.onkeydown = (e) => {
    if (e.key === "Enter") input.blur();
    else if (e.key === "Escape") {
      done = true;
      buildTabs();
    }
  };
  input.onblur = commit;
}
function moveHi() {
  const el = tabTrack.querySelectorAll(".tab")[active];
  if (!el) return;
  tabHi.style.width = el.offsetWidth + "px";
  tabHi.style.transform = `translateX(${el.offsetLeft - tabTrack.clientLeft}px)`;
}
function markActiveTab() {
  tabTrack
    .querySelectorAll(".tab")
    .forEach((b, i) => b.classList.toggle("active", i === active));
}
async function selectTab(i) {
  if (i === active || busy) return;
  active = i;
  markActiveTab();
  moveHi();
  await render(invoke("set_active", { idx: i }));
}

// ── render pipeline ─────────────────────────────────────────────────

async function render(promise, { skeleton = true } = {}) {
  busy = true;
  if (skeleton && !reduce) showSkeleton();
  let data = null;
  try {
    data = await promise;
  } catch (e) {
    data = null;
  }
  busy = false;
  if (data) buildView(data);
  else showError();
}
function refreshCurrent(silent) {
  return render(invoke("get_account", { idx: active }), { skeleton: !silent });
}

function showSkeleton() {
  view.innerHTML =
    '<div class="skel skel-hero"></div><div class="skel skel-row"></div>' +
    '<div class="skel skel-tall"></div><div class="skel skel-row"></div>';
}
function showError() {
  view.innerHTML =
    '<div class="card"><div class="notice">Couldn\'t load usage.<br>Retrying automatically…</div></div>';
}

function buildView(data) {
  const u = data.usage;
  const local = data.local;
  const parts = [];

  if (u) {
    parts.push(heroCard(u));
    parts.push(limitsCard(u));
  } else {
    parts.push(
      `<section class="card rise"><div class="notice">Couldn't reach Anthropic.<br><b>${esc(
        data.name
      )}</b> · retrying automatically…</div></section>`
    );
  }

  if (local && (local.weekly_tokens.requests > 0 || sumModels(local.by_model) > 0)) {
    if (sumModels(local.by_model) > 0) parts.push(distCard(local));
    parts.push(dailyCard(local));
    parts.push(tokensCard(local));
  }

  parts.push(footer(u));
  view.innerHTML = parts.join("");
  stagger();
  requestAnimationFrame(() => animateValues(data));
  wireFooter();
}

// ── cards ───────────────────────────────────────────────────────────

function heroCard(u) {
  const left = Math.round(Math.min(Math.max(100 - u.session.utilization, 0), 100));
  const { started, burn } = sessionExtras(u);
  const badges = (u.subscription.models || [])
    .map((m) => `<span class="badge">${esc(m)}</span>`)
    .join("");
  const rows = [
    ["Resets in", esc(u.session.resets_in || "now")],
    started && ["Window started", esc(started)],
    burn && ["Burn rate", esc(burn)],
  ]
    .filter(Boolean)
    .map(([k, v]) => `<div class="srow"><span class="srow-k">${k}</span><span class="srow-v">${v}</span></div>`)
    .join("");
  return `
  <section class="card flyout rise">
    <div class="flyout-head">
      <div class="flyout-id">
        <span class="chip"><span class="chip-fill" data-left="${left}"></span></span>
        <span class="flyout-title">Session</span>
      </div>
      <span class="flyout-plan">${esc(u.subscription.display)}</span>
    </div>
    <div class="flyout-big">
      <span class="big-num" data-left="${left}">0</span>
      <span class="big-pct">%</span>
      <span class="big-cap">of session left</span>
    </div>
    <div class="prog"><div class="prog-fill" data-left="${left}"></div></div>
    <div class="srows">${rows}</div>
    ${badges ? `<div class="badges">${badges}</div>` : ""}
  </section>`;
}

function meterRow(name, sub, pct) {
  return `<div class="meter">
    <div class="meter-head">
      <div><div class="meter-name">${esc(name)}</div><div class="meter-sub">${sub}</div></div>
      <div class="meter-val">${Math.round(pct)}%</div>
    </div>
    <div class="bar"><div class="bar-fill" data-pct="${Math.min(pct, 100)}"></div></div>
  </div>`;
}
function limitsCard(u) {
  const rows = [
    meterRow("All models", `Resets in ${esc(u.weekly_all.resets_in || "now")}`, u.weekly_all.utilization),
  ];
  if (u.subscription.has_sonnet && u.weekly_sonnet.resets_at) {
    rows.push(meterRow("Sonnet", `Resets in ${esc(u.weekly_sonnet.resets_in || "now")}`, u.weekly_sonnet.utilization));
  }
  if (u.weekly_opus && u.weekly_opus.resets_at) {
    rows.push(meterRow("Opus", `Resets in ${esc(u.weekly_opus.resets_in || "now")}`, u.weekly_opus.utilization));
  }
  return `<section class="card rise"><div class="card-title">Weekly limits</div>${rows.join("")}</section>`;
}

function distCard(local) {
  const bm = local.by_model;
  const total = Math.max(sumModels(bm), 1);
  let segs = "", leg = "";
  MODEL_ORDER.forEach((m) => {
    const c = bm[m] || 0;
    if (!c) return;
    const p = (c / total) * 100;
    segs += `<div class="dist-seg" style="background:var(--${m})" data-w="${p}"></div>`;
    leg += `<div class="legend-item"><span class="dot" style="background:var(--${m})"></span>${MODEL_NAME[m]} ${Math.round(p)}%</div>`;
  });
  return `<section class="card rise"><div class="card-title">Model distribution</div><div class="dist-bar">${segs}</div><div class="legend">${leg}</div></section>`;
}

function dailyCard(local) {
  const days = local.daily;
  const max = Math.max(...days.map((d) => d.total), 1);
  const AREA = 72;
  const cols = days
    .map((d, i) => {
      const today = i === days.length - 1;
      const h = d.total > 0 ? Math.max((d.total / max) * AREA, 6) : 0;
      return `<div class="col ${today ? "today" : ""}">
        <div class="col-count">${d.total > 0 ? d.total : ""}</div>
        <div class="col-bar ${d.total > 0 ? "" : "empty"}" data-h="${h.toFixed(0)}"></div>
        <div class="col-day">${esc(d.day)}</div>
      </div>`;
    })
    .join("");
  return `<section class="card rise">
    <div class="card-head">
      <div class="card-title">Daily activity</div>
      <div class="card-note">requests / day</div>
    </div>
    <div class="chart">${cols}</div></section>`;
}

function tokensCard(local) {
  const wt = local.weekly_tokens;
  const tile = (label, val) =>
    `<div class="tile"><div class="tile-val">${val}</div><div class="tile-label">${label}</div></div>`;
  return `<section class="card rise"><div class="card-title">Token usage this week</div>
    <div class="tiles">
      ${tile("Input", fmtTokens(wt.input))}
      ${tile("Output", fmtTokens(wt.output))}
      ${tile("Requests", "" + wt.requests)}
    </div></section>`;
}

function footer(u) {
  const stale = u && u.stale;
  const status = u
    ? `Updated ${esc(u.updated_ago)}${stale ? " · cached (rate-limited)" : ""}`
    : "Waiting for data…";
  return `<div class="foot">
    <span class="foot-status ${stale ? "stale" : ""}">${status}</span>
    <button class="refresh" id="refreshBtn">
      <svg width="13" height="13" viewBox="0 0 24 24"><path d="M21 12a9 9 0 1 1-2.64-6.36M21 3v6h-6"/></svg>
      Refresh
    </button>
  </div>`;
}

// ── animation + wiring ──────────────────────────────────────────────

function stagger() {
  if (reduce) return;
  view.querySelectorAll(".rise").forEach((el, i) => {
    el.style.animationDelay = i * 55 + "ms";
  });
}

function animateValues(data) {
  // session flyout: fill the chip + progress bar, snap the big numeral
  view.querySelectorAll(".chip-fill, .prog-fill").forEach((el) => {
    const dim = el.classList.contains("chip-fill") ? "height" : "width";
    requestAnimationFrame(() => (el.style[dim] = el.dataset.left + "%"));
  });
  const big = view.querySelector(".big-num");
  if (big) big.textContent = big.dataset.left;
  view.querySelectorAll(".bar-fill").forEach((el) => {
    const p = Math.max(+el.dataset.pct, 0);
    requestAnimationFrame(() => (el.style.width = p + "%"));
  });
  view.querySelectorAll(".dist-seg").forEach((el) => {
    requestAnimationFrame(() => (el.style.width = +el.dataset.w + "%"));
  });
  view.querySelectorAll(".col-bar").forEach((el) => {
    if (el.dataset.h !== undefined)
      requestAnimationFrame(() => (el.style.height = +el.dataset.h + "px"));
  });
}

function wireFooter() {
  const btn = view.querySelector("#refreshBtn");
  if (btn)
    btn.onclick = async () => {
      if (busy) return;
      btn.classList.add("spin");
      await refreshCurrent(true);
    };
}

// ── auto-update ─────────────────────────────────────────────────────

async function checkUpdate() {
  let info = null;
  try {
    info = await invoke("check_update");
  } catch (e) {
    return;
  }
  if (!info) return;
  const bar = $("#update");
  const text = $("#updateText");
  const btn = $("#updateBtn");
  text.textContent = `Version ${info.version} is available`;
  bar.hidden = false;
  $("#updateDismiss").onclick = () => (bar.hidden = true);
  btn.onclick = async () => {
    btn.disabled = true;
    text.textContent = "Downloading update…";
    try {
      await invoke("install_update", { url: info.url });
      text.textContent = "Restarting…";
    } catch (e) {
      text.textContent = "Update failed — " + e;
      btn.disabled = false;
    }
  };
}

// ── init ────────────────────────────────────────────────────────────

async function init() {
  $("#min").onclick = () => getCurrentWindow().minimize();
  $("#close").onclick = () => getCurrentWindow().hide();

  accounts = await invoke("list_accounts");
  active = await invoke("get_active");
  buildTabs();
  await render(invoke("get_account", { idx: active }));

  await listen("account-changed", (e) => {
    const i = e.payload;
    if (i === active) return;
    active = i;
    markActiveTab();
    moveHi();
    render(invoke("get_account", { idx: i }));
  });
  await listen("usage-updated", () => {
    if (!busy) refreshCurrent(true);
  });
  window.addEventListener("resize", moveHi);
  checkUpdate();
}

init();
