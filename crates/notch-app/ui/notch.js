// notch.js — the notch HUD. Collapsed it fills the menu-bar row either side of
// the physical notch; Rust grows the window when the cursor arrives, and this page
// follows the window's own size rather than guessing at hover state.

const { invoke } = window.__TAURI__.core;

const POLL_COLLAPSED_MS = 15000;
const POLL_EXPANDED_MS = 5000;
/// How long the cursor must rest on a tab pill before it switches.
const TAB_DWELL_MS = 250;

const el = (id) => document.getElementById(id);
const panel = () => el("panel");

let pollTimer = null;
let lastPayload = null;

// --- geometry -------------------------------------------------------------

async function applyMetrics() {
  try {
    const m = await invoke("notch_metrics");
    const root = document.documentElement.style;
    root.setProperty("--menubar-h", `${Math.max(20, Number(m.menubar_h) || 37)}px`);
    root.setProperty("--notch-w", `${Math.max(0, Number(m.notch_w) || 0)}px`);
    root.setProperty("--collapsed-w", `${Math.max(180, Number(m.collapsed_w) || 268)}px`);
    // A reload while the cursor is already inside must not leave us shut.
    return Boolean(m.open);
  } catch (_) {
    return false; // keep the CSS defaults, start collapsed
  }
}

// --- open / close ---------------------------------------------------------

// Rust owns the hover decision and calls setOpen: a webview's own mouseenter only
// fires once its window is key, and this panel never takes focus, so it would
// only work after you clicked it. The window stays one size; only this class
// changes, and CSS animates the shape.
function setOpen(open) {
  panel().classList.toggle("panel--open", open);
  if (open) {
    restartPolling(POLL_EXPANDED_MS);
    load(); // fresh numbers the moment the panel opens
  } else {
    restartPolling(POLL_COLLAPSED_MS);
    setTab("overview");
    hotTab = null;
    setPinned(false);
  }
}

// --- pin ------------------------------------------------------------------

// Click the sliver to pin the panel open; click it again to close. Hover stays a
// peek, so glancing costs nothing and reading doesn't need a held cursor.
function setPinned(on) {
  panel().classList.toggle("panel--pinned", Boolean(on));
}

el("strip").addEventListener("click", async () => {
  try {
    setPinned(await invoke("notch_pin", {}));
  } catch (_) {
    /* the panel still works unpinned */
  }
});

// --- formatting -----------------------------------------------------------

/// Percentage, clamped to something a meter can draw.
function pct(value) {
  return Math.max(0, Math.min(100, Number(value) || 0));
}

/// Meters and the battery are sized through a custom property rather than a
/// direct style write, so the stylesheet keeps ownership of how it is drawn.
function setFill(id, value) {
  el(id).style.setProperty("--pct", `${pct(value)}%`);
}

/// Mirrors the visible value for anything reading the tree instead of the pixels.
function setProgress(id, value) {
  el(id).setAttribute("aria-valuenow", String(Math.round(pct(value))));
}

// --- rendering ------------------------------------------------------------

function renderPill(p) {
  el("pill-clock").textContent = p.clock || "—";
  el("pill-meridiem").textContent = p.meridiem ?? "";

  // Three segments of day progress: morning, afternoon, evening at a glance.
  const lit = p.day_pct == null ? 0 : Math.min(3, Math.ceil(pct(p.day_pct) / 33.4));
  [...el("pill-day").children].forEach((seg, i) => {
    seg.classList.toggle("micro__seg--on", i < lit);
  });

  const batt = p.battery_pct;
  el("pill-batt").textContent = batt == null ? "" : `${batt}%${p.charging ? " ⚡" : ""}`;
}

function renderDayCards(d) {
  const shown = Boolean(d);
  el("card-clock").hidden = !shown;
  el("card-week").hidden = !shown;
  if (!shown) return;

  el("d-clock").textContent = d.clock ?? "—";
  el("d-meridiem").textContent = d.meridiem ?? "";
  el("d-date").textContent = d.date ?? "";
  el("d-remaining").textContent = d.remaining ?? "";
  setFill("d-day-fill", d.progress);
  setProgress("d-day-track", d.progress);

  el("d-week-pct").textContent = `${Math.round(pct(d.week_progress))}%`;
  setFill("d-week-fill", d.week_progress);
  setProgress("d-week-track", d.week_progress);
  el("d-week-note").textContent = weekNote(d.week_progress);

  renderBattery(d.battery);
}

/// Says where in the week you are, which a bare percentage does not.
function weekNote(weekPct) {
  const done = pct(weekPct);
  if (done < 20) return "The week is just starting.";
  if (done < 50) return "Not yet halfway through the week.";
  if (done < 80) return "Past the middle of the week.";
  return "The week is nearly done.";
}

