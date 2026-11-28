// The toolbar popup's shared pieces (#1496), for popup.js and popup-downloads.js: its icons
// (drawn for MDM, one stroke weight, no icon font), sizes, speeds and times in the browser's
// language, a download's status line and the buttons it has, a menu, a confirmation, moving
// through a group with the arrow keys, and a long name cut in its middle. Nothing here runs at
// load but definitions: tools/extension/test.mjs loads this file to test them.
"use strict";

const popupExt = globalThis.browser ?? globalThis.chrome;
// The browser's text for `key` (its _locales), with its placeholders filled.
const tx = (key, ...subs) => popupExt.i18n.getMessage(key, subs.map(String)) || key;
const uiLang = () => popupExt.i18n.getUILanguage();
const SVG = "http://www.w3.org/2000/svg";

// ---- icons ---------------------------------------------------------------------------------

// A gear's outline: eight teeth around a ring (16-unit grid, centre 8,8).
function gearPath() {
  const pts = [];
  const teeth = 8;
  for (let i = 0; i < teeth; i++) {
    const a = (i / teeth) * Math.PI * 2;
    const half = Math.PI / teeth;
    for (const [r, da] of [
      [5.1, -half * 0.95],
      [6.6, -half * 0.45],
      [6.6, half * 0.45],
      [5.1, half * 0.95],
    ]) {
      pts.push([8 + r * Math.cos(a + da), 8 + r * Math.sin(a + da)]);
    }
  }
  return `M${pts.map(([x, y]) => `${x.toFixed(2)} ${y.toFixed(2)}`).join("L")}Z`;
}

// Each icon on a 16-unit grid: its strokes (`d`), and dots filled (`dots`: [x, y, r]).
const ICONS = {
  download: { d: ["M8 2.25v7.5", "M4.75 6.75 8 10l3.25-3.25", "M2.75 10.5v1.75a1 1 0 0 0 1 1h8.5a1 1 0 0 0 1-1V10.5"] },
  page: { d: ["M9.25 1.75H4.5a1 1 0 0 0-1 1v10.5a1 1 0 0 0 1 1h7a1 1 0 0 0 1-1V5z", "M9.25 1.75V5h3.25", "M5.75 8.25h4.5", "M5.75 10.75h3"] },
  gear: { d: [gearPath(), "M8 6a2 2 0 1 0 0 4 2 2 0 0 0 0-4z"] },
  search: { d: ["M7 2.75a4.25 4.25 0 1 0 0 8.5 4.25 4.25 0 0 0 0-8.5z", "m10.1 10.1 3.4 3.4"] },
  folder: { d: ["M2 4.5a1 1 0 0 1 1-1h3.1l1.4 1.5H13a1 1 0 0 1 1 1v6.5a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1z", "M2 6.75h12"] },
  pause: { d: ["M5.75 3.75v8.5", "M10.25 3.75v8.5"] },
  play: { d: ["M5 3.4v9.2a.5.5 0 0 0 .76.43l7.4-4.6a.5.5 0 0 0 0-.86l-7.4-4.6A.5.5 0 0 0 5 3.4z"] },
  more: { d: [], dots: [[3.5, 8, 1.15], [8, 8, 1.15], [12.5, 8, 1.15]] },
  copy: { d: ["M6 5.5h6a1 1 0 0 1 1 1v6.75a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V6.5a1 1 0 0 1 1-1z", "M3 10.5V3a1 1 0 0 1 1-1h6.5"] },
  share: { d: ["M8 9.75V1.75", "M5 4.5l3-2.75 3 2.75", "M5.25 6.75H4a1 1 0 0 0-1 1v5.5a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1v-5.5a1 1 0 0 0-1-1h-1.25"] },
  link: { d: ["M6.6 9.4l2.8-2.8", "M7.4 4.6l1.15-1.15a2.5 2.5 0 0 1 3.55 3.55L10.95 8.15", "M8.6 11.4l-1.15 1.15a2.5 2.5 0 0 1-3.55-3.55L5.05 7.85"] },
  again: { d: ["M13.25 8a5.25 5.25 0 1 1-1.54-3.71", "M12.25 1.75v2.9h-2.9"] },
  trash: { d: ["M2.75 4.25h10.5", "M6.25 4.25V2.75h3.5v1.5", "M4.25 4.25l.65 8.6a1 1 0 0 0 1 .9h4.2a1 1 0 0 0 1-.9l.65-8.6", "M6.75 7v4", "M9.25 7v4"] },
  remove: { d: ["M2.5 4h11", "M2.5 8h6", "M2.5 12h6", "M10.75 9.75l3 3", "M13.75 9.75l-3 3"] },
  open: { d: ["M9.25 2.25h4.5v4.5", "M13.75 2.25 7.5 8.5", "M12 9.25v3.5a1 1 0 0 1-1 1H3.25a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h3.5"] },
  file: { d: ["M9.25 1.75H4.5a1 1 0 0 0-1 1v10.5a1 1 0 0 0 1 1h7a1 1 0 0 0 1-1V5z", "M9.25 1.75V5h3.25"] },
  alert: { d: ["M8 1.75a6.25 6.25 0 1 0 0 12.5 6.25 6.25 0 0 0 0-12.5z", "M8 4.75v3.75"], dots: [[8, 11, 0.85]] },
  server: { d: ["M3 2.75h10a1 1 0 0 1 1 1v2.5a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1v-2.5a1 1 0 0 1 1-1z", "M3 8.75h10a1 1 0 0 1 1 1v2.5a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1v-2.5a1 1 0 0 1 1-1z"], dots: [[4.75, 5, 0.7], [4.75, 11, 0.7]] },
  check: { d: ["M3.25 8.5l3 3 6.5-7"] },
  close: { d: ["M4 4l8 8", "M12 4l-8 8"] },
};

