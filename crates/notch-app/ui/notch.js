// notch.js — the notch HUD. Collapsed it fills the menu-bar row either side of
// the physical notch; Rust grows the window when the cursor arrives, and this page
// follows the window's own size rather than guessing at hover state.
//
// Every row is built with createElement and textContent, never innerHTML: some of
// what lands here comes from a transcript, a `gh` response or a briefing written by
// a language model, and none of it should ever be able to act as markup.

const { invoke } = window.__TAURI__.core;

const POLL_COLLAPSED_MS = 15000;
const POLL_EXPANDED_MS = 5000;
/// How long the cursor must rest on a tab pill before it switches.
const TAB_DWELL_MS = 250;
/// Session rows the Overview shows before deferring to the Sessions tab. The
/// panel's height in notch.rs (EXPANDED_H) is sized for this — change both.
const SESSION_ROWS = 5;
/// Days of the heatmap the Overview strip draws; the Git tab draws all 90.
const OVERVIEW_HEAT_DAYS = 30;
/// Pushes the Git tab lists.
const PUSH_ROWS = 40;

const el = (id) => document.getElementById(id);
const panel = () => el("panel");

let pollTimer = null;
let lastPayload = null;

// --- DOM helpers ----------------------------------------------------------

/// A `<tag class="cls">text</tag>` node. Text goes in as text, so nothing in the
/// payload can be read as markup — no escaping to get right.
function node(tag, cls, text) {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text !== undefined && text !== null) n.textContent = String(text);
  return n;
}

/// Replaces an element's children in one go.
function fill(parent, children) {
  parent.replaceChildren(...(Array.isArray(children) ? children : [children]));
}

/// A one-line "nothing here" placeholder.
function empty(text) {
  return node("p", "empty", text);
}

/// Renders `rows` through `build`, or the placeholder when there are none.
function fillRows(parent, rows, build, emptyText) {
  fill(parent, rows.length ? rows.map(build) : empty(emptyText));
}

function setText(id, text) {
  el(id).textContent = text ?? "";
}

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
// fires once its window is key, and this panel never takes focus, so it would only
// work after you clicked it. The window stays one size; only this class changes,
// and CSS animates the shape.
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
  // The clamp mark is the only thing that says the panel is held, now that the
  // clock it used to tint is gone.
  el("pill-pin").hidden = !on;
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

function round(value) {
  return Math.round(pct(value));
}

/// The band a usage percentage falls in, which decides the meter's colour.
function level(value) {
  if (value >= 90) return "danger";
  if (value >= 70) return "warn";
  return "";
}

/// Meters and the battery are sized through a custom property rather than a direct
/// style write, so the stylesheet keeps ownership of how it is drawn.
function setMeter(fillId, trackId, value, modifier) {
  const kind = modifier === undefined ? level(pct(value)) : modifier;
  const fillEl = el(fillId);
  fillEl.style.setProperty("--pct", `${pct(value)}%`);
  fillEl.className = `meter__fill${kind ? ` meter__fill--${kind}` : ""}`;
  el(trackId).setAttribute("aria-valuenow", String(round(value)));
}