function renderBattery(b) {
  const wrap = el("d-batt");
  wrap.hidden = !b;
  if (!b) return;

  el("d-batt-pct").textContent = `${b.percent}%`;
  setFill("d-batt-fill", b.percent);
  const fill = el("d-batt-fill");
  fill.classList.toggle("batt__fill--charging", Boolean(b.charging));
  fill.classList.toggle("batt__fill--low", !b.charging && b.percent <= 15);
  wrap.title = `Battery ${b.percent}% — ${b.state}`;
}

/// A `<tag class="cls">text</tag>` node. Text goes in as text, so nothing in the
/// payload can be read as markup — no escaping to get right, and no innerHTML.
function node(tag, cls, text) {
  const n = document.createElement(tag);
  n.className = cls;
  n.textContent = text;
  return n;
}

/// Replaces an element's children in one go.
function fill(parent, children) {
  parent.replaceChildren(...children);
}

/// The readings the Day tab spells out, as key/value pairs.
function dayFacts(d) {
  return [
    ["Time", `${d.clock ?? "—"} ${d.meridiem ?? ""}`.trim()],
    ["Date", d.date ?? "—"],
    ["Day elapsed", `${Math.round(pct(d.progress))}%`],
    ["Left today", d.remaining ?? "—"],
    ["Week elapsed", `${Math.round(pct(d.week_progress))}%`],
    ["Battery", d.battery ? `${d.battery.percent}% — ${d.battery.state}` : "no battery"],
  ];
}

/// The Day tab: the same readings spelled out, for when a meter is not enough.
function renderDayFacts(d) {
  const list = el("day-facts");
  if (!d) {
    fill(list, [node("p", "empty", "The day module is off — run: notch module day on")]);
    return;
  }

  fill(
    list,
    dayFacts(d).map(([key, value]) => {
      const row = node("div", "facts__row", "");
      row.append(node("dt", "facts__key", key), node("dd", "facts__val", value));
      return row;
    }),
  );
}

function render(data) {
  lastPayload = data;
  setPinned(data.pinned);
  renderPill(data.pill ?? {});
  renderDayCards(data.day);
  renderDayFacts(data.day);
}

// --- data ----------------------------------------------------------------

async function load() {
  try {
    render(await invoke("notch_payload"));
  } catch (err) {
    // A HUD with no console has to say when it breaks, rather than sitting on
    // stale placeholders and looking merely empty.
    if (!lastPayload) {
      fill(el("day-facts"), [node("p", "empty", String(err?.message ?? err))]);
    }
  }
}

function restartPolling(ms) {
  clearInterval(pollTimer);
  pollTimer = setInterval(load, ms);
}

// --- tabs -----------------------------------------------------------------

let hotTab = null;
let hotSince = 0;

function setTab(name) {
  document.querySelectorAll(".tab").forEach((t) => {
    const on = t.dataset.tab === name;
    t.classList.toggle("tab--on", on);
    t.setAttribute("aria-selected", String(on));
  });
  document.querySelectorAll(".view").forEach((v) => {
    v.classList.toggle("view--on", v.id === `view-${name}`);
  });
}

/// Rust reports where the cursor is, in this page's own pixels, because WebKit's
/// :hover only fires for a key window and this panel is never key. The page does
/// the hit-testing, so Rust needs to know nothing about the layout.
function cursor(x, y) {
  const under = document.elementFromPoint(x, y);
  const tab = under?.closest?.(".tab") ?? null;
  document.querySelectorAll(".tab").forEach((t) => t.classList.toggle("tab--hot", t === tab));

  if (!tab) {
    hotTab = null;
    return;
  }
  if (tab !== hotTab) {
    hotTab = tab;
    hotSince = performance.now();
    return;
  }
  // Resting on a pill switches to it; passing over doesn't.
  if (performance.now() - hotSince >= TAB_DWELL_MS) setTab(tab.dataset.tab);
}

// Clicking works too — the window takes the cursor back while it's open.
document.querySelectorAll(".tab").forEach((t) => {
  t.addEventListener("click", (e) => {
    e.stopPropagation(); // not a click on the sliver, so don't toggle the pin
    setTab(t.dataset.tab);
  });
});

document.addEventListener("contextmenu", (e) => e.preventDefault());

// Rust's handles into this page: hover state, and a refresh after a module is
// toggled.
window.notchHud = {
  setOpen,
  setPinned,
  refresh: load,
  cursor,
};

// Data first, and never gated on IPC for the geometry: the numbers are the whole
// point, so they must not be lost to a command that hangs. Geometry and the
// initial open state are a best-effort follow-up.
load();
restartPolling(POLL_COLLAPSED_MS);
applyMetrics()
  .then(setOpen)
  .catch(() => {});