// The icon `name` as an SVG element, `size` pixels square, in the text's colour; hidden from
// screen readers (the button it is in has the name).
function icon(name, size = 16) {
  const spec = ICONS[name] || ICONS.file;
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("viewBox", "0 0 16 16");
  svg.setAttribute("width", String(size));
  svg.setAttribute("height", String(size));
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("focusable", "false");
  svg.classList.add("ico");
  for (const d of spec.d) {
    const p = document.createElementNS(SVG, "path");
    p.setAttribute("d", d);
    svg.append(p);
  }
  for (const [cx, cy, r] of spec.dots || []) {
    const c = document.createElementNS(SVG, "circle");
    c.setAttribute("cx", String(cx));
    c.setAttribute("cy", String(cy));
    c.setAttribute("r", String(r));
    c.classList.add("dot-fill");
    svg.append(c);
  }
  return svg;
}

// ---- numbers in the browser's language ------------------------------------------------------

// A size in the browser's language (its own unit names): "48.7 MB".
function sizeText(bytes, lang = uiLang()) {
  const n = Math.max(0, Number(bytes) || 0);
  const units = [["gigabyte", 1 << 30], ["megabyte", 1 << 20], ["kilobyte", 1 << 10]];
  const [unit, per] = units.find(([, u]) => n >= u) || ["byte", 1];
  try {
    return new Intl.NumberFormat(lang, { style: "unit", unit, unitDisplay: "short", maximumFractionDigits: 1 }).format(n / per);
  } catch {
    return `${(n / per).toFixed(1)} ${unit}`;
  }
}

// A speed: "2.1 MB/s".
function speedText(bps, lang = uiLang()) {
  const n = Math.max(0, Number(bps) || 0);
  const units = [["gigabyte-per-second", 1 << 30], ["megabyte-per-second", 1 << 20], ["kilobyte-per-second", 1 << 10]];
  const [unit, per] = units.find(([, u]) => n >= u) || ["byte-per-second", 1];
  try {
    return new Intl.NumberFormat(lang, { style: "unit", unit, unitDisplay: "short", maximumFractionDigits: 1 }).format(n / per);
  } catch {
    return `${(n / per).toFixed(1)} ${unit.replace("-per-second", "")}/s`;
  }
}

// A length of time as a clock: "0:42", "12:05", "1:02:09".
function clockText(seconds) {
  const s = Math.max(0, Math.round(Number(seconds) || 0));
  const two = (n) => String(n).padStart(2, "0");
  return s >= 3600 ? `${Math.floor(s / 3600)}:${two(Math.floor(s / 60) % 60)}:${two(s % 60)}` : `${Math.floor(s / 60)}:${two(s % 60)}`;
}

