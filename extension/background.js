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
// Safari has no downloads API at all (nor proxy): it can't take a download from the browser, so
// the links clicked in a page are handed over instead (`link`, below; page.js).
const SAFARI = !ext.downloads;
// Firefox has no pause before the file name (onDeterminingFilename): its own paths below.
const FIREFOX = !SAFARI && !ext.downloads.onDeterminingFilename;
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

// `message` as it goes to the host: with the browser and the extension's version.
async function stamped(message) {
  const app = await APP_NAME;
  const brands = app ? [...BRANDS, app] : BRANDS;
  return { ...message, userAgent: navigator.userAgent, brands, v: VERSION };
}

// A message to this computer's MDM, through its host: only ever this computer's, whatever "Send
// downloads to" says (a download for another MDM goes through `download`). Every message says
// which browser and extension version it comes from (the app keeps each browser's status, and
// the rules depend on the browser). Never throws: failures come back as `{ok: false, why}`.
async function native(message, ms = HOST_WAIT) {
  const sent = await stamped(message);
  try {
    const reply = (await withTimeout(ext.runtime.sendNativeMessage(HOST, sent), ms)) || {};
    if (reply.version) {
      await noteStatus(older(reply.version, VERSION) ? "update-app" : null, reply.version);
      // The answer to `rules` also says whether the app looks videos up for the button.
      await noteLookup(reply.videoLookup === true);
    } else if (reply.ok || reply.why === "options") await noteStatus(null);
    else if (reply.why && reply.why !== "refused") await noteStatus(reply.why);
    return reply;
  } catch (e) {
    const why = hostProblem(e);
    // With another MDM chosen, this computer's needn't be there.
    if (!(await remoteTarget())) await noteStatus(why);
    return { ok: false, why, error: String((e && e.message) || e) };
  }
}

// The kinds of message that are a download, and only these, go to another MDM when one is
// chosen (#157: its remote control's api/add, which adds the address to its list). A whitelist:
// any other message (the video button's "may it show here", the rules, a look-up, a setting, a
// window to open) is for this computer's MDM, never a download there. Pointing at videos added
// the page to the other MDM's list every few seconds while the button's question went there too.
//  - "file": a download handed over (`handOff`: the browser's own, a link page.js held, the
//    right-click menu's link, a file in the popup's media list);
//  - "video": Download video asked for (the button's click, the right-click menu over a player
//    or on a stream's manifest, a stream in the popup's media list).
const REMOTE_ADD = new Set(["file", "video"]);

// A download (`kind`, one of REMOTE_ADD) for MDM: to the other MDM when one is chosen, else to
// this computer's. Never throws.
async function download(kind, message) {
  if (REMOTE_ADD.has(kind) && typeof message.url === "string" && message.url) {
    const target = await remoteTarget();
    if (target) return remoteSend(target, message);
  }
  const reply = await native(message);
  // A download handed over: the toolbar button counts it while it runs (#1496).
  if (reply.ok) watchRecent();
  return reply;
}

// ---- another MDM (#157): a server's, through its remote control ------------------------------

// A list of all known paired servers: [{url, token}, ...]
async function readRemotes() {
  const { remotes, remote } = await ext.storage.local.get({ remotes: null, remote: null });
  if (Array.isArray(remotes)) return remotes.filter((r) => r && r.url && r.token);
  if (remote && remote.url && remote.token) {
    const list = [remote];
    await ext.storage.local.set({ remotes: list });
    return list;
  }
  return [];
}

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
  if (r.status === 401) {
    const list = await readRemotes();
    const updated = list.filter((item) => !(item.url === target.url && item.token === target.token));
    const { remote } = await ext.storage.local.get({ remote: null });
    const nextRemote = remote && remote.url === target.url && remote.token === target.token ? null : remote;
    await ext.storage.local.set({ remotes: updated, remote: nextRemote });
  }
  return { ok: false, why: "refused" };
}

// The file types the app takes by default (mdm_ipc::capture::DEFAULT_AUTO_TYPES; a test keeps
// them the same): what is caught for another MDM when this computer has none to ask.
const DEFAULT_TYPES =
  "00* 3DM 3DS 3GP 3MF 7Z AAB AAC ABC ACE AFDESIGN AFPHOTO AI AIF AIFF APE APK APKM APKS APPIMAGE APPX APPXBUNDLE ARJ ARROW ARW ASF AU AVI AVRO AZW AZW3 BAK BAM BIN BLEND BZ BZ2 C4D CAB CAF CBR CBZ CCD CHD CKPT CPIO CR2 CR3 CRAM CSO DAE DAT DCM DDS DEB DFF DIVX DJVU DMG DNG DOC DOCX DSF DWG DXF EPS EPUB EXE EXR F3D F4V FASTA FASTQ FB2 FBX FEATHER FIG FITS FLAC FLATPAK FQ GCODE GCZ GEM GGML GGUF GLB GLTF GPKG GRIB GRIB2 GZ GZIP H5 HDF5 HDR IGES IGS IMG INDD IPA ISO ISZ JAR JOBLIB JSONL KERAS KRA LAS LAZ LLAMAFILE LWO LZ LZ4 LZH LZMA M2T M2TS M2V M4A M4B M4V MA MAT MAX MB MBTILES MDF MDS MID MIDI MKA MKV MLMODEL MOBI MOV MP3 MP4 MPA MPE MPEG MPG MSI MSIX MSIXBUNDLE MSP MSU MTS MXF NC NDJSON NEF NEMO NII NPY NPZ NRG NSP NSZ NUPKG OBJ ODP ODS ODT OGG OGM OGV ONNX OPUS ORC ORF OTF OVA OVF PAK PARQUET PB PBF PDF PICKLE PK3 PK4 PKG PKL PLJ PLY POTX PPS PPSX PPT PPTX PSB PSD PT PTH QCOW QCOW2 QT R0* R1* R2* R3* R4* R5* R6* R7* R8* R9* RA RAF RAR RDS RM RMVB RPM RTF RUN RVZ RW2 SAFETENSORS SEA SF2 SFZ SHP SIT SITX SKETCH SKP SLDASM SLDPRT SNAP SQLITE SQLITE3 STEP STL STP TAR TBZ TBZ2 TFLITE TGA TGZ TIF TIFF TLZ TORRENT TTA TTC TTF TXZ TZST UNITYPACKAGE USD USDA USDC USDZ VDB VDI VHD VHDX VMDK VOB VPK VSIX WAV WBFS WEBM WHL WIM WMA WMV WRL WTV WUA WV X3D XAPK XCF XCI XLS XLSB XLSM XLSX XZ Z Z0* ZIP ZIPX ZPR ZST ZTL";

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
    const list = await readRemotes();
    const updated = list.filter((item) => item.url !== base);
    const item = { url: base, token: r.body.token };
    updated.push(item);
    await ext.storage.local.set({ remotes: updated, remote: item });
    await refreshRules();
    await showStatus();
    return { ok: true };
  }
  const why = { 0: "unreachable", 403: "wrong-code", 410: "no-code", 429: "too-soon" }[r.status] || "failed";
  return { ok: false, why, left: r.body && r.body.left };
}

// Switch active remote to a previously paired MDM at `url`.
async function remoteSelect(url) {
  const list = await readRemotes();
  const target = list.find((item) => item.url === url);
  if (target) {
    await ext.storage.local.set({ remote: target });
    await refreshRules();
    await showStatus();
    return { ok: true };
  }
  return { ok: false, why: "not-found" };
}

// Forget a paired MDM at `url`: unpair there (when it answers) and remove from remotes.
async function remoteForget(url) {
  const list = await readRemotes();
  const target = list.find((item) => item.url === url);
  if (target) await remoteCall(target.url, "api/forget", {}, target.token);
  const updated = list.filter((item) => item.url !== url);
  const { remote } = await ext.storage.local.get({ remote: null });
  const nextRemote = remote && remote.url === url ? null : remote;
  await ext.storage.local.set({ remotes: updated, remote: nextRemote });
  await refreshRules();
  await showStatus();
  return { ok: true };
}

// Back to this computer: active remote set to null without forgetting paired remotes.
async function remoteOff() {
  await ext.storage.local.set({ remote: null });
  await refreshRules();
  await showStatus();
  return { ok: true };
}

// ---- the popup's Downloads tab and the toolbar button's count (#1496) ------------------------

// While the popup is open, its Downloads tab gets MDM's newest downloads every second through a
// port of its own ("recent"): from this computer's MDM over one native port (one helper process
// for as long as it is open, not one per question; Safari, which has none, asks one message at
// a time), or from another MDM's remote control (#157), whose list is cut to the same view here
// (`recentView`). The toolbar button counts this computer's running downloads: watched every few
// seconds while any runs (an open native port keeps the background alive in Chromium), and no
// longer once none does. A download handed over starts the watch, as does the popup.
const RECENT_EVERY = 1000;
const BADGE_EVERY = 3000;
// How long one answer through the native port may take (the app answers within 3 s, or says it
// is busy).
const RECENT_WAIT = 6000;
// The most rows the popup asks for (the app's limit is mdm_ipc::RECENT_MAX).
const RECENT_MAX = 200;

let nativePort = null;
// Resolvers of the port's questions, in the order asked: the helper answers each in turn.
let hostWaiting = [];
// The popup's ports, each with what it asks: {port, ask, sent: icon keys it has}.
const recentPorts = new Set();
let recentTimer = null;
let recentBusy = false;
// The file type icons, by key, at `iconPx` pixels (the popup's size for them on this screen).
let icons = new Map();
let iconPx = 0;
// Downloads running in this computer's MDM, as last heard (the toolbar button's count).
let runningNow = 0;

// One question to the helper through the native port, opened when needed; never throws.
function hostAsk(message) {
  if (SAFARI || !ext.runtime.connectNative) return nativeQuietly(message);
  return new Promise(async (resolve) => {
    if (!nativePort) {
      try {
        nativePort = ext.runtime.connectNative(HOST);
      } catch (e) {
        resolve({ ok: false, why: hostProblem(e) });
        return;
      }
      const port = nativePort;
      port.onMessage.addListener((reply) => {
        const next = hostWaiting.shift();
        if (next) next(reply || {});
      });
      port.onDisconnect.addListener(() => {
        const e = ext.runtime.lastError || port.error;
        if (nativePort === port) nativePort = null;
        const why = e ? hostProblem(e) : "not-running";
        for (const r of hostWaiting.splice(0)) r({ ok: false, why });
      });
    }
    let done = false;
    const timer = setTimeout(() => {
      if (done) return;
      done = true;
      // A helper that stopped answering: started again for the next question.
      const i = hostWaiting.indexOf(finish);
      if (i >= 0) hostWaiting.splice(i, 1);
      closeHostPort();
      resolve({ ok: false, why: "not-running" });
    }, RECENT_WAIT);
    const finish = (reply) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      resolve(reply);
    };
    hostWaiting.push(finish);
    try {
      nativePort.postMessage(await stamped(message));
    } catch (e) {
      finish({ ok: false, why: hostProblem(e) });
    }
  });
}

// One message to the helper, by itself (Safari), with nothing noted of the browser's status: the
// list's answers carry no version (`native` would take the browser for connected to an app that
// needs no update), and say whether the app runs in their own words.
async function nativeQuietly(message) {
  try {
    return (await withTimeout(ext.runtime.sendNativeMessage(HOST, await stamped(message)), RECENT_WAIT)) || {};
  } catch (e) {
    return { ok: false, why: hostProblem(e) };
  }
}

function closeHostPort() {
  const port = nativePort;
  nativePort = null;
  for (const r of hostWaiting.splice(0)) r({ ok: false, why: "not-running" });
  try {
    if (port) port.disconnect();
  } catch {
    // Already gone.
  }
}

// A download's place in the popup's filters (mdm_ipc::api::recent_filter).
function recentFilterOf(status) {
  return { Downloading: "running", Complete: "done", Error: "failed" }[status] || null;
}

// The popup's view of a whole list (another MDM's, whose remote control lists every download):
// the same as the app's (mdm_ipc::api::RecentAnswer::of; a test keeps them alike): every word
// of the search in the file's name or its address, the filter, newest first, the limit; the
// counts of each filter for the search, the running ones whatever the search.
function recentView(downloads, ask, speed = 0) {
  const list = Array.isArray(downloads) ? downloads : [];
  const words = String(ask.search || "").toLowerCase().split(/\s+/).filter(Boolean);
  const found = (d) => {
    const file = String(d.file || "").toLowerCase();
    const url = String(d.url || "").toLowerCase();
    return words.every((w) => file.includes(w) || url.includes(w));
  };
  const running = list.filter((d) => recentFilterOf(d.status) === "running").length;
  const matching = list.filter(found);
  const counts = { all: matching.length, running: 0, done: 0, failed: 0 };
  for (const d of matching) {
    const f = recentFilterOf(d.status);
    if (f) counts[f]++;
  }
  const filter = ["running", "done", "failed"].includes(ask.filter) ? ask.filter : null;
  const rows = (filter ? matching.filter((d) => recentFilterOf(d.status) === filter) : matching)
    .slice()
    .sort((a, b) => b.id - a.id)
    .slice(0, Math.max(0, Math.min(Number(ask.limit) || 0, RECENT_MAX)));
  return { downloads: rows, counts, running, speed, icons: {}, share: false };
}

// The question as the helper takes it: within bounds, with the icons already here.
function recentQuestion(ask) {
  const px = Math.min(64, Math.max(16, Math.round(Number(ask.px) || 24)));
  if (px !== iconPx) {
    icons = new Map();
    iconPx = px;
  }
  return {
    filter: ["all", "running", "done", "failed"].includes(ask.filter) ? ask.filter : "all",
    search: String(ask.search || "").slice(0, 200),
    limit: Math.max(0, Math.min(Number(ask.limit) || 0, RECENT_MAX)),
    have: [...icons.keys()],
    px,
  };
}

