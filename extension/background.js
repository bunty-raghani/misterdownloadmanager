// Mister Download Manager's browser extension: the background part (Chrome, Edge, Brave, Opera
// and Vivaldi run it as a service worker, Firefox as a background script; one file for both, see
// manifest.json and manifest.firefox.json).
//
// Downloads of the file types Mister Download Manager takes are handed to it: the browser's own
// download is cancelled, and the address, referrer, cookies, User-Agent, file name, size, type
// and (for a form) the posted body go to the app through the native messaging host
// (crates/nmhost). When it can't be reached the download stays with the browser, so nothing is
// lost, and the toolbar button says why, linking the help page's section for it
// (docs/browser-help.md). The browser may stop this script whenever it is idle: what must last
// (the rules, the status, the pause) is kept in storage, not in memory.

const ext = globalThis.browser ?? globalThis.chrome;
const HOST = "com.misterdownloadmanager.host";
const VERSION = ext.runtime.getManifest().version;
const HELP = "https://misterdownloadmanager.com/help/browser-help.html";
const action = ext.action ?? ext.browserAction;
// Firefox has no pause before the file name (onDeterminingFilename): its own paths below.
const FIREFOX = !ext.downloads.onDeterminingFilename;
// Brave and Opera say who they are only in the client hints, Firefox's forks (Zen, LibreWolf,
// Floorp, Waterfox) only in runtime.getBrowserInfo, which only Firefox's family has. Both go to
// the host as `brands` (Options lists them by name; crates/ipc capture::browser_named).
const BRANDS = (globalThis.navigator?.userAgentData?.brands || []).map((b) => b.brand);
const APP_NAME = ext.runtime.getBrowserInfo
  ? ext.runtime.getBrowserInfo().then((i) => (i && i.name) || null, () => null)
  : Promise.resolve(null);

const t = (key, ...subs) => ext.i18n.getMessage(key, subs) || key;

// ---- the native host ---------------------------------------------------------------------

// How long an answer may take: the host starts Mister Download Manager when it isn't running
// and waits for it (crates/nmhost START_WAIT); the browser's download waits meanwhile.
const HOST_WAIT = 30000;

function withTimeout(promise, ms) {
  let timer;
  return Promise.race([
    promise,
    new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error("no answer")), ms);
    }),
  ]).finally(() => clearTimeout(timer));
}

// Why the browser couldn't reach the host, as the help page's sections name it.
function hostProblem(error) {
  const m = String((error && error.message) || error);
  if (/not found|no such native application/i.test(m)) return "not-installed";
  if (/forbidden/i.test(m)) return "forbidden";
  if (/no answer/i.test(m)) return "not-running";
  return "host-failed";
}

// Every message says which browser and extension version it comes from (the app keeps each
// browser's status, and the rules depend on the browser). Never throws: failures come back as
// `{ok: false, why}`.
async function native(message, ms = HOST_WAIT) {
  // A download for another MDM (a server, #157): sent there, not to this computer's.
  if (typeof message.url === "string" && message.url) {
    const target = await remoteTarget();
    if (target) return remoteSend(target, message);
  }
  const app = await APP_NAME;
  const brands = app ? [...BRANDS, app] : BRANDS;
  const sent = { ...message, userAgent: navigator.userAgent, brands, v: VERSION };
  try {
    const reply = (await withTimeout(ext.runtime.sendNativeMessage(HOST, sent), ms)) || {};
    if (reply.version) await noteStatus(older(reply.version, VERSION) ? "update-app" : null, reply.version);
    else if (reply.ok || reply.why === "options") await noteStatus(null);
    else if (reply.why && reply.why !== "refused") await noteStatus(reply.why);
    return reply;
  } catch (e) {
    const why = hostProblem(e);
    // With another MDM chosen, this computer's needn't be there.
    if (!(await remoteTarget())) await noteStatus(why);
    return { ok: false, why, error: String((e && e.message) || e) };
  }
}

// ---- another MDM (#157): a server's, through its remote control ------------------------------

// {url, token} while downloads go to another MDM instead of this computer's, else null.
async function remoteTarget() {
  const { remote } = await ext.storage.local.get({ remote: null });
  return remote && remote.url && remote.token ? remote : null;
}

// One request to the remote control's API at `base` (https://server:8463/). Never throws:
// {status, body}, status 0 when it isn't reached (not running, or its certificate not accepted
// in this browser yet). The browser's cookies never go with it.
async function remoteCall(base, path, body, token) {
  try {
    const r = await withTimeout(
      fetch(new URL(path, base), {
        method: "POST",
        headers: { "Content-Type": "application/json", ...(token ? { Authorization: `Bearer ${token}` } : {}) },
        body: JSON.stringify(body),
        credentials: "omit",
        cache: "no-store",
        redirect: "error",
      }),
      15000,
    );
    let json = null;
    try {
      json = await r.json();
    } catch {
      json = null;
    }
    return { status: r.status, body: json };
  } catch {
    return { status: 0, body: null };
  }
}

// A download handed to the other MDM: its address only (a site's sign-in stays in this browser).
async function remoteSend(target, message) {
  const r = await remoteCall(target.url, "api/add", { url: message.url, start: true }, target.token);
  if (r.status === 200 && r.body && !r.body.error) return { ok: true };
  if (r.status === 401) await ext.storage.local.set({ remote: null }); // unpaired there
  return { ok: false, why: "refused" };
}

// The file types the app takes by default (mdm_ipc::capture::DEFAULT_AUTO_TYPES; a test keeps
// them the same): what is caught for another MDM when this computer has none to ask.
const DEFAULT_TYPES =
  "3GP 7Z AAC ACE AIF APK APPIMAGE APPX APPXBUNDLE ARJ ASF AVI BIN BZ2 CAB DEB DMG EXE FLAC GZ GZIP IMG ISO JAR LZH M4A M4V MKV MOV MP3 MP4 MPA MPE MPEG MPG MSI MSIX MSU OGG OGV OPUS PDF PKG PLJ PPS PPT QT R0* R1* RA RAR RM RMVB RPM SEA SIT SITX TAR TGZ TIF TIFF TORRENT TXZ WAV WEBM WMA WMV XZ Z ZIP ZST";

// Pair with the MDM at `address` with a code it shows; {ok} or {ok: false, why, left}.
async function remotePair(address, code) {
  let base;
  try {
    base = new URL(String(address).trim());
  } catch {
    return { ok: false, why: "address" };
  }
  if (base.protocol !== "https:") return { ok: false, why: "address" };
  base = `${base.origin}/`;
  const name = ext.i18n.getMessage("remote_device_name") || "Browser";
  const r = await remoteCall(base, "api/pair", { code: String(code), name }, null);
  if (r.status === 200 && r.body && typeof r.body.token === "string") {
    await ext.storage.local.set({ remote: { url: base, token: r.body.token } });
    await refreshRules();
    await showStatus();
    return { ok: true };
  }
  const why = { 0: "unreachable", 403: "wrong-code", 410: "no-code", 429: "too-soon" }[r.status] || "failed";
  return { ok: false, why, left: r.body && r.body.left };
}

// Back to this computer: unpaired there too (when it answers).
async function remoteOff() {
  const target = await remoteTarget();
  if (target) await remoteCall(target.url, "api/forget", {}, target.token);
  await ext.storage.local.set({ remote: null });
  await refreshRules();
  await showStatus();
  return { ok: true };
}

// a < b, both "x.y.z".
function older(a, b) {
  const pa = String(a).split(/[.-]/).map(Number);
  const pb = String(b).split(/[.-]/).map(Number);
  for (let i = 0; i < 3; i++) {
    if ((pa[i] || 0) !== (pb[i] || 0)) return (pa[i] || 0) < (pb[i] || 0);
  }
  return false;
}

// ---- status: the toolbar button's badge and title, the popup's first line ------------------

let status = null; // {why, app, at}
async function readStatus() {
  if (!status) status = (await ext.storage.local.get({ status: { why: null } })).status;
  return status;
}

// `why`: null when connected, else the reason (the help page's section). Written only when it
// changes.
async function noteStatus(why, app) {
  const now = await readStatus();
  if (now.why === why && (!app || now.app === app)) return;
  const was = !now.why || now.why === "update-app";
  status = { why, app: app || now.app || null, at: Date.now() };
  await ext.storage.local.set({ status });
  await showStatus();
  // Unreachable, PDFs and music open in the browser again (and back once connected).
  if (was !== (!why || why === "update-app")) await syncOpenedRule();
}

async function showStatus() {
  const why = (await remoteTarget()) ? null : (await readStatus()).why;
  const { paused } = await ext.storage.local.get({ paused: false });
  try {
    await action.setBadgeBackgroundColor({ color: why ? "#C42B1C" : "#6B6B6B" });
    await action.setBadgeText({ text: why && why !== "update-app" ? "!" : paused ? "II" : "" });
    await action.setTitle({ title: why ? t(`status_${why.replace(/-/g, "_")}`) : paused ? t("status_paused") : t("status_connected") });
  } catch {
    // No toolbar button (a browser without one): the popup and the help page still say it.
  }
}

// ---- the rules: Options in the app, kept here between the browser's stops ------------------

// {types, capture, captureOff, excludedSites, excludedAddresses, opened, minKb, at}; null until
// first asked. `captureOff`: why `capture` is false (a host before #208 doesn't say).
let rules = null;

async function storedRules() {
  if (!rules) rules = (await ext.storage.local.get({ rules: null })).rules;
  return rules;
}

// One question at a time: callers meanwhile get its answer.
let asking = null;
function refreshRules() {
  asking = asking || askRules().finally(() => (asking = null));
  return asking;
}

async function askRules() {
  const reply = await native({ rules: true });
  if (reply.ok && typeof reply.types === "string") {
    rules = {
      types: reply.types,
      capture: reply.capture !== false,
      captureOff: typeof reply.captureOff === "string" ? reply.captureOff : "",
      excludedSites: reply.excludedSites || "",
      excludedAddresses: Array.isArray(reply.excludedAddresses) ? reply.excludedAddresses : [],
      opened: reply.opened !== false,
      minKb: Number(reply.minKb) || 0,
      at: Date.now(),
    };
    await ext.storage.local.set({ rules });
    await syncOpenedRule();
    return rules;
  }
  // No MDM on this computer, another one chosen: the app's default file types (the browser
  // keeps the files it would open).
  if (await remoteTarget()) {
    rules = { types: DEFAULT_TYPES, capture: true, excludedSites: "", excludedAddresses: [], opened: false, minKb: 0, at: Date.now() };
    return rules;
  }
  return null;
}

// The rules to decide by now: the stored ones at once (refreshed meanwhile once a minute old;
// the host checks every offer again), else asked. None while the host can't be reached: then
// the browser keeps everything.
async function currentRules() {
  const r = await storedRules();
  const { why } = await readStatus();
  if (!r || (why && why !== "update-app")) return refreshRules();
  if (Date.now() - r.at > 60000) refreshRules();
  return r;
}

