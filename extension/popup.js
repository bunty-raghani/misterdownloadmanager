// The toolbar button's popup: whether the browser reaches Mister Download Manager (and, when it
// doesn't, a link to the help page's section for why), then three tabs (#1496): Downloads (MDM's
// newest downloads, popup-downloads.js), This page (the media the tab loaded, #38, each to
// download; leaving its site to the browser, #404) and Settings (pausing capture, the browser's
// proxy, the video button in full screen, where downloads go, #157; MDM's Options). background.js
// does the work; popup-ui.js has the shared pieces.
const ext = globalThis.browser ?? globalThis.chrome;
const HELP = "https://misterdownloadmanager.com/help/browser-help.html";
// The creator's GitHub Sponsors page (the app's Help → Support MDM): a plain link, in a new tab.
const SPONSORS = "https://github.com/sponsors/anefzaoui";
const t = (key, ...subs) => ext.i18n.getMessage(key, subs) || key;
const $ = (id) => document.getElementById(id);
const ask = (m) => ext.runtime.sendMessage(m).catch(() => ({ ok: false }));
const lang = ext.i18n.getUILanguage();
const query = new URLSearchParams(location.search);

document.documentElement.lang = lang;
document.documentElement.dir = t("@@bidi_dir") === "rtl" ? "rtl" : "ltr";
for (const el of document.querySelectorAll("[data-i18n]")) el.textContent = t(el.dataset.i18n);
for (const el of document.querySelectorAll("[data-i18n-placeholder]")) el.placeholder = t(el.dataset.i18nPlaceholder);
for (const el of document.querySelectorAll("[data-i18n-label]")) {
  el.setAttribute("aria-label", t(el.dataset.i18nLabel));
  if (el.tagName === "BUTTON") el.title = t(el.dataset.i18nLabel);
}
for (const el of document.querySelectorAll("[data-icon]")) el.prepend(icon(el.dataset.icon));
$("help").href = HELP;
$("support").href = SPONSORS;

// ---- its size --------------------------------------------------------------------------------

// 400 pixels wide by default (popup.css), down to the 360 or so Firefox gives a popup in its
// overflow menu, which an add-on can't tell (it then fills that width), and up to the 800 a
// browser allows. `?w=` sets it for the pictures.
if (/^\d{3}$/.test(query.get("w") || "")) {
  document.documentElement.style.setProperty("--popup-w", `${Math.min(800, Math.max(320, Number(query.get("w"))))}px`);
}
const firefox = typeof globalThis.browser === "object" && Boolean(ext.runtime.getBrowserInfo);
function fitPanel() {
  // Only where the popup's width isn't ours to choose: Firefox's fixed-width panel.
  const narrow = firefox && window.innerWidth > 0 && window.innerWidth < document.body.offsetWidth - 2;
  document.documentElement.classList.toggle("fit", narrow);
}
if (firefox) {
  window.addEventListener("resize", fitPanel);
  requestAnimationFrame(fitPanel);
}

// ---- tabs ------------------------------------------------------------------------------------

const TABS = ["downloads", "page", "settings"];
function selectTab(name, focus = false) {
  for (const n of TABS) {
    const on = n === name;
    const tab = $(`tab-${n}`);
    tab.setAttribute("aria-selected", String(on));
    tab.tabIndex = on ? 0 : -1;
    $(`panel-${n}`).hidden = !on;
    if (on && focus) tab.focus();
  }
  document.body.dataset.tab = name;
}
const tabs = () => TABS.map((n) => $(`tab-${n}`));
roving(document.querySelector(".tabs"), tabs, { axis: "x", onMove: (el) => selectTab(el.id.slice(4)) });
for (const tab of tabs()) tab.addEventListener("click", () => selectTab(tab.id.slice(4)));
// The first tab: Downloads; This page when the popup is opened for a page in a tab of its own
// (`?page=`, the tests); `?tab=` for the pictures.
const firstTab = TABS.includes(query.get("tab")) ? query.get("tab") : query.has("page") ? "page" : "downloads";
selectTab(firstTab);
Downloads.start();