// When something happened, from now: "5 minutes ago", "yesterday" (the browser's words).
function agoText(when, now = Date.now(), lang = uiLang()) {
  const secs = Math.round((now - when) / 1000);
  if (!(secs >= 0) || secs < 45) return tx("popup_dl_just_now");
  const steps = [
    ["minute", 60, 60],
    ["hour", 3600, 24],
    ["day", 86400, 7],
    ["week", 604800, 5],
    ["month", 2592000, 12],
    ["year", 31536000, Infinity],
  ];
  for (const [unit, per, max] of steps) {
    const n = Math.round(secs / per);
    if (n < max) {
      try {
        return new Intl.RelativeTimeFormat(lang, { numeric: "auto" }).format(-Math.max(1, n), unit);
      } catch {
        return `${Math.max(1, n)} ${unit}`;
      }
    }
  }
  return "";
}

function percentText(fraction, lang = uiLang()) {
  try {
    return new Intl.NumberFormat(lang, { style: "percent", maximumFractionDigits: 0 }).format(fraction);
  } catch {
    return `${Math.floor(fraction * 100)}%`;
  }
}

// ---- a download, as a row shows it ----------------------------------------------------------

// How far it is, 0 to 1, or null when its size isn't known.
function fractionOf(d) {
  return d.size > 0 ? Math.min(1, Math.max(0, (Number(d.downloaded) || 0) / d.size)) : null;
}

// Seconds left: the app's (`left`), else from its speed (another MDM older than the field).
function leftOf(d) {
  if (Number.isFinite(d.left)) return d.left;
  if (d.speed > 0 && d.size > 0) return Math.max(0, (d.size - (Number(d.downloaded) || 0)) / d.speed);
  return null;
}

// The row's kind of state: running, paused, waiting, done, gone (finished, its file moved or
// deleted), failed.
function stateOf(d) {
  if (d.status === "Downloading") return "running";
  if (d.status === "Complete") return d.exists === false ? "gone" : "done";
  if (d.status === "Error") return "failed";
  if (d.status === "Queued" && !(d.downloaded > 0)) return "waiting";
  return "paused";
}

// What the row says under the name: {text, tone ("" | "bad" | "ok"), bar: fraction | null |
// undefined (no bar), busy (a bar with no size to measure)}. `now`: the time, for "5 min ago".
function statusOf(d, now = Date.now(), lang = uiLang()) {
  const state = stateOf(d);
  const f = fractionOf(d);
  const parts = [];
  const sizes = () =>
    d.size > 0 ? tx("popup_dl_of", sizeText(d.downloaded, lang), sizeText(d.size, lang)) : d.downloaded > 0 ? sizeText(d.downloaded, lang) : "";
  switch (state) {
    case "running": {
      parts.push(sizes());
      if (d.speed > 0) parts.push(speedText(d.speed, lang));
      const left = leftOf(d);
      if (left !== null && d.speed > 0) parts.push(tx("popup_dl_left", clockText(left)));
      return { text: parts.filter(Boolean).join(" · ") || tx("popup_dl_starting"), tone: "", bar: f, busy: f === null };
    }
    case "paused":
      parts.push(tx("popup_dl_paused"));
      if (f !== null) parts.push(percentText(f, lang));
      parts.push(sizes());
      return { text: parts.filter(Boolean).join(" · "), tone: "", bar: f, busy: false };
    case "waiting":
      parts.push(tx("popup_dl_waiting"));
      if (d.size > 0) parts.push(sizeText(d.size, lang));
      return { text: parts.join(" · "), tone: "" };
    case "failed": {
      const why = String(d.error || "").trim();
      return { text: why ? tx("popup_dl_failed_why", why) : tx("popup_dl_failed"), tone: "bad" };
    }
    case "gone":
      return { text: tx("popup_dl_gone"), tone: "muted" };
    default: {
      if (d.size > 0) parts.push(sizeText(d.size, lang));
      const finished = Number(d.finished) > 0 ? Number(d.finished) * 1000 : null;
      parts.push(finished && now - finished >= 60000 ? agoText(finished, now, lang) : tx("popup_dl_done"));
      return { text: parts.join(" · "), tone: finished && now - finished >= 60000 ? "" : "ok" };
    }
  }
}