/// 8s / 4m / 1h20m — short enough for a 9px column.
function ago(secs) {
  if (secs == null || secs < 0) return "";
  if (secs < 60) return `${Math.round(secs)}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  return m ? `${h}h${m}m` : `${h}h`;
}

/// "4m" / "3h" / "2d" from an RFC3339 timestamp.
function since(iso) {
  const t = Date.parse(iso);
  if (!t) return "";
  return ago(Math.max(0, (Date.now() - t) / 1000));
}

/// "resets 3h52m · Tue 2:40 AM" — the countdown and the clock time it ends at.
function resetLine(w) {
  if (!w?.resets_in) return "";
  return w.resets_at ? `resets ${w.resets_in} · ${w.resets_at}` : `resets ${w.resets_in}`;
}

/// Which heat band a day's commit count falls in.
function heatLevel(count) {
  if (!count) return "";
  if (count <= 2) return "l1";
  if (count <= 6) return "l2";
  if (count <= 12) return "l3";
  return "l4";
}

/// One heat cell, for either the strip or the 90-day grid.
function heatCell(block, day) {
  const lvl = heatLevel(day.count);
  const cell = node("i", `${block}${lvl ? ` ${block}--${lvl}` : ""}`);
  cell.title = `${day.date}: ${day.count}`;
  return cell;
}

// --- collapsed strip ------------------------------------------------------

function renderPill(p) {
  // A percentage in the strip always means Claude session usage. Day progress gets
  // the meter but no number, so the two can never be read as each other.
  setText("pill-session", p.session_pct == null ? "" : `${round(p.session_pct)}%`);

  // Three segments: of session usage when that module is on, of day progress
  // otherwise, so the strip still says something with only `day` switched on.
  const source = p.session_pct ?? p.day_pct;
  const micro = el("pill-day");
  const lit = source == null ? 0 : Math.min(3, Math.ceil(pct(source) / 33.4));
  const band = p.session_pct == null ? "" : level(pct(p.session_pct));
  micro.className = `micro${band ? ` micro--${band}` : ""}`;
  micro.title = p.session_pct == null ? "Day elapsed" : "Claude session used";
  [...micro.children].forEach((seg, i) => seg.classList.toggle("micro__seg--on", i < lit));

  setText("pill-todos", p.todos ? `☑ ${p.todos}` : "");
  setText("pill-commits", p.commits == null ? "" : `⎇ ${p.commits}`);

  const agents = p.agents ?? 0;
  el("pill-dot").hidden = !agents;
  el("pill-dot").className = `dot${agents ? " dot--live" : ""}`;
  setText("pill-agents", agents ? String(agents) : "");

  setText("pill-batt", p.battery_pct == null ? "" : `${p.battery_pct}%${p.charging ? " ⚡" : ""}`);
}

// --- overview: usage card -------------------------------------------------

function renderUsageCard(u) {
  el("card-usage").hidden = !u;
  if (!u) return;

  if (!u.available) {
    setText("u-session", "—");
    setText("u-session-reset", u.error ?? "unavailable");
    setText("u-week", "—");
    setText("u-week-reset", "");
    setMeter("u-session-fill", "u-session-track", 0, "neutral");
    setMeter("u-week-fill", "u-week-track", 0, "neutral");
    return;
  }

  const s = u.session ?? {};
  const w = u.week ?? {};
  setText("u-session", `${round(s.percent)}%`);
  setText("u-session-reset", resetLine(s));
  setText("u-week", `${round(w.percent)}%`);
  setText("u-week-reset", resetLine(w));
  setMeter("u-session-fill", "u-session-track", s.percent);
  setMeter("u-week-fill", "u-week-track", w.percent);
}

// --- overview: day card ---------------------------------------------------

function renderDayCard(d) {
  el("card-day").hidden = !d;
  if (!d) return;

  setText("d-clock", d.clock ?? "—");
  setText("d-meridiem", d.meridiem);
  setText("d-date", d.date);
  setText("d-remaining", d.remaining);
  setMeter("d-day-fill", "d-day-track", d.progress, "neutral");

  setText("d-week-pct", `${round(d.week_progress)}%`);
  setMeter("d-week-fill", "d-week-track", d.week_progress, "neutral");

  renderBattery(d.battery);
}

function renderBattery(b) {
  const wrap = el("d-batt");
  wrap.hidden = !b;
  if (!b) return;

  setText("d-batt-pct", `${b.percent}%`);
  const fillEl = el("d-batt-fill");
  fillEl.style.setProperty("--pct", `${pct(b.percent)}%`);
  const state = b.charging ? "charging" : b.percent <= 15 ? "low" : "";
  fillEl.className = `batt__fill${state ? ` batt__fill--${state}` : ""}`;
  wrap.title = `Battery ${b.percent}% — ${b.state}`;
}

// --- overview: contributions card ----------------------------------------

function renderGitCard(g) {
  el("card-git").hidden = !g;
  if (!g) return;

  if (!g.available) {
    setText("g-today", "—");
    // `gh` takes seconds; a cold cache is pending, not broken.
    setText("g-week", g.pending ? "loading…" : "gh unavailable");
    fill(el("g-heat"), []);
    setText("g-prs", "");
    setText("g-login", "");
    return;
  }

  setText("g-today", g.today ?? 0);
  setText("g-week", `${g.week ?? 0} this week`);
  setText("g-prs", `${(g.open_prs ?? []).length} open PRs`);
  setText("g-login", g.login ? `@${g.login}` : "");

  // The payload carries 90 days for the Git tab, so take the tail.
  const days = (g.heatmap ?? []).slice(-OVERVIEW_HEAT_DAYS);
  fill(
    el("g-heat"),
    days.map((d) => heatCell("heat__cell", d)),
  );
}

// --- sessions -------------------------------------------------------------

/// One session row, shared by the Overview and the Sessions tab.
function sessionRow(r) {
  const row = node("div", "sess");
  const state = r.status === "active" ? "live" : r.status === "tool" ? "tool" : "";
  row.append(node("span", `dot${state ? ` dot--${state}` : ""}`));

  const head = node("div", "sess__head");
  head.append(node("span", "sess__title", r.title));
  if (r.model) head.append(node("span", "chip", r.model));

  // An untitled session falls back to its project name; don't print it twice.
  const branch = r.branch ? `⎇ ${r.branch}` : "";
  const where = r.title === r.project ? branch : [r.project, branch].filter(Boolean).join(" ");
  if (where) head.append(node("span", "sess__where", where));
  head.append(node("span", "sess__meta", `${r.messages} msgs · ${ago(r.idle_secs)}`));

  const main = node("div", "sess__main");
  main.append(head);
  if (r.preview) main.append(node("div", "sess__preview", r.preview));
  row.append(main);
  return row;
}

const NO_SESSIONS = "No sessions in the last 12 hours.";

/// The Overview's list: the newest few, with a note about the rest.
function renderSessionsCard(s) {
  el("pane-agents").hidden = !s;
  if (!s) return;

  const list = el("agent-list");
  const more = el("agent-more");

  if (!s.available) {
    setText("a-count", "");
    fill(list, empty(s.error ?? "unavailable"));
    more.hidden = true;
    return;
  }

  setText("a-count", s.active ? `· ${s.active} active` : "");
  const all = s.list ?? [];
  const rows = all.slice(0, SESSION_ROWS);
  const hidden = all.length - rows.length;
  more.hidden = hidden <= 0;
  more.textContent = hidden > 0 ? `+${hidden} older ${hidden === 1 ? "session" : "sessions"}` : "";

  fillRows(list, rows, sessionRow, NO_SESSIONS);
}

/// The Sessions tab: everything, scrollable, no cap.
function renderSessionsTab(s) {
  const list = el("session-list");
  if (!s) {
    setText("s-count", "");
    fill(list, empty("The sessions module is off — run: notch module sessions on"));
    return;
  }
  if (!s.available) {
    setText("s-count", "");
    fill(list, empty(s.error ?? "unavailable"));
    return;
  }
  const all = s.list ?? [];
  setText("s-count", all.length ? `· ${all.length}` : "");
  fillRows(list, all, sessionRow, NO_SESSIONS);
}

// --- to-do ---------------------------------------------------------------

const TODO_SECTIONS = [
  ["td-today", "td-today-n", "today", "Nothing urgent today 🎉"],
  ["td-week", "td-week-n", "week", "All clear for the week!"],
  ["td-prog", "td-prog-n", "in_progress", "Nothing in progress."],
  ["td-done", "td-done-n", "done", "Nothing marked done yet."],
];

function todoRow(item, done) {
  const row = node("div", `todo${done ? " todo--done" : ""}`);
  row.append(node("span", "todo__mark", done ? "✓" : "•"));

  const text = node("span", "todo__text", item.text);
  const src = [item.channel, item.who].filter(Boolean).join(" · ");
  if (src) {
    text.append(document.createTextNode(" "), node("span", "todo__src", src));
  }
  row.append(text);
  return row;
}

/// The briefing, in the four sections its producer writes. Nothing here talks to
/// Slack — this only renders the file.
function renderTodos(t) {
  const foot = el("td-foot");

  if (!t?.available) {
    const why = !t
      ? "The to-do module is off — run: notch module todos on"
      : t.missing
        ? `No briefing yet — have a producer write ${t.path ?? "todos.json"}`
        : (t.error ?? "unavailable");
    for (const [listId, countId] of TODO_SECTIONS) {
      fill(el(listId), empty("—"));
      setText(countId, "");
    }
    fill(el("td-today"), empty(why));
    foot.textContent = "";
    foot.className = "foot";
    return;
  }

  for (const [listId, countId, key, emptyText] of TODO_SECTIONS) {
    const rows = t[key] ?? [];
    setText(countId, rows.length ? String(rows.length) : "");
    fillRows(el(listId), rows, (item) => todoRow(item, key === "done"), emptyText);
  }

  // A briefing nobody refreshed is worse than none, so say how old it is.
  const age = t.age_hours;
  foot.textContent =
    age == null
      ? "briefing has no timestamp"
      : `briefed ${age === 0 ? "under an hour" : ago(age * 3600)} ago${t.stale ? " — stale" : ""}`;
  foot.className = t.stale ? "foot foot--warn" : "foot";
}

// --- usage tab -----------------------------------------------------------

function renderUsageTab(u) {
  if (!u?.available) {
    setText("ut-status", u ? "" : "module off");
    setText("ut-session", "—");
    setText("ut-week", "—");
    setText("ut-session-reset", !u ? "run: notch module usage on" : (u.error ?? "unavailable"));
    setText("ut-week-reset", "");
    setMeter("ut-session-fill", "ut-session-track", 0, "neutral");
    setMeter("ut-week-fill", "ut-week-track", 0, "neutral");
    return;
  }

  const s = u.session ?? {};
  const w = u.week ?? {};
  setText("ut-status", u.status);
  setText("ut-session", `${round(s.percent)}%`);
  setText("ut-session-reset", resetLine(s));
  setText("ut-week", `${round(w.percent)}%`);
  setText("ut-week-reset", resetLine(w));
  setMeter("ut-session-fill", "ut-session-track", s.percent);
  setMeter("ut-week-fill", "ut-week-track", w.percent);
}

/// The reset-window list. Grouping happens in Rust (see history::windows) — one
/// row per real Anthropic window, newest first.
function renderWindows(d) {
  const box = el("ut-windows");
  if (!d?.available) {
    fill(box, empty(d?.error ?? "no history yet"));
    return;
  }

  const sPeak = round(d.session_peak);
  const wPeak = round(d.week_peak);
  setText("ut-speak", `${sPeak}%`);
  setText("ut-wpeak", `${wPeak}%`);

  fillRows(
    box,
    d.windows ?? [],
    (w) => {
      const sHi = round(w.session_peak);
      const wHi = round(w.week_high);
      const wLo = round(w.week_low);

      const row = node("div", "win");
      row.append(node("span", "win__when", w.label));
      row.append(node("b", `win__session${sHi === sPeak ? " win__session--peak" : ""}`, `${sHi}%`));
      row.append(
        node(
          "span",
          `win__week${wHi === wPeak ? " win__week--peak" : ""}`,
          wLo === wHi ? `${wHi}%` : `${wLo}–${wHi}%`,
        ),
      );
      return row;
    },
    "No samples in the last 7 days.",
  );
}

/// Fetched only for the Usage tab — 7 days of samples is more than the strip needs,
/// and nothing else reads it.
async function loadWindows() {
  try {
    renderWindows(await invoke("notch_windows"));
  } catch (e) {
    renderWindows({ available: false, error: String(e?.message ?? e) });
  }
}

// --- git tab -------------------------------------------------------------

function renderGitTab(g) {
  const pushes = el("gt-pushes");

  if (!g?.available) {
    fill(
      pushes,
      empty(
        !g
          ? "The git module is off — run: notch module git on"
          : g.pending
            ? "loading…"
            : "gh unavailable",
      ),
    );
    return;
  }

  setText("gt-login", g.login ? `@${g.login}` : "");
  setText("gt-today", g.today ?? 0);
  setText("gt-week", g.week ?? 0);
  setText("gt-30", g.last30d ?? 0);
  setText("gt-year", g.year ?? 0);
  setText("gt-prs", `${(g.open_prs ?? []).length} open PRs`);

  fill(
    el("gt-grid"),
    (g.heatmap ?? []).map((d) => heatCell("grid__cell", d)),
  );

  fillRows(
    pushes,
    (g.recent_pushes ?? []).slice(0, PUSH_ROWS),
    (p) => {
      const row = node("div", "push");
      row.append(node("span", "push__repo", p.repo));
      row.append(node("span", "push__branch", `⎇ ${p.branch || "—"}`));
      row.append(node("span", "push__when", since(p.at)));
      return row;
    },
    "No pushes found.",
  );
}

// --- data ----------------------------------------------------------------

function render(data) {
  lastPayload = data;
  setPinned(data.pinned);
  renderPill(data.pill ?? {});

  renderUsageCard(data.usage);
  renderDayCard(data.day);
  renderGitCard(data.git);
  renderSessionsCard(data.sessions);

  renderTodos(data.todos);
  renderUsageTab(data.usage);
  renderSessionsTab(data.sessions);
  renderGitTab(data.git);

  if (activeTab() === "usage") loadWindows();
}

async function load() {
  try {
    render(await invoke("notch_payload"));
  } catch (err) {
    // A HUD with no console has to say when it breaks, rather than sitting on
    // stale placeholders and looking merely empty.
    if (!lastPayload) {
      fill(el("agent-list"), empty(String(err?.message ?? err)));
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

function activeTab() {
  return document.querySelector(".tab--on")?.dataset.tab ?? "overview";
}

function setTab(name) {
  const changed = activeTab() !== name;
  document.querySelectorAll(".tab").forEach((t) => {
    const on = t.dataset.tab === name;
    t.classList.toggle("tab--on", on);
    t.setAttribute("aria-selected", String(on));
  });
  document.querySelectorAll(".view").forEach((v) => {
    v.classList.toggle("view--on", v.id === `view-${name}`);
  });
  // The window history is only read for the tab that shows it.
  if (changed && name === "usage") loadWindows();
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
// point, so they must not be lost to a command that hangs. Geometry and the initial
// open state are a best-effort follow-up.
load();
restartPolling(POLL_COLLAPSED_MS);
applyMetrics()
  .then(setOpen)
  .catch(() => {});