function typePatterns(list) {
  return list
    .split(/\s+/)
    .filter(Boolean)
    .map((t) => new RegExp("^" + t.replace(/[.+?^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*") + "$", "i"));
}

function extensionOf(name) {
  const base = name.split(/[\\/]/).pop() || "";
  const dot = base.lastIndexOf(".");
  return dot > 0 ? base.slice(dot + 1) : "";
}

// As the app matches them (mdm_ipc::capture): `*.example.com` takes the domain and its
// subdomains; an address pattern's `*` any run of characters.
function hostMatches(host, pattern) {
  let p = pattern.trim().toLowerCase().replace(/^[a-z]+:\/\//, "").split("/")[0];
  if (p.startsWith("*.")) {
    p = p.slice(2);
    return host === p || host.endsWith("." + p);
  }
  return p !== "" && host === p;
}

function wildcard(pattern, text) {
  const re = new RegExp("^" + pattern.toLowerCase().split("*").map((s) => s.replace(/[.+?^${}()|[\]\\]/g, "\\$&")).join(".*") + "$");
  return re.test(text.toLowerCase());
}

// An address pattern with a scheme is the whole address; without one, the rest after it.
function addressMatches(pattern, url) {
  const p = pattern.trim();
  if (!p) return false;
  return wildcard(p, p.includes("://") ? url : url.replace(/^[a-z][a-z0-9+.-]*:\/\//i, ""));
}

// Which of Options' lists leaves `url` to the browser: "site", "address", or "" (neither).
function excludedBy(r, url) {
  let host = "";
  try {
    host = new URL(url).hostname.toLowerCase();
  } catch {
    return "site";
  }
  if (r.excludedSites.split(/\s+/).some((p) => p && hostMatches(host, p))) return "site";
  if (r.excludedAddresses.some((p) => addressMatches(p, url))) return "address";
  return "";
}

function excluded(r, url) {
  return excludedBy(r, url) !== "";
}

// ---- downloads left to the browser: why, for the popup and MDM's log (#208) ----------------

// A host's refusal by Options names the rule (`declined`); one before #208 says it only in its
// error text, by the rule's Rust name (mdm_ipc::capture::Declined).
const DECLINED_NAMES = {
  IntegrationOff: "integration-off",
  Browser: "browser",
  FileType: "file-type",
  Site: "site",
  Address: "address",
  Small: "small",
};
function declinedOf(reply) {
  if (typeof reply.declined === "string" && reply.declined) return reply.declined;
  const m = /left to the browser by Options \((\w+)\)/.exec(String(reply.error || ""));
  return (m && DECLINED_NAMES[m[1]]) || "";
}

function siteOf(url) {
  try {
    return new URL(url).hostname;
  } catch {
    return "";
  }
}

// The last download left to the browser by Options (or because the extension may not read the
// site): kept for the popup ({why, name, site, at}; a private window's without its name or
// site), and, when the extension decided alone (`told` false), told to the host for MDM's log.
// The host logs its own refusals itself. A download taken later clears it.
async function noteLeft(why, name, url, incognito, told) {
  const declined = incognito ? { why, name: "", site: "", at: Date.now() } : { why, name, site: siteOf(url), at: Date.now() };
  await ext.storage.local.set({ declined });
  if (told) return;
  // Under a key of its own, with no address: an older host refuses it as one it can't
  // download, and hands nothing over.
  await native({ declined: { why, name: declined.name, site: declined.site }, ...(incognito ? { private: true } : {}) });
}

// ---- what the page told us: forms posted, links clicked with Alt -------------------------

// Forms posted in a page or frame, by address: a download they lead to is posted again by the
// app, with the same body (it can't be sent as a GET). Kept two minutes.
const posts = new Map();
const BODY_LIMIT = 512 * 1024;

function forget(map, ms) {
  const now = Date.now();
  for (const [k, v] of map) if (now - v.at > ms) map.delete(k);
}

// A form's body as the browser sent it: its fields (formData: put together again in the
// form's own encoding once its Content-Type is known, `postOf`), or the raw bytes when they
// are text. Anything else (a file upload) can't be sent again: `body: null` leaves its download
// to the browser.
function formBody(details) {
  const rb = details.requestBody;
  if (!rb) return { body: null };
  if (rb.formData) {
    const fields = Object.entries(rb.formData).flatMap(([k, values]) => values.map((v) => [k, String(v)]));
    const size = fields.reduce((n, [k, v]) => n + k.length + v.length + 2, 0);
    return size > BODY_LIMIT ? { body: null } : { fields, type: "application/x-www-form-urlencoded" };
  }
  if (Array.isArray(rb.raw) && rb.raw.every((p) => p.bytes)) {
    const size = rb.raw.reduce((n, p) => n + p.bytes.byteLength, 0);
    if (size > BODY_LIMIT) return { body: null };
    const bytes = new Uint8Array(size);
    let at = 0;
    for (const p of rb.raw) {
      bytes.set(new Uint8Array(p.bytes), at);
      at += p.bytes.byteLength;
    }
    try {
      return { body: new TextDecoder("utf-8", { fatal: true }).decode(bytes), type: null };
    } catch {
      return { body: null };
    }
  }
  return { body: null };
}

if (ext.webRequest) {
  const navigations = { urls: ["<all_urls>"], types: ["main_frame", "sub_frame"] };
  ext.webRequest.onBeforeRequest.addListener(
    (d) => {
      if (d.method !== "POST") return;
      forget(posts, 120000);
      posts.set(d.url, { ...formBody(d), at: Date.now(), id: d.requestId });
    },
    navigations,
    ["requestBody"],
  );
  // The Content-Type the form went with (a multipart boundary must stay the same).
  ext.webRequest.onBeforeSendHeaders.addListener(
    (d) => {
      if (d.method !== "POST") return;
      const p = posts.get(d.url);
      const ct = (d.requestHeaders || []).find((h) => h.name.toLowerCase() === "content-type");
      if (p && p.id === d.requestId && ct && ct.value) p.type = ct.value;
    },
    navigations,
    ["requestHeaders"],
  );
}

// The form posted to `url` (within two minutes), as the app sends it again: urlencoded fields,
// or a raw text body with its own type. null: it can't be sent again (a file in it; a multipart
// form, whose fields the browser gives without its files): the browser keeps the download.
function postOf(url) {
  const p = posts.get(url);
  if (!p || Date.now() - p.at > 120000) return undefined;
  if (p.body === null || (p.fields && /multipart/i.test(p.type || ""))) return null;
  if (!p.fields) return { body: p.body, type: p.type };
  const params = new URLSearchParams();
  for (const [k, v] of p.fields) params.append(k, v);
  return { body: params.toString(), type: "application/x-www-form-urlencoded" };
}

// Where page loads' redirects started, by where they ended (two minutes): Firefox names a
// download by its last address only, and the app keeps the first (an expired signed link is
// renewed from it).
const redirected = new Map();
if (ext.webRequest && ext.webRequest.onBeforeRedirect) {
  ext.webRequest.onBeforeRedirect.addListener(
    (d) => {
      forget(redirected, 120000);
      const first = (redirected.get(d.url) || {}).from || d.url;
      redirected.set(d.redirectUrl, { from: first, at: Date.now() });
    },
    { urls: ["<all_urls>"], types: ["main_frame", "sub_frame"] },
  );
}

// The address a download was first asked for: the browser's own (Chromium), else the start of
// the redirects that led to it.
function firstUrl(item) {
  if (item.finalUrl && item.finalUrl !== item.url) return item.url;
  const r = redirected.get(item.url);
  return r && Date.now() - r.at < 120000 ? r.from : item.url;
}

// Links clicked with Alt held (page.js): the browser keeps their download.
const bypass = new Map();

// ---- the media a tab loaded (#38) ------------------------------------------------------------

// What the tab's pages loaded that can be downloaded, per tab, in the order seen: streams'
// manifests (HLS .m3u8, DASH .mpd; a master playlist comes before its variants) and plain video
// and sound files (a <video>'s or <audio>'s own file, or a large whole file a player fetched). The
// popup lists them (`mediaList`); the download button over a video sends the stream its frame
// loaded first (`streamOf`): a player that plays from a blob: has no address of its own, and
// yt-dlp reads the manifest (qualities, separate audio). Segments and byte ranges are pieces of a
// stream, never listed. Bounded: MEDIA_MAX per tab, MEDIA_TABS tabs; forgotten when the tab goes
// to another page or closes.
const media = new Map(); // tab id -> {page, items: [{url, kind, frameId, mime, size, info}], frames}
const MEDIA_MAX = 60;
const MEDIA_TABS = 50;
const URL_MAX = 8192;
// A player's own fetch of a whole file is listed from this size (smaller ones are pieces).
const FETCHED_MIN = 1 << 20;
const MANIFEST = /\.(m3u8|mpd)(?:[?#]|$)/i;
const SEGMENT_TYPE = /^(video\/mp2t|video\/iso\.segment|audio\/iso\.segment)$/;
const SEGMENT_EXT = /\.(ts|m4s|m4f|cmfv|cmfa|cmft)$/;
const MEDIA_EXT = /\.(mp4|m4v|webm|mkv|mov|avi|wmv|flv|3gp|ogv|mp3|m4a|aac|ogg|oga|opus|flac|wav|wma)$/;
const AUDIO_EXT = /^(mp3|m4a|aac|oga|ogg|opus|flac|wav|wma)$/;
// The types a manifest is sometimes served as, besides its own (#171): plain text, no type, a
// file's, XML. Such an answer is only noted (`maybe`, MAYBE_MAX a tab), never listed unread.
const MAYBE_TYPE = /^(text\/plain|(application|binary)\/octet-stream|(application|text)\/xml|)$/;
const MAYBE_MAX = 20;
const MANIFEST_MAX = 2 << 20;

const pathOf = (url) => url.split(/[?#]/)[0];
const headerOf = (d, name) => ((d.responseHeaders || []).find((h) => h.name.toLowerCase() === name) || {}).value || "";

// What a response the tab got is, for the list: {kind: "hls" | "dash" | "video" | "audio", mime,
// size}, or null (not media, a segment, a piece of a file, too small to be a file of its own).
function mediaKind(d) {
  if (d.tabId < 0 || !/^https?:/i.test(d.url) || d.url.length > URL_MAX) return null;
  if (d.statusCode && (d.statusCode < 200 || d.statusCode > 299)) return null;
  const mime = headerOf(d, "content-type").split(";")[0].trim().toLowerCase();
  const path = pathOf(d.url).toLowerCase();
  if (/\.m3u8$/.test(path) || /mpegurl/.test(mime)) return { kind: "hls", mime, size: 0 };
  if (/\.mpd$/.test(path) || /dash\+xml/.test(mime)) return { kind: "dash", mime, size: 0 };
  if (SEGMENT_TYPE.test(mime) || SEGMENT_EXT.test(path)) return null;
  const ext = (MEDIA_EXT.exec(path) || [])[1] || "";
  // Maybe a manifest whose address and type don't say so (#171): a player's own small text or
  // untyped answer, read only when a stream is asked for and none is known (`sniffTab`).
  if (d.type !== "media" && !ext && MAYBE_TYPE.test(mime) && d.statusCode !== 206 && !headerOf(d, "content-range")) {
    const len = Number(headerOf(d, "content-length"));
    return len > MANIFEST_MAX ? null : { kind: "maybe", mime, size: 0 };
  }
  const typed = /^(video|audio)\//.test(mime) || mime === "application/ogg";
  const untyped = !mime || /^(application|binary)\/octet-stream$/.test(mime);
  if (!typed && !(ext && untyped)) return null;
  // The whole file's size: a range's total, else the length of a whole answer.
  const range = headerOf(d, "content-range");
  const total = /\/\s*(\d+)\s*$/.exec(range);
  const size = total ? Number(total[1]) : d.statusCode === 206 ? 0 : Number(headerOf(d, "content-length")) || 0;
  // A <video>'s or <audio>'s own file comes in ranges; a player's fetch of a range, or of a
  // small file, is a piece of a stream.
  if (d.type !== "media" && (d.statusCode === 206 || range || /[?&](range|bytestart|byterange)=/i.test(d.url) || size < FETCHED_MIN)) {
    return null;
  }
  const audio = mime.startsWith("audio/") || (!mime.startsWith("video/") && AUDIO_EXT.test(ext));
  return { kind: audio ? "audio" : "video", mime, size };
}

const isStream = (it) => it.kind === "hls" || it.kind === "dash";
// The folder a manifest's pieces are in ("https://cdn/v/"), or null for a site's root (every
// file of the site would be its piece).
function folderOf(url) {
  const p = pathOf(url);
  const dir = p.slice(0, p.lastIndexOf("/") + 1);
  return /^https?:\/\/[^/]+\/.+/i.test(dir) ? dir : null;
}

function noteMedia(d, found) {
  const tab = media.get(d.tabId) || { page: null, items: [] };
  const old = tab.items.find((it) => it.url === d.url);
  if (old) {
    if (found.size && !old.size) old.size = found.size;
    return;
  }
  // A file in a stream's own folder is one of its pieces (a DASH init segment, a fragment).
  const inStream = (it) => isStream(it) && folderOf(it.url) && d.url.startsWith(folderOf(it.url));
  if (!isStream(found) && tab.items.some(inStream)) return;
  tab.items.push({ url: d.url, kind: found.kind, frameId: d.frameId || 0, mime: found.mime, size: found.size });
  // Over the cap: the oldest go, but each frame's first stream stays (the button sends it).
  while (tab.items.length > MEDIA_MAX) {
    const firsts = new Set();
    const i = tab.items.findIndex((it) => {
      if (!isStream(it) || firsts.has(it.frameId)) return true;
      firsts.add(it.frameId);
      return false;
    });
    tab.items.splice(Math.max(i, 0), 1);
  }
  // The tab seen last goes last; the least recent ones go past MEDIA_TABS.
  media.delete(d.tabId);
  media.set(d.tabId, tab);
  for (const id of media.keys()) {
    if (media.size <= MEDIA_TABS) break;
    media.delete(id);
  }
  keepMedia();
}

// A response that may be a manifest (#171), noted apart: the newest MAYBE_MAX of the tab.
function noteMaybe(d, found) {
  const tab = media.get(d.tabId) || { page: null, items: [] };
  if (tab.items.some((it) => it.url === d.url)) return;
  tab.maybe = (tab.maybe || []).filter((m) => m.url !== d.url);
  tab.maybe.push({ url: d.url, frameId: d.frameId || 0, mime: found.mime });
  if (tab.maybe.length > MAYBE_MAX) tab.maybe.splice(0, tab.maybe.length - MAYBE_MAX);
  if (!media.has(d.tabId)) {
    media.set(d.tabId, tab);
    for (const id of media.keys()) {
      if (media.size <= MEDIA_TABS) break;
      media.delete(id);
    }
  }
  keepMedia();
}

// What a text starts as: an HLS playlist ("hls"), a DASH manifest ("dash"), or neither ("").
function manifestKind(text) {
  const t = String(text).replace(/^﻿/, "").trimStart();
  if (t.startsWith("#EXTM3U")) return "hls";
  // An XML declaration, comments, then the MPD element (with a namespace prefix or not).
  if (/^(?:<\?xml[^>]*\?>\s*)?(?:<!--[\s\S]*?-->\s*)*<(?:[\w-]+:)?MPD[\s>/]/.test(t)) return "dash";
  return "";
}

// The start of a response (SNIFF_BYTES), fetched again with the site's cookies as `probe` does:
// what it is (`manifestKind`), or "" (not a manifest, refused, too slow).
const SNIFF_BYTES = 4096;
async function sniff(url) {
  let reader = null;
  try {
    const r = await withTimeout(fetch(url, { credentials: "include" }), 5000);
    if (!r.ok) return "";
    if (!r.body || !r.body.getReader) return manifestKind((await withTimeout(r.text(), 5000)).slice(0, SNIFF_BYTES));
    reader = r.body.getReader();
    const dec = new TextDecoder();
    let text = "";
    while (text.length < SNIFF_BYTES) {
      const { done, value } = await withTimeout(reader.read(), 5000);
      if (done) break;
      text += dec.decode(value, { stream: true });
    }
    return manifestKind(text);
  } catch {
    return "";
  } finally {
    // The rest is never read.
    if (reader) reader.cancel().catch(() => {});
  }
}

// The tab's possible manifests read, those of `frames` only (a Set of frame ids, or null: all),
// each once: the ones that are become its streams, in the order they were loaded. One reading at
// a time per tab: a second ask waits for the first.
const sniffing = new Map();
function sniffTab(tabId, frames) {
  const run = async () => {
    const tab = media.get(tabId);
    if (!tab || !tab.maybe || !tab.maybe.length) return;
    const asked = tab.maybe.filter((m) => !frames || frames.has(m.frameId));
    if (!asked.length) return;
    tab.maybe = tab.maybe.filter((m) => !asked.includes(m));
    const kinds = await Promise.all(asked.map((m) => sniff(m.url)));
    // The tab went to another page meanwhile: its old page's answers don't count.
    if (media.get(tabId) !== tab) return;
    asked.forEach((m, i) => {
      if (kinds[i]) noteMedia({ tabId, url: m.url, frameId: m.frameId }, { kind: kinds[i], mime: m.mime, size: 0 });
    });
    keepMedia();
  };
  const next = (sniffing.get(tabId) || Promise.resolve()).then(run, run);
  sniffing.set(tabId, next);
  next.finally(() => sniffing.get(tabId) === next && sniffing.delete(tabId)).catch(() => {});
  return next;
}

if (ext.webRequest && ext.webRequest.onResponseStarted) {
  ext.webRequest.onResponseStarted.addListener(
    (d) => {
      const found = mediaKind(d);
      if (found) mediaBack.then(() => (found.kind === "maybe" ? noteMaybe(d, found) : noteMedia(d, found)));
    },
    { urls: ["<all_urls>"], types: ["xmlhttprequest", "media", "other"] },
    ["responseHeaders"],
  );
}
ext.tabs.onRemoved.addListener((id) => mediaBack.then(() => media.delete(id) && keepMedia()));
// Another page in the tab: what the last one loaded goes (not for a move within the page, #part).
ext.tabs.onUpdated.addListener((id, change) => {
  if (!change.url) return;
  mediaBack.then(() => {
    const page = change.url.split("#")[0];
    const tab = media.get(id);
    if (tab && tab.page === page) return;
    media.set(id, { page, items: [] });
    keepMedia();
  });
});

// Kept in the session's storage (memory only, gone with the browser): the browser stops this
// script when idle, and a tab's player loaded its stream long before it is asked for. Read back
// once at start, merged with what was seen meanwhile; everything else waits for it.
const session = ext.storage.session;
let keeping = null;
function keepMedia() {
  if (!session || keeping) return;
  keeping = setTimeout(() => {
    keeping = null;
    session.set({ media: [...media] }).catch(() => {});
  }, 200);
}
const mediaBack = session
  ? session
      .get({ media: [] })
      .then((v) => {
        for (const [id, kept] of v.media || []) {
          const now = media.get(id);
          const fresh = ((now && now.items) || []).filter((s) => !kept.items.some((o) => o.url === s.url));
          const maybe = [...(kept.maybe || []), ...((now && now.maybe) || [])].filter((m, i, all) => all.findIndex((o) => o.url === m.url) === i);
          media.set(id, { ...kept, page: (now && now.page) || kept.page, items: [...kept.items, ...fresh].slice(-MEDIA_MAX), maybe: maybe.slice(-MAYBE_MAX) });
        }
      })
      .catch(() => {})
  : Promise.resolve();

// The stream a frame's player loaded first (its master playlist), else the tab's.
function streamOf(sender) {
  const list = ((sender.tab && media.get(sender.tab.id)) || { items: [] }).items.filter(isStream);
  const mine = list.find((s) => s.frameId === sender.frameId);
  return (mine || list[0] || {}).url || "";
}

// The same for a player that plays from a blob: (asked by the button over it or the right-click
// menu), reading the tab's possible manifests when no stream is known (#171): the frame's own
// first, then the tab's other frames'.
async function streamFor(sender) {
  const tabId = sender.tab && sender.tab.id;
  if (tabId === undefined || tabId === null) return streamOf(sender);
  const frameId = sender.frameId || 0;
  const own = () => ((media.get(tabId) || { items: [] }).items.some((it) => isStream(it) && it.frameId === frameId));
  if (!own()) await sniffTab(tabId, new Set([frameId]));
  if (!own() && !streamOf(sender)) await sniffTab(tabId, null);
  return streamOf(sender);
}

// ---- a stream's manifest: its qualities, length, and whether it is protected -------------------

// An HLS tag's attributes (KEY=value,KEY="value").
function hlsAttrs(s) {
  const out = {};
  for (const [, k, v] of s.matchAll(/([A-Z0-9-]+)=("[^"]*"|[^,]*)/g)) out[k] = v.replace(/^"|"$/g, "");
  return out;
}

// A copy-protected (DRM) key: a key system's (Apple's FairPlay, Google's Widevine, Microsoft's
// PlayReady), or SAMPLE-AES, which no download tool decrypts. AES-128 with a plain key
// ("identity") is not DRM: yt-dlp fetches its key as the player does.
function drmKey(a) {
  const method = (a.METHOD || "").toUpperCase();
  if (!method || method === "NONE") return false;
  return method.startsWith("SAMPLE-AES") || (a.KEYFORMAT !== undefined && a.KEYFORMAT !== "identity");
}

// {heights, duration, drm, live, children}: `children`, the addresses (without their query) a
// master playlist names: its variants and renditions, the same stream.
function parseHls(text, base) {
  const lines = text.replace(/^﻿/, "").split(/\r?\n/).map((l) => l.trim());
  if (!lines[0].startsWith("#EXTM3U")) return null;
  const info = { heights: [], duration: 0, drm: false, live: true, children: [] };
  const child = (uri) => {
    try {
      info.children.push(pathOf(new URL(uri, base).href));
    } catch {
      // Not an address.
    }
  };
  for (let i = 0; i < lines.length; i++) {
    const l = lines[i];
    const value = l.slice(l.indexOf(":") + 1);
    if (l.startsWith("#EXT-X-STREAM-INF:")) {
      const h = Number((/x(\d+)$/.exec(hlsAttrs(value).RESOLUTION || "") || [])[1]);
      if (h) info.heights.push(h);
      const uri = lines.slice(i + 1).find((x) => x && !x.startsWith("#"));
      if (uri) child(uri);
    } else if (l.startsWith("#EXT-X-MEDIA:") || l.startsWith("#EXT-X-I-FRAME-STREAM-INF:")) {
      const a = hlsAttrs(value);
      if (a.URI) child(a.URI);
    } else if (l.startsWith("#EXT-X-KEY:") || l.startsWith("#EXT-X-SESSION-KEY:")) {
      if (drmKey(hlsAttrs(value))) info.drm = true;
    } else if (l.startsWith("#EXTINF:")) {
      info.duration += parseFloat(value) || 0;
    } else if (l.startsWith("#EXT-X-ENDLIST") || /^#EXT-X-PLAYLIST-TYPE:\s*VOD/.test(l)) {
      info.live = false;
    }
  }
  // A master playlist says nothing of an end: its variants do.
  if (info.children.length) info.live = false;
  return info;
}

// Seconds of an ISO 8601 duration (PT1H2M3.5S).
function isoSeconds(d) {
  const m = /P(?:(\d+)D)?T?(?:(\d+)H)?(?:(\d+)M)?(?:([\d.]+)S)?/.exec(d || "") || [];
  return (+m[1] || 0) * 86400 + (+m[2] || 0) * 3600 + (+m[3] || 0) * 60 + (+m[4] || 0);
}

// A DASH manifest, read as text (a service worker has no XML parser). Any ContentProtection is
// encryption: Widevine, PlayReady, or CENC with a key the site gives its own player only.
function parseDash(text) {
  if (!/<(?:\w+:)?MPD[\s>]/.test(text)) return null;
  const attr = (tag, name) => (new RegExp(`\\s${name}\\s*=\\s*["']([^"']*)["']`).exec(tag) || [])[1];
  const mpd = (/<(?:\w+:)?MPD\b[^>]*>/.exec(text) || [""])[0];
  const heights = [];
  for (const [tag] of text.matchAll(/<(?:\w+:)?(?:Representation|AdaptationSet)\b[^>]*>/g)) {
    const h = Number(attr(tag, "height"));
    if (h) heights.push(h);
  }
  return {
    heights,
    duration: isoSeconds(attr(mpd, "mediaPresentationDuration")),
    drm: /<(?:\w+:)?ContentProtection\b/.test(text),
    live: attr(mpd, "type") === "dynamic",
    children: [],
  };
}

// A manifest, fetched again by the extension (the player's copy can't be read): its details, or
// {failed: true}: a site that refuses the extension's request (it checks the page that asks) is
// listed without them.
async function probe(item) {
  try {
    const r = await withTimeout(fetch(item.url, { credentials: "include" }), 5000);
    if (!r.ok || Number(r.headers.get("content-length")) > MANIFEST_MAX) return { failed: true };
    const text = await withTimeout(r.text(), 5000);
    if (text.length > MANIFEST_MAX) return { failed: true };
    return (item.kind === "hls" ? parseHls(text, item.url) : parseDash(text)) || { failed: true };
  } catch {
    return { failed: true };
  }
}

// Run in each frame of the tab: its address and its players (their file, picture size, length,
// and whether the page gave them a DRM key system).
function mediaElements() {
  const players = [...document.querySelectorAll("video, audio")].map((v) => ({
    src: v.currentSrc || v.src || "",
    w: v.videoWidth || 0,
    h: v.videoHeight || 0,
    duration: Number.isFinite(v.duration) ? v.duration : 0,
    drm: Boolean(v.mediaKeys),
  }));
  return { href: location.href, players };
}

// The tab's media as the popup lists them: a stream's variants inside it, each with what is known
// (qualities, length, size, protected). Manifests are read once.
async function mediaList(tabId) {
  await mediaBack;
  if (!media.get(tabId)) return [];
  const frames = {};
  try {
    const found = await withTimeout(ext.scripting.executeScript({ target: { tabId, allFrames: true }, func: mediaElements }), 3000);
    for (const f of found || []) if (f && f.result) frames[f.frameId || 0] = f.result;
  } catch {
    // A page extensions may not read: listed without its players' details.
  }
  // #171: the possible manifests of frames whose player plays from a blob: (a MediaSource), read.
  const fed = Object.entries(frames)
    .filter(([, f]) => f.players.some((v) => /^blob:/i.test(v.src)))
    .map(([id]) => Number(id));
  if (fed.length) await sniffTab(tabId, new Set(fed));
  const tab = media.get(tabId);
  if (!tab) return [];
  await Promise.all(tab.items.filter((it) => isStream(it) && !it.info).map(async (it) => (it.info = await probe(it))));
  tab.frames = Object.fromEntries(Object.entries(frames).map(([id, f]) => [id, f.href]));
  keepMedia();
  const out = [];
  const shown = [];
  for (const it of tab.items) {
    // A variant of a master playlist listed already: it names it, or (unread) the variant is in
    // its folder or below.
    const p = pathOf(it.url);
    const variantOf = (s) =>
      s.info && !s.info.failed ? s.info.children.includes(p) : s.frameId === it.frameId && folderOf(s.url) && p.startsWith(folderOf(s.url));
    if (it.kind === "hls" && shown.some((s) => s.kind === "hls" && variantOf(s))) continue;
    shown.push(it);
    const frame = frames[it.frameId] || { players: [] };
    const player = frame.players.find((v) => v.src === it.url);
    const info = it.info && !it.info.failed ? it.info : null;
    out.push({
      url: it.url,
      kind: it.kind,
      mime: it.mime,
      size: it.size || 0,
      heights: info ? [...new Set(info.heights)].sort((a, b) => b - a) : player && player.h ? [player.h] : [],
      width: (!info && player && player.w) || 0,
      duration: (info ? info.duration : player && player.duration) || 0,
      live: Boolean(info && info.live),
      // Protected: its manifest says so, or (unread) its frame's player has a DRM key system.
      drm: info ? info.drm : isStream(it) && frame.players.some((v) => v.drm),
    });
  }
  return out;
}

// One of the tab's media, handed to the app as the popup asked: a stream to the Download video
// window (yt-dlp), a file as a download; with the page that played it (the referrer), the cookies
// its own server gets, and the browser's User-Agent (`native`). A protected one is never sent.
async function mediaDownload(tabId, url) {
  const listed = await mediaList(tabId);
  const shown = listed.find((x) => x.url === url);
  if (!shown) return { ok: false, why: "gone" };
  if (shown.drm) return { ok: false, why: "drm" };
  const kept = media.get(tabId);
  const it = kept.items.find((x) => x.url === url);
  let tab;
  try {
    tab = await ext.tabs.get(tabId);
  } catch {
    return { ok: false, why: "gone" };
  }
  const frame = (kept.frames || {})[it.frameId];
  const page = frame && /^https?:/i.test(frame) ? frame : tab.url || "";
  const ctx = { incognito: Boolean(tab.incognito), storeId: tab.cookieStoreId };
  if (isStream(it)) {
    return native({
      video: true,
      // Asked for: whether the button over videos shows on this site doesn't matter.
      explicit: true,
      url: page,
      src: it.url,
      title: String(tab.title || "").slice(0, 300),
      referrer: tab.url && tab.url !== page ? tab.url : "",
      ...(await videoCookies(it.url, ctx)),
      ...(ctx.incognito ? { private: true } : {}),
    });
  }
  return handOff({ url: it.url, referrer: page, size: it.size, mime: it.mime, explicit: true, ...ctx });
}

// ---- the hand-off ------------------------------------------------------------------------

// Cookies from the store the download belongs to, never another: Firefox names it on the
// download (private windows and containers each have their own); Chrome's incognito store is "1".
async function cookiesFor(url, { incognito, storeId }) {
  try {
    const query = storeId ? { url, storeId } : incognito ? { url, storeId: "1" } : { url };
    return (await ext.cookies.getAll(query)) || [];
  } catch {
    return [];
  }
}

function header(list) {
  return list.map((c) => `${c.name}=${c.value}`).join("; ");
}

async function cookieHeader(url, ctx) {
  return header(await cookiesFor(url, ctx));
}

// For yt-dlp, which may ask another host of the site than the page's (a YouTube Music page is
// looked up on www.youtube.com): each cookie with the domain, path and flags the browser keeps,
// so the app files `.youtube.com`'s sign-in for every host of it and a host's own cookie for
// that host alone. The header goes too, for an older app.
async function videoCookies(url, ctx) {
  const list = await cookiesFor(url, ctx);
  const cookieJar = list.map((c) => ({
    name: c.name,
    value: c.value,
    domain: c.domain,
    hostOnly: Boolean(c.hostOnly),
    path: c.path || "/",
    secure: Boolean(c.secure),
    ...(typeof c.expirationDate === "number" ? { expirationDate: c.expirationDate } : {}),
  }));
  return { cookies: header(list), cookieJar };
}

// Has the user left this extension the site? With access limited ("On click", some sites), its
// cookies can't be read, and a download handed over without them would fail.
async function siteAccess(url) {
  try {
    const u = new URL(url);
    // By host name: Firefox's match patterns take no port.
    return await ext.permissions.contains({ origins: [`${u.protocol}//${u.hostname}/*`] });
  } catch {
    return false;
  }
}

// ---- the browser's proxy (#42) -------------------------------------------------------------

// A VPN or proxy extension is a proxy for the browser only: the app downloads through the same
// one (Options → Proxy / Socks → "Use proxy/socks from a browser's request"), or the download
// would leave from the user's own address. What goes with each download:
// - `proxy`: the proxy the browser uses for its address, "scheme://host:port";
// - `proxyPac`: the browser's proxy auto-config script (its text), when it uses one;
// - `proxyLogin`: that proxy asked the browser for a login. The extension that set it answers
//   with its own, which no other program can have: the app says so rather than going without.
// Firefox tells each request's proxy (`proxyInfo`), what proxy extensions set per request
// (`proxy.onRequest`) included; Chromium has no such event, only its proxy settings
// (`proxy.settings.get`: a fixed server per scheme with its bypass list, or a PAC script).
const proxyLogins = new Set();
const proxyUsed = new Map();
const PAC_MAX = 256 * 1024;

// Reading the browser's proxy is the user's choice, the popup's switch (off as installed):
// - Firefox: the optional "proxy" permission, asked for by the switch and taken back by it.
// - Chromium can't make "proxy" optional (chrome.permissions), so its manifest has it from the
//   install, and the switch is the extension's own setting (`proxyOn` in storage.local). While it
//   is off, nothing of the browser's proxy is read or sent.
// `proxyAllowed` follows the permission or the setting as it changes.
const PROXY_PERMISSION = { permissions: ["proxy"] };
const PROXY_SWITCH = "proxyOn";
const [PROXY_DECLARED, PROXY_REQUIRED] = (() => {
  try {
    const m = ext.runtime.getManifest();
    const required = (m.permissions || []).includes("proxy");
    return [required || (m.optional_permissions || []).includes("proxy"), required];
  } catch {
    return [false, false];
  }
})();
// Off until checked.
let proxyAllowed = false;
async function checkProxy() {
  try {
    proxyAllowed = PROXY_REQUIRED
      ? Boolean((await ext.storage.local.get({ [PROXY_SWITCH]: false }))[PROXY_SWITCH])
      : PROXY_DECLARED && Boolean(await ext.permissions.contains(PROXY_PERMISSION));
  } catch {
    proxyAllowed = false;
  }
  if (!proxyAllowed) {
    // Nothing kept of what was seen while it was allowed.
    proxyLogins.clear();
    proxyUsed.clear();
  }
}
const proxyChecked = PROXY_DECLARED ? checkProxy() : Promise.resolve();
if (PROXY_REQUIRED && ext.storage.onChanged) {
  ext.storage.onChanged.addListener((changes, area) => {
    if (area === "local" && PROXY_SWITCH in changes) checkProxy();
  });
} else if (PROXY_DECLARED && ext.permissions && ext.permissions.onAdded) {
  ext.permissions.onAdded.addListener(checkProxy);
  ext.permissions.onRemoved.addListener(checkProxy);
}

if (PROXY_DECLARED && ext.webRequest && ext.webRequest.onAuthRequired) {
  try {
    ext.webRequest.onAuthRequired.addListener(
      (d) => {
        if (proxyAllowed && d.isProxy && d.challenger) proxyLogins.add(`${d.challenger.host}:${d.challenger.port}`.toLowerCase());
      },
      { urls: ["<all_urls>"] },
    );
  } catch {
    // Not offered here: no login is noticed.
  }
}
if (PROXY_DECLARED && FIREFOX && ext.webRequest && ext.webRequest.onResponseStarted) {
  ext.webRequest.onResponseStarted.addListener(
    (d) => {
      if (!proxyAllowed || !d.proxyInfo || d.proxyInfo.type === "direct") return;
      forget(proxyUsed, 120000);
      proxyUsed.set(d.url, { info: d.proxyInfo, at: Date.now() });
    },
    { urls: ["<all_urls>"] },
  );
}

const hostPort = (host, port) => `${String(host).includes(":") ? `[${host}]` : host}:${port}`;

// "scheme://host:port" for one of the browser's proxy descriptions, or null.
function proxyUrl(scheme, host, port, remoteDns) {
  if (!host || !port) return null;
  const s = String(scheme || "http").toLowerCase();
  const kind =
    s === "socks" || s === "socks5" ? (remoteDns === false ? "socks5" : "socks5h") : s === "socks4" ? "socks4" : s === "https" ? "https" : s === "http" ? "http" : null;
  return kind ? `${kind}://${hostPort(host, port)}` : null;
}

// Does `pattern` (a proxy bypass entry: "<local>", "*.example.com", ".example.com", "host",
// "host:port") cover `u`? CIDR entries aren't read (the proxy is then used).
function bypassed(pattern, u) {
  const p = String(pattern || "").trim().toLowerCase();
  if (!p) return false;
  const host = u.hostname.toLowerCase().replace(/^\[|\]$/g, "");
  if (p === "<local>") return !host.includes(".") || host === "localhost" || host === "127.0.0.1" || host === "::1";
  const bare = p.replace(/^[a-z]+:\/\//, "").replace(/:\d+$/, "");
  if (bare.startsWith("*.")) return host.endsWith(bare.slice(1));
  if (bare.startsWith(".")) return host.endsWith(bare) || host === bare.slice(1);
  return host === bare;
}

// Chromium's bypass list (its proxy rules' `bypassList`) for `u`, as Chromium reads one: "<local>"
// (a name without a dot), "[scheme://]host-pattern[:port]" with "*" anywhere (".example.com" is
// "*.example.com"), an IPv4 range "10.0.0.0/8"; and, unless "<-loopback>" is listed, the names
// Chromium never sends to a proxy (localhost, 127.x, ::1, link-local). IPv6 ranges aren't read
// (the proxy is then used).
function chromiumBypassed(list, u) {
  const host = u.hostname.toLowerCase().replace(/^\[|\]$/g, "");
  const scheme = u.protocol.replace(/:$/, "");
  const port = u.port || (scheme === "https" ? "443" : "80");
  const v4 = /^\d{1,3}(\.\d{1,3}){3}$/.test(host) ? host.split(".").reduce((n, b) => n * 256 + Number(b), 0) : null;
  const ip = v4 !== null || host.includes(":");
  const rules = (list || []).map((r) => String(r).trim().toLowerCase()).filter(Boolean);
  if (!rules.includes("<-loopback>")) {
    if (host === "localhost" || host.endsWith(".localhost") || host === "::1" || (host.includes(":") && /^fe[89ab]/.test(host))) return true;
    if (v4 !== null && (host.startsWith("127.") || host.startsWith("169.254."))) return true;
  }
  return rules.some((r) => {
    if (r === "<-loopback>") return false;
    if (r === "<local>") return !ip && !host.includes(".");
    const range = /^(\d{1,3}(?:\.\d{1,3}){3})\/(\d{1,2})$/.exec(r);
    if (range) {
      if (v4 === null) return false;
      const base = range[1].split(".").reduce((n, b) => n * 256 + Number(b), 0);
      const bits = Number(range[2]);
      const size = 2 ** (32 - Math.min(bits, 32));
      return Math.floor(v4 / size) === Math.floor(base / size);
    }
    const m = /^(?:([a-z][a-z0-9+.-]*):\/\/)?(\[[^\]]+\]|[^:/]+)(?::(\d+))?\/?$/.exec(r);
    if (!m || (m[1] && m[1] !== scheme) || (m[3] && m[3] !== port)) return false;
    const pattern = m[2].replace(/^\[|\]$/g, "").replace(/^\./, "*.");
    const glob = pattern.replace(/[.+?^$|\\()[\]{}]/g, "\\$&").replace(/\*/g, ".*");
    return new RegExp("^" + glob + "$").test(host);
  });
}

// The port Chromium takes for a proxy given without one.
const chromiumPort = (scheme) => ({ https: 443, quic: 443, socks: 1080, socks4: 1080, socks5: 1080 })[String(scheme || "http").toLowerCase()] || 80;

async function pacText(url) {
  try {
    const r = await withTimeout(fetch(url, { credentials: "omit" }), 10000);
    if (!r.ok) return null;
    const text = await r.text();
    return text.length <= PAC_MAX && /FindProxyForURL/.test(text) ? text : null;
  } catch {
    return null;
  }
}

// What the app needs to download `url` through the browser's proxy: {} when it uses none (or
// the system's, which the app reads itself), and {} unless the user switched it on (above).
async function browserProxy(url, incognito) {
  // Chromium's switch is read again for each download: the popup may have just changed it.
  await (PROXY_REQUIRED ? checkProxy() : proxyChecked);
  if (!proxyAllowed) return {};
  let u;
  try {
    u = new URL(url);
  } catch {
    return {};
  }
  const login = (host, port) => proxyLogins.has(`${host}:${port}`.toLowerCase());
  // Firefox: the proxy its own request for this address went through.
  const used = proxyUsed.get(url);
  if (used) {
    const p = used.info;
    const proxy = proxyUrl(p.type, p.host, p.port, p.proxyDNS);
    if (!proxy) return {};
    return { proxy, ...(p.username || login(p.host, p.port) ? { proxyLogin: true } : {}) };
  }
  if (!ext.proxy || !ext.proxy.settings) return {};
  let v;
  try {
    v = (await ext.proxy.settings.get(incognito ? { incognito: true } : {})).value || {};
  } catch {
    return {};
  }
  // Chromium: {mode, rules, pacScript}, the browser's effective proxy whoever set it (its
  // levelOfControl): a VPN or proxy extension ("controlled_by_other_extensions", the case this
  // is for), a policy or the command line ("not_controllable"), or nobody ({mode: "system"}).
  // "direct", "system" and "auto_detect" send nothing: the app then goes by its own Options
  // (which read the system's proxy). The proxy's login is never in it, on Chromium either.
  if (v.mode === "fixed_servers" && v.rules) {
    const r = v.rules;
    if (chromiumBypassed(r.bypassList, u)) return {};
    const s = r.singleProxy || (u.protocol === "https:" ? r.proxyForHttps : r.proxyForHttp) || r.fallbackProxy;
    const port = s && (s.port || chromiumPort(s.scheme));
    const proxy = s && proxyUrl(s.scheme, s.host, port);
    if (!proxy) return {};
    return { proxy, ...(login(s.host, port) ? { proxyLogin: true } : {}) };
  }
  if (v.mode === "pac_script" && v.pacScript) {
    const data = v.pacScript.data && v.pacScript.data.length <= PAC_MAX ? v.pacScript.data : null;
    const text = data || (v.pacScript.url ? await pacText(v.pacScript.url) : null);
    return text ? { proxyPac: text } : {};
  }
  // Firefox: {proxyType, http, ssl, socks, socksVersion, proxyDNS, autoConfigUrl, passthrough}.
  if (v.proxyType === "manual") {
    if (String(v.passthrough || "").split(/[,\s]+/).some((b) => bypassed(b, u))) return {};
    const pick = (hp) => {
      const m = /^\[?([^\]]+?)\]?:(\d+)$/.exec(String(hp || "").trim());
      return m ? { host: m[1], port: Number(m[2]) } : null;
    };
    const http = pick(v.http);
    const ssl = v.httpProxyAll ? http : pick(v.ssl);
    const socks = pick(v.socks);
    const s = (u.protocol === "https:" ? ssl : http) || null;
    const proxy = s
      ? proxyUrl("http", s.host, s.port)
      : socks && proxyUrl(v.socksVersion === 4 ? "socks4" : "socks5", socks.host, socks.port, v.proxyDNS);
    if (!proxy) return {};
    const at = s || socks;
    return { proxy, ...(login(at.host, at.port) ? { proxyLogin: true } : {}) };
  }
  if (v.proxyType === "autoConfig" && v.autoConfigUrl) {
    const text = await pacText(v.autoConfigUrl);
    return text ? { proxyPac: text } : {};
  }
  return {};
}

// `explicit`: the user asked for this one (the right-click item), so Options' automatic-capture
// rules (file types, excluded sites) don't apply; the host checks them for everything else.
async function handOff(d) {
  const message = {
    url: d.url,
    finalUrl: d.finalUrl && d.finalUrl !== d.url ? d.finalUrl : "",
    referrer: d.referrer || "",
    cookies: await cookieHeader(d.url, d),
    filename: d.filename || "",
    ...(d.size > 0 ? { size: d.size } : {}),
    ...(d.mime ? { mime: d.mime } : {}),
    ...(d.post ? { post: d.post.body, postType: d.post.type || "" } : {}),
    ...(d.incognito ? { private: true } : {}),
    ...(d.explicit ? { explicit: true } : {}),
    // Through the browser's proxy too (#42): the one its own request for the file took.
    ...(await browserProxy(d.finalUrl || d.url, d.incognito)),
  };
  return native(message);
}

// The file name to offer the app the download under (`{name}`), or why Options leave it to
// the browser (`{left, name}`), or null: the browser keeps it and there is nothing to say (not a
// web address, paused, Alt held, a form that can't be posted again, MDM not reached).
// Telegram's own page downloader streams protected media through its service worker: the
// page-only endpoint (`/k/d/…`) answers 302 to anything outside the page. Leave its downloads
// to the page; `telegramFinished` below adds the finished file to MDM.
function isTelegramUrl(u) {
  try {
    let s = String(u || "");
    if (s.startsWith("blob:")) s = s.slice(5);
    const host = new URL(s).hostname.toLowerCase();
    return (
      host === "web.telegram.org" ||
      host === "webk.telegram.org" ||
      host === "webz.telegram.org"
    );
  } catch {
    return false;
  }
}

async function candidate(item, url) {
  if (!/^https?:/i.test(url)) return null; // blob:, data:, file: stay the browser's
  // Telegram: its page's own downloader has the session and the media (protected channels
  // included). Handing the page-only URL to the app made it answer 302.
  if (isTelegramUrl(url) || isTelegramUrl(item.referrer)) return null;
  const { paused } = await ext.storage.local.get({ paused: false });
  if (paused) return null;
  forget(bypass, 10000);
  if (bypass.has(item.url) || bypass.has(url)) return null;
  const first = firstUrl(item);
  if (bypass.has(first) || postOf(first) === null) return null; // Alt; a form not to post again
  const r = await currentRules();
  if (!r) return null;
  const name = (item.filename || "").split(/[\\/]/).pop() || url.split(/[?#]/)[0].split("/").pop() || "";
  const left = (why) => ({ left: why, name });
  if (!r.capture) return left(r.captureOff || "browser");
  const by = excludedBy(r, first) || excludedBy(r, url);
  if (by) return left(by);
  const type = extensionOf(name);
  if (!type || !typePatterns(r.types).some((p) => p.test(type))) return left("file-type");
  // Smaller than Options' minimum (when the browser knows the size already).
  const size = item.totalBytes > 0 ? item.totalBytes : item.fileSize;
  if (r.minKb > 0 && size > 0 && size < r.minKb * 1024) return left("small");
  if (!(await siteAccess(first))) return left("site-access");
  return { name };
}

// Why it stays with the browser, said (the popup, MDM's log).
function leftAlone(c, item, url) {
  if (c && c.left) noteLeft(c.left, c.name, firstUrl(item) || url, item.incognito, false).catch(() => {});
}

// Offer the download to the app under `name`; true if it took it.
async function offer(item, url, name) {
  const first = firstUrl(item);
  const reply = await handOff({
    url: first,
    finalUrl: url,
    referrer: item.referrer,
    filename: name,
    size: item.totalBytes > 0 ? item.totalBytes : item.fileSize,
    mime: item.mime,
    post: postOf(first),
    incognito: item.incognito,
    storeId: item.cookieStoreId,
  });
  if (reply.ok) {
    await ext.storage.local.remove("declined").catch(() => {});
  } else {
    // Refused by Options in the app (changed since the extension asked, or a rule only the app
    // checks, as a redirect's end): the host logged it; the popup says it.
    const why = declinedOf(reply);
    if (why) await noteLeft(why, name, first, item.incognito, true).catch(() => {});
  }
  return Boolean(reply.ok);
}

// Tabs opened in the last seconds: one the download was opened in (a link opened in a new tab
// or window, Shift+click) is closed with it, as when the browser keeps the download.
const newTabs = new Map();
ext.tabs.onCreated.addListener((tab) => {
  forget(newTabs, 10000);
  newTabs.set(tab.id, { at: Date.now() });
});

async function closeLeftTab(urls) {
  forget(newTabs, 10000);
  for (const id of newTabs.keys()) {
    try {
      const tab = await ext.tabs.get(id);
      const at = tab.pendingUrl || tab.url || "";
      // Only a tab opened for the download's own address (never a new tab the user opened).
      if (urls.includes(at)) {
        newTabs.delete(id);
        await ext.tabs.remove(id);
      }
    } catch {
      newTabs.delete(id);
    }
  }
}

// The browser's download, gone without a trace once the app has it.
async function drop(item) {
  const id = item.id;
  try {
    await ext.downloads.cancel(id);
  } catch {
    // Already finished: its file goes.
  }
  try {
    const [d] = await ext.downloads.search({ id });
    if (d && d.state === "complete" && d.exists !== false) await ext.downloads.removeFile(id);
  } catch {
    // Gone already.
  }
  ext.downloads.erase({ id }).catch(() => {});
  closeLeftTab([firstUrl(item), item.url, item.finalUrl].filter(Boolean)).catch(() => {});
}

if (!FIREFOX) {
  // Chromium: once the final file name is known (after redirects and Content-Disposition). The
  // download waits for `suggest`, writing nothing, while the app is asked: taken, it is
  // cancelled; else it goes on as the browser's. So it is never lost or kept by both (the app
  // asks the address once more: a form's download is posted again with its body).
  ext.downloads.onDeterminingFilename.addListener((item, suggest) => {
    const url = item.finalUrl || item.url;
    candidate(item, url)
      .then(async (c) => {
        const taken = Boolean(c && c.name && !c.left) && (await offer(item, url, c.name));
        if (taken) return drop(item);
        // The browser goes on first; why is said after.
        suggest();
        leftAlone(c, item, url);
      })
      .catch(() => suggest());
    return true; // suggest() comes later
  });
} else {
  // Firefox names the file before the download is created and has no such pause: the download
  // is paused at once when it may be the app's (the stored rules say so without asking), then
  // cancelled or resumed; one that finished meanwhile (a small file) has its file removed.
  ext.downloads.onCreated.addListener(async (item) => {
    const url = item.finalUrl || item.url;
    const c = await candidate(item, url).catch(() => null);
    if (!c || c.left) return leftAlone(c, item, url);
    const name = c.name;
    let paused = false;
    try {
      await ext.downloads.pause(item.id);
      paused = true;
    } catch {
      // Not pausable (no resume support), or finished already.
    }
    const taken = await offer(item, url, name).catch(() => false);
    if (taken) drop(item);
    else if (paused) ext.downloads.resume(item.id).catch(() => {});
  });
}

// Telegram's page finishes protected media itself (its service worker carries the session).
// Take the finished file into MDM's list and drop the browser's own entry, so it isn't kept
// twice. The file stays where the browser saved it.
const telegramImported = new Set();
ext.downloads.onChanged.addListener(async (delta) => {
  if (!delta.state || delta.state.current !== "complete") return;
  const id = delta.id;
  if (telegramImported.has(id)) return;
  try {
    const [item] = await ext.downloads.search({ id });
    if (!item || item.exists === false || !item.filename || item.incognito) return;
    if (![item.url, item.finalUrl, item.referrer].some(isTelegramUrl)) return;
    telegramImported.add(id);
    const name = (item.filename || "").split(/[\\/]/).pop() || "";
    const size = item.fileSize > 0 ? item.fileSize : item.totalBytes;
    const reply = await native({
      telegramDone: true,
      url: item.finalUrl || item.url,
      filePath: item.filename,
      filename: name,
      ...(size > 0 ? { size } : {}),
      ...(item.mime ? { mime: item.mime } : {}),
    });
    if (reply && reply.ok) ext.downloads.erase({ id }).catch(() => {});
  } catch {
    // The browser's own download stays; nothing is lost.
  }
});

// ---- files the browser would open itself (a PDF, an MP3): made downloads -----------------

// Options → General in the app: also take the listed types the browser shows or plays when a
// link leads to one. Chromium: a session rule marks such answers as attachments, so they come
// down the download path above (where the rules and Alt apply). Firefox: the same by changing
// the answer's headers.
const OPENED = {
  PDF: ["application/pdf"],
  MP3: ["audio/mpeg", "audio/mp3"],
  MP4: ["video/mp4"],
  M4A: ["audio/mp4", "audio/x-m4a"],
  M4V: ["video/x-m4v"],
  OGG: ["audio/ogg"],
  OGV: ["video/ogg"],
  WAV: ["audio/wav", "audio/x-wav", "audio/wave"],
  AAC: ["audio/aac"],
  MOV: ["video/quicktime"],
  TIF: ["image/tiff"],
  TIFF: ["image/tiff"],
  WMA: ["audio/x-ms-wma"],
  WMV: ["video/x-ms-wmv"],
  AVI: ["video/x-msvideo", "video/avi"],
  MKV: ["video/x-matroska"],
};
const OPENED_RULE = 1;
// The allow rules for exact excluded hosts follow it (ids from 2).
const OPENED_ALLOW = 2;
let openedTypes = [];

// Options' excluded sites as the rules need them, read as the app reads them: `*.example.com`
// is the domain and its subdomains (a rule's domain condition), a plain name that host only
// (an allow rule of its own). As the browser writes a host: no port, punycode.
function excludedHosts() {
  const wide = [];
  const exact = [];
  for (const raw of ((rules && rules.excludedSites) || "").split(/\s+/)) {
    const p = raw.trim().replace(/^[a-z]+:\/\//i, "").split("/")[0];
    const sub = p.startsWith("*.");
    let host = "";
    try {
      host = new URL(`http://${sub ? p.slice(2) : p}`).hostname;
    } catch {
      continue;
    }
    if (!host || host.includes("*")) continue;
    (sub ? wide : exact).push(host);
  }
  return { wide, exact };
}

function openedMimes(r) {
  if (!r || !r.capture || !r.opened) return [];
  const listed = new Set(r.types.split(/\s+/).map((s) => s.toUpperCase()));
  return [...new Set(Object.entries(OPENED).flatMap(([k, m]) => (listed.has(k) ? m : [])))];
}

async function syncOpenedRule() {
  const { paused } = await ext.storage.local.get({ paused: false });
  const { why } = await readStatus();
  openedTypes = paused || (why && why !== "update-app") ? [] : openedMimes(rules);
  const dnr = ext.declarativeNetRequest;
  if (FIREFOX || !dnr || !dnr.updateSessionRules) return;
  const hosts = excludedHosts();
  const rule = {
    id: OPENED_RULE,
    priority: 1,
    action: {
      type: "modifyHeaders",
      responseHeaders: [{ header: "content-disposition", operation: "set", value: "attachment" }],
    },
    condition: {
      resourceTypes: ["main_frame"],
      responseHeaders: [{ header: "content-type", values: openedTypes.map((m) => `${m}*`) }],
      // Options' excluded sites keep opening in the browser.
      ...(hosts.wide.length ? { excludedRequestDomains: hosts.wide } : {}),
    },
  };
  const esc = (h) => h.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const allows = hosts.exact.slice(0, 500).map((h, i) => ({
    id: OPENED_ALLOW + i,
    priority: 2,
    action: { type: "allow" },
    condition: { resourceTypes: ["main_frame"], regexFilter: `^[a-z]+://${esc(h)}(:[0-9]+)?/` },
  }));
  try {
    const old = (await dnr.getSessionRules()).map((r) => r.id);
    await dnr.updateSessionRules({
      removeRuleIds: old.filter((id) => id >= OPENED_RULE),
      addRules: openedTypes.length ? [rule, ...allows] : [],
    });
  } catch {
    // A browser without header conditions (Chrome before 128): it opens them as before.
  }
}

if (FIREFOX && ext.webRequest && ext.webRequest.onHeadersReceived) {
  ext.webRequest.onHeadersReceived.addListener(
    (d) => {
      const first = (redirected.get(d.url) || {}).from || d.url;
      if (!openedTypes.length || bypass.has(d.url) || bypass.has(first)) return {};
      if (rules && (excluded(rules, d.url) || excluded(rules, first))) return {};
      const headers = d.responseHeaders || [];
      const get = (n) => headers.find((h) => h.name.toLowerCase() === n);
      const type = ((get("content-type") || {}).value || "").toLowerCase();
      const cd = get("content-disposition");
      if (!openedTypes.some((m) => type.startsWith(m)) || (cd && /^\s*attachment/i.test(cd.value))) return {};
      // Under Options' minimum it would only go back to the browser as a download: it opens.
      const size = Number((get("content-length") || {}).value) || 0;
      if (rules && rules.minKb > 0 && size > 0 && size < rules.minKb * 1024) return {};
      return {
        responseHeaders: [
          ...headers.filter((h) => h.name.toLowerCase() !== "content-disposition"),
          { name: "Content-Disposition", value: "attachment" },
        ],
      };
    },
    { urls: ["<all_urls>"], types: ["main_frame"] },
    ["blocking", "responseHeaders"],
  );
}

// The Telegram page's own downloader, called in its main world: it has the current media and
// the signed-in session (protected channels and groups included). Its browser download is added
// to MDM when done (`telegramImported` above). Self-contained: `scripting.executeScript` sends
// this function's source to the page.
// The Telegram page's own fetch (its service worker only serves the page's requests). It
// streams chunks to the content script, which uploads them to MDM; no app token reaches the page.
function telegramFetchInPage(requestId, src) {
  (async () => {
    const CHUNK = 4 * 1024 * 1024;
    const post = (m, transfer) =>
      window.postMessage({ mdmTelegram: requestId, ...m }, "*", transfer || []);
    let offset = 0;
    let total = null;
    let mime = "";
    let seq = 0;
    const waitAck = (n) =>
      new Promise((resolve, reject) => {
        const on = (e) => {
          if (
            e.source !== window ||
            !e.data ||
            e.data.mdmTelegramAck !== requestId ||
            e.data.seq !== n
          ) {
            return;
          }
          clearTimeout(timer);
          window.removeEventListener("message", on);
          if (e.data.abort) reject(new Error("cancelled"));
          else resolve();
        };
        const timer = setTimeout(() => {
          window.removeEventListener("message", on);
          reject(new Error("MDM did not answer"));
        }, 30000);
        window.addEventListener("message", on);
      });
    try {
      while (total === null || offset < total) {
        const res = await fetch(src, {
          headers: { Range: `bytes=${offset}-${offset + CHUNK - 1}` },
          credentials: "include",
        });
        if (res.status !== 200 && res.status !== 206) {
          throw new Error(`Telegram answered ${res.status}`);
        }
        const type = (res.headers.get("content-type") || "")
          .split(";")[0]
          .trim()
          .toLowerCase();
        if (type) mime = type;
        if (total === null) {
          const cr = res.headers.get("content-range") || "";
          const m = /^bytes \d+-\d+\/(\d+)$/.exec(cr);
          if (m) total = Number(m[1]);
          else if (res.status === 200) {
            const cl = Number(res.headers.get("content-length"));
            if (cl > 0 && !res.headers.get("content-encoding")) total = cl;
          }
        }
        const data = await res.arrayBuffer();
        if (!data.byteLength) throw new Error("Telegram returned no media");
        if (seq === 0) post({ kind: "start", total, mime });
        const ack = waitAck(seq);
        post({ kind: "chunk", seq, data }, [data]);
        await ack;
        offset += data.byteLength;
        seq += 1;
        if (res.status === 200) break;
        if (total !== null && offset >= total) break;
        if (total === null && data.byteLength < CHUNK) break;
      }
      post({ kind: "end" });
    } catch (error) {
      post({ kind: "error", why: String((error && error.message) || error) });
    }
  })();
  return { ok: true };
}

// ---- right-click menus -------------------------------------------------------------------

// Created at install and update (removed first: they persist, and a second create fails), in
// the browser's language (_locales, from the app's catalogs by tools/extension/locales.py).
function createMenus() {
  ext.contextMenus.removeAll(() => {
    const add = (id, contexts, key) =>
      ext.contextMenus.create({ id, title: t(key), contexts }, () => void ext.runtime.lastError);
    add("mdm-link", ["link", "video", "audio", "image"], "menu");
    add("mdm-selected", ["selection"], "menu_selected");
    add("mdm-all", ["page"], "menu_all");
  });
}

// The links in the page (all of them, or those in the selection), collected in the page.
function pageLinks(selectedOnly) {
  const ok = (h) => /^(https?|ftps?|ftpes):/i.test(h);
  let anchors = [...document.querySelectorAll("a[href]")];
  if (selectedOnly) {
    const sel = getSelection();
    anchors = sel.rangeCount ? anchors.filter((a) => sel.containsNode(a, true)) : [];
  }
  const seen = new Set();
  const links = [];
  for (const a of anchors) {
    if (ok(a.href) && !seen.has(a.href)) {
      seen.add(a.href);
      links.push(a.href);
    }
  }
  for (const m of document.querySelectorAll("video[src], audio[src], source[src]")) {
    if (ok(m.src) && !seen.has(m.src) && !selectedOnly) {
      seen.add(m.src);
      links.push(m.src);
    }
  }
  return links;
}

// When the user asked and it couldn't be done: the help page's section opens beside (a link
// MDM refused has none of its own: the connection's).
const SECTIONS = ["not-installed", "forbidden", "not-running", "host-failed", "too-large", "update-app", "options"];
function explain(why) {
  const at = SECTIONS.includes(why) ? why : "host-failed";
  ext.tabs.create({ url: `${HELP}#${at}` }).catch(() => {});
}

ext.contextMenus.onClicked.addListener(async (info, tab) => {
  const incognito = Boolean(tab && tab.incognito);
  const storeId = tab && tab.cookieStoreId;
  if (info.menuItemId === "mdm-link") {
    // Telegram: the page's own fetch (its session/service worker) streams the media into MDM's
    // local upload server; never hand the page-only URL to the app (302).
    if (tab && isTelegramUrl(tab.url) && (info.mediaType || info.srcUrl)) {
      ext.tabs
        .sendMessage(tab.id, {
          type: "telegram-upload",
          src: info.srcUrl || "",
          title: tab.title || "",
        })
        .catch(() => {});
      return;
    }
    const url = info.linkUrl || info.srcUrl;
    // A player's own stream (blob:, a MediaSource) or a stream's manifest: the Download video
    // window, which reads streams (a manifest downloaded as a file is only a short list).
    // The manifest's own cookies (its server may not be the page's), as the media list sends.
    if (url && MANIFEST.test(url)) {
      const reply = await native({ video: true, explicit: true, url: info.pageUrl, src: url, ...(await videoCookies(url, { incognito, storeId })), private: incognito });
      if (!reply.ok && reply.why !== "options") explain(reply.why);
      return;
    }
    // A player playing from a blob: has no address yt-dlp can read: the stream its frame loaded,
    // as the button over videos sends (#155: the page alone was sent, and yt-dlp, not knowing
    // the site, answered "Unsupported URL"). Firefox names no address at all for a player fed
    // by a MediaSource (nothing was sent then).
    const player = info.mediaType === "video" || info.mediaType === "audio";
    if ((url && /^(blob|mediastream):/i.test(url)) || (player && !url)) {
      const page = info.frameUrl || info.pageUrl;
      await mediaBack;
      const src = await streamFor({ tab, frameId: info.frameId || 0 });
      const reply = await native({
        video: true,
        explicit: true,
        url: page,
        ...(src ? { src } : {}),
        referrer: info.pageUrl && info.pageUrl !== page ? info.pageUrl : "",
        ...(await videoCookies(src || page, { incognito, storeId })),
        private: incognito,
      });
      if (!reply.ok && reply.why !== "options") explain(reply.why);
      return;
    }
    // ftp links too: browsers no longer download them, MDM does.
    if (!url || !/^(https?|ftps?|ftpes):/i.test(url)) return;
    const reply = await handOff({ url, referrer: info.pageUrl, incognito, storeId, explicit: true });
    if (!reply.ok) explain(reply.why);
    return;
  }
  if ((info.menuItemId === "mdm-selected" || info.menuItemId === "mdm-all") && tab) {
    let links = [];
    try {
      const [res] = await ext.scripting.executeScript({
        target: { tabId: tab.id, frameIds: [info.frameId || 0] },
        func: pageLinks,
        args: [info.menuItemId === "mdm-selected"],
      });
      links = (res && res.result) || [];
    } catch {
      // A page extensions may not read (the browser's own, a store).
    }
    if (!links.length) return;
    const reply = await native({ links, referrer: info.pageUrl, cookies: await cookieHeader(info.pageUrl, { incognito, storeId }), private: incognito });
    if (!reply.ok) explain(reply.why);
  }
});

// ---- install, start, the toolbar popup ------------------------------------------------------

ext.runtime.onInstalled.addListener(async (details) => {
  createMenus();
  // Hello: the app learns the extension is there (its offer window says Connected).
  const r = await refreshRules();
  if (details.reason === "install" && !r) {
    const { why } = await readStatus();
    explain(why || "not-installed");
  }
  injectOpenTabs();
});

ext.runtime.onStartup.addListener(() => {
  refreshRules();
  showStatus();
});

// Pages open before the extension was installed or updated get its scripts now (else the
// button waits for a reload).
async function injectOpenTabs() {
  if (!ext.scripting) return;
  const tabs = await ext.tabs.query({ url: ["http://*/*", "https://*/*"] }).catch(() => []);
  // A match pattern's host and path as a test (`*://*.youtube.com/*`).
  const matcher = (pattern) => {
    const [, host = "", path = "/*"] = /^[^:]+:\/\/([^/]*)(\/.*)?$/.exec(pattern) || [];
    const glob = (s) => s.split("*").map((x) => x.replace(/[.+?^${}()|[\]\\]/g, "\\$&")).join(".*");
    const h = host.startsWith("*.") ? new RegExp(`^(.*\\.)?${glob(host.slice(2))}$`, "i") : new RegExp(`^${glob(host)}$`, "i");
    const pth = new RegExp(`^${glob(path)}$`);
    return (u) => h.test(u.hostname) && pth.test(u.pathname + u.search);
  };
  for (const entry of ext.runtime.getManifest().content_scripts || []) {
    const excludes = (entry.exclude_matches || []).map(matcher);
    for (const tab of tabs) {
      let u;
      try {
        u = new URL(tab.url);
      } catch {
        continue;
      }
      if (excludes.some((m) => m(u))) continue;
      ext.scripting
        .executeScript({ target: { tabId: tab.id, allFrames: Boolean(entry.all_frames) }, files: entry.js })
        .catch(() => {});
    }
  }
}

// ---- messages from the page (video.js, page.js) and the popup (popup.js) -------------------

// The download button over videos asks: may it show on this page, the button was clicked, its
// menu. The app decides through the native host; its answer for a site is kept a minute (one
// question per page load at most). A failure to reach it isn't kept.
const panelCache = new Map();
async function panelOn(page) {
  // The host name without a port: as video.js names the site, and as the app matches sites.
  const site = new URL(page).hostname;
  forget(panelCache, 60000);
  const kept = panelCache.get(site);
  if (kept) return kept.on;
  const reply = await native({ panel: true, url: page });
  if (reply.ok || !reply.why) panelCache.set(site, { on: Boolean(reply.ok), at: Date.now() });
  return Boolean(reply.ok);
}

// The page the message came from, as the browser knows it (never what the page says).
function senderPage(sender) {
  const u = sender.url || (sender.tab && sender.tab.url) || "";
  return /^https?:/i.test(u) ? u : null;
}

// The page's address as it is now. A content script loaded before an extension update has no
// `page` in its message, and `sender.url` is the document's first address (YouTube changes the
// video in the same document, so it still names the previous video). Asking the frame itself
// is right for both new and old content scripts.
async function currentPage(sender) {
  const tabId = sender.tab && sender.tab.id;
  if (tabId !== undefined && tabId !== null && sender.frameId !== undefined && ext.scripting) {
    try {
      const [found] = await ext.scripting.executeScript({
        target: { tabId, frameIds: [sender.frameId] },
        func: () => location.href,
      });
      if (found && typeof found.result === "string" && /^https?:/i.test(found.result)) {
        return found.result;
      }
    } catch {
      // A frame the extension may not script (a browser page, a store): the tab's own address.
    }
  }
  return senderPage(sender);
}

// The object starting at `from` (`{`), to its matching `}`: strings and escapes are skipped,
// so a `};` inside a description doesn't end it early.
function objectAt(text, from) {
  let depth = 0;
  let quoted = false;
  let escaped = false;
  for (let i = from; i < text.length; i++) {
    const c = text[i];
    if (quoted) {
      if (escaped) escaped = false;
      else if (c === "\\") escaped = true;
      else if (c === '"') quoted = false;
      continue;
    }
    if (c === '"') {
      quoted = true;
    } else if (c === "{") {
      depth++;
    } else if (c === "}") {
      depth--;
      if (depth === 0) return text.slice(from, i + 1);
    }
  }
  return null;
}

// The qualities in a YouTube page's own player response: the page has already fetched them,
// so the window can show its list before yt-dlp's slower look-up ends. Nothing is kept: the
// page is read again for each click, and the exact list replaces this one.
async function pageHeights(page) {
  try {
    const r = await withTimeout(fetch(page, { credentials: "include" }), 5000);
    if (!r.ok) return [];
    const text = await withTimeout(r.text(), 5000);
    const at = text.indexOf("ytInitialPlayerResponse");
    if (at < 0) return [];
    const begin = text.indexOf("{", text.indexOf("=", at));
    if (begin < 0) return [];
    const raw = objectAt(text, begin);
    if (!raw) return [];
    const data = JSON.parse(raw);
    const streams = data.streamingData || {};
    const heights = new Set();
    for (const f of [...(streams.formats || []), ...(streams.adaptiveFormats || [])]) {
      const h = Number(f.height);
      if (h > 0) heights.add(h);
    }
    return [...heights].sort((a, b) => b - a);
  } catch {
    return [];
  }
}

async function pageMessage(m, sender) {
  // YouTube changes the video in the same page (a history entry, not a new document): the
  // content script's message carries the video's current address; `sender.url` can still be
  // the first video's, which made the button send video 1 after switching to video 2.
  const page =
    m.type === "video" && typeof m.page === "string" && /^https?:/i.test(m.page)
      ? m.page
      : await currentPage(sender);
  const tab = sender.tab || {};
  if (m.type === "panel") return { on: page ? await panelOn(page) : false };
  if (m.type === "media-server") {
    const r = await native({ media: true });
    return { ok: Boolean(r && r.ok), port: r && r.mediaPort, token: r && r.mediaToken };
  }
  if (m.type === "telegram-fetch" && page && isTelegramUrl(page) && sender.tab) {
    const [r] = await ext.scripting.executeScript({
      target: { tabId: sender.tab.id },
      world: "MAIN",
      func: telegramFetchInPage,
      args: [String(m.requestId || ""), String(m.src || "")],
    });
    return { ok: Boolean(r && r.result && r.result.ok) };
  }
  if (m.type === "bypass" && typeof m.url === "string") {
    bypass.set(m.url, { at: Date.now() });
    return { ok: true };
  }
  if (m.type === "telegram-download" && page && isTelegramUrl(page)) {
    // The content script does the upload (the page's media fetch + MDM's local server); this
    // path is only for an old copy of video.js. Never hand the page-only URL to the app.
    return { ok: false, why: "options" };
  }
  if (m.type === "video" && page) {
    // Telegram's page-only media cannot be fetched by the app (its endpoint answers 302); its
    // own downloader and the finished-file import are the path.
    if (isTelegramUrl(page)) return { ok: false, why: "options" };
    await mediaBack;
    const src = typeof m.src === "string" && /^https?:/i.test(m.src) ? m.src : await streamFor(sender);
    // Each cookie with the domain and flags the browser keeps, so the app files a `.youtube.com`
    // sign-in for every host of it (YouTube Music's page included); the header goes too, for an
    // older app.
    const cookieContext = await videoCookies(src || page, {
      incognito: tab.incognito,
      storeId: tab.cookieStoreId,
    });
    const reply = await native({
      video: true,
      url: page,
      ...(src ? { src } : {}),
      title: typeof m.title === "string" ? m.title.slice(0, 300) : "",
      referrer: tab.url && tab.url !== page ? tab.url : "",
      // The cookies of what yt-dlp is given (the app files them under its host): the stream's
      // own server may not be the page's, and the page's cookies are not for it.
      ...cookieContext,
      ...(tab.incognito ? { private: true } : {}),
    });
    // Sent apart so the window opens at once; its qualities follow in a second, when the
    // page's player response has been read.
    pageHeights(page).then((heights) => {
      if (heights.length) {
        native({
          video: true,
          url: page,
          ...(src ? { src } : {}),
          referrer: tab.url && tab.url !== page ? tab.url : "",
          ...cookieContext,
          heights,
        }).catch(() => {});
      }
    });
    return { ok: Boolean(reply.ok), why: reply.why };
  }
  if (m.type === "hide-site" && page) {
    const host = new URL(page).hostname;
    panelCache.set(host, { on: false, at: Date.now() });
    const reply = await native({ hideSite: host });
    return { ok: Boolean(reply.ok) };
  }
  if (m.type === "options") return { ok: Boolean((await native({ openOptions: true })).ok) };
  return { ok: false };
}

async function popupMessage(m) {
  if (m.type === "state") {
    const r = await refreshRules();
    const { paused } = await ext.storage.local.get({ paused: false });
    const s = await readStatus();
    const { declined } = await ext.storage.local.get({ declined: null });
    const remote = await remoteTarget();
    if (remote) return { connected: true, why: null, remote: remote.url, version: VERSION, paused, declined };
    return { connected: Boolean(r) && (!s.why || s.why === "update-app"), why: s.why, app: s.app, version: VERSION, paused, declined };
  }
  if (m.type === "remote-pair" && typeof m.url === "string" && typeof m.code === "string") return remotePair(m.url, m.code);
  if (m.type === "remote-off") return remoteOff();
  if (m.type === "pause") {
    await ext.storage.local.set({ paused: Boolean(m.paused) });
    await syncOpenedRule();
    await showStatus();
    return { ok: true };
  }
  if (m.type === "exclude-site" && typeof m.host === "string" && m.host) {
    const reply = await native({ excludeSite: m.host });
    await refreshRules();
    return { ok: Boolean(reply.ok) };
  }
  if (m.type === "open-app") return { ok: Boolean((await native({ open: true })).ok) };
  if (m.type === "open-options") return { ok: Boolean((await native({ openOptions: true, page: "general" })).ok) };
  // The media the popup's tab loaded (#38), and one of them handed to the app.
  if (m.type === "media" && Number.isInteger(m.tab)) return { media: await mediaList(m.tab) };
  if (m.type === "media-download" && Number.isInteger(m.tab) && typeof m.url === "string") {
    const reply = await mediaDownload(m.tab, m.url);
    return { ok: Boolean(reply.ok), why: reply.why };
  }
  return { ok: false };
}

ext.runtime.onMessage.addListener((m, sender, sendResponse) => {
  // Only our own scripts (pages can't message an extension without being allowed to).
  if (!m || typeof m.type !== "string" || sender.id !== ext.runtime.id) return false;
  // The popup, or its page opened in a tab (popup.html?page=), is one of the extension's own
  // pages; content scripts send from the web page they are in.
  const own = String(sender.url || "").startsWith(ext.runtime.getURL(""));
  const fromPage = Boolean(sender.tab) && !own;
  (fromPage ? pageMessage(m, sender) : popupMessage(m)).then(sendResponse, () => sendResponse({ ok: false }));
  return true; // answered later
});

showStatus();
// Rules kept from before, else asked now (a first start, storage cleared).
storedRules().then((r) => (r ? syncOpenedRule() : refreshRules()));