// The tab it is about: the window's active tab; with `?page=ADDRESS` (this page opened in a tab of
// its own, for the tests and the pictures), the tab showing that address.
async function currentTab() {
  const page = query.get("page");
  if (page) return (await ext.tabs.query({}).catch(() => [])).find((x) => x.url === page) || null;
  const [tab] = await ext.tabs.query({ active: true, currentWindow: true }).catch(() => []);
  return tab || null;
}

// ---- the header: connected, to which MDM -----------------------------------------------------

async function show() {
  const s = await ask({ type: "state" });
  const box = $("status");
  const text = $("status-text");
  box.classList.remove("ok", "bad", "warn");
  let problem = "";
  if (s.connected && s.why === "update-app") {
    box.classList.add("warn");
    text.textContent = t("popup_conn_update");
    problem = t("status_update_app");
  } else if (s.connected && s.remote) {
    box.classList.add(s.paused ? "warn" : "ok");
    let host = s.remote;
    try {
      host = new URL(s.remote).host;
    } catch {
      // As given.
    }
    text.textContent = s.paused ? t("popup_conn_paused") : t("popup_conn_remote", host);
    box.title = s.paused ? t("status_paused") : t("status_remote", s.remote);
  } else if (s.connected) {
    box.classList.add(s.paused ? "warn" : "ok");
    text.textContent = s.paused ? t("popup_conn_paused") : s.app ? t("popup_conn", s.app) : t("popup_conn_plain");
    box.title = s.paused ? t("status_paused") : s.app ? t("status_connected_version", s.app) : t("status_connected");
  } else {
    box.classList.add("bad");
    text.textContent = t("popup_conn_off");
    problem = t(`status_${(s.why || "not_installed").replace(/-/g, "_")}`);
  }
  $("problem").hidden = !problem;
  $("problem-text").textContent = problem;
  const fix = $("fix");
  fix.hidden = s.connected && s.why !== "update-app";
  fix.href = `${HELP}#${s.why || "not-installed"}`;
  $("take").checked = !s.paused;
  showDeclined(s.connected && s.declined);
  $("take").disabled = !s.connected;
  $("open").disabled = $("options").disabled = $("gear").disabled = !s.connected || Boolean(s.remote);
  $("version").textContent = s.version ? `v${s.version}` : "";
  // Send downloads to: this computer, or another MDM (#157).
  const remotes = Array.isArray(s.remotes) ? s.remotes : [];
  renderRemotes(remotes, s.remote);
  $("to-here").checked = !s.remote && $("remote-form").hidden;
  $("to-remote").checked = (remotes.length === 0 ? Boolean(s.remote) : false) || !$("remote-form").hidden;
  const tab = await currentTab();
  let site = "";
  try {
    const u = new URL(tab && tab.url);
    if (/^https?:$/.test(u.protocol)) site = u.hostname;
  } catch {
    // Not a web page (the browser's own): nothing to leave.
  }
  $("page-none").hidden = Boolean(site);
  await showExcluded(site, s.connected);
  if (site) await showMedia(tab, s.connected);
}

// ---- This page --------------------------------------------------------------------------------