// The buttons a download has: `local` (this computer's MDM: its file is here), `share` (the app
// opens the system's share sheet). Each: open, folder, copyFile, share, copyLink, pause,
// resume, again (download it again), remove, del.
function actionsOf(d, { local = true, share = false } = {}) {
  const state = stateOf(d);
  const running = state === "running";
  const done = state === "done";
  return {
    open: local && done,
    folder: local && (done || state === "paused" || state === "failed" || running || state === "gone"),
    copyFile: local && done,
    share: local && share && done,
    copyLink: /^(https?|ftps?|magnet):/i.test(String(d.url || "")),
    pause: running,
    resume: state === "paused" || state === "waiting" || state === "failed",
    again: local && Boolean(d.redownload) && !running,
    remove: !running,
    del: local && !running && state !== "gone",
  };
}

// The popup's word for why a button did nothing (mdm_ipc::api::recent_why, the host's
// appState, the remote control's), or null when it worked.
function actWhyText(why) {
  const known = {
    gone: "popup_act_gone",
    missing: "popup_act_missing",
    running: "popup_act_running",
    cant: "popup_act_cant",
    unsupported: "popup_act_unsupported",
    off: "popup_act_off",
    old: "popup_list_old",
    busy: "popup_act_busy",
    unreachable: "popup_list_remote_unreachable",
  };
  return tx(known[why] || "popup_act_failed");
}

// ---- a name cut in its middle -----------------------------------------------------------------

// `name` within `fits(text)` (whether that text fits): the start and the end kept, the
// extension whole, "…" between. The whole name when it fits.
function middleCut(name, fits) {
  const s = String(name);
  if (fits(s)) return s;
  const chars = Array.from(s);
  const dot = s.lastIndexOf(".");
  // The end kept: at least the extension and a few characters before it.
  const extLen = dot > 0 && s.length - dot <= 8 ? Array.from(s.slice(dot)).length : 0;
  let lo = 0;
  let hi = chars.length - 1;
  let best = `${chars[0] || ""}…`;
  while (lo <= hi) {
    const keep = (lo + hi) >> 1;
    const tail = Math.min(chars.length - 1, Math.max(extLen + Math.ceil(keep * 0.35), Math.ceil(keep * 0.4)));
    const head = Math.max(1, keep - tail);
    const text = `${chars.slice(0, head).join("").trimEnd()}…${chars.slice(chars.length - tail).join("").trimStart()}`;
    if (head + tail < chars.length && fits(text)) {
      best = text;
      lo = keep + 1;
    } else hi = keep - 1;
  }
  return best;
}

// Cut `el`'s text (`name`) to its width, measured in its own font; again whenever it is
// resized. The whole name stays in its title (a pointer's tip) and for screen readers.
const cutOnResize = typeof ResizeObserver === "function" ? new ResizeObserver((entries) => entries.forEach((e) => fitName(e.target))) : null;
const measureCanvas = typeof document === "object" ? document.createElement("canvas") : null;