// A GET of the remote control's API (its list); {status, body} as remoteCall.
async function remoteGet(base, path, token) {
  try {
    const r = await withTimeout(
      fetch(new URL(path, base), {
        headers: { Authorization: `Bearer ${token}` },
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

// The popup's list for `ask`: {ok, list, remote} or {ok: false, appState, why, remote}.
async function recentList(ask) {
  const target = await remoteTarget();
  if (target) {
    const r = await remoteGet(target.url, "api/list", target.token);
    if (r.status === 200 && r.body && Array.isArray(r.body.downloads)) {
      return { ok: true, remote: target.url, list: recentView(r.body.downloads, ask, r.body.speed || 0) };
    }
    return { ok: false, remote: target.url, why: r.status === 401 ? "not-paired" : "unreachable" };
  }
  const reply = await hostAsk({ recent: recentQuestion(ask) });
  if (reply.ok && reply.recent) {
    // Answered: the helper and the app are there (an update the app needs is still said).
    const { why } = await readStatus();
    if (why && why !== "update-app") await noteStatus(null);
    for (const [key, url] of Object.entries(reply.recent.icons || {})) {
      if (typeof url === "string" && /^data:image\/(png|svg\+xml);base64,/.test(url)) icons.set(key, url);
    }
    await noteRunning(Number(reply.recent.running) || 0);
    return { ok: true, remote: null, list: reply.recent };
  }
  // Not answered: MDM not running (`off`), older than the list (`old`: an older helper refuses
  // the question), busy, or not reached (`why`).
  const old = reply.why === "refused";
  if (reply.why && !old) await noteStatus(reply.why);
  await noteRunning(0);
  return { ok: false, remote: null, appState: reply.appState || (old ? "old" : null), why: old ? null : reply.why || null };
}

// Ask again now, then every RECENT_EVERY while a popup is open, every BADGE_EVERY while this
// computer's MDM runs a download; else the watch ends (and the helper with it).
function watchRecent(now = true) {
  if (recentTimer) clearTimeout(recentTimer);
  recentTimer = setTimeout(recentTick, now ? 0 : recentPorts.size ? RECENT_EVERY : BADGE_EVERY);
}

async function recentTick() {
  recentTimer = null;
  if (recentBusy) return watchRecent(false);
  recentBusy = true;
  try {
    if (recentPorts.size) {
      for (const p of [...recentPorts]) {
        const got = await recentList(p.ask);
        if (!recentPorts.has(p)) continue;
        const fresh = {};
        for (const [k, v] of icons) {
          if (!p.sent.has(k)) {
            fresh[k] = v;
            p.sent.add(k);
          }
        }
        try {
          p.port.postMessage({ type: "list", ...got, icons: fresh, px: iconPx });
        } catch {
          recentPorts.delete(p);
        }
      }
    } else if (!(await remoteTarget())) {
      await recentList({ limit: 0 });
    }
  } finally {
    recentBusy = false;
  }
  if (recentPorts.size || (runningNow > 0 && !(await remoteTarget()))) watchRecent(false);
  else closeHostPort();
}

// One of the popup's buttons on a download: here, or on the other MDM (what its remote control
// takes: pause, start, remove).
async function recentAct(act, id) {
  const target = await remoteTarget();
  let reply;
  if (target) {
    const action = { pause: "pause", resume: "start", remove: "remove" }[act];
    if (!action) return { ok: false, why: "unsupported" };
    const r = await remoteCall(target.url, "api/act", { action, ids: [id] }, target.token);
    const done = r.status === 200 && r.body && Array.isArray(r.body.done) && r.body.done.includes(id);
    reply = done ? { ok: true } : { ok: false, why: r.status === 0 ? "unreachable" : "cant" };
  } else {
    const r = await hostAsk({ downloadAct: { act, id: Number(id) || 0 } });
    reply = r.act ? { ok: Boolean(r.act.done), why: r.act.why || null } : { ok: false, why: r.appState || r.why || "failed" };
  }
  watchRecent();
  return reply;
}

// The toolbar button's count, kept for the browser's next start of this script (storage.session:
// memory only, gone with the browser), where the watch goes on while it is above 0.
async function noteRunning(n) {
  if (n === runningNow) return;
  runningNow = n;
  await showStatus();
  if (ext.storage.session) await ext.storage.session.set({ running: n }).catch(() => {});
}

ext.runtime.onConnect.addListener((port) => {
  const sender = port.sender || {};
  // Only the popup (or its page in a tab of its own): never a page's script.
  if (port.name !== "recent" || sender.id !== ext.runtime.id || !String(sender.url || "").startsWith(ext.runtime.getURL(""))) return;
  const p = { port, ask: { filter: "all", search: "", limit: 60, px: 24 }, sent: new Set() };
  recentPorts.add(p);
  port.onMessage.addListener((m) => {
    if (!m || m.type !== "ask") return;
    p.ask = { ...p.ask, ...(m.ask || {}) };
    watchRecent();
  });
  port.onDisconnect.addListener(() => {
    recentPorts.delete(p);
    if (!recentPorts.size) watchRecent(false);
  });
  watchRecent();
});

// a < b, both "x.y.z".
// Is version `a` an older release than `b`? Only the first two numbers count: the store can publish
// a bug-fix add-on before its app is out, and nothing it offers needs that app.
function older(a, b) {
  const pa = String(a).split(/[.-]/).map(Number);
  const pb = String(b).split(/[.-]/).map(Number);
  for (let i = 0; i < 2; i++) {
    if ((pa[i] || 0) !== (pb[i] || 0)) return (pa[i] || 0) < (pb[i] || 0);
  }
  return false;
}

// ---- status: the toolbar button's badge and title, the popup's first line ------------------

let status = null; // {why, app, at, videoLookup}
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
  status = { ...now, why, app: app || now.app || null, at: Date.now() };
  await ext.storage.local.set({ status });
  await showStatus();
  // Unreachable, PDFs and music open in the browser again (and back once connected).
  if (was !== (!why || why === "update-app")) await syncOpenedRule();
}

// Whether this computer's MDM looks videos up for the button over videos (its qualities listed in
// the page, video.js), kept with the status beside the app's version: from every answer to
// `rules`, and dropped when a lookup's answer says otherwise (an older host or app).
async function noteLookup(on) {
  const now = await readStatus();
  if (Boolean(now.videoLookup) === on) return;
  status = { ...now, videoLookup: on };
  await ext.storage.local.set({ status });
}

// The badge: "!" while MDM can't be reached, "II" while paused, else how many downloads run in
// this computer's MDM (#1496), in MDM's blue.
async function showStatus() {
  const remote = await remoteTarget();
  const why = remote ? null : (await readStatus()).why;
  const { paused } = await ext.storage.local.get({ paused: false });
  const running = !remote && !why && !paused && runningNow > 0 ? (runningNow > 99 ? "99+" : String(runningNow)) : "";
  try {
    await action.setBadgeBackgroundColor({ color: why ? "#C42B1C" : running ? "#0078D4" : "#6B6B6B" });
    if (action.setBadgeTextColor) await action.setBadgeTextColor({ color: "#FFFFFF" }).catch(() => {});
    await action.setBadgeText({ text: why && why !== "update-app" ? "!" : paused ? "II" : running });
    const title = why ? t(`status_${why.replace(/-/g, "_")}`) : paused ? t("status_paused") : t("status_connected");
    await action.setTitle({ title: running ? `${title}\n${t("status_running", running)}` : title });
  } catch {
    // No toolbar button (a browser without one): the popup and the help page still say it.
  }
}

// ---- the rules: Options in the app, kept here between the browser's stops ------------------

// {types, capture, captureOff, excludedSites, excludedAddresses, opened, minKb, torrents, at};
// null until first asked. `captureOff`: why `capture` is false (a host before #208 doesn't say).
// `torrents`: false while the app's Torrent settings leave .torrent files to the browser (#1058;
// a host before it doesn't say, and takes them as listed).
let rules = null;

async function storedRules() {
  if (!rules) rules = (await ext.storage.local.get({ rules: null })).rules;
  return rules;
}

// One question at a time: callers meanwhile get its answer.
let asking = null;
// When Safari's page.js last had them asked for (once a minute at most, from every page).
let rulesAskedAt = 0;
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
      torrents: reply.torrents !== false,
      at: Date.now(),
    };
    await ext.storage.local.set({ rules });
    await syncOpenedRule();
    return rules;
  }
  // No MDM on this computer, another one chosen: the app's default file types (the browser
  // keeps the files it would open).
  if (await remoteTarget()) {
    rules = { types: DEFAULT_TYPES, capture: true, excludedSites: "", excludedAddresses: [], opened: false, minKb: 0, torrents: true, at: Date.now() };
    // Kept as the app's are: Safari's page.js holds links by the stored copy.
    await ext.storage.local.set({ rules });
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

// A .torrent, as the app tells one (mdm_ipc::capture::is_torrent): its name or an address's
// path ends in .torrent, or its type is application/x-bittorrent.
function isTorrent(name, urls, mime) {
  const ends = (s) => /\.torrent$/i.test(s || "");
  const path = (u) => {
    try {
      return new URL(u).pathname;
    } catch {
      return "";
    }
  };
  return ends(name) || urls.some((u) => ends(path(u))) || /^\s*application\/x-bittorrent\s*(;|$)/i.test(mime || "");
}

function extensionOf(name) {
  const base = name.split(/[\\/]/).pop() || "";
  const dot = base.lastIndexOf(".");
  return dot > 0 ? base.slice(dot + 1) : "";
}

// As the app matches them (mdm_ipc::capture): a pattern's `*` is any run of characters.
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

// A site pattern against `host`, as the app reads one (mdm_ipc::capture::panel_site_matches):
// `*` matches anything (#759), and `*.example.com` the domain too; `example.com.` (the root's
// dot) is `example.com`.
function siteMatches(host, pattern) {
  const p = pattern.trim().toLowerCase().replace(/^[^/]*:\/\//, "").split("/")[0].replace(/\.+$/, "");
  if (!p) return false;
  host = String(host).toLowerCase().replace(/\.+$/, "");
  return (p.startsWith("*.") && wildcard(p.slice(2), host)) || wildcard(p, host);
}

// Whether `host` is on a site of Options' list (mdm_ipc::capture::site_listed): a name without
// `*` takes its subdomains too, as the Download panels' list does (#1049).
function siteListed(r, host) {
  return r.excludedSites.split(/\s+/).some((p) => p && (siteMatches(host, p) || (!p.startsWith("*") && siteMatches(host, `*.${p}`))));
}

// Which of Options' lists leaves `url` to the browser: "site", "address", or "" (neither).
function excludedBy(r, url) {
  let host = "";
  try {
    host = new URL(url).hostname.toLowerCase();
  } catch {
    return "site";
  }
  if (siteListed(r, host)) return "site";
  if (r.excludedAddresses.some((p) => addressMatches(p, url))) return "address";
  return "";
}

// Whether the page a download came from (its referrer) is on a site of Options' list: a file the
// site serves from another host (a CDN, a mirror) is left to the browser too (#1049). No page
// known: its own address decides.
function pageListed(r, page) {
  if (!/^https?:\/\//i.test(page || "")) return false;
  try {
    return siteListed(r, new URL(page).hostname.toLowerCase());
  } catch {
    return false;
  }
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
// The Referer each page load was sent with, by request (Firefox, two minutes).
const referrers = new Map();

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

// The loads a browser shows a file in: a tab, a frame, an <embed> or <object> (#1087: a PDF a
// page shows in one is taken as one the tab goes to is).
const PAGE_LOADS = ["main_frame", "sub_frame", "object"];

// The page each page load was started from, by address (two minutes, #607): Firefox names it
// (`originUrl`: the page whose link, form or script asked for it); Chromium gives only the tab it
// loads in (and `initiator`, the site alone). That tab is looked at as the load starts
// (`tabPage`), not once its answer has turned out to be a download: by then a link opened in a
// tab of its own (a file host's Download button) has had that tab closed by the browser, and the
// page was lost to the Referer, cut to the site's address (a file page on a host's /file/… gave
// only the host's home, its download coming from another of its servers).
const startedFrom = new Map();

// The web page tab `tabId` shows, else (a tab just opened for a link, still blank) the one of the
// tab that opened it; null when neither shows one (`pageOf` checks it isn't the file itself).
async function tabPage(tabId) {
  const web = (u) => typeof u === "string" && /^https?:\/\//i.test(u);
  try {
    const tab = await ext.tabs.get(tabId);
    if (web(tab.url)) return { url: tab.url, incognito: Boolean(tab.incognito) };
    if (tab.openerTabId >= 0) {
      const opener = await ext.tabs.get(tab.openerTabId);
      if (web(opener.url)) return { url: opener.url, incognito: Boolean(opener.incognito) };
    }
  } catch {
    // Gone already: `pageOf` looks again.
  }
  return null;
}

if (ext.webRequest) {
  const navigations = { urls: ["<all_urls>"], types: PAGE_LOADS };
  ext.webRequest.onBeforeRequest.addListener(
    (d) => {
      forget(startedFrom, 120000);
      if (!startedFrom.has(d.url)) {
        const page = d.originUrl || d.documentUrl || "";
        const seen = { page, tabId: d.tabId, at: Date.now() };
        // No page named (Chromium): its tab's, now, before the load can close or leave it.
        if (!page && d.tabId >= 0) seen.tab = tabPage(d.tabId);
        startedFrom.set(d.url, seen);
      }
      if (d.method !== "POST") return;
      forget(posts, 120000);
      posts.set(d.url, { ...formBody(d), at: Date.now(), id: d.requestId });
    },
    navigations,
    ["requestBody"],
  );
  // The Content-Type the form went with (a multipart boundary must stay the same), and (Firefox)
  // the page a load came from, for a download caught from its answer (`headersReceived`).
  ext.webRequest.onBeforeSendHeaders.addListener(
    (d) => {
      const header = (n) => (d.requestHeaders || []).find((h) => h.name.toLowerCase() === n);
      if (FIREFOX) {
        forget(referrers, 120000);
        const ref = header("referer");
        if (ref && ref.value) referrers.set(d.requestId, { url: ref.value, at: Date.now() });
      }
      if (d.method !== "POST") return;
      const p = posts.get(d.url);
      const ct = header("content-type");
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
    { urls: ["<all_urls>"], types: PAGE_LOADS },
  );
}

// Pages the browser loaded in a tab or frame (two minutes), by address (the listener below).
const loadedPages = new Map();

// The address a download was first asked for: the browser's own (Chromium), else the start of
// the redirects that led to it. Chromium also counts the page a script or a meta refresh left
// from as a download's start (#374: a thank-you page setting location.href to the installer);
// that page isn't the file, and asking it again would save the page under the file's name.
function firstUrl(item) {
  if (item.finalUrl && item.finalUrl !== item.url) {
    const page = loadedPages.get(item.url);
    return page && Date.now() - page.at < 120000 ? item.finalUrl : item.url;
  }
  const r = redirected.get(item.url);
  return r && Date.now() - r.at < 120000 ? r.from : item.url;
}

// Links clicked with Alt (Option on a Mac) held (page.js): the browser keeps their download. By
// the link's address (the click's own download, the start of a redirect), and once per click the
// download that follows it under another address (#332): one whose referrer is the link's own
// page (a bot check's page that sends to the file once its script ran, #1494), or, in the next
// seconds, one in the same tab (Firefox says which tab a page load is in) or whose referrer is the
// page the link was on (a page's script that starts the download, a link to a page whose script
// does).
const bypass = new Map(); // address -> {at, tab, page, used}
// The page a link opens may be a site's bot check ("Just a moment...", Cloudflare's): its script
// runs in the tab (some want a box ticked) before the file comes. The link's own address, and a
// download its page starts, stay the browser's that long; anything else only BYPASS_MS.
const BYPASS_CHECK_MS = 60000;
const BYPASS_MS = 10000;

// `clicked`: the download is a link the user just clicked without Alt (Safari's `link`), never
// one that follows an Alt+click.
function altClicked(item, url, first, clicked = false) {
  forget(bypass, BYPASS_CHECK_MS);
  const exact = [item.url, url, first].map((u) => u && bypass.get(u)).find(Boolean);
  if (exact) {
    exact.used = true;
    return true;
  }
  if (clicked) return false;
  const ref = item.referrer || "";
  const now = Date.now();
  for (const [link, b] of bypass) {
    if (b.used) continue;
    const soon = now - b.at <= BYPASS_MS;
    const tab = soon && Number.isInteger(item.tabId) && item.tabId >= 0 && item.tabId === b.tab;
    if (tab || linkPage(ref, link) || (soon && (fromPage(ref, link) || fromPage(ref, b.page)))) {
      b.used = true;
      return true;
    }
  }
  return false;
}

// Whether `ref` (a download's referrer) is the page at `link` itself: the same address, give or
// take what its own script added to its query (Cloudflare's check puts its token there) and its
// #part. Never only the site's address: that may be any of its pages.
function linkPage(ref, link) {
  if (!ref || !link) return false;
  try {
    const r = new URL(ref);
    const l = new URL(link);
    if (r.origin !== l.origin || r.pathname !== l.pathname) return false;
    for (const [k, v] of l.searchParams) if (!r.searchParams.getAll(k).includes(v)) return false;
    return true;
  } catch {
    return false;
  }
}

// Whether `ref` (a download's referrer) names `page`: the same address without its #part, or
// only its site's, as browsers cut a referrer sent to another site by default.
function fromPage(ref, page) {
  if (!ref || !page) return false;
  try {
    const r = new URL(ref);
    const p = new URL(page);
    p.hash = "";
    r.hash = "";
    return r.href === p.href || (r.pathname === "/" && !r.search && r.origin === p.origin);
  } catch {
    return false;
  }
}

// ---- the media a tab loaded (#38) ------------------------------------------------------------

// What the tab's pages loaded that can be downloaded, per tab, in the order seen: streams'
// manifests (HLS .m3u8, DASH .mpd; a master playlist comes before its variants) and plain video
// and sound files (a <video>'s or <audio>'s own file, or a large whole file a player fetched). The
// popup lists them (`mediaList`); the download button over a video sends the stream its frame
// loaded first (`streamOf`): a player that plays from a blob: has no address of its own, and
// yt-dlp reads the manifest (qualities, separate audio). Segments and byte ranges are pieces of a
// stream, never listed. Bounded: MEDIA_MAX per tab, MEDIA_TABS tabs; forgotten when the tab goes
// to another page or closes.
const media = new Map(); // tab id -> {page, items: [{url, kind, frameId, mime, size, at, info}], ranged, maybe, frames}
const MEDIA_MAX = 60;
// Whole files a player fetched in ranges (#1092), kept per tab apart from the list.
const RANGED_MAX = 40;
const MEDIA_TABS = 50;
const URL_MAX = 8192;
// A player's own fetch of a whole file is listed from this size (smaller ones are pieces).
const FETCHED_MIN = 1 << 20;
// Less than this of a single file, played by the page itself: a page's sound effect (mediaKind).
const SOUND_MIN = 64 << 10;
const MANIFEST = /\.(m3u8|mpd)(?:[?#]|$)/i;
const SEGMENT_TYPE = /^(video\/mp2t|video\/iso\.segment|audio\/iso\.segment)$/;
const SEGMENT_EXT = /\.(ts|m4s|m4f|cmfv|cmfa|cmft)$/;
const MEDIA_EXT = /\.(mp4|m4v|webm|mkv|mov|avi|wmv|flv|3gp|ogv|mp3|m4a|aac|ogg|oga|opus|flac|wav|wma)$/;
const AUDIO_EXT = /^(mp3|m4a|aac|oga|ogg|opus|flac|wav|wma)$/;
// The types a manifest is sometimes served as, besides its own (#171): plain text, no type, a
// file's, XML. Such an answer is only noted (`maybe`, MAYBE_MAX a tab), never listed unread.
const MAYBE_TYPE = /^(text\/plain|(application|binary)\/octet-stream|(application|text)\/xml|)$/;
const MAYBE_MAX = 20;
// A manifest's own types: HLS's (Apple's and the x- ones), DASH's (and its older video/ name).
const MANIFEST_TYPE = /mpegurl|dash\+xml|dash\.mpd/;
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
  // The address says manifest, the type says it isn't: a page or JSON that names one (SVT Play's
  // switcher answers .../dash-full.mpd with JSON naming the real manifest; loaded first, it went
  // to the app and yt-dlp found no video in it, #377). Read before believed, like any `maybe`.
  if (/\.(m3u8|mpd)$/.test(path) && !MANIFEST_TYPE.test(mime) && !MAYBE_TYPE.test(mime)) {
    return Number(headerOf(d, "content-length")) > MANIFEST_MAX ? null : { kind: "maybe", mime, size: 0 };
  }
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
  // A page's own short sound (a click, a voice-search chime: 6 kB on YouTube in Edge), not media
  // anyone came for: the list would show nothing else on a page whose video is streamed.
  const whole = Number(headerOf(d, "content-length"));
  if (d.type === "media" && whole > 0 && whole < SOUND_MIN) return null;
  const typed = /^(video|audio)\//.test(mime) || mime === "application/ogg";
  const untyped = !mime || /^(application|binary)\/octet-stream$/.test(mime);
  if (!typed && !(ext && untyped)) return null;
  // The whole file's size: a range's total, else the length of a whole answer.
  const range = headerOf(d, "content-range");
  const total = /\/\s*(\d+)\s*$/.exec(range);
  const size = total ? Number(total[1]) : d.statusCode === 206 ? 0 : Number(headerOf(d, "content-length")) || 0;
  const audio = mime.startsWith("audio/") || (!mime.startsWith("video/") && AUDIO_EXT.test(ext));
  // A <video>'s or <audio>'s own file comes in ranges; a player's fetch of a range, or of a
  // small file, is a piece of a stream.
  const sliced = /[?&](range|bytestart|byterange)=/i.test(d.url);
  if (d.type !== "media" && (d.statusCode === 206 || range || sliced || size < FETCHED_MIN)) {
    // #1092: a whole file (its total in Content-Range) a player fetches in ranges for its
    // MediaSource, as Douyin's does: kept apart (`ranged`), never listed, since it may be one of
    // a video's tracks alone. The button offers it when its page can't be looked up and the
    // file's own header says it is the video under it (`playedChoice`).
    if (total && size >= SOUND_MIN && !sliced) return { kind: audio ? "audio" : "video", mime, size, ranged: true };
    return null;
  }
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

function noteMedia(d, found, sent) {
  const tab = media.get(d.tabId) || { page: null, items: [] };
  const list = found.ranged ? (tab.ranged = tab.ranged || []) : tab.items;
  const old = list.find((it) => it.url === d.url);
  if (old) {
    if (found.size && !old.size) old.size = found.size;
    if (sent) old.sent = sent;
    return;
  }
  // A file in a stream's own folder is one of its pieces (a DASH init segment, a fragment).
  const inStream = (it) => isStream(it) && folderOf(it.url) && d.url.startsWith(folderOf(it.url));
  if (!isStream(found) && tab.items.some(inStream)) return;
  // `at`: when the tab first got it (#1092: what a feed's player loaded while a video played).
  const item = { url: d.url, kind: found.kind, frameId: d.frameId || 0, mime: found.mime, size: found.size, at: Date.now(), ...(sent ? { sent } : {}) };
  if (found.ranged) {
    // The newest RANGED_MAX, apart from the list (a feed's player fetches many).
    if (tab.items.some((it) => it.url === d.url)) return;
    list.push(item);
    if (list.length > RANGED_MAX) list.splice(0, list.length - RANGED_MAX);
    if (!media.has(d.tabId)) {
      media.set(d.tabId, tab);
      for (const id of media.keys()) {
        if (media.size <= MEDIA_TABS) break;
        media.delete(id);
      }
    }
    keepMedia();
    return;
  }
  tab.items.push(item);
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
function noteMaybe(d, found, sent) {
  const tab = media.get(d.tabId) || { page: null, items: [] };
  if (tab.items.some((it) => it.url === d.url)) return;
  tab.maybe = (tab.maybe || []).filter((m) => m.url !== d.url);
  tab.maybe.push({ url: d.url, frameId: d.frameId || 0, mime: found.mime, ...(sent ? { sent } : {}) });
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
  // An XML declaration, comments, then the MPD element (with a namespace prefix or not), read
  // by a plain scan: each part is skipped once, whatever the text holds.
  let rest = t;
  if (rest.startsWith("<?xml")) {
    const end = rest.indexOf("?>");
    if (end < 0) return "";
    rest = rest.slice(end + 2).trimStart();
  }
  while (rest.startsWith("<!--")) {
    const end = rest.indexOf("-->", 4);
    if (end < 0) return "";
    rest = rest.slice(end + 3).trimStart();
  }
  return /^<(?:[\w-]+:)?MPD[\s>/]/.test(rest) ? "dash" : "";
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
      if (kinds[i]) noteMedia({ tabId, url: m.url, frameId: m.frameId }, { kind: kinds[i], mime: m.mime, size: 0 }, m.sent);
    });
    keepMedia();
  };
  const next = (sniffing.get(tabId) || Promise.resolve()).then(run, run);
  sniffing.set(tabId, next);
  next.finally(() => sniffing.get(tabId) === next && sniffing.delete(tabId)).catch(() => {});
  return next;
}

// #900: what the browser sent with each of a tab's requests for media and data (its Referer, Origin
// and Cookie headers), by request, until its answer starts: a stream's stay with it, and go to the
// app with it, which asks for the stream, its playlists and each of its pieces as the page's
// player did. A site's video server may serve its own pages' players only (their Origin and
// Referer), and a player in another site's frame may have cookies no store lists (a browser that
// keeps a frame's cookies apart, for that page only). In memory only, as long as `media`.
const SENT_HEADERS = ["referer", "origin", "cookie"];
const sentWith = new Map();
const SENT_MAX = 512;
const SENT_TYPES = { urls: ["<all_urls>"], types: ["xmlhttprequest", "media", "other"] };
if (ext.webRequest && ext.webRequest.onSendHeaders) {
  try {
    ext.webRequest.onSendHeaders.addListener(
      (d) => {
        if (d.tabId < 0) return;
        const sent = {};
        for (const h of d.requestHeaders || []) {
          const name = String(h.name).toLowerCase();
          if (SENT_HEADERS.includes(name) && typeof h.value === "string" && h.value) sent[name] = h.value;
        }
        // A redirect's next request is the one answered.
        sentWith.delete(d.requestId);
        if (Object.keys(sent).length) sentWith.set(d.requestId, sent);
        // Requests never answered (cancelled, failed before an answer) don't pile up.
        while (sentWith.size > SENT_MAX) sentWith.delete(sentWith.keys().next().value);
      },
      SENT_TYPES,
      // Chromium shows a request's Referer, Origin and Cookie to extensions that ask for its
      // extra headers only; Firefox shows them all and knows no such option.
      FIREFOX ? ["requestHeaders"] : ["requestHeaders", "extraHeaders"],
    );
  } catch {
    // A browser without extraHeaders: streams go with their page as the Referer, as before.
  }
}
const takeSent = (requestId) => {
  const sent = sentWith.get(requestId);
  sentWith.delete(requestId);
  return sent;
};
if (ext.webRequest && ext.webRequest.onErrorOccurred) {
  ext.webRequest.onErrorOccurred.addListener((d) => sentWith.delete(d.requestId), SENT_TYPES);
}

if (ext.webRequest && ext.webRequest.onResponseStarted) {
  ext.webRequest.onResponseStarted.addListener(
    (d) => {
      if (d.type === "main_frame" || d.type === "sub_frame") {
        const type = (d.responseHeaders || []).find((h) => h.name.toLowerCase() === "content-type");
        if (d.statusCode === 200 && type && /^\s*(text\/html|application\/xhtml)/i.test(type.value || "")) {
          forget(loadedPages, 120000);
          loadedPages.set(d.url, { at: Date.now() });
        }
        return;
      }
      const sent = takeSent(d.requestId);
      const found = mediaKind(d);
      if (found) mediaBack.then(() => (found.kind === "maybe" ? noteMaybe(d, found, sent) : noteMedia(d, found, sent)));
    },
    { urls: ["<all_urls>"], types: ["main_frame", "sub_frame", "xmlhttprequest", "media", "other"] },
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
    // #1092: the files a player fetched in ranges stay while the tab stays on the site: a feed
    // moves to another address as it shows its first video (Douyin's goes from /jingxuan to
    // /?recommend=1), after its player fetched it. Offered only for a video of their length.
    const site = (u) => (/^https?:\/\/[^/]+/i.exec(u || "") || [""])[0].toLowerCase();
    const ranged = tab && tab.ranged && (!tab.page || site(tab.page) === site(page)) ? tab.ranged : null;
    media.set(id, { page, items: [], ...(ranged ? { ranged } : {}) });
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
          const ranged = [...(kept.ranged || []), ...((now && now.ranged) || [])].filter((m, i, all) => all.findIndex((o) => o.url === m.url) === i);
          media.set(id, {
            ...kept,
            page: (now && now.page) || kept.page,
            items: [...kept.items, ...fresh].slice(-MEDIA_MAX),
            maybe: maybe.slice(-MAYBE_MAX),
            ranged: ranged.slice(-RANGED_MAX),
          });
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

// Whether a player in the tab plays from a blob: (a MediaSource fed by the page).
async function playsStream(tabId) {
  try {
    const found = await withTimeout(ext.scripting.executeScript({ target: { tabId, allFrames: true }, func: mediaElements }), 3000);
    return (found || []).some((f) => f && f.result && f.result.players.some((v) => /^blob:/i.test(v.src)));
  } catch {
    return false;
  }
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
  // A file's name as it will be saved (#800): the tab's title and the popup's switch name it too.
  const { mediaTitleName } = await ext.storage.local.get({ mediaTitleName: false });
  let title = "";
  try {
    title = tabTitle(await ext.tabs.get(tabId));
  } catch {
    // The tab is gone: names without its title.
  }
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
    // A stream is named in the Download video window; a file here, with its type (#800).
    const named = isStream(it) ? null : mediaName(it.url, it.mime, title, mediaTitleName).name;
    out.push({
      url: it.url,
      kind: it.kind,
      mime: it.mime,
      ...(named ? { name: named, ext: splitExtension(named)[1].toLowerCase() } : {}),
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

// ---- what the tab played, for the video under the button (#1092) -------------------------------
// When the page can't be looked up (a feed, a post yt-dlp can't read, a site it doesn't know), the
// button offers what the tab played for that video: a file or a stream of its frame whose length
// is the video's. A file's length and tracks come from its own header, read as the player read it.

// `len` bytes of `url` from `start` (fewer at its end), with the site's cookies as `probe` asks;
// null when refused, too slow, or (a server ignoring Range) not at the start.
async function bytesAt(url, start, len) {
  let reader = null;
  try {
    const r = await withTimeout(fetch(url, { credentials: "include", headers: { Range: `bytes=${start}-${start + len - 1}` } }), 5000);
    if (!r.ok || (r.status !== 206 && start > 0) || !r.body || !r.body.getReader) return null;
    reader = r.body.getReader();
    const parts = [];
    let got = 0;
    while (got < len) {
      const { done, value } = await withTimeout(reader.read(), 5000);
      if (done) break;
      parts.push(value);
      got += value.length;
    }
    const out = new Uint8Array(Math.min(got, len));
    let at = 0;
    for (const p of parts) {
      out.set(p.subarray(0, Math.min(p.length, out.length - at)), at);
      at += p.length;
      if (at >= out.length) break;
    }
    return out;
  } catch {
    return null;
  } finally {
    // The rest is never read.
    if (reader) reader.cancel().catch(() => {});
  }
}

// An MP4's (ISO BMFF) header, from a few small reads at most (`read(start, len)`: bytes or null):
// {duration (s; 0 unknown), video, audio (its tracks: null when not all were read), height}, or
// null when it isn't one. The movie's length is its mvhd's, or a fragmented file's mehd's; each
// track's kind its hdlr's (vide, soun), the picture's height its tkhd's.
const MP4_CHUNK = 4096;
const MP4_READS = 6;
async function mp4Head(read) {
  let buf = new Uint8Array(0);
  let base = 0;
  let reads = 0;
  // `n` bytes at `pos`, read when they aren't there yet.
  const at = async (pos, n) => {
    if (pos < base || pos + n > base + buf.length) {
      if (reads >= MP4_READS) return null;
      reads += 1;
      const got = await read(pos, Math.max(n, MP4_CHUNK));
      if (!got) return null;
      buf = got;
      base = pos;
    }
    const s = buf.subarray(pos - base, pos - base + n);
    return s.length === n ? s : null;
  };
  const u32 = (b, i) => ((b[i] << 24) >>> 0) + (b[i + 1] << 16) + (b[i + 2] << 8) + b[i + 3];
  const u64 = (b, i) => u32(b, i) * 2 ** 32 + u32(b, i + 4);
  // The box at `pos`: {pos, size, type, hdr}; null past the reads or for one that can't be.
  const box = async (pos, end) => {
    const h = (await at(pos, 16)) || (await at(pos, 8));
    if (!h) return null;
    const type = String.fromCharCode(h[4], h[5], h[6], h[7]);
    let size = u32(h, 0);
    let hdr = 8;
    if (size === 1 && h.length >= 16) {
      size = u64(h, 8);
      hdr = 16;
    } else if (size === 0) {
      size = end - pos;
    }
    return size >= hdr && /^[\w ©-]{4}$/.test(type) ? { pos, size, type, hdr } : null;
  };
  const kids = async function* (parent) {
    const end = parent.pos + parent.size;
    for (let p = parent.pos + parent.hdr, i = 0; p + 8 <= end && i < 64; i += 1) {
      const b = await box(p, end);
      if (!b) return;
      yield b;
      p += b.size;
    }
  };
  // The file: an ftyp (or a movie, or free space) first, then its boxes up to the movie.
  let moov = null;
  for (let p = 0, i = 0; i < 8; i += 1) {
    const b = await box(p, Infinity);
    if (!b || (i === 0 && !/^(ftyp|moov|free|skip|wide)$/.test(b.type)) || !Number.isFinite(b.size)) return null;
    if (b.type === "moov") {
      moov = b;
      break;
    }
    p += b.size;
  }
  if (!moov) return null;
  const info = { duration: 0, video: null, audio: null, height: 0 };
  let timescale = 0;
  let fragments = 0;
  const kinds = [];
  let whole = true;
  let traks = 0;
  // Where the movie's boxes read so far end: short of its end, some tracks weren't seen.
  let reached = moov.pos + moov.hdr;
  for await (const b of kids(moov)) {
    reached = b.pos + b.size;
    if (b.type === "mvhd") {
      const h = await at(b.pos + b.hdr, 32);
      if (!h) continue;
      const v1 = h[0] === 1;
      timescale = u32(h, v1 ? 20 : 12);
      const d = v1 ? u64(h, 24) : u32(h, 16);
      // All ones: unknown (a fragmented file's).
      if (timescale && d && d !== 0xffffffff) info.duration = d / timescale;
    } else if (b.type === "mvex") {
      for await (const e of kids(b)) {
        if (e.type !== "mehd") continue;
        const h = await at(e.pos + e.hdr, 12);
        if (h) fragments = h[0] === 1 ? u64(h, 4) : u32(h, 4);
      }
    } else if (b.type === "trak") {
      traks += 1;
      let kind = "";
      let height = 0;
      for await (const t of kids(b)) {
        if (t.type === "tkhd") {
          const h = await at(t.pos + t.hdr, 96);
          if (h) height = h[0] === 1 ? u32(h, 92) / 65536 : u32(h, 80) / 65536;
        } else if (t.type === "mdia") {
          for await (const m of kids(t)) {
            if (m.type !== "hdlr") continue;
            const h = await at(m.pos + m.hdr, 12);
            if (h) kind = String.fromCharCode(h[8], h[9], h[10], h[11]);
            break;
          }
          break;
        }
      }
      if (!kind) whole = false;
      kinds.push(kind);
      if (kind === "vide" && height > info.height) info.height = Math.round(height);
    }
  }
  if (!info.duration && fragments && timescale) info.duration = fragments / timescale;
  // Every track read: what the file holds; else not known.
  if (whole && traks && reached >= moov.pos + moov.size) {
    info.video = kinds.includes("vide");
    info.audio = kinds.includes("soun");
  }
  return info;
}

// A file that may be an MP4, by its type or its address: its header is read.
const MP4_TYPE = /^(video|audio)\/(mp4|x-m4v|x-m4a|quicktime|3gpp|3gpp2)$/;
const MP4_EXT = /\.(mp4|m4v|m4a|mov|3gp|3g2)$/i;
const mp4Like = (it) => MP4_TYPE.test(it.mime || "") || MP4_EXT.test(pathOf(it.url)) || (it.ranged && /octet-stream|^$/.test(it.mime || ""));

// The files of a frame read at most at once, the newest first.
const PLAYED_FILES = 16;

// What a frame played, as candidates for `playedChoice`: its streams (a master playlist with the
// length of its variants the tab loaded, never those variants again) and its files (listed and
// ranged), each with what is known of it.
function playedCandidates(tab, frameId) {
  const mine = (it) => it.frameId === frameId;
  const streams = tab.items.filter((it) => mine(it) && isStream(it));
  const out = [];
  for (const it of streams) {
    const info = it.info && !it.info.failed ? it.info : null;
    const p = pathOf(it.url);
    // A variant (or a sound's playlist) of a master here: the master stands for it.
    if (streams.some((s) => s !== it && s.info && !s.info.failed && s.info.children.includes(p))) continue;
    let duration = info ? info.duration : 0;
    let live = Boolean(info && info.live);
    if (info && info.children.length) {
      const parts = streams.filter((s) => s.info && !s.info.failed && info.children.includes(pathOf(s.url)));
      duration = Math.max(0, ...parts.map((s) => s.info.duration));
      live = parts.length > 0 && parts.every((s) => s.info.live);
    }
    out.push({
      url: it.url,
      kind: it.kind,
      stream: true,
      at: it.at || 0,
      size: 0,
      duration: live ? 0 : duration,
      live,
      tracks: null,
      height: info && info.heights.length ? Math.max(...info.heights) : 0,
      drm: Boolean(info && info.drm),
    });
  }
  for (const it of [...tab.items.filter((x) => mine(x) && !isStream(x)), ...(tab.ranged || []).filter(mine)].slice(-PLAYED_FILES)) {
    const head = it.head && !it.head.failed ? it.head : null;
    out.push({
      url: it.url,
      kind: it.kind,
      stream: false,
      at: it.at || 0,
      size: it.size || 0,
      duration: head ? head.duration : 0,
      live: false,
      tracks: head && head.video !== null ? { video: head.video, audio: head.audio } : null,
      height: head ? head.height : 0,
      drm: false,
    });
  }
  return out;
}

// Which of `cands` (playedCandidates) is the video `v` the button is over ({duration, live,
// height, audio (a sound player), began, nextBegan, alone}): {pick} for one, {several: n} when more
// than one fits and none is clearly it (said, never guessed), {} for none. Never a protected
// stream, a picture or a sound alone (one of a video's tracks), nor anything whose length isn't
// the video's. One whose length isn't known fits only where the frame plays nothing else, or when
// it came while this video loaded, before the next one began to (a feed's player loads the next
// video while this one plays).
function playedChoice(cands, v) {
  const kindFits = (c) => {
    if (c.drm) return false;
    if (v.audio) return c.kind === "audio" || Boolean(c.tracks && !c.tracks.video && c.tracks.audio);
    return c.kind !== "audio" && !(c.tracks && !(c.tracks.video && c.tracks.audio));
  };
  // "yes", "no", or "" (not known).
  const lengthFits = (c) => {
    if (v.live || c.live) return v.live && c.live ? "yes" : v.live && !c.duration ? "" : "no";
    if (!(c.duration > 0) || !(v.duration > 0)) return "";
    return Math.abs(c.duration - v.duration) <= Math.max(1, v.duration * 0.03) ? "yes" : "no";
  };
  // The one of `list`: a stream before files (a fitting stream is the video; its files beside it
  // are its pieces), else the one with the player's picture height.
  const one = (list) => {
    const streams = list.filter((c) => c.stream);
    const pool = streams.length ? streams : list;
    if (pool.length === 1) return pool[0];
    const tall = pool.filter((c) => c.height && c.height === v.height);
    return tall.length === 1 ? tall[0] : null;
  };
  const ok = cands.filter(kindFits);
  const sure = ok.filter((c) => lengthFits(c) === "yes");
  if (sure.length) {
    const c = one(sure);
    return c ? { pick: c } : { several: sure.length };
  }
  const unsure = ok.filter((c) => lengthFits(c) === "");
  const loading = unsure.filter((c) => v.alone || (v.began > 0 && c.at >= v.began - 2000 && (!v.nextBegan || c.at < v.nextBegan)));
  if (loading.length) {
    const c = one(loading);
    return c ? { pick: c } : { several: loading.length };
  }
  return unsure.length ? { several: unsure.length } : {};
}

// The button's question when the page's look-up found nothing (video.js playedLook): what the
// tab played for the video under it, from that video's frame. {played: {url, stream, ext, size,
// height, duration}}, {several: n}, or {}.
async function videoPlayed(m, sender) {
  const tabId = sender.tab && sender.tab.id;
  if (!Number.isInteger(tabId)) return {};
  await mediaBack;
  const frameId = sender.frameId || 0;
  // The frame's possible manifests read first (#171), as for a stream it plays.
  await sniffTab(tabId, new Set([frameId]));
  const tab = media.get(tabId);
  if (!tab) return {};
  const mine = (it) => it.frameId === frameId;
  const files = [...tab.items.filter((x) => mine(x) && !isStream(x)), ...(tab.ranged || []).filter(mine)].slice(-PLAYED_FILES);
  await Promise.all([
    ...tab.items.filter((it) => mine(it) && isStream(it) && !it.info).map(async (it) => (it.info = await probe(it))),
    ...files
      .filter((it) => !it.head)
      .map(async (it) => (it.head = (mp4Like(it) && (await mp4Head((start, len) => bytesAt(it.url, start, len)))) || { failed: true })),
  ]);
  if (media.get(tabId) !== tab) return {};
  keepMedia();
  const num = (x) => (Number.isFinite(Number(x)) && Number(x) > 0 ? Number(x) : 0);
  const v = {
    duration: num(m.duration),
    live: m.live === true,
    height: num(m.height),
    audio: m.audio === true,
    began: num(m.began),
    nextBegan: num(m.nextBegan),
    alone: m.alone === true,
  };
  const r = playedChoice(playedCandidates(tab, frameId), v);
  if (r.several) return { several: r.several };
  if (!r.pick) return {};
  const c = r.pick;
  const it = [...tab.items, ...(tab.ranged || [])].find((x) => x.url === c.url) || {};
  const ext = c.stream ? "" : splitExtension(mediaName(c.url, it.mime, "", false).name)[1].toLowerCase();
  return { played: { url: c.url, stream: c.stream, ext, size: c.size, height: c.height, duration: c.duration } };
}

// One of the tab's media named by the page as what it played (`video-played`'s answer), of
// `frameId` when given: the tab's own record of it, or null (never an address it didn't load).
function playedItem(tabId, url, frameId) {
  const tab = typeof url === "string" && media.get(tabId);
  const it = tab && [...tab.items, ...(tab.ranged || [])].find((x) => x.url === url);
  return it && (frameId === undefined || it.frameId === frameId) ? it : null;
}

// ---- a page's media file's name (#345, #493, #695, #800): as the app's engine names it
// (crates/engine/src/naming.rs, answer_name; tools/extension/test.mjs checks they agree) ---------

// The types a file's extension is taken from (naming.rs' TYPES): media, pictures, documents and
// archives, never a type that runs.
const TYPE_EXTENSIONS = {
  "video/mp4": "mp4",
  "video/x-m4v": "m4v",
  "video/webm": "webm",
  "video/x-matroska": "mkv",
  "video/quicktime": "mov",
  "video/x-msvideo": "avi",
  "video/avi": "avi",
  "video/x-ms-wmv": "wmv",
  "video/x-flv": "flv",
  "video/mpeg": "mpg",
  "video/mp2t": "ts",
  "video/3gpp": "3gp",
  "video/3gpp2": "3g2",
  "video/ogg": "ogv",
  "audio/mpeg": "mp3",
  "audio/mp3": "mp3",
  "audio/mp4": "m4a",
  "audio/x-m4a": "m4a",
  "audio/aac": "aac",
  "audio/aacp": "aac",
  "audio/webm": "weba",
  "audio/ogg": "ogg",
  "audio/opus": "opus",
  "audio/flac": "flac",
  "audio/x-flac": "flac",
  "audio/wav": "wav",
  "audio/x-wav": "wav",
  "audio/wave": "wav",
  "audio/x-ms-wma": "wma",
  "application/ogg": "ogg",
  "image/jpeg": "jpg",
  "image/png": "png",
  "image/gif": "gif",
  "image/webp": "webp",
  "image/avif": "avif",
  "application/pdf": "pdf",
  "application/epub+zip": "epub",
  "application/zip": "zip",
  "application/x-zip-compressed": "zip",
  "application/x-7z-compressed": "7z",
  "application/vnd.rar": "rar",
  "application/x-rar-compressed": "rar",
  "application/gzip": "gz",
  "application/x-gzip": "gz",
};
const TYPED = new Set(Object.values(TYPE_EXTENSIONS));
// A web server's programs (naming.rs' SCRIPT_EXTENSIONS) and a page's: such an address names the
// program that answers, not the file it sends.
const SCRIPT_EXTENSIONS = ["php", "php3", "php4", "php5", "phtml", "asp", "aspx", "ashx", "axd", "jsp", "jspx", "do", "action", "cgi", "pl", "cfm", "py", "rb"];
const PAGE_EXTENSIONS = ["htm", "html", "xhtml", "shtml", "dhtml"];

// A name's stem and extension ("clip.mp4": "clip", "mp4"); ".profile", "clip", "Mr. Smith" have none.
function splitExtension(name) {
  const dot = name.lastIndexOf(".");
  const ext = dot > 0 ? name.slice(dot + 1) : "";
  return ext && !/\s/.test(ext) ? [name.slice(0, dot), ext] : [name, ""];
}
const typeExtension = (mime) => TYPE_EXTENSIONS[String(mime || "").split(";")[0].trim().toLowerCase()] || "";
const isScriptName = (name) => SCRIPT_EXTENSIONS.includes(splitExtension(name)[1].toLowerCase());
// Its extension doesn't say what the file is: a script's, a page's, or none.
function untyped(name) {
  const ext = splitExtension(name)[1].toLowerCase();
  return !ext || SCRIPT_EXTENSIONS.includes(ext) || PAGE_EXTENSIONS.includes(ext);
}
// A name that says nothing of the file, for the tab's title to replace: a script's, none (a bare
// id, `videoplayback`), or a long hash (#345).
function meansNothing(name) {
  const [stem, ext] = splitExtension(name);
  const hash = stem.length >= 24 && /^[A-Za-z0-9=_-]+$/.test(stem) && (stem.match(/[-_]/g) || []).length <= 2;
  return !ext || isScriptName(name) || hash;
}
// The tab's title as a file's stem: what a file name can't hold becomes a space, at most 150 characters.
function cleanTitle(title) {
  const spaced = String(title || "")
    .replace(/[\u00AD\u200B-\u200F\u202A-\u202E\u2060-\u2064\u2066-\u206F\uFEFF]/g, "")
    .replace(/[\x00-\x1f\x7f-\x9f<>:"/\\|?*]/g, " ");
  return [...spaced.split(/\s+/).filter(Boolean).join(" ")].slice(0, 150).join("").replace(/[. ]+$/, "");
}
// A site's own name in its address: the part before its domain's ending (www.shortverse.com:
// "shortverse"; news.bbc.co.uk: "bbc", a country's second level left out). None for an IP
// address.
function siteNameOf(url) {
  const host = siteOf(url).toLowerCase();
  if (!host || host.includes(":") || /^[\d.]+$/.test(host)) return "";
  const labels = host.split(".").filter(Boolean);
  let at = labels.length - 2;
  if (at > 0 && labels[labels.length - 1].length === 2 && /^(co|com|org|net|gov|edu|ac|or|ne|go)$/.test(labels[at])) at -= 1;
  return labels[Math.max(at, 0)] || "";
}
// A tab's title without its site's name: "Before The - Short Film | Shortverse" on
// shortverse.com is "Before The - Short Film". The title names what the app knows by no name of
// its own: a video yt-dlp knows only by its address (a stream's `playlist`, #345), a page's file
// whose address names nothing (#800). The site's name goes where it ends the title after a
// separator (| - – — · • » : ::), when its letters and digits are the address's own name's
// (siteNameOf) or its first word's are ("BBC News" on bbc.co.uk); where it starts the title, only
// when it is that name. The title stays whole when nothing else would be left.
const TITLE_SEPARATOR = /\s+(?:[|\-–—·•»]|::?)\s+/g;
function withoutSiteName(title, url) {
  const whole = String(title || "").trim();
  const letters = (s) => s.toLowerCase().replace(/[^\p{L}\p{N}]/gu, "");
  const site = letters(siteNameOf(url));
  if (!site) return whole;
  let rest = whole;
  const ends = [...rest.matchAll(TITLE_SEPARATOR)].pop();
  if (ends) {
    const tail = rest.slice(ends.index + ends[0].length);
    if (letters(tail) === site || letters(tail.split(/\s+/)[0]) === site) rest = rest.slice(0, ends.index).trim();
  }
  const starts = [...rest.matchAll(TITLE_SEPARATOR)][0];
  if (starts && letters(rest.slice(0, starts.index)) === site) rest = rest.slice(starts.index + starts[0].length).trim();
  return rest || whole;
}
// The tab's title as it names what the tab shows: without its site's name (withoutSiteName).
const tabTitle = (tab) => withoutSiteName((tab && tab.title) || "", tab && tab.url).slice(0, 300);
// The address's own name (naming.rs' address_name): a script's `file=` or `filename=` value naming
// a file of a known type (#695), else its last part.
function addressFileName(url) {
  const last = addressName(url);
  if (splitExtension(last)[1] && !isScriptName(last)) return last;
  try {
    for (const [k, v] of new URL(url).searchParams) {
      if (!/^(file|filename)$/i.test(k)) continue;
      const name = v.trim().split(/[\\/]/).pop().trim();
      const [stem, ext] = splitExtension(name);
      if (stem.trim() && TYPED.has(ext.toLowerCase())) return name;
    }
  } catch {
    // Not an address: its last part.
  }
  return last;
}

// A page's media file's name as the app saves it ({name, own}): its address's (a script's `file=`
// value), typed by the type its server said (`mime`) when its own extension doesn't say it
// (stream.php: stream.mp4), and the tab's title instead of a name that means nothing (a script's,
// a bare id, a hash; any on a server whose names are all random ids, #493: X's video.twimg.com).
// With the popup's switch on (`includeOriginal`), the title before a real name ("Title -
// clip.mp4"). `own`: the address names the file as the app would name it with no name sent, so
// none is (a server's Content-Disposition then still wins).
function mediaName(url, mime, title, includeOriginal = false) {
  const given = addressFileName(url);
  const typed = untyped(given) ? typeExtension(mime) : "";
  const [stem, ext] = splitExtension(given);
  const name = cleanTitle(title);
  const randomServer = siteOf(url).toLowerCase() === "video.twimg.com";
  const bare = given === addressName(url) && (meansNothing(given) || randomServer);
  const dotted = typed || ext ? `.${typed || ext}` : "";
  if (bare && name) return { name: `${name}${dotted}`, own: false };
  const own = typed ? `${stem || "download"}.${typed}` : given;
  if (includeOriginal && name && own) return { name: `${name} - ${own}`, own: false };
  return { name: own, own: !typed };
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
    return download("video", {
      video: true,
      // Asked for: whether the button over videos shows on this site doesn't matter.
      explicit: true,
      url: page,
      src: it.url,
      title: tabTitle(tab),
      referrer: tab.url && tab.url !== page ? tab.url : "",
      ...(await streamContext(tabId, it.url, ctx)),
      ...(ctx.incognito ? { private: true } : {}),
    });
  }
  const { mediaTitleName } = await ext.storage.local.get({ mediaTitleName: false });
  const title = tabTitle(tab);
  const named = mediaName(it.url, it.mime, title, mediaTitleName);
  // The name the list shows; none when the address names the file as the app would (a server's
  // Content-Disposition may still name it). The title goes too: a file whose address names nothing
  // and whose type only the server or its bytes say is named by it in the app (#800).
  return handOff({ url: it.url, referrer: page, page: tab.url || page, size: it.size, mime: it.mime, explicit: true, filename: named.own ? "" : named.name, ...(title ? { title } : {}), ...ctx });
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

// The cookies of a tab's stream (`videoCookies`) and what the browser sent with it (#900): the
// Referer and Origin its player asked for it with (`fetchedWith`: the app sends them again with
// its playlists and pieces), and the cookies that went with it that no store lists (a frame's
// own, kept for its page only), added for its host. Nothing more for an address the tab didn't
// load (a page's).
async function streamContext(tabId, url, ctx) {
  const c = await videoCookies(url, ctx);
  const tab = media.get(tabId);
  const it = tab && [...tab.items, ...(tab.ranged || []), ...(tab.maybe || [])].find((x) => x.url === url);
  const sent = (it && it.sent) || {};
  const web = (v) => typeof v === "string" && /^https?:\/\/\S+$/i.test(v) && v.length <= URL_MAX;
  const fetchedWith = {
    ...(web(sent.referer) ? { referer: sent.referer } : {}),
    ...(web(sent.origin) ? { origin: sent.origin } : {}),
  };
  if (typeof sent.cookie === "string" && sent.cookie) {
    let host = "";
    let secure = false;
    try {
      const u = new URL(url);
      host = u.hostname;
      secure = u.protocol === "https:";
    } catch {
      // Not an address: no cookie of its own.
    }
    const known = new Set(c.cookieJar.map((x) => x.name));
    for (const pair of sent.cookie.split(/;\s*/)) {
      const at = pair.indexOf("=");
      const name = at > 0 ? pair.slice(0, at).trim() : "";
      if (!host || !name || known.has(name)) continue;
      known.add(name);
      c.cookieJar.push({ name, value: pair.slice(at + 1), domain: host, hostOnly: true, path: "/", secure });
    }
    // The header the browser sent for this very address.
    c.cookies = sent.cookie;
  }
  return Object.keys(fetchedWith).length ? { ...c, fetchedWith } : c;
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
// Firefox: the proxy a request went through, kept for its hand-over. Noted as its response
// starts, and from its answer's headers when the extension takes the download there (#208): that
// request is cancelled, so it never starts.
function noteProxy(d) {
  if (!proxyAllowed || !d.proxyInfo || d.proxyInfo.type === "direct") return;
  forget(proxyUsed, 120000);
  proxyUsed.set(d.url, { info: d.proxyInfo, at: Date.now() });
}
if (PROXY_DECLARED && FIREFOX && ext.webRequest && ext.webRequest.onResponseStarted) {
  ext.webRequest.onResponseStarted.addListener(noteProxy, { urls: ["<all_urls>"] });
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

// Firefox never sends loopback addresses through its proxy settings' proxy
// (network.proxy.allow_hijacking_localhost, off by default): localhost and its names, 127.0.0.0/8, ::1.
function firefoxLoopback(u) {
  const host = u.hostname.toLowerCase().replace(/^\[|\]$/g, "");
  return host === "localhost" || host.endsWith(".localhost") || host === "::1" || /^127(\.\d{1,3}){3}$/.test(host);
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
    if (firefoxLoopback(u)) return {};
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

// The web page a download was started from, whole (#607): the Referer the browser sends with it
// is often the site's address only (the page's referrer policy), and the app's "Refresh download
// address" opens this page, where a fresh link can be taken. Firefox names the page a load came
// from (`startedFrom`); else the tab it was started in, as its load started (`tabPage`: the tab
// may be closed or elsewhere by now), or as it is now (for a link opened in a tab of its own,
// the tab that opened it), else the active tab of the window last used, from the same kind of
// window (private or not). "" when none is a web page other than the file itself.
async function pageOf(item, urls) {
  const web = (u) => typeof u === "string" && /^https?:\/\//i.test(u) && u.length <= URL_MAX && !urls.includes(u);
  forget(startedFrom, 120000);
  const seen = urls.map((u) => startedFrom.get(u)).find(Boolean);
  if (seen && web(seen.page)) return seen.page;
  const same = (tab) => tab && Boolean(tab.incognito) === Boolean(item.incognito);
  // The page its tab showed as the load started (`tabPage`).
  const then = seen && seen.tab ? await seen.tab : null;
  if (same(then) && web(then.url)) return then.url;
  const tabId = item.tabId >= 0 ? item.tabId : seen && seen.tabId >= 0 ? seen.tabId : -1;
  try {
    let tab = tabId >= 0 ? await ext.tabs.get(tabId) : (await ext.tabs.query({ active: true, lastFocusedWindow: true }))[0];
    if (same(tab) && !web(tab.url) && tab.openerTabId >= 0) tab = await ext.tabs.get(tab.openerTabId);
    if (same(tab) && web(tab.url)) return tab.url;
  } catch {
    // The tab is gone: no page.
  }
  return "";
}

// `explicit`: the user asked for this one (the right-click item), so Options' automatic-capture
// rules (file types, excluded sites) don't apply; the host checks them for everything else.
// `page`: the web page it was started from (`pageOf`).
async function handOff(d) {
  const message = {
    url: d.url,
    finalUrl: d.finalUrl && d.finalUrl !== d.url ? d.finalUrl : "",
    referrer: d.referrer || "",
    ...(d.page && /^https?:\/\//i.test(d.page) && d.page.length <= URL_MAX ? { pageUrl: d.page } : {}),
    cookies: await cookieHeader(d.url, d),
    filename: d.filename || "",
    // The browser's name is only its address's guess (#948, `addressNamed`): the host checks
    // Options' file types by it, and MDM names the file from the server's own answer.
    ...(d.nameGuessed ? { nameGuessed: true } : {}),
    ...(d.size > 0 ? { size: d.size } : {}),
    ...(d.mime ? { mime: d.mime } : {}),
    ...(d.post ? { post: d.post.body, postType: d.post.type || "" } : {}),
    ...(d.incognito ? { private: true } : {}),
    ...(d.explicit ? { explicit: true } : {}),
    // A page's media from the popup's list (#800): its page's title, for a name the address lacks.
    ...(d.title ? { title: d.title } : {}),
    // Through the browser's proxy too (#42): the one its own request for the file took.
    ...(await browserProxy(d.finalUrl || d.url, d.incognito)),
  };
  return download("file", message);
}

// Telegram Web (web.telegram.org: its K and A apps) downloads a file through its own service
// worker, which fetches the file's encrypted pieces over the page's connection to Telegram and
// hands the browser the result under an address of its own (`/k/d/…`, `/a/progressive/…`).
// That address is nothing on Telegram's servers: outside the page it only redirects to the app
// (302), so MDM can't fetch it and the download sat at 0 % (#1345). Every download from there is
// the browser's.
function telegramWebStream(...urls) {
  return urls.some((value) => {
    if (!value) return false;
    try {
      return new URL(value).hostname.toLowerCase() === "web.telegram.org";
    } catch {
      return false;
    }
  });
}

// Colab's runtime proxy uses an ephemeral token tied to the browser's partitioned session.
// Only the actual proxy transfer URL is left to the browser: a normal file linked from a Colab
// notebook must still be capturable by MDM (#603).
function colabRuntimeProxy(...urls) {
  return urls.some((value) => {
    if (!value) return false;
    try {
      const u = new URL(value);
      const host = u.hostname.toLowerCase();
      const proxyHost =
        host === "prod.r.colab.dev"
        || host.endsWith(".prod.r.colab.dev")
        || /(?:^|[-.])colab\.googleusercontent\.com$/.test(host);
      return proxyHost && u.searchParams.has("colab-runtime-proxy-token");
    } catch {
      return false;
    }
  });
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


// `clicked`: a link just clicked without Alt (Safari's `link`, see `altClicked`).
async function candidate(item, url, clicked = false) {
  if (!/^https?:/i.test(url)) return null; // blob:, data:, file: stay the browser's
  // Telegram: its page's own downloader has the session and the media (protected channels
  // included). Handing the page-only URL to the app made it answer 302.
  if (isTelegramUrl(url) || isTelegramUrl(item.referrer)) return null;
  const { paused } = await ext.storage.local.get({ paused: false });
  if (paused) return null;
  const first = firstUrl(item);
  if (altClicked(item, url, first, clicked) || postOf(first) === null) return null; // Alt; a form not to post again
  if (colabRuntimeProxy(first, url)) return null;
  if (telegramWebStream(first, url)) return null;
  const r = await currentRules();
  if (!r) return null;
  const name = (item.filename || "").split(/[\\/]/).pop() || url.split(/[?#]/)[0].split("/").pop() || "";
  const left = (why) => ({ left: why, name });
  if (!r.capture) return left(r.captureOff || "browser");
  const by = excludedBy(r, first) || excludedBy(r, url) || (pageListed(r, item.referrer) ? "site" : "");
  if (by) return left(by);
  const type = extensionOf(name);
  if (!type || !typePatterns(r.types).some((p) => p.test(type))) return left("file-type");
  // The app's Torrent settings leave torrents to the browser (its own torrent client, #1058).
  if (r.torrents === false && isTorrent(name, [first, url], item.mime)) return left("torrents");
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

// Offer the download to the app under `name`; true if it took it. `nameGuessed`: `name` is only
// its address's (`addressNamed`).
async function offer(item, url, name, nameGuessed = false) {
  const first = firstUrl(item);
  const reply = await handOff({
    url: first,
    finalUrl: url,
    referrer: item.referrer,
    page: item.page || (await pageOf(item, [first, url, item.url].filter(Boolean))),
    filename: name,
    nameGuessed,
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

// Safari: a link clicked in a page (page.js) that may be a download, offered as the browser's own
// downloads are: through `candidate` (Options' rules, Alt, pause) then `offer`. `{taken: true}`
// when the app took it; else page.js lets the click go on, so nothing is lost. The link's file
// name is its `download` attribute's (`name`), else its address's (Safari names a download only
// when it starts, which is too late).
// The name an address gives, readable: the app saves a name as it is told it (the browsers' own
// downloads come already named), so `My%20file.zip` would stay that.
function addressName(url) {
  const last = url.split(/[?#]/)[0].split("/").pop() || "";
  try {
    return decodeURIComponent(last);
  } catch {
    return last; // not valid percent-encoding
  }
}

async function link(url, referrer, incognito, name) {
  // What the browser opens itself (a PDF, a song) is taken only when Options say so (`opened`,
  // "Also take files the browser would open"), as in the other browsers.
  const r = await currentRules();
  const type = extensionOf(name || url.split(/[?#]/)[0]).toUpperCase();
  if (r && r.opened === false && OPENED[type]) return { taken: false };
  const filename = name || addressName(url);
  const item = { url, referrer, page: referrer, incognito, ...(filename ? { filename } : {}) };
  const c = await candidate(item, url, true).catch(() => null);
  if (!c || c.left) {
    leftAlone(c, item, url);
    return { taken: false };
  }
  return { taken: await offer(item, url, c.name).catch(() => false) };
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

// A download left to the browser keeps the name it has, or the one another extension gives it
// (Save In's folders, #578; a `downloads.download` with a `filename`). Chromium counts every
// listener's answer as a name of its own, an empty one too: `suggest()` without a suggestion
// reaches it as an empty name with no conflict action, and when the most recently installed
// extension answers so, its empty name wins and the others' are dropped (the download lands in
// the Downloads folder, and the browser may warn of a conflict). Only an answer with
// "uniquify" and no name is no suggestion at all (downloads_api.cc, DeterminerCallback), and
// the binding lets no empty name through: a name the browser refuses, which it empties after
// counting the answer, is how a listener gives one ("Invalid filename" is logged once).
function noName(suggest) {
  suggest({ filename: "..", conflictAction: "uniquify" });
}

// Chromium: whether the browser's `name` for a download is only its address's (#948). The session
// rule below makes a file the browser would show or play a download by setting its answer's
// Content-Disposition to "attachment": an inline answer's own name (`inline; filename="Movie.mkv"`)
// goes with it, and the browser names the file after its address (`main.mkv`). Such a download (a
// type the rule takes, named after its address, give or take the extension the type adds) goes to
// MDM unnamed: MDM reads the server's own name from its answer, as Firefox's way does here
// (`answerFile`). A name of its own (an attachment's, which the rule leaves alone; a link's
// `download` name) stays the browser's.
function addressNamed(item, url, name) {
  const mime = String(item.mime || "").toLowerCase();
  if (!mime || !openedTypes.some((m) => mime.startsWith(m))) return false;
  const stem = (n) => splitExtension(n)[0].toLowerCase();
  const own = addressName(url);
  return Boolean(own) && stem(name) === stem(own);
}

if (SAFARI) {
  // Nothing to listen to: see `link`.
} else if (!FIREFOX) {
  // Chromium: once the final file name is known (after redirects and Content-Disposition). The
  // download waits for `suggest`, writing nothing, while the app is asked: taken, it is
  // cancelled; else it goes on as the browser's. So it is never lost or kept by both (the app
  // asks the address once more: a form's download is posted again with its body).
  ext.downloads.onDeterminingFilename.addListener((item, suggest) => {
    const url = item.finalUrl || item.url;
    candidate(item, url)
      .then(async (c) => {
        const taken =
          Boolean(c && c.name && !c.left) && (await offer(item, url, c.name, addressNamed(item, url, c.name)));
        if (taken) return drop(item);
        // The browser goes on first; why is said after.
        noName(suggest);
        leftAlone(c, item, url);
      })
      .catch(() => noName(suggest));
    return true; // suggest() comes later
  });
} else {
  // Firefox names the file before the download is created and has no such pause. Most are
  // caught earlier, from the answer to the page load (`headersReceived`, below); the others (a
  // download started by a page's script, a type the answer didn't say) here: the download is
  // paused at once when it may be the app's, then cancelled or resumed; one that finished
  // meanwhile (a small file) has its file removed.
  // A page load decided from its answer already (`headersReceived`, #208) isn't asked again.
  ext.downloads.onCreated.addListener(async (item) => {
    const url = item.finalUrl || item.url;
    forget(decided, 120000);
    if (decided.has(url) || decided.has(firstUrl(item))) return;
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
// link leads to one, or a page shows one in a frame, an <embed> or an <object> (a course's PDF,
// #1087). Chromium: a session rule marks such answers as attachments, so they come down the
// download path above (where the rules and Alt apply). Firefox: taken from the answer as a
// download is (`headersReceived`), else it opens. A file a page's script fetches (its own PDF
// viewer reading it, then showing what it read) stays the page's: taken, the page would get
// nothing and say the file failed to load.
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
let openedTypes = [];

// Options' excluded sites as the rule needs them, read as the app reads them: `example.com` and
// `*.example.com` alike are the domain and its subdomains (#1049), a rule's domain condition. As
// the browser writes a host: no port, punycode. A pattern with another `*` (`*.youtube.*`) is no
// domain a rule can name: such a site's PDF is made a download, which the browser then keeps.
function excludedHosts() {
  const hosts = [];
  for (const raw of ((rules && rules.excludedSites) || "").split(/\s+/)) {
    const p = raw.trim().replace(/^[a-z]+:\/\//i, "").split("/")[0].replace(/^\*\./, "");
    let host = "";
    try {
      host = new URL(`http://${p}`).hostname.replace(/\.+$/, "");
    } catch {
      continue;
    }
    if (host && !host.includes("*") && !hosts.includes(host)) hosts.push(host);
  }
  return hosts;
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
      resourceTypes: PAGE_LOADS,
      responseHeaders: [{ header: "content-type", values: openedTypes.map((m) => `${m}*`) }],
      // An attachment is a download already, with the server's own name, which "set" would drop
      // (#948): left as it is.
      excludedResponseHeaders: [{ header: "content-disposition", values: ["attachment*"] }],
      // Options' excluded sites keep opening in the browser: their files, and those their pages
      // lead to on another host (the page that started the load, #1049).
      ...(hosts.length ? { excludedRequestDomains: hosts, excludedInitiatorDomains: hosts } : {}),
    },
  };
  try {
    const old = (await dnr.getSessionRules()).map((r) => r.id);
    await dnr.updateSessionRules({
      removeRuleIds: old.filter((id) => id >= OPENED_RULE),
      addRules: openedTypes.length ? [rule] : [],
    });
  } catch {
    // A browser without header conditions (Chrome before 128): it opens them as before.
  }
}

// ---- Firefox: a download caught from its answer, before the browser's dialog (#208) ----------

// Firefox creates a download (downloads.onCreated, above) only once it has a file to save to:
// when it asks first ("What should Firefox do with this file?", which Waterfox always asks for
// a new type), only after the user answered, while it downloads the whole file behind the
// dialog. So a page load whose answer would be a download is caught as the answer arrives: the
// load waits while the app is asked, as Chromium's download does, and is cancelled when the app
// took it (no dialog, no browser download). Else it goes on as before, and the download it
// becomes is left to the browser (`decided`). A file the browser would show or play (a PDF, an
// MP3) is taken the same way when Options say so (`openedTypes`); else it opens.

// What Firefox shows or plays itself: a download only when its answer says "attachment".
const SHOWN = /^(text\/|image\/|video\/|audio\/|multipart\/|application\/(pdf|xhtml\+xml|xml|json|javascript|x-javascript|ecmascript|ogg|x-xpinstall|[\w.+-]+\+(xml|json))$)/;

// The file name a Content-Disposition gives (filename*=UTF-8''…, else filename="…"), or "".
function dispositionName(cd) {
  const star = /filename\*\s*=\s*([\w-]+)'[^']*'([^;]+)/i.exec(cd);
  if (star) {
    try {
      return decodeURIComponent(escapedSeparators(star[2].trim().replace(/^"|"$/g, "")));
    } catch {
      // Not percent-encoded as it says: the plain name below.
    }
  }
  const plain = /(?:^|;)\s*filename\s*=\s*("((?:[^"\\]|\\.)*)"|[^;]+)/i.exec(cd);
  if (!plain) return "";
  return decodedPlain((plain[2] !== undefined ? plain[2].replace(/\\(.)/g, "$1") : plain[1]).trim());
}

// A plain `filename` as Chrome and Firefox read it (#1427), and the app (naming.rs
// `decode_plain`): RFC 2047's encoded words (`=?UTF-8?B?…?=`) decoded, else its percent-encoding
// whenever that decodes to UTF-8 (`My%20Report.pdf` is "My Report.pdf"), else as sent.
function decodedPlain(name) {
  const words = encodedWords(name);
  if (words !== null) return words;
  if (!name.includes("%")) return name;
  // Byte by byte, as the app: an escape that isn't one (`50%off`) stays as it is.
  const bytes = new TextEncoder().encode(escapedSeparators(name));
  const out = [];
  for (let i = 0; i < bytes.length; i++) {
    const hex = String.fromCharCode(bytes[i + 1] || 0, bytes[i + 2] || 0);
    if (bytes[i] === 0x25 && /^[0-9a-f]{2}$/i.test(hex)) {
      out.push(parseInt(hex, 16));
      i += 2;
    } else out.push(bytes[i]);
  }
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(Uint8Array.from(out));
  } catch {
    return name; // not UTF-8 once decoded
  }
}

// A name's percent-encoded `/` and `\` as `_`, as Chrome and Firefox save them (naming.rs
// `escaped_separators`): a character of the name, never a folder.
function escapedSeparators(name) {
  return name.replace(/%(2f|5c)/gi, "_");
}

// `name` with its RFC 2047 encoded words (`=?charset?B|Q?text?=`, UTF-8 or ISO-8859-1) decoded,
// the space between two of them dropped; null when it has none, or one it can't read (naming.rs
// `encoded_words`).
function encodedWords(name) {
  const word = /=\?([^?\s]+)\?([bq])\?([^?\s]*)\?=/gi;
  let out = "";
  let at = 0;
  let any = false;
  for (let m; (m = word.exec(name)); ) {
    const text = wordText(m[1], m[2], m[3]);
    if (text === null) return null;
    const between = name.slice(at, m.index);
    // Only space between two encoded words isn't the name's.
    out += any && /^\s*$/.test(between) ? "" : between;
    out += text;
    at = m.index + m[0].length;
    any = true;
  }
  return any ? out + name.slice(at) : null;
}

// An encoded word's text: its bytes base64 (B) or quoted-printable with `_` for a space (Q), in
// UTF-8 or ISO-8859-1 (and US-ASCII); null for anything else.
function wordText(charset, kind, text) {
  let bytes;
  if (kind.toLowerCase() === "b") {
    try {
      bytes = Uint8Array.from(atob(text), (c) => c.charCodeAt(0));
    } catch {
      return null;
    }
  } else {
    const out = [];
    for (let i = 0; i < text.length; i++) {
      if (text[i] === "_") out.push(0x20);
      else if (text[i] === "=") {
        const hex = text.slice(i + 1, i + 3);
        if (!/^[0-9a-f]{2}$/i.test(hex)) return null;
        out.push(parseInt(hex, 16));
        i += 2;
      } else if (text.charCodeAt(i) < 0x80) out.push(text.charCodeAt(i));
      else return null;
    }
    bytes = Uint8Array.from(out);
  }
  // RFC 2231's language after a `*` (`UTF-8*en`) isn't the charset's.
  const cs = charset.split("*")[0].toLowerCase();
  if (cs === "utf-8" || cs === "utf8") {
    try {
      return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    } catch {
      return null;
    }
  }
  if (["iso-8859-1", "iso_8859-1", "latin1", "us-ascii"].includes(cs)) return String.fromCharCode(...bytes);
  return null;
}

// A response header's value as text. Firefox gives its bytes one character each (ISO-8859-1), so
// a name sent as raw UTF-8 (`filename="تقرير.pdf"`) arrived as `ØªÙ…` (#447). It is read as UTF-8
// when those bytes are valid UTF-8, as Firefox reads the name for its own downloads and the app
// for its own requests; bytes that aren't (ISO-8859-1's `café`) stay as they came.
function headerText(value) {
  if (!/[\x80-\xff]/.test(value) || /[^\x00-\xff]/.test(value)) return value;
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(Uint8Array.from(value, (c) => c.charCodeAt(0)));
  } catch {
    return value;
  }
}

// {name, size, mime, opened} when this answer to a page load becomes a download (or is one of
// the files Options take though the browser would open them), else null: a page, a picture, a
// redirect, an error, a piece of a file.
function answerFile(d) {
  const code = d.statusCode || 0;
  if (code < 200 || code > 299 || code === 204 || code === 205 || code === 206) return null;
  const headers = d.responseHeaders || [];
  const get = (n) => ((headers.find((h) => h.name.toLowerCase() === n) || {}).value || "").trim();
  const mime = get("content-type").split(";")[0].trim().toLowerCase();
  const cd = get("content-disposition");
  const attachment = /^attachment/i.test(cd);
  const opened = !attachment && openedTypes.some((m) => mime.startsWith(m));
  // No type: the browser guesses from the bytes (a page, mostly); a download it becomes is
  // still seen by onCreated.
  if (!attachment && !opened && (!mime || SHOWN.test(mime))) return null;
  let name = dispositionName(headerText(cd));
  if (!name) {
    try {
      name = decodeURIComponent(pathOf(d.url).split("/").pop() || "");
    } catch {
      name = pathOf(d.url).split("/").pop() || "";
    }
  }
  name = name.split(/[\\/]/).pop() || "";
  // The size the file will have: the length of an answer that isn't compressed on the way.
  const size = get("content-encoding") ? 0 : Number(get("content-length")) || 0;
  return { name, size, mime, opened };
}

// The page loads decided here (by their first and last addresses, two minutes): the download
// one of them becomes after the browser's dialog is left to the browser (asked once).
const decided = new Map();

// A tab opened for the download only (a link opened in a new tab or window), closed with it.
async function closeDownloadTab(tabId, urls) {
  forget(newTabs, 10000);
  if (!(tabId >= 0) || !newTabs.has(tabId)) return;
  try {
    const tab = await ext.tabs.get(tabId);
    const at = tab.url || "";
    if (at === "about:blank" || urls.includes(at)) {
      newTabs.delete(tabId);
      await ext.tabs.remove(tabId);
    }
  } catch {
    // Closed already.
  }
}

async function headersReceived(d) {
  if (d.method !== "GET" && d.method !== "POST") return {};
  const file = answerFile(d);
  if (!file) return {};
  const ref = referrers.get(d.requestId);
  const item = {
    url: d.url,
    tabId: d.tabId,
    filename: file.name,
    totalBytes: file.size,
    fileSize: file.size,
    mime: file.mime,
    referrer: (ref && ref.url) || "",
    incognito: Boolean(d.incognito),
    cookieStoreId: d.cookieStoreId,
  };
  if (PROXY_DECLARED) {
    await proxyChecked;
    noteProxy(d);
  }
  const first = firstUrl(item);
  forget(decided, 120000);
  decided.set(d.url, { at: Date.now() });
  decided.set(first, { at: Date.now() });
  const c = await candidate(item, d.url).catch(() => null);
  if (!c || c.left) {
    // A file the browser opens stays a page: there is no download to say anything about.
    if (!file.opened) leftAlone(c, item, d.url);
    return {};
  }
  const taken = await offer(item, d.url, c.name).catch(() => false);
  if (!taken) return {};
  // A frame's or an <embed>'s file leaves its page where it is.
  if (d.type === "main_frame") closeDownloadTab(d.tabId, [first, d.url]).catch(() => {});
  return { cancel: true };
}

if (FIREFOX && ext.webRequest && ext.webRequest.onHeadersReceived) {
  ext.webRequest.onHeadersReceived.addListener(
    (d) => headersReceived(d).catch(() => ({})),
    { urls: ["<all_urls>"], types: PAGE_LOADS },
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
  // The viewer's <video> may be a blob:/MediaSource, whose URL the page cannot fetch; the
  // browser's own download for the same message uses `/<k|a>/d/<media id>`, which the page's
  // service worker serves with the session. Try the element's own address first (a real Blob
  // works), then the page's download endpoint. Read the media object for its name and id.
  const info = (() => {
    let id = "";
    let name = "";
    let mime = "";
    let size = 0;
    try {
      const viewer = window.appMediaViewer;
      let doc = null;
      if (viewer && viewer.target) {
        const media = viewer.target.message && viewer.target.message.media;
        doc = media && (media.document || media.photo);
        if (!doc && viewer.target.photo) doc = viewer.target.photo;
      }
      if (!doc && typeof window.getGlobal === "function") {
        const global = window.getGlobal();
        const tabs = global && global.byTabId;
        if (tabs) {
          for (const tab of Object.values(tabs)) {
            const v = tab && tab.mediaViewer;
            if (!v) continue;
            let media = v.standaloneMedia && v.standaloneMedia[v.mediaIndex || 0];
            if (!media && v.chatId && v.messageId) {
              const chat = global.messages && global.messages.byChatId && global.messages.byChatId[v.chatId];
              const msg = chat && (chat.byId?.[v.messageId] || chat.ephemeralById?.[v.messageId]);
              const content = msg && msg.content;
              media = content && (content.photo || content.video || content.document || (content.action && content.action.photo));
            }
            const candidate = media && (media.document || media.photo || media);
            if (candidate && candidate.id) {
              doc = candidate;
              break;
            }
          }
        }
      }
      if (doc) {
        id = String(doc.id || "");
        name = String(doc.file_name || doc.fileName || "");
        if (!name && Array.isArray(doc.attributes)) {
          const file = doc.attributes.find((a) => a && (a.fileName || a.file_name));
          name = String((file && (file.fileName || file.file_name)) || "");
        }
        mime = String(doc.mime_type || doc.mimeType || "");
        size = Number(doc.size) || 0;
      }
    } catch (error) {
      // Fall back to the element's own address.
    }
    const prefix = location.pathname.startsWith("/a/") ? "/a" : "/k";
    return {
      id,
      name,
      mime,
      size,
      direct: /^(https?|blob):/i.test(src) ? src : "",
      endpoint: id ? `https://${location.host}${prefix}/d/${id}` : "",
    };
  })();
  const candidates = [...new Set([info.direct, info.endpoint].filter(Boolean))];
  (async () => {
    const CHUNK = 4 * 1024 * 1024;
    const post = (m, transfer) =>
      window.postMessage({ mdmTelegram: requestId, ...m }, "*", transfer || []);
    let offset = 0;
    let total = info.size > 0 ? info.size : null;
    let mime = info.mime;
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
    const usableType = (type) =>
      !type ||
      type.startsWith("video/") ||
      type.startsWith("audio/") ||
      type.startsWith("image/") ||
      type === "application/octet-stream" ||
      type === "binary/octet-stream";
    // The first response decides which address works: a blob created from media fetches whole;
    // a MediaSource blob (or a dead URL) fails, and the page's own download endpoint follows.
    let source = "";
    let first = null;
    for (const candidate of candidates) {
      try {
        const res = await fetch(candidate, {
          headers: { Range: `bytes=0-${CHUNK - 1}` },
          credentials: "include",
        });
        if (res.status !== 200 && res.status !== 206) continue;
        const type = (res.headers.get("content-type") || "")
          .split(";")[0]
          .trim()
          .toLowerCase();
        if (!usableType(type)) continue;
        source = candidate;
        first = res;
        break;
      } catch (error) {
        // Try the next address.
      }
    }
    if (!first) {
      post({ kind: "error", why: "Telegram did not serve the media" });
      return;
    }
    try {
      let res = first;
      while (true) {
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
        let data = await res.arrayBuffer();
        if (!data.byteLength) throw new Error("Telegram returned no media");
        if (total !== null && offset + data.byteLength > total) {
          data = data.slice(0, total - offset);
        }
        // Transferring the buffer detaches it here, so keep its size before posting.
        const received = data.byteLength;
        if (seq === 0) post({ kind: "start", total, mime, name: info.name });
        const ack = waitAck(seq);
        post({ kind: "chunk", seq, data }, [data]);
        await ack;
        offset += received;
        seq += 1;
        if (res.status === 200) break;
        if (total !== null && offset >= total) break;
        if (total === null && received < CHUNK) break;
        res = await fetch(source, {
          headers: { Range: `bytes=${offset}-${offset + CHUNK - 1}` },
          credentials: "include",
        });
        if (res.status !== 200 && res.status !== 206) {
          throw new Error(`Telegram answered ${res.status}`);
        }
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

// ---- a page's links, in parts ------------------------------------------------------------

// The most the host takes in one message, as the browser writes it (crates/ipc MAX_MESSAGE). A
// page's links past it (a folder's listing of 20,000 files) go in parts under it, each with the
// page's referrer and cookies, and MDM puts them back together into one list for Link grabber.
const MESSAGE_MAX = 1 << 20;
// The most parts one list goes in (crates/ipc MAX_LINK_PARTS).
const PARTS_MAX = 256;

// The most bytes `value` takes as JSON in the message the browser writes to the host: Firefox
// writes JSON.stringify's text in UTF-8; Chromium also writes < as \u003C, and DEL and the line
// and paragraph separators (U+2028, U+2029) escaped; Safari's helper (tools/safari) writes / as
// \/. Counted at the most any of them writes, so a part measured within MESSAGE_MAX is within it
// in every browser.
function wireSize(value) {
  const json = JSON.stringify(value);
  let n = 0;
  for (let i = 0; i < json.length; i++) {
    const c = json.charCodeAt(i);
    if (c === 0x3c || c === 0x7f) n += 6;
    else if (c === 0x2f) n += 2;
    else if (c < 0x80) n += 1;
    else if (c < 0x800) n += 2;
    // Each half of a surrogate pair: 4 bytes the pair (JSON.stringify escapes a lone one).
    else if (c >= 0xd800 && c <= 0xdfff) n += 2;
    else if (c === 0x2028 || c === 0x2029) n += 6;
    else n += 3;
  }
  return n;
}

// `links` cut into parts, in order, each of whose messages (`base` with its links) is at most
// `limit` bytes (wireSize). `left`: the links too long to go even alone, left out.
function linkParts(base, links, limit = MESSAGE_MAX) {
  const empty = wireSize({ ...base, links: [] });
  const parts = [];
  const left = [];
  let part = [];
  let size = empty;
  for (const link of links) {
    const own = wireSize(link);
    if (empty + own > limit) {
      left.push(link);
      continue;
    }
    // With the comma before it, after the part's first link.
    if (part.length && size + 1 + own > limit) {
      parts.push(part);
      part = [];
      size = empty;
    }
    size += own + (part.length ? 1 : 0);
    part.push(link);
  }
  if (part.length) parts.push(part);
  return { parts, left };
}

// A page's links (`message.links`, with its referrer, cookies and window) to the app: in one
// message when they fit, as before; else in parts, one after the other, each answered before the
// next goes. An answer that isn't ok stops it (the app drops a list it didn't get whole) and is
// the answer. `left`: how many links were too long to send at all (#too-large says so).
async function sendLinks(message) {
  const sent = await stamped(message);
  if (wireSize(sent) <= MESSAGE_MAX) return native(message);
  const batch = globalThis.crypto && crypto.randomUUID ? crypto.randomUUID() : `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  // Measured with the widest place a part can have.
  const { parts, left } = linkParts({ ...sent, linksPart: { batch, index: PARTS_MAX, count: PARTS_MAX } }, message.links);
  if (!parts.length || parts.length > PARTS_MAX) {
    // The page's cookies alone fill a message, or the list is beyond any page's.
    await noteStatus("too-large");
    return { ok: false, why: "too-large", error: `${message.links.length} links in ${parts.length} parts` };
  }
  for (const [index, links] of parts.entries()) {
    const reply = await native({ ...message, links, linksPart: { batch, index, count: parts.length } });
    if (!reply.ok) return reply;
  }
  return { ok: true, parts: parts.length, left: left.length };
}

// When the user asked and it couldn't be done: the help page's section opens beside (a link
// MDM refused has none of its own: the connection's).
const SECTIONS = ["not-installed", "forbidden", "not-running", "host-failed", "too-large", "update-app", "options"];
function explain(why) {
  const at = SECTIONS.includes(why) ? why : "host-failed";
  ext.tabs.create({ url: `${HELP}#${at}` }).catch(() => {});
}

// The right-click menu's items, clicked (named: tools/extension/e2e.mjs calls it in Chromium,
// whose menus a headless browser can't open).
async function menuClicked(info, tab) {
  const incognito = Boolean(tab && tab.incognito);
  const storeId = tab && tab.cookieStoreId;
  if (info.menuItemId === "mdm-link") {
    // Telegram: the page's own fetch (its session/service worker) streams the media into MDM's
    // local upload server; never hand the page-only URL to the app (302).
    if (tab && isTelegramUrl(tab.url)) {
      ext.tabs
        .sendMessage(
          tab.id,
          {
            type: "telegram-upload",
            src: "",
            title: tab.title || "",
          },
          { frameId: 0 },
        )
        .catch(() => {});
      return;
    }
    const url = info.linkUrl || info.srcUrl;
    // The tab's title, as the button's click sends it: the app names a video yt-dlp knows only
    // by its address (a stream's `master`, a long hash) after it (#345).
    const title = tabTitle(tab);
    // A player's own stream (blob:, a MediaSource) or a stream's manifest: the Download video
    // window, which reads streams (a manifest downloaded as a file is only a short list).
    // The manifest's own cookies (its server may not be the page's), as the media list sends.
    if (url && MANIFEST.test(url)) {
      await mediaBack;
      const reply = await download("video", { video: true, explicit: true, url: info.pageUrl, src: url, title, ...(await streamContext(tab && tab.id, url, { incognito, storeId })), private: incognito });
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
      const reply = await download("video", {
        video: true,
        explicit: true,
        url: page,
        ...(src ? { src } : {}),
        ...(title ? { title } : {}),
        referrer: info.pageUrl && info.pageUrl !== page ? info.pageUrl : "",
        ...(await streamContext(tab && tab.id, src || page, { incognito, storeId })),
        private: incognito,
      });
      if (!reply.ok && reply.why !== "options") explain(reply.why);
      return;
    }
    // ftp links too: browsers no longer download them, MDM does.
    if (!url || !/^(https?|ftps?|ftpes):/i.test(url)) return;
    const reply = await handOff({ url, referrer: info.pageUrl, page: info.pageUrl, incognito, storeId, explicit: true });
    if (!reply.ok) explain(reply.why);
    return;
  }
  if ((info.menuItemId === "mdm-selected" || info.menuItemId === "mdm-all") && tab) {
    // Telegram's page menu ("Download all links...") would collect the viewer's thumbnails and
    // media fragments (the old "8 files"): on Telegram, every Mister menu entry downloads the
    // media the viewer is showing, through MDM.
    if (isTelegramUrl(tab.url)) {
      ext.tabs
        .sendMessage(
          tab.id,
          { type: "telegram-upload", src: "", title: tab.title || "" },
          { frameId: 0 },
        )
        .catch(() => {});
      return;
    }
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
    const reply = await sendLinks({ links, referrer: info.pageUrl, cookies: await cookieHeader(info.pageUrl, { incognito, storeId }), private: incognito });
    // Sent, but a link longer than a whole message was left out: the help page says so.
    if (!reply.ok || reply.left) explain(reply.ok ? "too-large" : reply.why);
  }
}
ext.contextMenus.onClicked.addListener(menuClicked);

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
  // Downloads MDM runs from before the browser started: counted at once.
  watchRecent();
});

// A match pattern's host and path as a test (`*://*.youtube.com/*`).
function matcher(pattern) {
  const [, host = "", path = "/*"] = /^[^:]+:\/\/([^/]*)(\/.*)?$/.exec(pattern) || [];
  const glob = (s) => s.split("*").map((x) => x.replace(/[.+?^${}()|[\]\\]/g, "\\$&")).join(".*");
  const h = host.startsWith("*.") ? new RegExp(`^(.*\\.)?${glob(host.slice(2))}$`, "i") : new RegExp(`^${glob(host)}$`, "i");
  const pth = new RegExp(`^${glob(path)}$`);
  return (u) => h.test(u.hostname) && pth.test(u.pathname + u.search);
}

// Whether this copy of the extension leaves the video button off `url` (the Chrome Web Store's
// copy: its policy, tools/extension/pack.ps1). Read from its manifest, so the popup says what is true.
function videoKeptOff(url) {
  const entry = (ext.runtime.getManifest().content_scripts || []).find((e) => (e.js || []).includes("video.js"));
  try {
    const u = new URL(url);
    return ((entry && entry.exclude_matches) || []).some((p) => matcher(p)(u));
  } catch {
    return false;
  }
}

// Pages open before the extension was installed or updated get its scripts now (else the
// button waits for a reload).
async function injectOpenTabs() {
  if (!ext.scripting) return;
  const tabs = await ext.tabs.query({ url: ["http://*/*", "https://*/*"] }).catch(() => []);
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
// menu. The app decides through the native host, from Options as saved now; its answer for a
// site is kept a few seconds (the page's frames ask together, and each page asks again when its
// answer is older: video.js ASK_MS), so a change in Options reaches the pages already open
// (#532: it was kept a minute, and by the page for good). A failure to reach it isn't kept.
// `top`: the tab's address, when the frame asking is another page's (a player embedded from
// another site): the button stays off there too when the tab's site is listed. With it, its size
// (#1278: Options → Download panels → Button size): `compact`, the round button over every video.
const PANEL_KEEP_MS = 5000;
const panelCache = new Map();
async function panelOn(page, top) {
  // The host name without a port: as video.js names the site, and as the app matches sites.
  const site = new URL(page).hostname;
  const tabSite = top ? new URL(top).hostname : "";
  const key = tabSite && tabSite !== site ? `${site} ${tabSite}` : site;
  forget(panelCache, PANEL_KEEP_MS);
  if (panelCache.get(site)?.hidden) return { on: false, compact: false };
  const kept = panelCache.get(key);
  if (kept) return { on: kept.on, compact: kept.compact };
  // Always this computer's MDM's Options: a question, never a download (`download`). With another
  // MDM chosen and none here to ask, the button shows, as Options have it at first: the other
  // MDM's remote control adds and lists downloads, it has no Options to ask.
  let reply = await native({ panel: true, url: page, ...(key !== site ? { referrer: top } : {}) });
  if (!reply.ok && reply.why && (await remoteTarget())) reply = { ok: true };
  const answer = { on: Boolean(reply.ok), compact: Boolean(reply.ok && reply.compact) };
  if (reply.ok || !reply.why) panelCache.set(key, { ...answer, at: Date.now() });
  return answer;
}

// The page the message came from, as the browser knows it (never what the page says). The top
// frame's is its tab's address, which follows a page that moves in place (history.pushState:
// YouTube, most video sites). `sender.url` is the document's and goes stale there: in Chromium it
// stays the address the document loaded at, in Firefox it lags a navigation behind, so the button
// sent the video watched before this one.
function senderPage(sender) {
  const top = sender.frameId === 0 && sender.tab && sender.tab.url;
  const u = top || sender.url || (sender.tab && sender.tab.url) || "";
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

// A YouTube page's player, asked for what it plays. In the page's own world, the only place its
// player's API exists (content scripts are isolated from it, and an injected <script> is refused
// by its Trusted Types; `scripting` isn't). Run in the page: no outside names, only its arguments.
function playingAt(x, y) {
  let best = null;
  for (const v of document.querySelectorAll("video")) {
    const r = v.getBoundingClientRect();
    // One with no box (YouTube's main player under its home page, a stopped preview) is at no
    // point: its box is the window's corner (#1271).
    if (!(r.width > 0 && r.height > 0)) continue;
    if (x < r.left || x > r.right || y < r.top || y > r.bottom) continue;
    if (!best || r.width * r.height < best.area) best = { v, area: r.width * r.height };
  }
  const player = best && best.v.closest(".html5-video-player");
  try {
    return player && player.getVideoData ? String(player.getVideoData().video_id || "") : "";
  } catch {
    return "";
  }
}

const isYouTube = (url) => /(^|\.)(youtube\.com|youtube-nocookie\.com|youtu\.be)$/i.test(siteOf(url));
const isDrive = (url) => /(^|\.)(?:drive|docs)\.google\.com$/i.test(siteOf(url));
const isDriveFile = (url) =>
  /\/file\/d\/[\w-]{25,}/.test(url) ||
  (/\/(?:uc|open)\b/.test(url) && /[?&]id=[\w-]{25,}/.test(url));

// The watch address of what the player at `at` (the video's middle, in the sender's frame) plays,
// or "": a page without such a player, one too old for `world`, a page extensions may not read.
async function playingVideo(sender, at) {
  if (!at || !ext.scripting || !sender.tab) return "";
  try {
    const [found] = await withTimeout(
      ext.scripting.executeScript({
        target: { tabId: sender.tab.id, frameIds: [sender.frameId || 0] },
        world: "MAIN",
        func: playingAt,
        args: [Number(at.x) || 0, Number(at.y) || 0],
      }),
      3000,
    );
    const id = found && found.result;
    return /^[\w-]{6,32}$/.test(id || "") ? `https://www.youtube.com/watch?v=${id}` : "";
  } catch {
    return "";
  }
}

// A site's name for comparing two addresses: without its www., mobile or other front host
// (old.reddit.com is reddit.com), its short-link hosts as the site (fb.watch is Facebook's,
// twitter.com is x.com).
const SITE_ALIASES = { "fb.watch": "facebook.com", "twitter.com": "x.com" };
function siteKey(url) {
  const host = siteOf(url).replace(/^(www|m|mobile|web|touch|old|new|sh|music|geo|player)\./, "");
  return SITE_ALIASES[host] || host;
}

// The address the app looks a video up by: the one video's, never a list's. On YouTube that is
// what the player plays; else the address the content script named (video.js videoPage: the post
// the video sits in on a feed, #689), if it is this site's; else the page's. "" where nothing
// names the video: on YouTube (its own address is a list, the home page, a channel, to yt-dlp),
// and on a feed (`url` null: video.js found the page is many videos', LinkedIn's or Facebook's
// home, and no post around the video naming it).
async function videoAddress(m, page, sender) {
  const named = typeof m.url === "string" && /^https?:/i.test(m.url) ? m.url : "";
  if (isYouTube(page)) {
    const playing = await playingVideo(sender, m.at);
    return playing || (isYouTube(named) ? named : "");
  }
  if (isDrive(page)) {
    if (isDriveFile(named)) return named;
    if (isDriveFile(page)) return page;
    return "";
  }
  if (m.url === null) return "";
  return named && siteKey(named) === siteKey(page) ? named : page;
}

// The title of the page the button's video is on, without its site's name (withoutSiteName): the
// app names the video by it when yt-dlp knows it only by its address (a stream's `playlist`,
// #345). A player in a frame of the page (an embed) has a title of its own, its site's ("Before The
// - Short Film from Justin Barry on Vimeo"): the tab's names its video, as the right-click menu and
// the toolbar's media list name it; the frame's only when the tab has none.
function shownTitle(m, page, sender) {
  const tab = sender.tab || {};
  const framed = Boolean(sender.frameId) && Boolean(tab.title);
  const title = framed ? tab.title : typeof m.title === "string" ? m.title : "";
  return withoutSiteName(title, framed ? tab.url : page).slice(0, 300);
}

// What the button's click is about, as the browser knows it: the address yt-dlp is given (`own`,
// the video's page), the video's own file or stream (`src`), the page's title, the tab as
// referrer, and the cookies of what yt-dlp asks. Null when nothing names the video.
async function videoTarget(m, page, sender) {
  const tab = sender.tab || {};
  await mediaBack;
  const named = await videoAddress(m, page, sender);
  const youtube = isYouTube(page);
  // The post a video in a feed sits in (#689): the app looks that up. Nothing names it (`url`
  // null): the feed is never sent as its page, only the video's own plain file, if it has one.
  // Either way no stream the tab loaded goes with it (a feed's first stream may be another
  // video's), nor the page's title (the feed's).
  const post = !youtube && Boolean(named) && named !== page;
  const feed = !youtube && m.url === null;
  // The video's own file (video.js leaves it out beside a post a big site's reader knows), or a
  // stream it plays, still goes as `src`, with the page for its Referer.
  const file = typeof m.src === "string" && /^https?:/i.test(m.src) ? m.src : "";
  // #1092: what the tab played for this video, found when its page couldn't be looked up
  // (`video-played`): goes as its `src`, a feed's or a post's too, with the page's title (the
  // only one there is for it).
  const played = playedItem(tab.id, m.played, sender.frameId || 0);
  const src = file || (played ? played.url : "") || (post || feed ? "" : await streamFor(sender));
  if (!named && !src) return null;
  const own = named || page;
  return {
    own,
    src,
    // The post's own page (#689), for the button's "Open post".
    post: post ? own : "",
    // The video's own plain file, sent as itself (#703).
    file,
    title: played || (!post && !feed) ? shownTitle(m, page, sender) : "",
    referrer: tab.url && tab.url !== own ? tab.url : "",
    // The cookies of what yt-dlp is given (the app files them under its host): the stream's
    // own server may not be the page's, and the page's cookies are not for it. With a stream's,
    // what the browser sent with it (#900).
    ...(await streamContext(tab.id, src || own, { incognito: tab.incognito, storeId: tab.cookieStoreId })),
  };
}

// ---- the button's qualities: the app looks the video up, the page lists what it found --------
// The click asks the app to look the video up (yt-dlp, in the background), video.js asks how it
// is going until it is done, and the quality picked is downloaded without the app's window. Only
// with an app that says it can (`videoLookup`, noteLookup) and never for another MDM: else
// ({fallback: true}) the click opens the app's Download video window, as it always did. None of
// these messages has `video` or a top-level `url`: an older host refuses them and does nothing.
// What a lookup comes to travels in `lookup`, never in `why`: the toolbar button's status (and
// the rule for PDFs and music that follows it) is the host's, not a video's.

async function videoLookup(m, page, sender) {
  if (await remoteTarget()) return { fallback: true };
  if (!(await readStatus()).videoLookup) {
    // An MDM updated since its rules were last asked may look up now: they are asked again,
    // for the next click.
    const r = await storedRules();
    if (!r || Date.now() - r.at > 60000) refreshRules();
    return { fallback: true };
  }
  const tab = sender.tab || {};
  const v = await videoTarget(m, page, sender);
  if (!v) return { ok: false, why: isYouTube(page) ? "unnamed" : "feed" };
  const reply = await native({
    videoLookup: {
      page: v.own,
      ...(v.src ? { src: v.src } : {}),
      ...(v.title ? { title: v.title } : {}),
      ...(v.referrer ? { referrer: v.referrer } : {}),
      // The lookup is the window's and the container's it was asked from (the app gives its
      // pick to no other).
      ...(tab.cookieStoreId ? { storeId: tab.cookieStoreId } : {}),
    },
    cookies: v.cookies,
    cookieJar: v.cookieJar,
    ...(v.fetchedWith ? { fetchedWith: v.fetchedWith } : {}),
    private: Boolean(tab.incognito),
  });
  // `show`: the address MDM lists a download of it under (the button's "Show"); `post`: the
  // post's own page, when the video sits in a feed (its "Open post").
  if (reply.lookup && typeof reply.lookup === "object") return { lookup: reply.lookup, show: v.src || v.own, ...(v.post ? { post: v.post } : {}) };
  // No `lookup`: a host or an app that can't look up after all (an older one refuses the message),
  // or a host that failed. Not kept as able; its rules are asked again, and this click opens
  // the window.
  await noteLookup(false);
  refreshRules();
  return { fallback: true };
}

// The token is the app's, from the lookup's answer; it means nothing to anyone else.
const lookupToken = (token) => typeof token === "string" && token.length > 0 && token.length <= 200;

async function videoPoll(m) {
  if (await remoteTarget()) return { fallback: true };
  if (!lookupToken(m.token)) return { lookup: { state: "gone" } };
  const reply = await native({ videoPoll: { token: m.token } });
  return { lookup: reply.lookup && typeof reply.lookup === "object" ? reply.lookup : { state: "gone" } };
}

async function videoPick(m, sender) {
  if (await remoteTarget()) return { fallback: true };
  if (!lookupToken(m.token) || !Number.isInteger(m.choice) || m.choice < 0) return { fallback: true };
  const tab = sender.tab || {};
  // The token and the row's number from the page, nothing else: private or not and the
  // container are the tab's, as the browser says; the app checks them against the lookup's.
  const reply = await native({
    videoPick: { token: m.token, choice: m.choice, ...(tab.cookieStoreId ? { storeId: tab.cookieStoreId } : {}) },
    private: Boolean(tab.incognito),
  });
  return reply.lookup && typeof reply.lookup === "object" ? { lookup: reply.lookup } : { fallback: true };
}

const isWeb = (u) => typeof u === "string" && /^https?:\/\//i.test(u);
// Where the button's "Get MDM" goes.
const GET_MDM = "https://misterdownloadmanager.com/download";
// A new tab next to `tab`, opened from it.
const beside = (tab) => ({
  ...(Number.isInteger(tab.id) ? { openerTabId: tab.id } : {}),
  ...(Number.isInteger(tab.index) ? { index: tab.index + 1 } : {}),
});

async function pageMessage(m, sender) {
  // YouTube changes the video in the same page (a history entry, not a new document): the
  // content script's message carries the video's current address; `sender.url` can still be
  // the first video's, which made the button send video 1 after switching to video 2.
  const page =
    m.type === "video" && typeof m.page === "string" && /^https?:/i.test(m.page)
      ? m.page
      : await currentPage(sender);
  const tab = sender.tab || {};
  // #758: on Google Drive, pointer movement inside a player's subframe informs the top frame
  // so the floating button stays visible while hovering over the cross-origin player.
  if (m.type === "drive-frame-pointer") {
    const tabId = tab.id;
    const tabUrl = tab.url || "";
    const senderUrl = sender.url || "";
    const isPlayer =
      (/(^|\.)youtube\.googleapis\.com$/i.test(siteOf(senderUrl)) &&
        (/[?&](?:driveid|docid)=/.test(senderUrl) || /[?&]ps=docs\b/.test(senderUrl))) ||
      (isDrive(senderUrl) && (/\/file\/d\/[^/]+\/preview/.test(senderUrl) || /\/viewer\/main\b/.test(senderUrl)));
    if (
      Number.isInteger(tabId) &&
      Number.isInteger(sender.frameId) &&
      sender.frameId !== 0 &&
      isDrive(tabUrl) &&
      isPlayer
    ) {
      ext.tabs.sendMessage(tabId, { type: "drive-frame-pointer" }, { frameId: 0 }).catch(() => {});
      return { ok: true };
    }
    return { ok: false };
  }
  if (m.type === "panel") {
    const top = tab.url && /^https?:/i.test(tab.url) ? tab.url : "";
    return page ? await panelOn(page, top) : { on: false, compact: false };
  }
  if (m.type === "media-server") {
    const r = await native({ media: true });
    return { ok: Boolean(r && r.ok), port: r && r.mediaPort, token: r && r.mediaToken };
  }
  if (m.type === "telegram-fetch" && page && isTelegramUrl(page) && sender.tab) {
    const [r] = await ext.scripting.executeScript({
      target: { tabId: sender.tab.id, frameIds: [sender.frameId || 0] },
      world: "MAIN",
      func: telegramFetchInPage,
      args: [String(m.requestId || ""), String(m.src || "")],
    });
    return { ok: Boolean(r && r.result && r.result.ok) };
  }
  if (m.type === "link" && typeof m.url === "string") {
    const name = typeof m.name === "string" ? m.name.split(/[\\/]/).pop().slice(0, 255) : "";
    return link(m.url, page || "", Boolean(tab.incognito), name);
  }
  // Safari's page.js, at each page: the rules it holds links by, refreshed when old (see `link`).
  if (m.type === "rules") {
    const r = await storedRules();
    if ((!r || Date.now() - r.at > 60000) && Date.now() - rulesAskedAt > 60000) {
      rulesAskedAt = Date.now();
      refreshRules().catch(() => {});
    }
    return { ok: true };
  }
  // Alt held on a link: as the button went down, and at the click (page.js).
  if (m.type === "bypass" && typeof m.url === "string") {
    const was = bypass.get(m.url);
    bypass.set(m.url, { at: Date.now(), tab: tab.id, page: page || "", used: Boolean(was && was.used) });
    return { ok: true };
  }
  if (m.type === "telegram-download" && page && isTelegramUrl(page)) {
    // The content script does the upload (the page's media fetch + MDM's local server); this
    // path is only for an old copy of video.js. Never hand the page-only URL to the app.
    return { ok: false, why: "options" };
  }
  if (m.type === "video" && page) {
    const v = await videoTarget(m, page, sender);
    if (!v) return { ok: false, why: isYouTube(page) ? "unnamed" : "feed" };
    // The video's own plain file (#703): downloaded as a file, no window; with the type its
    // server said when the tab loaded it, which names a script's answer at once (#800).
    const direct = Boolean(m.direct) && Boolean(v.file) && v.file === v.src;
    // A file a player fetched in ranges (#1092) as one it plays itself.
    const loaded = direct && playedItem(tab.id, v.src);
    const reply = await download("video", {
      video: true,
      url: v.own,
      ...(v.src ? { src: v.src } : {}),
      ...(direct ? { direct: true } : {}),
      ...(loaded && loaded.mime ? { mime: loaded.mime } : {}),
      title: v.title,
      referrer: v.referrer,
      cookies: v.cookies,
      cookieJar: v.cookieJar,
      ...(v.fetchedWith ? { fetchedWith: v.fetchedWith } : {}),
      ...(tab.incognito ? { private: true } : {}),
    });
    return { ok: Boolean(reply.ok), why: reply.why, ...(reply.ok && direct ? { show: v.src } : {}) };
  }
  if (m.type === "video-lookup" && page) return videoLookup(m, page, sender);
  // #1092: what the tab played for the video under the button, its page found nothing.
  if (m.type === "video-played" && page) return videoPlayed(m, sender);
  // The button's "Show" (#703): MDM's window with that download selected.
  if (m.type === "show-download" && isWeb(m.address)) {
    return { ok: Boolean((await native({ showDownload: m.address })).ok) };
  }
  // The button's "Turn on" (after its "Never on this site", or off here in Options): on again.
  if (m.type === "show-site" && page) {
    const host = new URL(page).hostname;
    for (const k of [...panelCache.keys()]) if (k.split(" ").includes(host)) panelCache.delete(k);
    return { ok: Boolean((await native({ showSite: host })).ok) };
  }
  // The button's "Get MDM" (MDM didn't answer): its download page, beside this tab.
  if (m.type === "get-mdm") {
    await ext.tabs.create({ url: GET_MDM, ...beside(tab) });
    return { ok: true };
  }
  // The button's "Open post": the post a video in a feed sits in (#689), on this page's site.
  if (m.type === "open-post" && page && isWeb(m.url) && siteKey(m.url) === siteKey(page)) {
    await ext.tabs.create({ url: m.url, ...beside(tab) });
    return { ok: true };
  }
  if (m.type === "video-poll") return videoPoll(m);
  if (m.type === "video-pick") return videoPick(m, sender);
  if (m.type === "hide-site" && page) {
    const host = new URL(page).hostname;
    // Off at once on this site, whatever the tab around it, while the app saves it.
    panelCache.set(host, { on: false, hidden: true, at: Date.now() });
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
    const remotes = (await readRemotes()).map((x) => x.url);
    if (remote) return { connected: true, why: null, remote: remote.url, remotes, version: VERSION, paused, declined };
    return { connected: Boolean(r) && (!s.why || s.why === "update-app"), why: s.why, app: s.app, remote: null, remotes, version: VERSION, paused, declined };
  }
  if (m.type === "remote-pair" && typeof m.url === "string" && typeof m.code === "string") return remotePair(m.url, m.code);
  if (m.type === "remote-select" && typeof m.url === "string") return remoteSelect(m.url);
  if (m.type === "remote-forget" && typeof m.url === "string") return remoteForget(m.url);
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
  // The popup's Undo (#404): the site taken off the excluded sites again, as it was added.
  if (m.type === "include-site" && typeof m.host === "string" && m.host) {
    const reply = await native({ includeSite: m.host });
    await refreshRules();
    return { ok: Boolean(reply.ok) };
  }
  // Are this site's downloads left to the browser by Options' excluded sites (#404), and is it
  // listed by its own name (Undo takes that off; a pattern of the user's own stays)?
  if (m.type === "site-excluded" && typeof m.host === "string" && m.host) {
    const r = await currentRules();
    const host = m.host.toLowerCase();
    const sites = ((r && r.excludedSites) || "").split(/\s+/).filter(Boolean);
    const undo = sites.some((p) => p.toLowerCase() === host);
    return { excluded: undo || Boolean(r && siteListed(r, host)), undo };
  }
  if (m.type === "open-app") return { ok: Boolean((await native({ open: true })).ok) };
  // The Downloads tab's buttons on a download (#1496).
  if (m.type === "download-act" && typeof m.act === "string" && Number.isInteger(m.id)) return recentAct(m.act, m.id);
  if (m.type === "open-options") return { ok: Boolean((await native({ openOptions: true, page: "general" })).ok) };
  // The media the popup's tab loaded (#38), and one of them handed to the app.
  if (m.type === "media" && Number.isInteger(m.tab)) {
    const list = await mediaList(m.tab);
    const tab = await ext.tabs.get(m.tab).catch(() => null);
    return {
      media: list,
      // Why the list is empty, when there's more to say than "nothing yet": this copy keeps the
      // video button off the site, or the player streams pieces (blob:) no address can name.
      keptOff: Boolean(tab && videoKeptOff(tab.url)),
      streamed: !list.length && (await playsStream(m.tab)),
    };
  }
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
// This script started again while MDM ran downloads (the browser stops it when idle): counted on.
if (ext.storage.session) {
  ext.storage.session
    .get({ running: 0 })
    .then((v) => (Number(v.running) > 0 ? ((runningNow = Number(v.running)), watchRecent()) : null))
    .catch(() => {});
}
// Rules kept from before, else asked now (a first start, storage cleared).
storedRules().then((r) => (r ? syncOpenedRule() : refreshRules()));