// "Never take downloads from <site>" (#404), a switch: on, the site's downloads stay with the
// browser (Options → File types → excluded sites), and it says so below; off again takes the
// site back off the list. A site excluded by a pattern of Options' own (`*.example.com`) is
// shown on, and can't be switched off here: Options holds it.
async function showExcluded(site, connected) {
  const box = $("site-box");
  const sw = $("exclude");
  box.hidden = !site || !connected;
  if (box.hidden) return;
  const listed = await ask({ type: "site-excluded", host: site });
  const label = t("popup_exclude_site", site);
  $("exclude-label").textContent = label;
  $("exclude-row").title = label;
  $("excluded-text").textContent = t("popup_excluded", site);
  const said = (excluded, own) => {
    sw.checked = excluded;
    sw.disabled = excluded && !own;
    $("excluded").hidden = !excluded;
    $("exclude-note").hidden = !(excluded && !own);
    $("exclude-note").textContent = excluded && !own ? t("popup_exclude_by_rule") : "";
  };
  said(Boolean(listed.excluded), Boolean(listed.undo));
  sw.onchange = async () => {
    const on = sw.checked;
    sw.disabled = true;
    const r = await ask({ type: on ? "exclude-site" : "include-site", host: site });
    sw.disabled = false;
    said(r.ok ? on : !on, true);
  };
}

// The last download left to the browser by MDM's Options, and which one (#208); a later one
// taken clears it (background.js noteLeft). A reason this version can't name says no more.
function showDeclined(d) {
  const box = $("declined");
  box.hidden = !(d && d.why);
  if (box.hidden) return;
  const what = d.name ? t("popup_declined", d.name) : t("popup_declined_unnamed");
  const key = `declined_${String(d.why).replace(/-/g, "_")}`;
  const why = t(key);
  $("declined-what").textContent = what;
  $("declined-what").title = what;
  $("declined-why").textContent = why === key ? "" : why;
}

// ---- the media the tab loaded ------------------------------------------------------------------

const TYPES = {
  "video/mp4": "MP4",
  "audio/mp4": "M4A",
  "audio/mpeg": "MP3",
  "video/webm": "WEBM",
  "audio/webm": "WEBM",
  "audio/ogg": "OGG",
  "video/ogg": "OGV",
  "video/quicktime": "MOV",
  "video/x-matroska": "MKV",
  "audio/flac": "FLAC",
  "audio/wav": "WAV",
};