function fitName(el) {
  const name = el.dataset.full || el.textContent;
  const width = el.clientWidth;
  if (!measureCanvas || width <= 0) {
    el.textContent = name;
    return;
  }
  const ctx = measureCanvas.getContext("2d");
  const style = getComputedStyle(el);
  ctx.font = `${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
  el.textContent = middleCut(name, (text) => ctx.measureText(text).width <= width - 1);
}

function setName(el, name) {
  if (el.dataset.full === name) return;
  el.dataset.full = name;
  el.title = name;
  el.textContent = name;
  if (cutOnResize) cutOnResize.observe(el);
  fitName(el);
}

// ---- moving with the arrow keys -------------------------------------------------------------

// One stop for Tab in `items()` (the one marked, else the first); the arrow keys of `axis`
// ("x": left and right, mirrored right to left; "y": up and down), Home and End move between
// them. `onMove(el)`: after each move (a tab or a filter chosen as it is reached).
function roving(container, items, { axis = "x", onMove } = {}) {
  const keys = (rtl) =>
    axis === "x" ? { next: rtl ? "ArrowLeft" : "ArrowRight", prev: rtl ? "ArrowRight" : "ArrowLeft" } : { next: "ArrowDown", prev: "ArrowUp" };
  const settle = (current) => {
    for (const el of items()) el.tabIndex = el === current ? 0 : -1;
  };
  container.addEventListener("keydown", (e) => {
    const list = items().filter((el) => !el.hidden && !el.disabled);
    const at = list.indexOf(document.activeElement);
    if (at < 0) return;
    const { next, prev } = keys(getComputedStyle(container).direction === "rtl");
    let to = -1;
    if (e.key === next) to = (at + 1) % list.length;
    else if (e.key === prev) to = (at - 1 + list.length) % list.length;
    else if (e.key === "Home") to = 0;
    else if (e.key === "End") to = list.length - 1;
    if (to < 0) return;
    e.preventDefault();
    settle(list[to]);
    list[to].focus();
    if (onMove) onMove(list[to]);
  });
  container.addEventListener("focusin", (e) => {
    if (items().includes(e.target)) settle(e.target);
  });
  return { settle };
}

// ---- a menu -----------------------------------------------------------------------------------

let openMenuState = null;

function closeMenu(focusBack = true) {
  const m = openMenuState;
  if (!m) return;
  openMenuState = null;
  m.menu.remove();
  m.button.setAttribute("aria-expanded", "false");
  document.removeEventListener("pointerdown", m.outside, true);
  if (focusBack && m.button.isConnected) m.button.focus();
}

// A menu of `items` ({label, icon, run, danger} or "-" between groups) under `button`, or above
// it when there's no room below; at its end in the reading direction. Arrow keys, Home, End and
// a letter move; Enter or Space chooses; Escape or Tab closes it.
function showMenu(button, items, label) {
  closeMenu(false);
  const menu = document.createElement("div");
  menu.className = "menu";
  menu.setAttribute("role", "menu");
  menu.setAttribute("aria-label", label);
  const entries = [];
  for (const item of items) {
    if (item === "-") {
      if (menu.lastChild && !menu.lastChild.classList.contains("sep")) {
        const sep = document.createElement("div");
        sep.className = "sep";
        sep.setAttribute("role", "separator");
        menu.append(sep);
      }
      continue;
    }
    const el = document.createElement("button");
    el.type = "button";
    el.className = `menu-item${item.danger ? " danger" : ""}`;
    el.setAttribute("role", "menuitem");
    el.tabIndex = -1;
    el.dataset.act = item.id || "";
    el.append(icon(item.icon || "file"));
    const span = document.createElement("span");
    span.textContent = item.label;
    el.append(span);
    el.addEventListener("click", () => {
      closeMenu(true);
      item.run();
    });
    entries.push(el);
    menu.append(el);
  }
  if (menu.lastChild && menu.lastChild.classList.contains("sep")) menu.lastChild.remove();
  document.body.append(menu);
  // Placed under the button, or above it, inside the popup.
  const r = button.getBoundingClientRect();
  const h = menu.offsetHeight;
  const w = menu.offsetWidth;
  const rtl = getComputedStyle(document.body).direction === "rtl";
  const below = r.bottom + 4 + h <= window.innerHeight - 4;
  menu.style.top = `${Math.max(4, below ? r.bottom + 4 : r.top - 4 - h)}px`;
  const left = rtl ? r.left : r.right - w;
  menu.style.left = `${Math.min(window.innerWidth - w - 4, Math.max(4, left))}px`;
  button.setAttribute("aria-expanded", "true");
  const outside = (e) => {
    if (!menu.contains(e.target) && e.target !== button && !button.contains(e.target)) closeMenu(false);
  };
  document.addEventListener("pointerdown", outside, true);
  openMenuState = { menu, button, outside };
  menu.addEventListener("keydown", (e) => {
    const at = entries.indexOf(document.activeElement);
    let to = -1;
    if (e.key === "ArrowDown") to = (at + 1) % entries.length;
    else if (e.key === "ArrowUp") to = (at - 1 + entries.length) % entries.length;
    else if (e.key === "Home") to = 0;
    else if (e.key === "End") to = entries.length - 1;
    else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      closeMenu(true);
      return;
    } else if (e.key === "Tab") {
      e.preventDefault();
      closeMenu(true);
      return;
    } else if (e.key.length === 1 && /\S/.test(e.key)) {
      const k = e.key.toLocaleLowerCase();
      const from = Math.max(0, at);
      for (let i = 1; i <= entries.length; i++) {
        const el = entries[(from + i) % entries.length];
        if (el.textContent.trim().toLocaleLowerCase().startsWith(k)) {
          to = (from + i) % entries.length;
          break;
        }
      }
    }
    if (to < 0) return;
    e.preventDefault();
    entries[to].focus();
  });
  if (entries[0]) entries[0].focus();
  return menu;
}

// ---- a question ---------------------------------------------------------------------------------

// Ask `text` in the popup's own dialog ({title, text, ok, danger}): true for the OK button,
// false for Cancel, Escape or a click outside. Focus starts on Cancel, the safe answer.
function confirmAsk({ title, text, ok, danger = false }) {
  return new Promise((resolve) => {
    const dialog = document.getElementById("ask");
    document.getElementById("ask-title").textContent = title;
    document.getElementById("ask-text").textContent = text;
    const yes = document.getElementById("ask-ok");
    const no = document.getElementById("ask-cancel");
    yes.textContent = ok;
    yes.classList.toggle("danger", danger);
    yes.classList.toggle("accent", !danger);
    const back = document.activeElement;
    const done = (answer) => {
      yes.onclick = no.onclick = null;
      dialog.onclose = null;
      if (dialog.open) dialog.close();
      if (back && back.isConnected) back.focus();
      resolve(answer);
    };
    yes.onclick = () => done(true);
    no.onclick = () => done(false);
    dialog.onclose = () => done(false);
    dialog.onclick = (e) => {
      if (e.target === dialog) done(false);
    };
    dialog.showModal();
    no.focus();
  });
}

// ---- a word for a moment ------------------------------------------------------------------------

let saidTimer = null;
// Say `text` at the popup's foot for a few seconds (and to screen readers, politely).
function sayBriefly(text, tone = "") {
  const el = document.getElementById("said");
  if (!el) return;
  el.textContent = text;
  el.className = `said${tone ? ` ${tone}` : ""}`;
  el.hidden = !text;
  clearTimeout(saidTimer);
  saidTimer = setTimeout(() => {
    el.hidden = true;
    el.textContent = "";
  }, 3500);
}

// Put `text` on the clipboard from the popup (a user's click: no permission needed); false when
// the browser refused.
async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    // An older browser, or no focus: the old way.
    const area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.append(area);
    area.select();
    let done = false;
    try {
      done = document.execCommand("copy");
    } catch {
      done = false;
    }
    area.remove();
    return done;
  }
}

// ---- a copy loaded by hand ----------------------------------------------------------------------

// The add-on's pages in the browsers' stores (the README's and the website's): the copy there is
// signed, stays after a restart and the browser keeps it up to date.
const STORE_FIREFOX = "https://addons.mozilla.org/firefox/addon/mister-download-manager/";
const STORE_CHROME = "https://chromewebstore.google.com/detail/mister-download-manager/ejoleppdfgjdndddcjoehopknmlglojp";

// What the popup says about how this copy was installed (#1613), from management.getSelf()'s
// `installType` (it needs no "management" permission, in Chromium or Firefox). Only a copy loaded
// by hand, "development" (Load unpacked in Chromium; Load Temporary Add-on in Firefox), is told
// it won't update itself, in Firefox that it goes when Firefox restarts too, with the link to its
// browser's store; null for the rest: the store's copy ("normal"), a policy's ("admin"), one
// another program installed ("sideload": MDM's setup has Chrome add the store's copy that way, and
// it updates from the store), and "other" (nothing to tell about it). `firefox`: Firefox's family
// (Zen, LibreWolf, Waterfox), the others take the Chrome Web Store's.
function installNote(self, firefox) {
  if (!self || self.installType !== "development") return null;
  return firefox
    ? { text: tx("popup_by_hand_firefox"), link: tx("popup_store_firefox"), href: STORE_FIREFOX }
    : { text: tx("popup_by_hand"), link: tx("popup_store_chrome"), href: STORE_CHROME };
}