// Its type as a short tag: HLS, DASH, else its file's extension as it will be saved (a script's
// answer: its type's, #800), else its type's.
function tagOf(m) {
  if (m.kind === "hls") return "HLS";
  if (m.kind === "dash") return "DASH";
  const ext = typeof m.name === "string" ? m.ext : (/\.([a-z0-9]{2,4})$/i.exec(m.url.split(/[?#]/)[0]) || [])[1];
  return (ext || TYPES[m.mime] || (m.kind === "audio" ? "AUDIO" : "VIDEO")).toUpperCase();
}

// Its name: a file's as it will be saved (#800), else the address's last part, readable.
function nameOf(m) {
  if (typeof m.name === "string" && m.name) return m.name;
  const url = m.url;
  const last = url.split(/[?#]/)[0].split("/").filter(Boolean).pop() || url;
  try {
    return decodeURIComponent(last);
  } catch {
    return last;
  }
}

// What is known of it: its qualities (or picture size), length, size; "Live" for a live stream.
function metaOf(m) {
  const parts = [];
  if (m.width && m.heights.length === 1) parts.push(`${m.width}×${m.heights[0]}`);
  else if (m.heights.length) parts.push(m.heights.map((h) => `${h}p`).join(" · "));
  if (m.live) parts.push(t("popup_media_live"));
  else if (m.duration > 0) parts.push(clockText(m.duration));
  if (m.size > 0) parts.push(sizeText(m.size, lang));
  return parts.join(" · ");
}

function said(text, bad) {
  const p = $("media-said");
  p.hidden = !text;
  p.textContent = text;
  p.classList.toggle("bad", Boolean(bad));
}

async function showMedia(tab, connected) {
  const r = await ask({ type: "media", tab: tab.id });
  const list = (r && r.media) || [];
  const kept = Boolean(r && r.keptOff);
  $("media").hidden = false;
  // Its count on the tab, so the list is found from Downloads.
  const pill = $("tab-media");
  pill.hidden = !list.length;
  pill.textContent = list.length ? String(list.length) : "";
  // Why there's nothing to list, honestly: this copy leaves the site alone (Help says what's left
  // to do), the player streams pieces no address names, or the page hasn't loaded any yet.
  const none = $("media-none");
  none.hidden = list.length > 0 && !kept;
  none.textContent = t(kept ? "popup_media_store" : r && r.streamed ? "popup_media_streamed" : "popup_media_none");
  $("help").href = kept ? `${HELP}#store-youtube` : HELP;
  const ul = $("media-list");
  ul.replaceChildren();
  for (const m of list) {
    const li = document.createElement("li");
    li.dataset.kind = m.kind;
    const tag = document.createElement("span");
    tag.className = "tag";
    tag.textContent = tagOf(m);
    const what = document.createElement("div");
    what.className = "what";
    const name = document.createElement("span");
    name.className = "name";
    name.textContent = nameOf(m);
    name.title = m.url;
    const meta = document.createElement("span");
    meta.className = "meta";
    meta.textContent = metaOf(m);
    what.append(name, meta);
    li.append(tag, what);
    if (m.drm) {
      // Copy-protected: listed, never sent.
      const drm = document.createElement("span");
      drm.className = "drm";
      drm.textContent = t("popup_media_protected");
      drm.title = t("video_drm");
      li.append(drm);
    } else {
      const get = document.createElement("button");
      get.type = "button";
      get.className = "btn small";
      get.textContent = t("popup_media_download");
      get.title = t("menu");
      get.disabled = !connected;
      get.onclick = async () => {
        get.disabled = true;
        const reply = await ask({ type: "media-download", tab: tab.id, url: m.url });
        said(reply.ok ? t("video_sent") : reply.why === "drm" ? `${t("video_drm")} · ${t("video_cant_save")}` : t("video_failed"), !reply.ok);
        get.disabled = false;
      };
      li.append(get);
    }
    ul.append(li);
  }
}

// This applies to files chosen in the media list, not ordinary browser downloads or streams.
const mediaTitleName = $("media-title-name");
ext.storage.local.get({ mediaTitleName: false })
  .then((v) => (mediaTitleName.checked = Boolean(v.mediaTitleName)))
  .catch(() => {});
// The list's names follow it at once.
mediaTitleName.addEventListener("change", () =>
  ext.storage.local.set({ mediaTitleName: mediaTitleName.checked }).then(show, () => {}));

// ---- Settings ---------------------------------------------------------------------------------

// Hide the download button over videos while the page is in full screen (#670), off as installed.
const hideFullscreen = $("video-hide-fullscreen");
$("video-hide-fullscreen-row").title = t("popup_video_hide_fullscreen_title");
ext.storage.local.get({ hideVideoFullscreen: false })
  .then((v) => (hideFullscreen.checked = Boolean(v.hideVideoFullscreen)))
  .catch(() => {});
hideFullscreen.addEventListener("change", () =>
  ext.storage.local.set({ hideVideoFullscreen: hideFullscreen.checked }).catch(() => {}));

// #42: reading the browser's proxy (a VPN or proxy extension's) is an opt-in, this switch, off as
// installed. Firefox: the optional "proxy" permission, asked for on it. Chromium can't make that
// permission optional, so the extension has it from the install, and the switch is a setting of
// its own (`proxyOn`, which background.js reads before each download).
const PROXY = { permissions: ["proxy"] };
const manifest = ext.runtime.getManifest();
if (!(manifest.optional_permissions || []).includes("proxy") && (manifest.permissions || []).includes("proxy")) {
  const row = $("proxy-row");
  const box = $("proxy");
  row.hidden = false;
  row.title = t("popup_proxy_title_chromium");
  ext.storage.local
    .get({ proxyOn: false })
    .then((v) => (box.checked = Boolean(v.proxyOn)))
    .catch(() => {});
  box.addEventListener("change", () => ext.storage.local.set({ proxyOn: box.checked }).catch(() => {}));
}
if ((manifest.optional_permissions || []).includes("proxy")) {
  const row = $("proxy-row");
  const box = $("proxy");
  row.hidden = false;
  row.title = t("popup_proxy_title");
  const sync = () =>
    ext.permissions
      .contains(PROXY)
      .catch(() => false)
      .then((on) => (box.checked = Boolean(on)));
  sync();
  box.addEventListener("change", () => {
    // Asked for in the click itself (the browser allows it from a user's action only): nothing
    // awaited before it. Refused, or taken back: the switch shows what the browser says.
    const asked = box.checked ? ext.permissions.request(PROXY) : ext.permissions.remove(PROXY);
    asked.catch(() => false).then(sync);
  });
}

$("take").addEventListener("change", async (e) => {
  await ask({ type: "pause", paused: !e.target.checked });
  show();
});
$("open").addEventListener("click", async () => {
  await ask({ type: "open-app" });
  window.close();
});
// Options in MDM: the Settings tab's button, and the header's gear.
for (const id of ["options", "gear"]) {
  $(id).addEventListener("click", async () => {
    await ask({ type: "open-options" });
    window.close();
  });
}

function renderRemotes(remotes, activeUrl) {
  const list = $("remotes-list");
  if (!list) return;
  list.textContent = "";
  for (const url of remotes) {
    const row = document.createElement("div");
    row.className = "remote-item";

    const label = document.createElement("label");
    label.className = "choice";

    const radio = document.createElement("input");
    radio.type = "radio";
    radio.name = "to";
    radio.checked = url === activeUrl && $("remote-form").hidden;
    radio.addEventListener("change", async () => {
      $("remote-form").hidden = true;
      await ask({ type: "remote-select", url });
      show();
      Downloads.ask();
    });

    const span = document.createElement("span");
    try {
      span.textContent = new URL(url).host;
    } catch {
      span.textContent = url;
    }
    span.title = url;
    span.dir = "ltr";

    label.append(radio, span);

    const forget = document.createElement("button");
    forget.type = "button";
    forget.className = "icon-btn small remote-forget";
    forget.append(icon("close", 14));
    const forgetTitle = t("popup_remote_forget");
    forget.title = forgetTitle;
    forget.setAttribute("aria-label", `${forgetTitle}: ${span.textContent}`);
    forget.addEventListener("click", async (e) => {
      e.stopPropagation();
      await ask({ type: "remote-forget", url });
      show();
      Downloads.ask();
    });

    row.append(label, forget);
    list.append(row);
  }
}

// Send downloads to (#157): another MDM is paired here with its address and a code it shows.
const openRemoteForm = () => {
  $("remote-form").hidden = false;
  $("remote-url").focus();
};
$("to-remote").addEventListener("change", openRemoteForm);
$("to-remote").addEventListener("click", openRemoteForm);
$("to-here").addEventListener("change", async () => {
  $("remote-form").hidden = true;
  await ask({ type: "remote-off" });
  show();
  Downloads.ask();
});
$("remote-open").addEventListener("click", () => {
  try {
    const u = new URL($("remote-url").value.trim());
    if (u.protocol === "https:") ext.tabs.create({ url: u.origin + "/" });
  } catch {
    $("remote-said").textContent = t("remote_address");
    $("remote-said").hidden = false;
  }
});
$("remote-pair").addEventListener("click", async () => {
  const said = $("remote-said");
  said.hidden = true;
  const r = await ask({ type: "remote-pair", url: $("remote-url").value, code: $("remote-code").value });
  if (r.ok) {
    $("remote-form").hidden = true;
    $("remote-code").value = "";
    show();
    Downloads.ask();
    return;
  }
  said.textContent = t(`remote_${String(r.why || "failed").replace(/-/g, "_")}`);
  said.hidden = false;
});

show();
