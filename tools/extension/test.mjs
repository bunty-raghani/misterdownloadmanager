// Runs extension/background.js against fake Firefox and Chrome APIs (no browser needed):
//   node tools/extension/test.mjs
import { readFileSync, readdirSync } from "node:fs";
import vm from "node:vm";
import assert from "node:assert/strict";

// A file's text with LF line endings: a Windows checkout (core.autocrlf=true) hands us CRLF, and
// the tests slice and match sources, JSON and locales by "\n".
const readText = (url) => readFileSync(url instanceof URL ? url : new URL(url, import.meta.url), "utf8").replace(/\r\n/g, "\n");

const code = readText("../../extension/background.js");
const DEFAULT_TYPES = "3GP 7Z AAC EXE ISO MP3 MP4 MSI PDF R0* RAR ZIP";

// A fake extension API; `firefox` picks which download event exists. `reply` is the app's
// answer to a download; `types` its Options -> File types when the rules are asked (`host: false`:
// the host isn't registered and every native message fails, as the browser says then).
// `appName`: what runtime.getBrowserInfo says (Firefox's family only; "Zen").
// `session`: storage.session's contents (shared between two loads: the browser stopped the
// script and started it again); `frames`: what each frame's players are (mediaElements);
// `tabs`: tabs.get's answers by id. `proxyPerm`: how the manifest has the "proxy" permission
// ("optional": Firefox's; "required"; none: Chrome's, it can't be optional there), `proxyGranted`
// whether it is granted (a required one always is), `proxyAnswer` what permissions.request says.
function fakeApi({
  firefox,
  // Safari: no downloads API at all (nor proxy and declarativeNetRequest).
  safari,
  reply = { ok: true },
  // What the page's own player says it plays (video.js's button on YouTube); undefined: it can't be read.
  playing,
  // The video button's exclude_matches (the Chrome Web Store's copy).
  videoExcludes,
  types = DEFAULT_TYPES,
  host = true,
  finished,
  panelOn,
  // Options → Download panels → Button size (#1278): Compact (a function: the app's answer now);
  // undefined: an older host, which doesn't say.
  panelCompact,
  access = true,
  rules = {},
  rulesReply,
  appName,
  proxy,
  session,
  frames,
  tabs = {},
  proxyPerm = firefox ? "optional" : undefined,
  proxyGranted = false,
  proxyAnswer = true,
  proxyOn = false,
  noticeReply,
  // The host's answer to the video button's lookups (`videoLookup`, `videoPoll`, `videoPick`): a
  // function of the message; none: `reply`.
  lookup,
  // The page's links, as the right-click menu's script collects them (pageLinks).
  pageLinks = ["https://x/1.zip", "https://x/2.zip"],
  // tabs.query's answer for the active tab of the window last used (#607).
  activeTab,
  // runtime.connectNative (#1496): a port to the host, answered as sendNativeMessage answers.
  nativePort = false,
}) {
  const raw = [];
  // Objects made inside the sandbox have its prototypes: compare plain copies.
  const calls = { push: (c) => raw.push(JSON.parse(JSON.stringify(c))) };
  const listeners = {};
  // Every listener of an event is called (background.js has two for some); the answer is the
  // last one given.
  const event = (name) => {
    const fns = [];
    return {
      addListener: (f, ...spec) => {
        fns.push(f);
        listeners[name] = (...a) => fns.reduce((r, g) => g(...a) ?? r, undefined);
        // Its filter and extra info, as the listener asked (#900: Chromium's extraHeaders).
        listeners[`${name}Spec`] = spec;
      },
    };
  };
  // `proxyOn`: Chromium's proxy switch (#42), stored by the popup.
  const store = proxyOn ? { proxyOn: true } : {};
  const granted = { proxy: proxyGranted };
  const downloads = {
    pause: async (id) => calls.push(["pause", id]),
    cancel: async (id) => calls.push(["cancel", id]),
    erase: async (q) => calls.push(["erase", q.id]),
    resume: async (id) => calls.push(["resume", id]),
    // `finished`: the download completed before the extension could stop it (a small file).
    search: async ({ id }) => [{ id, state: finished ? "complete" : "interrupted", exists: true }],
    removeFile: async (id) => calls.push(["removeFile", id]),
  };
  if (firefox) downloads.onCreated = event("created");
  else downloads.onDeterminingFilename = event("determining");
  const api = {
    downloads,
    storage: {
      local: {
        get: async (defaults) => ({ ...defaults, ...JSON.parse(JSON.stringify(store)) }),
        set: async (v) => {
          const changes = Object.fromEntries(Object.entries(v).map(([k, n]) => [k, { oldValue: store[k], newValue: n }]));
          Object.assign(store, JSON.parse(JSON.stringify(v)));
          listeners.storageChanged && listeners.storageChanged(changes, "local");
        },
        remove: async (k) => [k].flat().forEach((key) => delete store[key]),
      },
      onChanged: event("storageChanged"),
      ...(session
        ? {
            session: {
              get: async (defaults) => ({ ...defaults, ...JSON.parse(JSON.stringify(session)) }),
              set: async (v) => Object.assign(session, JSON.parse(JSON.stringify(v))),
            },
          }
        : {}),
    },
    cookies: {
      getAll: async (q) => {
        calls.push(["cookies", q]);
        // As Chrome gives one: its other fields aren't sent.
        return [{ name: "sid", value: "1", domain: ".tube.test", hostOnly: false, path: "/", secure: true, httpOnly: true, sameSite: "lax", session: false, expirationDate: 1893456000.5, storeId: "0" }];
      },
    },
    permissions: {
      // The "proxy" permission (#42), else a site's access.
      contains: async (q) => {
        if (!(q.permissions || []).includes("proxy")) return access;
        calls.push(["contains-proxy"]);
        return proxyPerm === "required" || (proxyPerm === "optional" && granted.proxy);
      },
      request: async (q) => {
        calls.push(["request", q.permissions]);
        if (proxyPerm !== "optional" || !proxyAnswer) return false;
        granted.proxy = true;
        listeners.permAdded && listeners.permAdded(q);
        return true;
      },
      remove: async (q) => {
        calls.push(["remove", q.permissions]);
        granted.proxy = false;
        listeners.permRemoved && listeners.permRemoved(q);
        return true;
      },
      onAdded: event("permAdded"),
      onRemoved: event("permRemoved"),
    },
    runtime: {
      sendNativeMessage: async (name, m) => {
        if (!host) throw new Error("Specified native messaging host not found.");
        if (m.rules) {
          calls.push(["rules", m]);
          // `rulesReply`: the whole answer, as an older app's host gives it.
          if (rulesReply) return rulesReply;
          return { ok: true, version: "0.2.5", types, capture: true, excludedSites: "*.update.microsoft.com", excludedAddresses: [], opened: true, ...rules };
        }
        calls.push(["native", name, m]);
        // The video button: "may it show" follows `panelOn` (a function: the app's answer now).
        if (m.panel) {
          const on = typeof panelOn === "function" ? panelOn(m) : panelOn !== false;
          const compact = typeof panelCompact === "function" ? panelCompact(m) : panelCompact;
          return { ok: on, ...(on && compact !== undefined ? { compact } : {}) };
        }
        // A notice of a download left alone (#208): an older host, which knows none, takes it
        // for an address it can't download (`noticeReply`).
        if (m.declined) return noticeReply || { ok: true };
        if (lookup && (m.videoLookup || m.videoPoll || m.videoPick)) return lookup(m);
        // A function: the app's answer to each message (a page's links' parts, one by one; it may
        // change its rules, as the app would).
        return typeof reply === "function" ? reply(m) : reply;
      },
      getManifest: () => ({
        version: "0.2.5",
        content_scripts: [{ js: ["video.js"], ...(videoExcludes ? { exclude_matches: videoExcludes } : {}) }, { js: ["page.js"] }],
        permissions: [...(safari ? [] : ["downloads"]), ...(proxyPerm === "required" ? ["proxy"] : [])],
        ...(safari ? { browser_specific_settings: { safari: {} } } : {}),
        ...(proxyPerm === "optional" ? { optional_permissions: ["proxy"] } : {}),
      }),
      onInstalled: event("installed"),
      onStartup: event("startup"),
      onMessage: event("message"),
      onConnect: event("connect"),
      id: "ours",
      getURL: (p) => `chrome-extension://ours/${p}`,
    },
    tabs: {
      onCreated: event("tabCreated"),
      onRemoved: event("tabRemoved"),
      onUpdated: event("tabUpdated"),
      get: async (id) => tabs[id] || { id, url: "about:blank", status: "complete" },
      remove: async (id) => calls.push(["closeTab", id]),
      query: async (q) => (q && q.active && activeTab ? [activeTab] : []),
      create: async (t) => calls.push(["openTab", t.url]),
      sendMessage: async (id, m, opts) => calls.push(["tabMessage", id, m, opts]),
    },
    webRequest: {
      onBeforeRequest: event("beforeRequest"),
      onBeforeSendHeaders: event("beforeSendHeaders"),
      onSendHeaders: event("sendHeaders"),
      onErrorOccurred: event("errorOccurred"),
      onHeadersReceived: event("headersReceived"),
      onBeforeRedirect: event("beforeRedirect"),
      onResponseStarted: event("responseStarted"),
      onAuthRequired: event("authRequired"),
    },
    scripting: {
      executeScript: async ({ func }) => {
        if (func && func.name === "playingAt") {
          if (playing === undefined) throw new Error("Cannot access contents of the page");
          return [{ result: playing }];
        }
        if (func && func.name === "mediaElements") {
          if (!frames) throw new Error("Cannot access contents of the page");
          return Object.entries(frames).map(([frameId, result]) => ({ frameId: Number(frameId), result }));
        }
        return func ? [{ result: pageLinks }] : [];
      },
    },
    action: {
      setBadgeText: async (b) => calls.push(["badge", b.text]),
      setBadgeBackgroundColor: async () => {},
      setTitle: async () => {},
    },
    contextMenus: {
      create: (item, done) => {
        calls.push(["menu-item", item]);
        if (done) done();
      },
      removeAll: (done) => done(),
      onClicked: event("menu"),
    },
    // The browser's language: German here.
    i18n: { getMessage: (key) => ({ menu: "Mit Mister Download Manager herunterladen" })[key] ?? "" },
  };
  if (nativePort) {
    api.runtime.connectNative = (name) => {
      calls.push(["port-open"]);
      const heard = [];
      const ended = [];
      return {
        onMessage: { addListener: (f) => heard.push(f) },
        onDisconnect: { addListener: (f) => ended.push(f) },
        postMessage: (m) => {
          api.runtime.sendNativeMessage(name, m).then((r) => setTimeout(() => heard.forEach((f) => f(JSON.parse(JSON.stringify(r)))), 1));
        },
        disconnect: () => calls.push(["port-closed"]),
      };
    };
  }
  if (safari) delete api.downloads;
  if (!firefox && !safari) api.declarativeNetRequest = { getSessionRules: async () => [], updateSessionRules: async (r) => calls.push(["dnr", r]) };
  if (appName) api.runtime.getBrowserInfo = async () => ({ name: appName, vendor: "Mozilla", version: "1.0" });
  // `proxy`: what proxy.settings.get says (#42; Chromium {mode, rules, pacScript}, Firefox {proxyType, …}).
  if (proxy) api.proxy = { settings: { get: async () => ({ value: proxy, levelOfControl: "controlled_by_other_extensions" }) } };
  return { api, calls: raw, listeners, store };
}

// The servers the extension reads manifests from (#38): address -> {status, body}.
let served = {};
const fetched = [];
async function fakeFetch(url, opts) {
  const range = /^bytes=(\d+)-(\d+)$/.exec((opts && opts.headers && opts.headers.Range) || "");
  fetched.push([String(url), opts && opts.credentials, ...(range ? [range[0]] : [])]);
  let r = served[url];
  if (typeof r === "function") r = r(opts);
  r = await r;
  if (!r) throw new TypeError("Failed to fetch");
  r.calls = [...(r.calls || []), opts];
  // Its body as a stream too, 1 KB a chunk: `r.read` counts what was pulled (#171 reads 4 KB).
  // `r.bytes`: a file's (#1092), whose ranges are served as a server does (206); `r.noRange`:
  // a server that ignores them (the whole file, 200).
  if (r.bytes && range && !r.noRange) {
    const part = r.bytes.slice(Number(range[1]), Number(range[2]) + 1);
    r.ranges = [...(r.ranges || []), range[0]];
    let sent = 0;
    const body = new ReadableStream({
      pull(c) {
        if (sent >= part.length) return c.close();
        c.enqueue(part.slice(sent, sent + 1024));
        sent += 1024;
      },
    });
    return { ok: true, status: 206, headers: { get: () => null }, body };
  }
  const bytes = r.bytes || new TextEncoder().encode(r.body);
  r.read = 0;
  const body = new ReadableStream({
    pull(c) {
      if (r.read >= bytes.length) return c.close();
      c.enqueue(bytes.slice(r.read, r.read + 1024));
      r.read += 1024;
    },
  });
  return {
    ok: (r.status || 200) < 300,
    status: r.status || 200,
    headers: { get: () => null },
    body,
    text: async () => r.body,
    json: async () => (typeof r.body === "string" ? JSON.parse(r.body) : r.body || null),
  };
}

// What background.js wrote to the console as an error or a warning (#42: none, granted or not).
const complaints = [];
const quietConsole = { ...console, error: (...a) => complaints.push(["error", ...a]), warn: (...a) => complaints.push(["warn", ...a]) };

// `extra`: more of the script's globals (a `Date` whose clock the test moves).
function load(api, firefox, extra = {}) {
  const ctx = {
    fetch: fakeFetch,
    navigator: { userAgent: "UA", userAgentData: { brands: [{ brand: "Chromium" }, { brand: "Brave" }] } },
    console: quietConsole,
    URL,
    URLSearchParams,
    TextDecoder,
    TextEncoder,
    atob,
    setTimeout,
    clearTimeout,
    Promise,
    crypto: globalThis.crypto,
  };
  if (firefox) ctx.browser = api;
  else ctx.chrome = api;
  Object.assign(ctx, extra);
  vm.runInNewContext(code, ctx);
  // Its globals (the functions it declares), for the tests of its parts (#1092: called as they are).
  return ctx;
}

const settle = () => new Promise((r) => setTimeout(r, 30));
// The messages to the host about downloads, and apart, the notices of downloads the extension
// left to the browser by Options (#208).
// The list's questions (#1496: the toolbar button's count, asked after a download is handed
// over) are apart too.
const natives = (calls) => calls.filter((c) => c[0] === "native" && !c[2].declined && !c[2].recent).map((c) => c[2]);
const notices = (calls) => calls.filter((c) => c[0] === "native" && c[2].declined).map((c) => c[2].declined);
const steps = (calls) => calls.filter((c) => ["pause", "cancel", "erase", "resume", "removeFile"].includes(c[0])).map((c) => c[0]);
let passed = 0;
const ok = (what) => {
  passed++;
  console.log(`ok - ${what}`);
};

// Firefox: a .zip in a container is taken over with that container's cookies and the browser's
// name, size and type; every message says which browser and version it comes from.
{
  const { api, calls, listeners } = fakeApi({ firefox: true });
  load(api, true);
  listeners.created({
    id: 7,
    url: "https://x/a.zip",
    filename: "C:\\dl\\a.zip",
    referrer: "https://page/",
    cookieStoreId: "firefox-container-2",
    totalBytes: 2048,
    mime: "application/zip",
  });
  await settle();
  const cookies = calls.find((c) => c[0] === "cookies");
  assert.deepEqual(cookies[1], { url: "https://x/a.zip", storeId: "firefox-container-2" });
  const native = calls.find((c) => c[0] === "native");
  assert.equal(native[1], "com.misterdownloadmanager.host");
  const m = native[2];
  assert.deepEqual(
    [m.url, m.referrer, m.cookies, m.filename, m.size, m.mime, m.userAgent, m.v],
    ["https://x/a.zip", "https://page/", "sid=1", "a.zip", 2048, "application/zip", "UA", "0.2.5"],
  );
  assert.deepEqual(m.brands, ["Chromium", "Brave"]);
  assert.equal(m.cookieJar, undefined, "a file's download: the header the app sends");
  assert.deepEqual(steps(calls), ["pause", "cancel", "erase"]);
  ok("Firefox: taken with its container's cookies, size and type");
}

// Zen (Firefox's family) says its own name through runtime.getBrowserInfo: it goes with the
// brands, so the app tells Zen from Firefox (#41).
{
  const { api, calls, listeners } = fakeApi({ firefox: true, appName: "Zen" });
  load(api, true);
  listeners.created({ id: 8, url: "https://x/b.zip", filename: "C:\\dl\\b.zip", totalBytes: 10, mime: "application/zip" });
  await settle();
  const m = natives(calls)[0];
  assert.deepEqual(m.brands, ["Chromium", "Brave", "Zen"]);
  ok("Zen: its own name goes with the brands");
}

// Firefox: the app doesn't take it → the browser's download resumes; nothing is lost.
{
  const { api, calls, listeners } = fakeApi({ firefox: true, reply: { ok: false } });
  load(api, true);
  listeners.created({ id: 8, url: "https://x/b.iso", filename: "/home/u/b.iso" });
  await settle();
  assert.deepEqual(steps(calls), ["pause", "resume"]);
  ok("Firefox: declined, resumed");
}

// A type not on the list (a web page) stays with the browser.
{
  const { api, calls, listeners } = fakeApi({ firefox: true });
  load(api, true);
  listeners.created({ id: 9, url: "https://x/page.html", filename: "C:\\dl\\page.html" });
  await settle();
  assert.deepEqual(natives(calls), []);
  assert.deepEqual(steps(calls), []);
  // Said, for MDM's log, under its own key and without an address.
  assert.deepEqual(notices(calls), [{ why: "file-type", name: "page.html", site: "x" }]);
  assert.ok(!calls.some((c) => c[0] === "native" && c[2].declined && "url" in c[2]));
  ok("a type not listed stays with the browser, and MDM's log hears why");
}

// Chrome: incognito downloads use the incognito store; the address asked for goes as the
// download's (an expired signed link is renewed from it), the redirect's end beside it.
{
  const { api, calls, listeners } = fakeApi({ firefox: false });
  load(api, false);
  let suggested = false;
  listeners.determining(
    { id: 3, url: "https://x/redirect", finalUrl: "https://cdn/x.msi", filename: "x.msi", incognito: true },
    () => (suggested = true),
  );
  await settle();
  assert.ok(!suggested, "a download the app took is never named: Chrome wrote none of it");
  const cookies = calls.find((c) => c[0] === "cookies");
  assert.deepEqual(cookies[1], { url: "https://x/redirect", storeId: "1" });
  const [m] = natives(calls);
  assert.deepEqual([m.url, m.finalUrl, m.private], ["https://x/redirect", "https://cdn/x.msi", true]);
  assert.deepEqual(steps(calls), ["cancel", "erase"], "held at its name, not paused and resumed");
  ok("Chrome: taken at its name, incognito cookies, both addresses");
}

// Chrome: a page that left for the file by script (Chromium counts it as the download's start,
// #374) isn't the address of the file.
{
  const { api, calls, listeners } = fakeApi({ firefox: false });
  load(api, false);
  const page = "https://site.test/welcome";
  listeners.responseStarted({ type: "main_frame", tabId: 1, frameId: 0, statusCode: 200, url: page, responseHeaders: [{ name: "Content-Type", value: "text/html; charset=UTF-8" }] });
  listeners.determining({ id: 7, url: page, finalUrl: "https://cdn.test/x.exe", filename: "x.exe" }, () => {});
  await settle();
  const [m] = natives(calls);
  assert.deepEqual([m.url, m.finalUrl], ["https://cdn.test/x.exe", ""], "the file's address, not the page's");
  ok("Chrome: a page that went to the file by script isn't the file's address");
}

// Chrome: one the app doesn't take goes on as the browser's own (its name confirmed).
{
  const { api, calls, listeners } = fakeApi({ firefox: false, reply: { ok: false } });
  load(api, false);
  let suggested = false;
  listeners.determining({ id: 4, url: "https://x/y.zip", filename: "y.zip" }, () => (suggested = true));
  await settle();
  assert.ok(suggested);
  assert.deepEqual(steps(calls), []);
  ok("Chrome: declined, the browser's own");
}

// Chrome, #578: a download left to the browser keeps another extension's name. What Chromium
// does with each listener's answer (downloads_custom_bindings.js, then downloads_api.cc
// DetermineFilename and DeterminerCallback): no valid suggestion goes as an empty name with no
// conflict action, which counts as a name; a name the browser refuses is emptied after the
// answer is counted, and only an empty name with "uniquify" is no name at all. The newest
// extension's name wins (here MDM's, updated after Save In was installed).
{
  const answer = (s) => {
    const valid = s && typeof s === "object" && typeof s.filename === "string" && s.filename.length > 0;
    const [name, action] = valid ? [s.filename, s.conflictAction || ""] : ["", ""];
    const safe = name && !name.split(/[\/]/).some((p) => p === "" || p === "." || p === "..") && !/^[a-z]:|^[\/]/i.test(name);
    const filename = safe ? name : "";
    return filename || action !== "uniquify" ? { filename } : null; // null: no suggestion
  };
  // The determination of one download: `answers` in install order, oldest first.
  const determined = (creator, answers) => {
    let name = creator;
    let by = -1;
    answers.forEach((a, i) => {
      const s = answer(a);
      if (s && i >= by) [name, by] = [s.filename, i];
    });
    return name;
  };
  for (const [what, item, creator] of [
    ["its own listener's folder (Save In)", { id: 70, url: "https://img.test/cat.jpg", filename: "cat.jpg", byExtensionId: "savein" }, ""],
    ["a downloads.download filename", { id: 71, url: "https://img.test/dog.jpg", filename: "dog.jpg", byExtensionId: "savein" }, "Pic/OthersW/dog.jpg"],
    ["a page's download", { id: 72, url: "https://x/page.html", filename: "page.html" }, ""],
  ]) {
    const { api, listeners } = fakeApi({ firefox: false });
    load(api, false);
    const other = item.byExtensionId && !creator ? { filename: `Pic/OthersW/${item.filename}` } : undefined;
    let mdm = "none";
    listeners.determining(item, (s) => (mdm = s));
    await settle();
    assert.notEqual(mdm, "none", `${what}: answered, the download doesn't wait`);
    const want = other ? other.filename : creator;
    assert.equal(determined(creator, other ? [other, mdm] : [mdm]), want, `${what}: MDM's answer gives no name of its own`);
  }
  // The model, checked: Chromium's own cases.
  assert.equal(determined("Pic/a.jpg", [undefined]), "", "suggest() empties the creator's name");
  assert.equal(determined("", [{ filename: "Pic/b.jpg" }, undefined]), "", "and the older extension's");
  assert.equal(determined("", [undefined, { filename: "Pic/b.jpg" }]), "Pic/b.jpg", "an older suggest() loses");
  ok("Chrome: a download left alone keeps another extension's name and folder (#578)");
}

// Firefox: a small file finished before it could be stopped: the browser's copy is removed.
{
  const { api, calls, listeners } = fakeApi({ firefox: true, finished: true });
  load(api, true);
  listeners.created({ id: 5, url: "https://x/s.zip", filename: "C:\\dl\\s.zip" });
  await settle();
  assert.ok(calls.some((c) => c[0] === "removeFile" && c[1] === 5));
  assert.ok(calls.some((c) => c[0] === "erase" && c[1] === 5));
  ok("Firefox: a finished small file is removed");
}

// Options -> File types in the app decides what is offered: its list, not a built-in one.
{
  const { api, calls, listeners } = fakeApi({ firefox: true, types: "PNG ISO" });
  load(api, true);
  listeners.created({ id: 20, url: "https://x/pic.png", filename: "C:\\dl\\pic.png" });
  await settle();
  listeners.created({ id: 21, url: "https://x/a.zip", filename: "C:\\dl\\a.zip" });
  await settle();
  assert.deepEqual(natives(calls).map((m) => m.filename), ["pic.png"], "PNG is the app's, ZIP is not listed there");
  assert.equal(calls.filter((c) => c[0] === "rules").length, 1, "the rules kept, not asked per download");
  ok("the app's file types decide, asked once");
}

// Mister Download Manager not installed (no host): everything stays with the browser, and the
// toolbar button says something is wrong.
{
  const { api, calls, listeners, store } = fakeApi({ firefox: false, host: false });
  load(api, false);
  let suggested = false;
  listeners.determining({ id: 30, url: "https://x/a.zip", filename: "a.zip" }, () => (suggested = true));
  await settle();
  assert.ok(suggested);
  assert.deepEqual(steps(calls), []);
  assert.equal(store.status.why, "not-installed");
  assert.ok(calls.some((c) => c[0] === "badge" && c[1] === "!"));
  // Firefox isn't paused for nothing either.
  const ff = fakeApi({ firefox: true, host: false });
  load(ff.api, true);
  ff.listeners.created({ id: 31, url: "https://x/a.zip", filename: "a.zip" });
  await settle();
  assert.deepEqual(steps(ff.calls), []);
  ok("no host: nothing taken, nothing paused, the badge says so");
}

// The extension installed without the app: the help page opens at its section.
{
  const { api, calls, listeners } = fakeApi({ firefox: false, host: false });
  load(api, false);
  await listeners.installed({ reason: "install" });
  await settle();
  assert.ok(calls.some((c) => c[0] === "openTab" && c[1].endsWith("#not-installed")));
  const items = calls.filter((c) => c[0] === "menu-item").map((c) => c[1].id);
  assert.deepEqual(items, ["mdm-link", "mdm-selected", "mdm-all"]);
  ok("installed without the app: the help page, the menus");
}

// A form's download: posted again by the app with the same body (it can't be a GET).
{
  const { api, calls, listeners } = fakeApi({ firefox: false });
  load(api, false);
  listeners.beforeRequest({ method: "POST", url: "https://x/form", requestId: "9", requestBody: { formData: { token: ["abc"], note: ["hello world"] } } });
  listeners.beforeSendHeaders({ method: "POST", url: "https://x/form", requestId: "9", requestHeaders: [{ name: "Content-Type", value: "application/x-www-form-urlencoded" }] });
  listeners.determining({ id: 40, url: "https://x/form", filename: "invoice.pdf" }, () => {});
  await settle();
  const [m] = natives(calls);
  assert.deepEqual([m.post, m.postType], ["token=abc&note=hello+world", "application/x-www-form-urlencoded"]);
  // A form with a file in it can't be posted again: the browser keeps its download.
  listeners.beforeRequest({ method: "POST", url: "https://x/upload", requestId: "10", requestBody: { raw: [{ file: "C:\\a.txt" }] } });
  let suggested = false;
  listeners.determining({ id: 41, url: "https://x/upload", filename: "out.zip" }, () => (suggested = true));
  await settle();
  assert.ok(suggested);
  assert.equal(natives(calls).length, 1);
  // A multipart form: the browser gives its fields without their files, so it stays there too.
  listeners.beforeRequest({ method: "POST", url: "https://x/multi", requestId: "11", requestBody: { formData: { a: ["1"] } } });
  listeners.beforeSendHeaders({ method: "POST", url: "https://x/multi", requestId: "11", requestHeaders: [{ name: "Content-Type", value: "multipart/form-data; boundary=xyz" }] });
  let kept = false;
  listeners.determining({ id: 42, url: "https://x/multi", filename: "out.zip" }, () => (kept = true));
  await settle();
  assert.ok(kept);
  assert.equal(natives(calls).length, 1);
  ok("forms: posted again with their body, or left to the browser");
}

// Alt held on a link, a site the extension may not read, an excluded site, a blob: address:
// each stays with the browser without asking the app.
{
  const { api, calls, listeners } = fakeApi({ firefox: false });
  load(api, false);
  const send = (m, sender) => new Promise((r) => listeners.message(m, sender, r));
  await send({ type: "bypass", url: "https://x/alt.zip" }, { id: "ours", url: "https://x/", tab: { url: "https://x/" } });
  const keep = async (item) => {
    let suggested = false;
    listeners.determining(item, () => (suggested = true));
    await settle();
    return suggested;
  };
  assert.ok(await keep({ id: 50, url: "https://x/alt.zip", filename: "alt.zip" }), "Alt");
  assert.ok(await keep({ id: 51, url: "https://dl.update.microsoft.com/kb.exe", filename: "kb.exe" }), "excluded site");
  assert.ok(await keep({ id: 52, url: "blob:https://x/1234", filename: "a.zip" }), "blob:");
  assert.deepEqual(natives(calls), []);
  // Alt and blob: are the user's and the browser's own: nothing to say. An excluded site is.
  assert.deepEqual(notices(calls), [{ why: "site", name: "kb.exe", site: "dl.update.microsoft.com" }]);
  const limited = fakeApi({ firefox: false, access: false });
  load(limited.api, false);
  let suggested = false;
  limited.listeners.determining({ id: 53, url: "https://x/a.zip", filename: "a.zip" }, () => (suggested = true));
  await settle();
  assert.ok(suggested && natives(limited.calls).length === 0, "site access withdrawn");
  assert.deepEqual(notices(limited.calls).map((n) => n.why), ["site-access"]);
  ok("Alt, excluded sites, blob: and withdrawn site access stay with the browser");
}

// #1049: a plain name in Options' excluded sites takes its subdomains, and a file its pages link
// to on another host (a CDN) is the site's too, by the download's referrer; with no page known,
// the file's own host decides.
{
  const { api, calls, listeners } = fakeApi({ firefox: false, rules: { excludedSites: "example.test *.mirror.* https://*kept*" } });
  load(api, false);
  const keep = async (item) => {
    let suggested = false;
    listeners.determining(item, () => (suggested = true));
    await settle();
    return suggested;
  };
  assert.ok(await keep({ id: 60, url: "https://www.example.test/a.zip", filename: "a.zip" }), "a subdomain");
  assert.ok(
    await keep({ id: 61, url: "https://cdn.files.test/b.zip", referrer: "https://www.example.test/get", filename: "b.zip" }),
    "from the site's page",
  );
  assert.deepEqual(natives(calls), []);
  assert.deepEqual(notices(calls).map((n) => [n.why, n.name]), [["site", "a.zip"], ["site", "b.zip"]]);
  assert.ok(!(await keep({ id: 62, url: "https://cdn.files.test/c.zip", referrer: "https://other.test/", filename: "c.zip" })), "another site's page");
  assert.ok(!(await keep({ id: 63, url: "https://cdn.files.test/d.zip", referrer: "", filename: "d.zip" })), "no page known");
  assert.ok(!(await keep({ id: 64, url: "https://badexample.test/e.zip", filename: "e.zip" })), "not a subdomain");
  assert.deepEqual(natives(calls).map((m) => m.url), ["https://cdn.files.test/c.zip", "https://cdn.files.test/d.zip", "https://badexample.test/e.zip"]);
  // Wildcards, as the app reads them (#759): `*.mirror.*`, a scheme left out.
  assert.ok(await keep({ id: 65, url: "https://dl.mirror.org/f.zip", filename: "f.zip" }), "*.mirror.*");
  assert.ok(await keep({ id: 66, url: "https://mirror.net/g.zip", filename: "g.zip" }), "*.mirror.* takes the domain");
  assert.ok(await keep({ id: 67, url: "https://cdn.files.test/h.zip", referrer: "https://www.kept.test/", filename: "h.zip" }), "*kept*");
  // `example.test.` is `example.test` (the root's dot), for the file and for its page.
  assert.ok(await keep({ id: 68, url: "https://www.example.test./i.zip", filename: "i.zip" }), "the root's dot");
  assert.ok(
    await keep({ id: 69, url: "https://cdn.files.test/j.zip", referrer: "https://www.example.test./get", filename: "j.zip" }),
    "a page with the root's dot",
  );
  assert.equal(natives(calls).length, 3);
  ok("#1049: excluded sites take their subdomains and the files their pages link to");
}

// Paused from the toolbar button: nothing is taken until it is on again.
{
  const { api, calls, listeners, store } = fakeApi({ firefox: false });
  load(api, false);
  const send = (m) => new Promise((r) => listeners.message(m, { id: "ours" }, (v) => r(JSON.parse(JSON.stringify(v)))));
  const state = await send({ type: "state" });
  assert.deepEqual([state.connected, state.app, state.paused], [true, "0.2.5", false]);
  await send({ type: "pause", paused: true });
  assert.equal(store.paused, true);
  let suggested = false;
  listeners.determining({ id: 60, url: "https://x/a.zip", filename: "a.zip" }, () => (suggested = true));
  await settle();
  assert.ok(suggested && natives(calls).length === 0);
  assert.ok(calls.some((c) => c[0] === "badge" && c[1] === "II"));
  await send({ type: "exclude-site", host: "files.test" });
  assert.ok(natives(calls).some((m) => m.excludeSite === "files.test"));
  ok("the popup: state, pause, never this site");
}

// #404: "Never take downloads from <site>" says it worked, and its Undo takes the site off the
// excluded sites again; the popup opened later on that site knows it is excluded. A site excluded
// by a pattern of Options' own is said excluded, without Undo (the app would keep the pattern).
{
  const rules = { excludedSites: "*.update.microsoft.com" };
  // As the app: the host name added (once), or taken off by its own name only.
  const reply = (m) => {
    const sites = rules.excludedSites.split(/\s+/).filter(Boolean);
    if (m.excludeSite && !sites.includes(m.excludeSite)) rules.excludedSites = [...sites, m.excludeSite].join(" ");
    if (m.includeSite) rules.excludedSites = sites.filter((p) => p.toLowerCase() !== m.includeSite.toLowerCase()).join(" ");
    return { ok: true };
  };
  const { api, calls, listeners } = fakeApi({ firefox: false, rules, reply });
  load(api, false);
  const send = (m) => new Promise((r) => listeners.message(m, { id: "ours" }, (v) => r(JSON.parse(JSON.stringify(v)))));
  await settle();
  assert.deepEqual(await send({ type: "site-excluded", host: "files.test" }), { excluded: false, undo: false });
  assert.deepEqual(await send({ type: "exclude-site", host: "files.test" }), { ok: true });
  assert.deepEqual(await send({ type: "site-excluded", host: "files.test" }), { excluded: true, undo: true });
  // Its downloads stay with the browser now.
  let suggested = false;
  listeners.determining({ id: 61, url: "https://files.test/a.zip", filename: "a.zip" }, () => (suggested = true));
  await settle();
  assert.ok(suggested && !natives(calls).some((m) => m.url === "https://files.test/a.zip"), "excluded: left to the browser");
  // Undo: the host goes to the app, the site's downloads come back.
  assert.deepEqual(await send({ type: "include-site", host: "files.test" }), { ok: true });
  assert.ok(natives(calls).some((m) => m.includeSite === "files.test"));
  assert.deepEqual(await send({ type: "site-excluded", host: "files.test" }), { excluded: false, undo: false });
  listeners.determining({ id: 62, url: "https://files.test/b.zip", filename: "b.zip" }, () => {});
  await settle();
  assert.ok(natives(calls).some((m) => m.url === "https://files.test/b.zip"), "taken again after Undo");
  // Excluded by Options' own pattern: said so, no Undo.
  assert.deepEqual(await send({ type: "site-excluded", host: "dl.update.microsoft.com" }), { excluded: true, undo: false });
  // An app that can't take it back (an older one): Undo says it didn't work.
  const old = fakeApi({ firefox: false, reply: (m) => (m.includeSite ? { ok: false, why: "refused" } : { ok: true }) });
  load(old.api, false);
  const ask = (m) => new Promise((r) => old.listeners.message(m, { id: "ours" }, (v) => r(JSON.parse(JSON.stringify(v)))));
  assert.deepEqual(await ask({ type: "include-site", host: "files.test" }), { ok: false });
  ok("#404 the popup: never this site, said, and its Undo");
}

// An app older than the extension: still connected, with a nudge to update it.
{
  const { api, store, listeners } = fakeApi({ firefox: false, rules: { version: "0.1.0" } });
  load(api, false);
  listeners.startup();
  await settle();
  assert.equal(store.status && store.status.why, "update-app");
  assert.equal(store.status.app, "0.1.0");
  ok("an older app: update-app");
}

// Only a bug-fix version apart (the store publishing the add-on first): no nudge, no update exists.
{
  const { api, store, listeners } = fakeApi({ firefox: false, rules: { version: "0.2.4" } });
  load(api, false);
  listeners.startup();
  await settle();
  assert.notEqual(store.status && store.status.why, "update-app");
  ok("a bug-fix version behind: no update-app");
}

// Files the browser would open (a PDF): a session rule makes them downloads, for the listed types
// only; none when the app says not to.
{
  const { api, calls } = fakeApi({ firefox: false, types: "PDF MP3 ZIP" });
  load(api, false);
  await settle();
  const rule = calls.filter((c) => c[0] === "dnr").pop()[1];
  const values = rule.addRules[0].condition.responseHeaders[0].values;
  assert.ok(values.includes("application/pdf*") && values.includes("audio/mpeg*"));
  assert.ok(!values.some((v) => v.startsWith("video/mp4")), "MP4 not listed");
  // #1087: one a page shows in a frame, an <embed> or an <object> too, never one its script
  // fetches (the page's own viewer reads it).
  assert.deepEqual(rule.addRules[0].condition.resourceTypes, ["main_frame", "sub_frame", "object"]);
  // Options' excluded sites keep opening in the browser, with their subdomains whether written
  // with *. or not (#1049), and so do the files their pages lead to on another host.
  const ex = fakeApi({ firefox: false, types: "PDF", rules: { excludedSites: "*.docs.test files.test:8080 docs.test" } });
  load(ex.api, false);
  await settle();
  const added = ex.calls.filter((c) => c[0] === "dnr").pop()[1].addRules;
  assert.equal(added.length, 1, "one rule: no allow rules of their own");
  assert.deepEqual(added[0].condition.excludedRequestDomains, ["docs.test", "files.test"]);
  assert.deepEqual(added[0].condition.excludedInitiatorDomains, ["docs.test", "files.test"]);
  const off = fakeApi({ firefox: false, rules: { opened: false } });
  load(off.api, false);
  await settle();
  assert.deepEqual(off.calls.filter((c) => c[0] === "dnr").pop()[1].addRules, []);
  ok("opened types: the session rule follows the app's list");
}

// #948: the rule leaves an attachment alone (its server's name stays the browser's). A file it
// made a download, which Chromium then names after its address (`main.mkv`, an inline answer's own
// name dropped with the header), goes to MDM as only a guess: MDM names it from the server's answer.
// A name of the browser's own, or a type the rule doesn't take, stays a name.
{
  const { api, calls, listeners } = fakeApi({ firefox: false, types: "MKV MP4 ZIP" });
  load(api, false);
  await settle();
  const rule = calls.filter((c) => c[0] === "dnr").pop()[1].addRules[0];
  assert.deepEqual(rule.condition.excludedResponseHeaders, [{ header: "content-disposition", values: ["attachment*"] }]);
  let id = 300;
  const offered = async (item) => {
    const before = natives(calls).length;
    listeners.determining({ id: id++, referrer: "https://site.test/watch", totalBytes: 1 << 20, ...item }, () => {});
    await settle();
    const sent = natives(calls).slice(before);
    assert.equal(sent.length, 1, `offered: ${JSON.stringify(item)}`);
    return sent[0];
  };
  for (const [url, filename, mime] of [
    ["https://cdn.test/v/9f3/main.mkv?token=a1", "C:\\dl\\main.mkv", "video/x-matroska"],
    ["https://cdn.test/v/9f3/MAIN.mp4", "main.mp4", "video/mp4"],
    // No extension in the address: the browser adds the type's.
    ["https://cdn.test/v/stream", "stream.mp4", "video/mp4"],
  ]) {
    const m = await offered({ url, finalUrl: url, filename, mime });
    assert.equal(m.nameGuessed, true, url);
    assert.equal(m.filename, filename.split("\\").pop(), "still sent, for Options' file types");
  }
  for (const [url, filename, mime] of [
    // An attachment's own name, or a link's `download` name.
    ["https://cdn.test/v/9f3/main.mkv", "Real Movie (2024).mkv", "video/x-matroska"],
    // Not a type the rule makes a download.
    ["https://cdn.test/f/main.zip", "main.zip", "application/zip"],
  ]) {
    const m = await offered({ url, finalUrl: url, filename, mime });
    assert.equal(m.nameGuessed, undefined, url);
    assert.equal(m.filename, filename);
  }
  ok("#948 Chromium: a name only the address gave is sent as a guess; the rule keeps attachments");
}

// Right-click → Download with Mister Download Manager: handed over as an explicit request, so
// the host takes any link (an .html page here); a failure opens the help page's section.
{
  const { api, calls, listeners } = fakeApi({ firefox: false });
  load(api, false);
  await listeners.menu({ menuItemId: "mdm-link", linkUrl: "https://x/page.html", pageUrl: "https://x/" }, { incognito: false });
  await settle();
  const [m] = natives(calls);
  assert.deepEqual([m.url, m.explicit, m.referrer], ["https://x/page.html", true, "https://x/"]);
  await listeners.menu({ menuItemId: "mdm-all", pageUrl: "https://x/" }, { id: 1, incognito: false });
  await settle();
  assert.deepEqual(natives(calls)[1].links, ["https://x/1.zip", "https://x/2.zip"]);
  const failing = fakeApi({ firefox: false, reply: { ok: false, why: "not-running" } });
  load(failing.api, false);
  await failing.listeners.menu({ menuItemId: "mdm-link", linkUrl: "https://x/a.zip", pageUrl: "https://x/" }, {});
  await settle();
  assert.ok(failing.calls.some((c) => c[0] === "openTab" && c[1].endsWith("#not-running")));
  ok("context menus: explicit links, a page's links, failures explained");
}

// A page's links past what the host takes in one message (1 MB, crates/ipc MAX_MESSAGE): sent in
// parts, each under it as every browser writes it, in order, each with the page's referrer and
// cookies and its place in the list; a list exactly at the limit still goes whole, without
// parts, as before.
{
  const MAX = 1 << 20;
  // As the host counts: the bytes of the message's JSON in UTF-8.
  const bytes = (m) => Buffer.byteLength(JSON.stringify(m), "utf8");
  const { api } = fakeApi({ firefox: false });
  const g = load(api, false);
  assert.equal(vm.runInContext("MESSAGE_MAX", g), MAX, "the host's limit (crates/ipc MAX_MESSAGE)");
  // wireSize: JSON.stringify's UTF-8, plus what Chromium (< DEL U+2028 U+2029) and Safari's
  // helper (/) escape: never less than any of them writes.
  assert.equal(g.wireSize({ a: "plain text, é, ✓, 😀" }), bytes({ a: "plain text, é, ✓, 😀" }));
  assert.equal(g.wireSize("a/b"), bytes("a/b") + 1);
  assert.equal(g.wireSize("<\u007f\u2028\u2029"), 2 + 4 * 6);
  assert.equal(g.wireSize("\ud800"), bytes("\ud800"), "a lone surrogate, escaped by JSON.stringify");
  // Cut in order, every part's message within the limit, nothing lost or doubled.
  const base = { referrer: "https://x.test/files/", cookies: "sid=1", private: false, userAgent: "UA", brands: [], v: "1", linksPart: { batch: "b", index: 256, count: 256 } };
  const many = Array.from({ length: 5000 }, (_, k) => `https://x.test/files/${"d".repeat(k % 300)}/file${k}.zip`);
  // The script's arrays are of its own realm: compared as plain copies.
  const cutAt = (...a) => JSON.parse(JSON.stringify(g.linkParts(...a)));
  const { parts, left } = cutAt(base, many, 100000);
  assert.ok(parts.length > 5);
  assert.deepEqual(parts.flat(), many);
  assert.equal(left.length, 0);
  for (const links of parts) assert.ok(g.wireSize({ ...base, links }) <= 100000);
  // Each part as full as the next link allows.
  for (let k = 0; k + 1 < parts.length; k++) assert.ok(g.wireSize({ ...base, links: [...parts[k], parts[k + 1][0]] }) > 100000);
  // A link too long to go even alone is left out, the others go.
  const huge = `https://x.test/${"x".repeat(100000)}`;
  const cut = cutAt(base, ["https://x.test/a.zip", huge, "https://x.test/b.zip"], 100000);
  assert.deepEqual([cut.parts, cut.left], [[["https://x.test/a.zip", "https://x.test/b.zip"]], [huge]]);
  // Exactly at the limit: one part; a byte more: two.
  const exact = cutAt(base, ["https://x.test/a"], g.wireSize({ ...base, links: ["https://x.test/a"] }));
  assert.equal(exact.parts.length, 1);
  const over = cutAt(base, ["https://x.test/a", "https://x.test/b"], g.wireSize({ ...base, links: ["https://x.test/a", "https://x.test/b"] }) - 1);
  assert.deepEqual(over.parts, [["https://x.test/a"], ["https://x.test/b"]]);
  ok("a page's links: measured as browsers write them, cut in order within the limit");
}

// Through the right-click menu: a page whose links' message would be exactly 1 MB goes as one
// message, as before (an older app knows nothing else); one byte more, in two parts; 30,000 long
// links (7 MB) in parts the host takes, as one list.
{
  const MAX = 1 << 20;
  const bytes = (m) => Buffer.byteLength(JSON.stringify(m), "utf8");
  // The message the menu sends for `links`, as background.js stamps it (fakeApi's browser).
  const message = (links) => ({ links, referrer: "https://x.test/files/", cookies: "sid=1", private: false, userAgent: "UA", brands: ["Chromium", "Brave"], v: "0.2.5" });
  const probe = load(fakeApi({ firefox: false }).api, false);
  const filler = (n) => `https://x.test/${"f".repeat(n)}`;
  const fixed = Array.from({ length: 2000 }, (_, k) => `https://x.test/files/file${k}.zip`);
  const room = MAX - probe.wireSize(message([...fixed, filler(0)]));
  const atLimit = [...fixed, filler(room)];
  assert.equal(probe.wireSize(message(atLimit)), MAX);
  const run = async (links, reply) => {
    const f = fakeApi({ firefox: false, pageLinks: links, ...(reply ? { reply } : {}) });
    load(f.api, false);
    // Its start's own asking for the rules done first.
    await settle();
    await f.listeners.menu({ menuItemId: "mdm-all", pageUrl: "https://x.test/files/", frameId: 0 }, { id: 1, incognito: false });
    await settle();
    return { sent: natives(f.calls), tabs: f.calls.filter((c) => c[0] === "openTab").map((c) => c[1]), store: f.store };
  };
  let { sent, tabs } = await run(atLimit);
  assert.equal(sent.length, 1);
  assert.equal(sent[0].linksPart, undefined, "whole, as before");
  assert.equal(probe.wireSize(sent[0]), MAX, "at the limit as the widest browser writes it");
  assert.ok(bytes(sent[0]) <= MAX);
  assert.deepEqual(tabs, []);
  ({ sent, tabs } = await run([...fixed, filler(room + 1)]));
  assert.equal(sent.length, 2);
  assert.deepEqual(sent.map((m) => [m.linksPart.index, m.linksPart.count]), [[0, 2], [1, 2]]);
  assert.equal(sent[0].linksPart.batch, sent[1].linksPart.batch);
  assert.deepEqual(sent.flatMap((m) => m.links), [...fixed, filler(room + 1)]);
  // 30,000 links of about 240 characters: parts within the limit, in order, one batch, each
  // with the page's context; nothing to explain.
  const long = Array.from({ length: 30000 }, (_, k) => `https://x.test/files/${"d".repeat(200)}/file${k}.zip?seed=${k}`);
  ({ sent, tabs } = await run(long));
  assert.ok(sent.length >= 7, `${sent.length} parts`);
  assert.deepEqual(sent.flatMap((m) => m.links), long);
  assert.deepEqual(sent.map((m) => m.linksPart.index), [...sent.keys()]);
  assert.ok(sent.every((m) => m.linksPart.count === sent.length && m.linksPart.batch === sent[0].linksPart.batch));
  assert.match(sent[0].linksPart.batch, /^[A-Za-z0-9-]{1,64}$/);
  for (const m of sent) {
    assert.ok(bytes(m) <= MAX, `${bytes(m)} bytes`);
    assert.deepEqual([m.referrer, m.cookies, m.private, m.v], ["https://x.test/files/", "sid=1", false, "0.2.5"]);
  }
  assert.deepEqual(tabs, []);
  // Another click: another batch.
  const again = await run(long);
  assert.notEqual(again.sent[0].linksPart.batch, sent[0].linksPart.batch);
  // A part refused (MDM quit meanwhile): no more parts, and the help page says why.
  let answered = 0;
  ({ sent, tabs } = await run(long, () => (++answered === 3 ? { ok: false, why: "not-running" } : { ok: true })));
  assert.equal(sent.length, 3);
  assert.deepEqual(tabs, ["https://misterdownloadmanager.com/help/browser-help.html#not-running"]);
  // A link longer than a whole message: the others go, and the help page says one was left out.
  const huge = `https://x.test/${"x".repeat(MAX)}`;
  ({ sent, tabs } = await run([...long, huge]));
  assert.deepEqual(sent.flatMap((m) => m.links), long);
  assert.deepEqual(tabs, ["https://misterdownloadmanager.com/help/browser-help.html#too-large"]);
  // Only such links: nothing goes, too large.
  let store;
  ({ sent, tabs, store } = await run([huge]));
  assert.deepEqual(sent, []);
  assert.deepEqual(tabs, ["https://misterdownloadmanager.com/help/browser-help.html#too-large"]);
  assert.equal(store.status.why, "too-large");
  ok("Download all links: a list at the limit whole, past it in parts the host takes, failures explained");
}

// The download button over videos: the page asks once a minute per site, a click opens the
// app's video window for the page the browser says sent it (with the tab as referrer and its
// cookies), "Never on this site" goes to the app and is remembered at once; nothing from
// another extension is answered.
{
  const { api, calls, listeners } = fakeApi({ firefox: false });
  load(api, false);
  const ask = (m, page = "https://tube.test/embed/1", sender) =>
    new Promise((resolve) => {
      // Plain copies: objects from the sandbox have its prototypes.
      const async_ = listeners.message(m, sender || { id: "ours", url: page, tab: { url: "https://blog.test/post", incognito: false } }, (r) =>
        resolve(JSON.parse(JSON.stringify(r))),
      );
      if (!async_) resolve(undefined);
    });
  assert.deepEqual(await ask({ type: "panel" }), { on: true, compact: false });
  assert.deepEqual(await ask({ type: "panel" }, "https://tube.test/embed/2"), { on: true, compact: false });
  assert.equal(natives(calls).filter((m) => m.panel).length, 1, "asked once for the site");
  assert.equal((await ask({ type: "video", page: "https://evil.test/" })).ok, true);
  const video = natives(calls).find((m) => m.video);
  assert.equal(video.url, "https://tube.test/embed/1", "the page the browser names, not the message's");
  assert.equal(video.referrer, "https://blog.test/post");
  assert.equal(video.cookies, "sid=1");
  // Each cookie with its domain and flags, for yt-dlp asking another host of the site.
  assert.deepEqual(video.cookieJar, [{ name: "sid", value: "1", domain: ".tube.test", hostOnly: false, path: "/", secure: true, expirationDate: 1893456000.5 }]);
  assert.deepEqual(await ask({ type: "hide-site", host: "elsewhere.test" }), { ok: true });
  assert.ok(natives(calls).some((m) => m.hideSite === "tube.test"));
  assert.deepEqual(await ask({ type: "panel" }, "https://tube.test/embed/3"), { on: false, compact: false }, "off at once");
  assert.deepEqual(await ask({ type: "panel" }, "https://tube.test:8443/embed/4"), { on: false, compact: false }, "whatever the port");
  assert.deepEqual(await ask({ type: "video" }, "javascript:alert(1)"), { ok: false });
  // The video's own address, when the content script names one of its site's: a watch page opened
  // from a mix or a playlist is the list in the address bar, but the video the button is on is one
  // video. The tab stays the referrer, and its cookies are read for that address.
  assert.equal((await ask({ type: "video", url: "https://tube.test/watch?v=abc123" })).ok, true);
  const own = natives(calls).filter((m) => m.video).at(-1);
  assert.equal(own.url, "https://tube.test/watch?v=abc123", "the video's own page");
  assert.equal(own.referrer, "https://blog.test/post");
  assert.equal(own.cookies, "sid=1");
  // Another site's address is not the page's: ignored, as any message content was before.
  assert.equal((await ask({ type: "video", url: "https://evil.test/watch?v=abc123" })).ok, true);
  assert.equal(
    natives(calls).filter((m) => m.video).at(-1).url,
    "https://tube.test/embed/1",
    "another site's address is ignored",
  );
  assert.equal(await ask({ type: "panel" }, "https://x.test/", { id: "someone-else", url: "https://x.test/", tab: {} }), undefined);
}
// A player playing a stream (HLS) from a blob: the button sends the manifest its frame loaded
// first; the tab going elsewhere forgets it.
{
  const { api, calls, listeners } = fakeApi({ firefox: false });
  load(api, false);
  const sender = { id: "ours", url: "https://news.test/story", frameId: 0, tab: { id: 5, url: "https://news.test/story" } };
  listeners.responseStarted({ tabId: 5, frameId: 0, url: "https://cdn.test/v/master.m3u8?t=1", responseHeaders: [] });
  listeners.responseStarted({ tabId: 5, frameId: 0, url: "https://cdn.test/v/720p.m3u8", responseHeaders: [] });
  listeners.responseStarted({ tabId: 5, frameId: 0, url: "https://cdn.test/v/seg1.ts", responseHeaders: [{ name: "Content-Type", value: "video/mp2t" }] });
  const send = (m) => new Promise((r) => listeners.message(m, sender, r));
  await send({ type: "video", src: "", title: "A story's film" });
  const first = natives(calls).find((m) => m.video);
  assert.deepEqual([first.src, first.title], ["https://cdn.test/v/master.m3u8?t=1", "A story's film"], "the page's title goes with its stream");
  listeners.tabUpdated(5, { url: "https://news.test/other" });
  await send({ type: "video", src: "" });
  assert.equal(natives(calls).filter((m) => m.video)[1].src, undefined, "forgotten on another page");
  ok("streams: the frame's manifest goes with the button");
}
// #155: the right-click menu on a player playing from a blob: sends the manifest its frame loaded
// (it sent the page alone, which yt-dlp answered "Unsupported URL"), with the manifest's cookies;
// a manifest's own link goes with its cookies too; the button asks for the stream's cookies.
{
  const { api, calls, listeners } = fakeApi({ firefox: false });
  load(api, false);
  listeners.responseStarted({ tabId: 7, frameId: 2, url: "https://cdn.test/s/master.m3u8", responseHeaders: [] });
  listeners.responseStarted({ tabId: 7, frameId: 0, url: "https://other.test/x/main.mpd", responseHeaders: [] });
  await listeners.menu(
    { menuItemId: "mdm-link", srcUrl: "blob:https://player.test/1", frameId: 2, frameUrl: "https://player.test/embed", pageUrl: "https://blog.test/post" },
    { id: 7, incognito: false, title: "A post with a film" },
  );
  await settle();
  const m = natives(calls).find((x) => x.video);
  assert.deepEqual(
    [m.url, m.src, m.referrer, m.explicit, m.title],
    ["https://player.test/embed", "https://cdn.test/s/master.m3u8", "https://blog.test/post", true, "A post with a film"],
    "the frame's stream, not the page alone; the tab's title names its file",
  );
  assert.ok(calls.some((c) => c[0] === "cookies" && c[1].url === "https://cdn.test/s/master.m3u8"), "the stream's cookies");
  await listeners.menu({ menuItemId: "mdm-link", linkUrl: "https://cdn.test/s/other.m3u8", pageUrl: "https://blog.test/post" }, { id: 7, incognito: false, title: "A post with a film" });
  await settle();
  const linked = natives(calls).filter((x) => x.video)[1];
  assert.deepEqual([linked.src, linked.title], ["https://cdn.test/s/other.m3u8", "A post with a film"], "a manifest's link: named after its tab");
  assert.ok(calls.some((c) => c[0] === "cookies" && c[1].url === "https://cdn.test/s/other.m3u8"), "the manifest's cookies");
  // Firefox names no address for a player fed by a MediaSource: the same stream goes.
  await listeners.menu({ menuItemId: "mdm-link", mediaType: "video", frameId: 2, frameUrl: "https://player.test/embed", pageUrl: "https://blog.test/post" }, { id: 7, incognito: false });
  await settle();
  assert.equal(natives(calls).filter((x) => x.video).pop().src, "https://cdn.test/s/master.m3u8", "Firefox: no srcUrl");
  // Nothing loaded in that tab: the page, as before (a page yt-dlp may know).
  await listeners.menu({ menuItemId: "mdm-link", srcUrl: "blob:https://tube.test/2", frameId: 0, pageUrl: "https://tube.test/watch" }, { id: 9, incognito: false });
  await settle();
  const page = natives(calls).filter((x) => x.video).pop();
  assert.deepEqual([page.url, page.src], ["https://tube.test/watch", undefined]);
  // The button: the stream's cookies, not the page's.
  const sender = { id: "ours", url: "https://blog.test/post", frameId: 0, tab: { id: 7, url: "https://blog.test/post" } };
  calls.length = 0;
  await new Promise((r) => listeners.message({ type: "video", src: "" }, sender, r));
  assert.ok(calls.some((c) => c[0] === "cookies" && c[1].url === "https://other.test/x/main.mpd"), "the button: the stream's cookies");
  ok("#155: the right-click menu on a stream's player sends its manifest; cookies of the stream");
}
// #532: Options changed while a page with a video is open: the app's answer is kept a few
// seconds, not a minute, so the button follows (the page asks again: video.js ASK_MS). An
// embedded player asks with its tab's address, so the site in the address bar counts too.
{
  const clock = { t: Date.now() };
  class FakeDate extends Date {
    static now() {
      return clock.t;
    }
  }
  let appSays = true;
  const { api, calls, listeners } = fakeApi({ firefox: false, panelOn: () => appSays });
  load(api, false, { Date: FakeDate });
  const embed = { id: "ours", url: "https://tube.test/embed/1", frameId: 3, tab: { url: "https://blog.test/post" } };
  const top = { id: "ours", url: "https://tube.test/watch", frameId: 0, tab: { url: "https://tube.test/watch" } };
  const ask = (sender) =>
    new Promise((r) => listeners.message({ type: "panel" }, sender, (v) => r(JSON.parse(JSON.stringify(v)))));
  assert.deepEqual(await ask(embed), { on: true, compact: false });
  const asked = () => natives(calls).filter((m) => m.panel);
  assert.equal(asked()[0].referrer, "https://blog.test/post", "the tab's address with an embedded player's question");
  assert.deepEqual(await ask(top), { on: true, compact: false });
  assert.equal(asked()[1].referrer, undefined, "none for the tab's own page");
  appSays = false;
  clock.t += 1000;
  assert.deepEqual(await ask(top), { on: true, compact: false }, "kept a moment (the page's frames ask together)");
  assert.equal(asked().length, 2);
  clock.t += 5000;
  assert.deepEqual(await ask(top), { on: false, compact: false }, "switched off in Options: the next question has it");
  assert.deepEqual(await ask(embed), { on: false, compact: false });
  appSays = true;
  clock.t += 6000;
  assert.deepEqual(await ask(embed), { on: true, compact: false }, "and on again");
  // "Never on this site" from the embed: off at once, in its tab and in any other.
  await new Promise((r) => listeners.message({ type: "hide-site" }, embed, r));
  assert.deepEqual(await ask(embed), { on: false, compact: false });
  assert.deepEqual(await ask({ ...embed, tab: { url: "https://news.test/" } }), { on: false, compact: false }, "whatever the tab");
  ok("#532: the video button follows Options within seconds; an embed asks with its tab's address");
}
// #1278: Options → Download panels → Button size comes with the answer, and follows a change in
// Options as "may it show" does (kept a few seconds); an older host that doesn't say: Full.
{
  const clock = { t: Date.now() };
  class FakeDate extends Date {
    static now() {
      return clock.t;
    }
  }
  let compact = true;
  let on = true;
  const { api, listeners } = fakeApi({ firefox: false, panelOn: () => on, panelCompact: () => compact });
  load(api, false, { Date: FakeDate });
  const page = { id: "ours", url: "https://tube.test/watch", frameId: 0, tab: { url: "https://tube.test/watch" } };
  const ask = () =>
    new Promise((r) => listeners.message({ type: "panel" }, page, (v) => r(JSON.parse(JSON.stringify(v)))));
  assert.deepEqual(await ask(), { on: true, compact: true });
  compact = false;
  clock.t += 1000;
  assert.deepEqual(await ask(), { on: true, compact: true }, "kept a moment");
  clock.t += 5000;
  assert.deepEqual(await ask(), { on: true, compact: false }, "Full again in Options: the next question has it");
  compact = true;
  on = false;
  clock.t += 6000;
  assert.deepEqual(await ask(), { on: false, compact: false }, "off: never compact");
  const older = fakeApi({ firefox: false, panelOn: () => true });
  load(older.api, false);
  const said = await new Promise((r) => older.listeners.message({ type: "panel" }, page, (v) => r(JSON.parse(JSON.stringify(v)))));
  assert.deepEqual(said, { on: true, compact: false }, "an older host: Full");
  ok("#1278: the video button's size comes from Options with its answer, and follows a change");
}
{
  const { api, listeners } = fakeApi({ firefox: true, panelOn: false });
  load(api, true);
  const on = await new Promise((r) =>
    listeners.message({ type: "panel" }, { id: "ours", url: "https://x.test/", tab: {} }, (v) => r(JSON.parse(JSON.stringify(v)))),
  );
  assert.deepEqual(on, { on: false, compact: false }, "where the app says no");
  ok("video button messages");
}
{
  const { api, calls, listeners } = fakeApi({ firefox: true });
  load(api, true);
  const ask = (m, sender) => new Promise((r) => listeners.message(m, sender, (x) => r(JSON.parse(JSON.stringify(x)))));

  // Valid pointer move in a Drive child frame -> forwarded to top frame (frameId 0)
  const driveSubframe = {
    id: "ours",
    url: "https://drive.google.com/file/d/1s52ek_4YTDRt_EOkx1FS53u-vJa0c4nu/preview",
    frameId: 2,
    tab: { id: 7, url: "https://drive.google.com/drive/folders/18l2MEarbNGo94E1x0sU2mpfkRmLrVVo3" },
  };
  const res1 = await ask({ type: "drive-frame-pointer" }, driveSubframe);
  assert.deepEqual(res1, { ok: true });
  assert.ok(
    calls.some(
      ([kind, tabId, msg, opts]) =>
        kind === "tabMessage" && tabId === 7 && msg.type === "drive-frame-pointer" && opts.frameId === 0,
    ),
    "forwarded to frameId 0",
  );

  // Subframe on another website -> rejected, no tabMessage
  const otherSubframe = {
    id: "ours",
    url: "https://other.test/frame",
    frameId: 2,
    tab: { id: 7, url: "https://other.test/watch" },
  };
  const res2 = await ask({ type: "drive-frame-pointer" }, otherSubframe);
  assert.deepEqual(res2, { ok: false });

  // Docs subframe that is not a player frame -> rejected
  const docsSubframe = {
    id: "ours",
    url: "https://docs.google.com/document/d/1s52ek_4YTDRt_EOkx1FS53u-vJa0c4nu/edit",
    frameId: 2,
    tab: { id: 7, url: "https://drive.google.com/drive/folders/18l2MEarbNGo94E1x0sU2mpfkRmLrVVo3" },
  };
  const resDocs = await ask({ type: "drive-frame-pointer" }, docsSubframe);
  assert.deepEqual(resDocs, { ok: false });

  // Top-level frame (frameId 0) -> rejected
  const topFrame = {
    id: "ours",
    url: "https://drive.google.com/drive/folders/18l2MEarbNGo94E1x0sU2mpfkRmLrVVo3",
    frameId: 0,
    tab: { id: 7, url: "https://drive.google.com/drive/folders/18l2MEarbNGo94E1x0sU2mpfkRmLrVVo3" },
  };
  const res3 = await ask({ type: "drive-frame-pointer" }, topFrame);
  assert.deepEqual(res3, { ok: false });

  ok("#758: background forwards frame pointer exclusively for Drive subframes to frameId 0");
}

// The button's qualities: an app whose rules say it looks videos up (`videoLookup`, kept with the
// status) gets the click as a lookup, never with `video` or a top-level `url` (an older host
// refuses such a message and does nothing), with the cookies of what yt-dlp is given; its answer
// goes back to the page as `lookup`. Without that (an older app, another MDM chosen, an answer
// that isn't a lookup's) the page gets {fallback: true}: the click as it always was.
{
  const ask = (listeners, m, sender) => new Promise((r) => listeners.message(m, sender, (x) => r(JSON.parse(JSON.stringify(x)))));
  const sender = { id: "ours", url: "https://news.test/story", frameId: 0, tab: { id: 5, url: "https://news.test/story", incognito: false, cookieStoreId: "firefox-container-2" } };
  const lookups = (calls) => natives(calls).filter((m) => m.videoLookup || m.videoPoll || m.videoPick);
  // The lookup: the page names the video, the background what the browser knows.
  {
    const { api, calls, listeners, store } = fakeApi({ firefox: true, rules: { videoLookup: true }, lookup: () => ({ ok: true, lookup: { state: "pending", token: "t1" } }) });
    load(api, true);
    await settle();
    assert.equal(store.status.videoLookup, true, "kept from the answer to rules");
    const r = await ask(listeners, { type: "video-lookup", src: "https://cdn.test/v/clip.mp4", title: "A story's film", url: "https://news.test/story", at: { x: 1, y: 1 } }, sender);
    // `show`: the address MDM lists its download under, for the button's "Show" (#703).
    assert.deepEqual(r, { lookup: { state: "pending", token: "t1" }, show: "https://cdn.test/v/clip.mp4" });
    const m = natives(calls).find((x) => x.videoLookup);
    assert.deepEqual([m.video, m.url], [undefined, undefined], "no `video`, no top-level `url`");
    assert.deepEqual(m.videoLookup, { page: "https://news.test/story", src: "https://cdn.test/v/clip.mp4", title: "A story's film", storeId: "firefox-container-2" });
    assert.deepEqual([m.cookies, m.private, m.userAgent, m.v], ["sid=1", false, "UA", "0.2.5"]);
    assert.equal(m.cookieJar.length, 1, "each cookie with its domain");
    assert.deepEqual(calls.filter((c) => c[0] === "cookies").at(-1)[1], { url: "https://cdn.test/v/clip.mp4", storeId: "firefox-container-2" }, "the file's cookies, from the tab's container");
    // A player's page in a frame, in a private window: that page's cookies, from the private
    // store, and the tab as its referrer.
    const framed = { id: "ours", url: "https://player.test/embed/1", frameId: 3, tab: { id: 6, url: "https://blog.test/post", incognito: true } };
    await ask(listeners, { type: "video-lookup", src: "blob:https://player.test/x", title: "" }, framed);
    const p = natives(calls).filter((x) => x.videoLookup).at(-1);
    assert.deepEqual([p.videoLookup, p.private], [{ page: "https://player.test/embed/1", referrer: "https://blog.test/post" }, true]);
    assert.deepEqual(calls.filter((c) => c[0] === "cookies").at(-1)[1], { url: "https://player.test/embed/1", storeId: "1" }, "the page's cookies, from the private store");
    assert.ok(!natives(calls).some((x) => x.video), "never the window's message");
  }
  // An app whose rules say nothing of it (an older one): the click as it was, no lookup sent.
  {
    const { api, calls, listeners, store } = fakeApi({ firefox: false });
    load(api, false);
    await settle();
    assert.ok(!store.status.videoLookup, "an older app's rules");
    assert.deepEqual(await ask(listeners, { type: "video-lookup", src: "https://cdn.test/v/clip.mp4" }, sender), { fallback: true });
    assert.deepEqual(lookups(calls), []);
  }
  // Another MDM chosen: none of the three goes to the host, nor to the other MDM.
  {
    const { api, calls, listeners, store } = fakeApi({ firefox: false, rules: { videoLookup: true }, lookup: () => ({ ok: true, lookup: { state: "pending", token: "t1" } }) });
    load(api, false);
    await settle();
    store.remote = { url: "https://server.test:8463/", token: "k" };
    fetched.length = 0;
    for (const m of [{ type: "video-lookup", src: "https://cdn.test/v/clip.mp4" }, { type: "video-poll", token: "t1" }, { type: "video-pick", token: "t1", choice: 0 }]) {
      assert.deepEqual(await ask(listeners, m, sender), { fallback: true }, m.type);
    }
    assert.deepEqual(lookups(calls), []);
    assert.deepEqual(fetched, [], "nothing to the other MDM");
  }
  // The ability kept from before, an older host or app now (MDM went back a version): its answer
  // has no `lookup`. Not kept as able, its rules asked again, the click as it was; the next click
  // sends no lookup at all. A refused message is no broken connection.
  {
    const { api, calls, listeners, store } = fakeApi({ firefox: false, reply: { ok: false, why: "refused", error: "not an http(s) or ftp(s) URL" } });
    store.status = { why: null, app: "0.2.5", videoLookup: true };
    store.rules = { types: DEFAULT_TYPES, capture: true, captureOff: "", excludedSites: "", excludedAddresses: [], opened: true, minKb: 0, at: Date.now() };
    load(api, false);
    await settle();
    const asked = () => calls.filter((c) => c[0] === "rules").length;
    const before = asked();
    assert.deepEqual(await ask(listeners, { type: "video-lookup", src: "https://cdn.test/v/clip.mp4" }, sender), { fallback: true });
    await settle();
    assert.equal(store.status.videoLookup, false, "not kept as able");
    assert.ok(asked() > before, "its rules asked again");
    assert.deepEqual(await ask(listeners, { type: "video-lookup", src: "https://cdn.test/v/clip.mp4" }, sender), { fallback: true });
    assert.equal(natives(calls).filter((x) => x.videoLookup).length, 1, "one lookup, then none");
    assert.ok(!calls.some((c) => c[0] === "badge" && c[1] === "!"), "no \"!\"");
  }
  // How the lookup is going, and the quality picked: the token and the row's number from the
  // page; private or not and the container from the tab, whatever the page says.
  {
    const ready = { state: "ready", token: "t1", title: "Film", choices: [{ label: "720p MP4", ext: "mp4", size: 187500000 }] };
    const answers = { videoPoll: { ok: true, lookup: ready }, videoPick: { ok: true, lookup: { state: "started" } } };
    const { api, calls, listeners } = fakeApi({ firefox: true, rules: { videoLookup: true }, lookup: (m) => answers[m.videoPoll ? "videoPoll" : "videoPick"] });
    load(api, true);
    await settle();
    assert.deepEqual(await ask(listeners, { type: "video-poll", token: "t1" }, sender), { lookup: ready });
    assert.deepEqual(natives(calls).find((x) => x.videoPoll).videoPoll, { token: "t1" });
    const privateTab = { id: 8, url: "https://news.test/story", incognito: true, cookieStoreId: "firefox-private" };
    const r = await ask(listeners, { type: "video-pick", token: "t1", choice: 0, private: false, storeId: "firefox-default" }, { id: "ours", url: "https://news.test/story", frameId: 0, tab: privateTab });
    assert.deepEqual(r, { lookup: { state: "started" } });
    const picked = natives(calls).find((x) => x.videoPick);
    assert.deepEqual(
      [picked.videoPick, picked.private, picked.url, picked.video],
      [{ token: "t1", choice: 0, storeId: "firefox-private" }, true, undefined, undefined],
      "the tab's window and container, not the page's say",
    );
    // An answer without `lookup`: the lookup is gone; a pick gets the window.
    answers.videoPoll = { ok: true };
    answers.videoPick = { ok: true };
    assert.deepEqual(await ask(listeners, { type: "video-poll", token: "t1" }, sender), { lookup: { state: "gone" } });
    assert.deepEqual(await ask(listeners, { type: "video-pick", token: "t1", choice: 0 }, sender), { fallback: true });
    // Nothing but a token and a row's number is sent on.
    const sent = natives(calls).length;
    assert.deepEqual(await ask(listeners, { type: "video-poll", token: { x: 1 } }, sender), { lookup: { state: "gone" } });
    assert.deepEqual(await ask(listeners, { type: "video-pick", token: "t1", choice: "0" }, sender), { fallback: true });
    assert.deepEqual(await ask(listeners, { type: "video-pick", token: "t1", choice: -1 }, sender), { fallback: true });
    assert.equal(natives(calls).length, sent);
  }
  // What a lookup comes to is never the connection's status: no "!" on the toolbar button, the
  // rule for PDFs and music untouched, still able.
  {
    const states = [
      { state: "failed", reason: "error", message: "ERROR: Unsupported URL" },
      { state: "failed", reason: "no-ytdlp" },
      { state: "busy" },
      { state: "off" },
      { state: "list" },
      { state: "use-window" },
      { state: "gone" },
    ];
    let next = 0;
    const { api, calls, listeners, store } = fakeApi({ firefox: false, rules: { videoLookup: true }, lookup: () => ({ ok: true, lookup: states[next++] }) });
    load(api, false);
    await settle();
    const rules = calls.filter((c) => c[0] === "dnr").length;
    for (let i = 0; i < states.length; i++) {
      const m = i % 2 ? { type: "video-poll", token: "t1" } : { type: "video-lookup", src: "https://cdn.test/v/clip.mp4" };
      assert.deepEqual((await ask(listeners, m, sender)).lookup, states[i]);
    }
    await settle();
    assert.equal(store.status.why, null);
    assert.ok(!calls.some((c) => c[0] === "badge" && c[1] === "!"), "no \"!\"");
    assert.equal(calls.filter((c) => c[0] === "dnr").length, rules, "the rule for PDFs and music untouched");
    assert.equal(store.status.videoLookup, true, "still able");
  }
  // #703: the button's own actions. "Show" (MDM's window, the download selected), "Turn on" (the
  // site off the list, at once here too), "Get MDM" and "Open post" (a tab next to this one; a post
  // only on this page's site), a plain file sent as a file. None of them has a top-level `url`.
  {
    const { api, calls, listeners } = fakeApi({ firefox: false, rules: { videoLookup: true } });
    const opened = [];
    api.tabs.create = async (o) => opened.push(JSON.parse(JSON.stringify(o)));
    load(api, false);
    await settle();
    const tab = { id: 5, index: 2, url: "https://news.test/story", incognito: false };
    const from = { id: "ours", url: "https://news.test/story", frameId: 0, tab };
    assert.deepEqual(await ask(listeners, { type: "show-download", address: "https://news.test/story" }, from), { ok: true });
    assert.deepEqual(await ask(listeners, { type: "show-download", address: "javascript:alert(1)" }, from), { ok: false });
    assert.deepEqual(await ask(listeners, { type: "show-site" }, from), { ok: true });
    const sent = natives(calls);
    assert.ok(sent.some((x) => x.showDownload === "https://news.test/story" && !x.url));
    assert.ok(sent.some((x) => x.showSite === "news.test" && !x.url));
    await ask(listeners, { type: "get-mdm" }, from);
    await ask(listeners, { type: "open-post", url: "https://news.test/p/1" }, from);
    assert.deepEqual(await ask(listeners, { type: "open-post", url: "https://elsewhere.test/p/1" }, from), { ok: false }, "a post on this page's site only");
    assert.deepEqual(opened, [
      { url: "https://misterdownloadmanager.com/download", openerTabId: 5, index: 3 },
      { url: "https://news.test/p/1", openerTabId: 5, index: 3 },
    ]);
    // A plain file: `direct` with it, and `show` back for "Show".
    const r = await ask(listeners, { type: "video", src: "https://cdn.test/v/clip.mp4", url: "https://news.test/story", direct: true, title: "" }, from);
    assert.deepEqual(r, { ok: true, show: "https://cdn.test/v/clip.mp4" });
    const v = natives(calls).filter((x) => x.video).at(-1);
    assert.deepEqual([v.direct, v.src, v.mime], [true, "https://cdn.test/v/clip.mp4", undefined]);
    // #800: a file the tab loaded goes with the type its server said (a script's answer is named
    // by it at once), and the page's title.
    listeners.responseStarted({ tabId: 5, frameId: 0, type: "media", url: "https://cdn.test/remote_control.php?id=4", statusCode: 200, responseHeaders: [{ name: "Content-Type", value: "video/mp4" }, { name: "Content-Length", value: "9000000" }] });
    await settle();
    await ask(listeners, { type: "video", src: "https://cdn.test/remote_control.php?id=4", url: "https://news.test/story", direct: true, title: "A talk" }, from);
    const s = natives(calls).filter((x) => x.video).at(-1);
    assert.deepEqual([s.direct, s.mime, s.title], [true, "video/mp4", "A talk"]);
    // `direct` without a file of its own: the page, as before.
    await ask(listeners, { type: "video", url: "https://news.test/story", direct: true, src: "blob:https://news.test/x" }, from);
    assert.equal(natives(calls).filter((x) => x.video).at(-1).direct, undefined);
  }
  ok("video button: qualities looked up by the app, picked in the page; else the window, as before; Show, Turn on, Get MDM, Open post, a plain file");
}

// The title a video is named by when yt-dlp knows it only by its address goes without the site's
// name, and a player in a frame of the page (another site's, an embed) is named by the tab's title,
// not by its own ("… from Justin Barry on Vimeo"): the film page a browser saw, reduced
// (crates/app/tests/data/embedded-player-title.json; the app's side is videodlg.rs').
{
  const film = JSON.parse(readText("../../crates/app/tests/data/embedded-player-title.json"));
  const ask = (listeners, m, sender) => new Promise((r) => listeners.message(m, sender, (x) => r(JSON.parse(JSON.stringify(x)))));
  const tab = { id: 8, url: film.tab.url, title: film.tab.title, status: "complete" };
  const frames = { 0: { href: film.tab.url, players: [] }, 4: { href: film.frame.url, players: [] } };
  const { api, calls, listeners } = fakeApi({ firefox: false, frames, tabs: { 8: tab }, rules: { videoLookup: true }, lookup: () => ({ ok: true, lookup: { state: "pending", token: "t1" } }) });
  load(api, false);
  await settle();
  listeners.responseStarted({ tabId: 8, frameId: 4, url: film.stream, responseHeaders: [{ name: "Content-Type", value: "application/vnd.apple.mpegurl" }] });
  const framed = { id: "ours", url: film.frame.url, frameId: 4, tab };
  const clicked = { src: "", title: film.frame.title, url: film.frame.url, at: { x: 1, y: 1 } };
  await ask(listeners, { type: "video-lookup", ...clicked }, framed);
  const looked = natives(calls).find((x) => x.videoLookup).videoLookup;
  assert.deepEqual([looked.page, looked.src, looked.title], [film.frame.url, film.stream, film.sent], "the lookup: the frame's stream, the tab's title");
  await ask(listeners, { type: "video", ...clicked }, framed);
  assert.equal(natives(calls).find((x) => x.video).title, film.sent, "the window: the same title");
  // The same page's own player (not in a frame): its title, without the site's name.
  const top = { id: "ours", url: film.tab.url, frameId: 0, tab: framed.tab };
  await ask(listeners, { type: "video", src: "", title: film.tab.title, url: film.tab.url }, top);
  assert.equal(natives(calls).filter((x) => x.video).at(-1).title, film.sent, "the page's own player");
  // A frame in a tab without a title: the frame's own.
  await ask(listeners, { type: "video", ...clicked }, { ...framed, tab: { id: 8, url: film.tab.url } });
  assert.equal(natives(calls).filter((x) => x.video).at(-1).title, film.frame.title);
  // The toolbar's media list: the stream goes with the tab's title, without the site's name.
  const listed = await ask(listeners, { type: "media-download", tab: 8, url: film.stream }, { id: "ours", url: "chrome-extension://ours/popup.html" });
  assert.ok(listed.ok, "sent from the list");
  assert.equal(natives(calls).filter((x) => x.video).at(-1).title, film.sent, "the media list");

  const ctx = { URL, URLSearchParams, console: quietConsole, setTimeout, clearTimeout, Promise, TextDecoder, fetch: fakeFetch, navigator: { userAgent: "UA" } };
  ctx.chrome = fakeApi({ firefox: false }).api;
  vm.runInNewContext(code, ctx);
  for (const [title, url, want] of [
    [film.tab.title, film.tab.url, film.sent],
    ["Election results – BBC News", "https://www.bbc.co.uk/news/x", "Election results"],
    ["The Verge | A phone, reviewed", "https://www.theverge.com/x", "A phone, reviewed"],
    ["Watch: a talk about download managers - YouTube", "https://m.youtube.com/watch?v=x", "Watch: a talk about download managers"],
    ["A talk :: Example", "https://example.org/", "A talk"],
    // Only the site's own name, only at an end; never the whole title.
    ["Before The - Short Film", film.tab.url, "Before The - Short Film"],
    ["A clip | Another site", film.tab.url, "A clip | Another site"],
    ["Apple pie - Easy dessert | Apple", "https://www.apple.com/", "Apple pie - Easy dessert"],
    ["Shortverse", film.tab.url, "Shortverse"],
    ["Shortverse | Shortverse", film.tab.url, "Shortverse"],
    ["Clip - 192", "http://192.168.1.5/clip", "Clip - 192"],
    [" A film | Shortverse ", "not an address", "A film | Shortverse"],
  ]) {
    assert.equal(ctx.withoutSiteName(title, url), want, title);
  }
  ok("a page's title names its video without the site's name; a player in a frame by the tab's title");
}

// What a fetched start says it is: an HLS playlist, a DASH manifest (after an XML declaration and
// comments, with a namespace prefix or not), or neither; read in one pass, so a long or odd start
// costs no more than its length.
{
  const ctx = { URL, URLSearchParams, console: quietConsole, setTimeout, clearTimeout, Promise, TextDecoder, fetch: async () => ({ ok: false }), navigator: { userAgent: "UA" } };
  ctx.chrome = fakeApi({ firefox: false }).api;
  vm.runInNewContext(code, ctx);
  for (const [text, want] of [
    ["#EXTM3U\n#EXT-X-VERSION:3", "hls"],
    ["﻿  #EXTM3U", "hls"],
    ['<?xml version="1.0" encoding="UTF-8"?>\n<MPD xmlns="urn:mpeg:dash:schema:mpd:2011">', "dash"],
    ['<!-- made by a packager -->\n<!-- two -->\n<MPD type="static">', "dash"],
    ['<?xml version="1.0"?><!-- c --><mpd:MPD xmlns:mpd="x">', "dash"],
    ["<MPD/>", "dash"],
    ["<html><body>MPD</body></html>", ""],
    ["<MPDX>", ""],
    ["<!-- never closed <MPD>", ""],
    ['<?xml version="1.0"', ""],
    ["", ""],
  ]) {
    assert.equal(ctx.manifestKind(text), want, JSON.stringify(text));
  }
  const odd = "<!--" + "--><!--".repeat(2000) + "x";
  const started = Date.now();
  assert.equal(ctx.manifestKind(odd), "");
  assert.ok(Date.now() - started < 200, `an odd start took ${Date.now() - started} ms`);
  ok("a fetched start is told an HLS playlist, a DASH manifest or neither, in one pass");
}

// #38: the media a tab loaded, as the popup lists them. A response is media by its type or its
// address; segments, byte ranges and small pieces a player fetched are parts of a stream.
const H = (type, more = {}) => [{ name: "Content-Type", value: type }, ...Object.entries(more).map(([name, value]) => ({ name, value }))];
const res = (url, type, headers = [], more = {}) => ({ tabId: 9, frameId: 0, url, type, statusCode: 200, responseHeaders: headers, ...more });
const popupAsk = (listeners, m) =>
  new Promise((r) => listeners.message(m, { id: "ours", url: "chrome-extension://ours/popup.html" }, (v) => r(JSON.parse(JSON.stringify(v)))));
const MASTER = `#EXTM3U
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="aud",NAME="English",URI="audio/index.m3u8"
#EXT-X-STREAM-INF:BANDWIDTH=1280000,RESOLUTION=1280x720,AUDIO="aud"
720p/index.m3u8?tok=1
#EXT-X-STREAM-INF:BANDWIDTH=640000,RESOLUTION=640x360,AUDIO="aud"
360p/index.m3u8?tok=1
`;
const MPD = (protect) => `<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT1M30S">
<Period><AdaptationSet mimeType="video/mp4">${protect ? '<ContentProtection schemeIdUri="urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed"/>' : ""}
<Representation id="1080p" bandwidth="4000000" width="1920" height="1080"/>
<Representation id="480p" bandwidth="900000" width="854" height="480"/>
</AdaptationSet></Period></MPD>`;
{
  served = {
    "https://cdn.test/v/master.m3u8": { body: MASTER },
    "https://cdn.test/d/manifest.mpd": { body: MPD(false) },
    "https://drm.test/p/manifest.mpd": { body: MPD(true) },
    "https://fair.test/f/master.m3u8": {
      body: '#EXTM3U\n#EXT-X-SESSION-KEY:METHOD=SAMPLE-AES,URI="skd://k",KEYFORMAT="com.apple.streamingkeydelivery"\n#EXT-X-STREAM-INF:RESOLUTION=1920x1080\nhi.m3u8\n',
    },
    "https://aes.test/a/index.m3u8": { body: '#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI="key.bin"\n#EXTINF:6.0,\ns0.ts\n#EXTINF:4.5,\ns1.ts\n#EXT-X-ENDLIST\n' },
  };
  const frames = {
    0: { href: "https://watch.test/v/1", players: [{ src: "https://files.test/movie.mp4", w: 1920, h: 1080, duration: 125, drm: false }] },
    3: { href: "https://player.test/embed/7", players: [{ src: "blob:https://player.test/x", w: 0, h: 0, duration: 0, drm: true }] },
  };
  const tabs = { 9: { id: 9, url: "https://watch.test/v/1", title: "A film", incognito: false } };
  const { api, calls, listeners } = fakeApi({ firefox: false, frames, tabs });
  load(api, false);
  listeners.tabUpdated(9, { url: "https://watch.test/v/1" });
  await settle();
  const seen = [
    // HLS: the master, then its variants (with a query) and segments.
    res("https://cdn.test/v/master.m3u8", "xmlhttprequest", H("application/vnd.apple.mpegurl")),
    res("https://cdn.test/v/720p/index.m3u8?tok=1", "xmlhttprequest", H("application/vnd.apple.mpegurl")),
    res("https://cdn.test/v/audio/index.m3u8", "xmlhttprequest", H("audio/mpegurl")),
    res("https://cdn.test/v/720p/seg0.ts", "xmlhttprequest", H("video/mp2t")),
    // DASH, its init segment (video/mp4, in its folder) and fragments.
    res("https://cdn.test/d/manifest.mpd", "xmlhttprequest", H("application/dash+xml")),
    res("https://cdn.test/d/1080p/init.mp4", "xmlhttprequest", H("video/mp4", { "Content-Length": "4000000" })),
    res("https://cdn.test/d/1080p/seg1.m4s", "xmlhttprequest", H("video/iso.segment")),
    // A protected DASH stream, and one in a frame whose manifest can't be read.
    res("https://drm.test/p/manifest.mpd", "xmlhttprequest", H("application/dash+xml")),
    res("https://locked.test/l/master.m3u8", "xmlhttprequest", H("application/x-mpegURL"), { frameId: 3 }),
    res("https://locked.test/l/hd/index.m3u8", "xmlhttprequest", H("application/x-mpegURL"), { frameId: 3 }),
    // FairPlay (SAMPLE-AES), and AES-128 with a plain key (not DRM).
    res("https://fair.test/f/master.m3u8", "xmlhttprequest", H("application/vnd.apple.mpegurl")),
    res("https://aes.test/a/index.m3u8", "xmlhttprequest", H("application/vnd.apple.mpegurl")),
    // A <video>'s own file in ranges (its size from Content-Range), asked twice.
    res("https://files.test/movie.mp4", "media", H("video/mp4", { "Content-Range": "bytes 0-1023/52428800" }), { statusCode: 206 }),
    res("https://files.test/movie.mp4", "media", H("video/mp4", { "Content-Range": "bytes 1024-2047/52428800" }), { statusCode: 206 }),
    // An <audio>'s file sent as octet-stream, known by its extension.
    res("https://files.test/song.mp3?x=1", "media", H("application/octet-stream", { "Content-Length": "3000000" })),
    // A player's fetch: a whole large file is listed; a range, a small piece, a 404 are not.
    res("https://files.test/whole.webm", "xmlhttprequest", H("video/webm", { "Content-Length": "9000000" })),
    res("https://files.test/part.mp4", "xmlhttprequest", H("video/mp4", { "Content-Range": "bytes 0-99/900000000" }), { statusCode: 206 }),
    res("https://yt.test/videoplayback?id=1&range=0-50000", "xmlhttprequest", H("video/mp4", { "Content-Length": "5000000" })),
    res("https://files.test/tiny.mp4", "xmlhttprequest", H("video/mp4", { "Content-Length": "40000" })),
    res("https://files.test/gone.mp4", "media", H("video/mp4"), { statusCode: 404 }),
    // Not media; not the tab's (the extension's own fetch); not a web address.
    res("https://files.test/page.html", "xmlhttprequest", H("text/html", { "Content-Length": "9000000" })),
    res("https://files.test/other.mp4", "media", H("video/mp4"), { tabId: -1 }),
    res("blob:https://files.test/1", "media", H("video/mp4")),
  ];
  for (const d of seen) listeners.responseStarted(d);
  await settle();
  const { media } = await popupAsk(listeners, { type: "media", tab: 9 });
  const by = Object.fromEntries(media.map((m) => [m.url, m]));
  assert.deepEqual(
    media.map((m) => m.url),
    [
      "https://cdn.test/v/master.m3u8",
      "https://cdn.test/d/manifest.mpd",
      "https://drm.test/p/manifest.mpd",
      "https://locked.test/l/master.m3u8",
      "https://fair.test/f/master.m3u8",
      "https://aes.test/a/index.m3u8",
      "https://files.test/movie.mp4",
      "https://files.test/song.mp3?x=1",
      "https://files.test/whole.webm",
    ],
    "streams with their variants inside, files once, no segment or piece",
  );
  const master = by["https://cdn.test/v/master.m3u8"];
  assert.deepEqual([master.kind, master.heights, master.drm, master.live], ["hls", [720, 360], false, false]);
  const dash = by["https://cdn.test/d/manifest.mpd"];
  assert.deepEqual([dash.kind, dash.heights, dash.duration, dash.drm], ["dash", [1080, 480], 90, false]);
  assert.equal(by["https://drm.test/p/manifest.mpd"].drm, true, "Widevine's ContentProtection");
  assert.equal(by["https://locked.test/l/master.m3u8"].drm, true, "unread, its frame's player has a key system");
  assert.equal(by["https://fair.test/f/master.m3u8"].drm, true, "FairPlay, SAMPLE-AES");
  const aes = by["https://aes.test/a/index.m3u8"];
  assert.deepEqual([aes.drm, aes.duration, aes.live], [false, 10.5, false], "AES-128 with its key: not DRM");
  const movie = by["https://files.test/movie.mp4"];
  assert.deepEqual([movie.kind, movie.size, movie.width, movie.heights, movie.duration], ["video", 52428800, 1920, [1080], 125]);
  const song = by["https://files.test/song.mp3?x=1"];
  assert.deepEqual([song.kind, song.size], ["audio", 3000000]);
  assert.ok(fetched.every(([, c]) => c === "include"), "manifests read with the site's cookies");
  const reads = fetched.length;
  await popupAsk(listeners, { type: "media", tab: 9 });
  assert.equal(fetched.length, reads, "each manifest read once");
  ok("media list: streams grouped, files, details, protected ones");

  // Download: a stream goes to the Download video window (explicitly), with the page that played
  // it and the cookies its own server gets; a file to the download path; a protected one nowhere.
  const sent = await popupAsk(listeners, { type: "media-download", tab: 9, url: "https://cdn.test/v/master.m3u8" });
  assert.equal(sent.ok, true);
  const v = natives(calls).find((m) => m.video);
  assert.deepEqual([v.explicit, v.src, v.url, v.title, v.cookies, v.referrer], [true, "https://cdn.test/v/master.m3u8", "https://watch.test/v/1", "A film", "sid=1", ""]);
  assert.deepEqual(calls.filter((c) => c[0] === "cookies").pop()[1], { url: "https://cdn.test/v/master.m3u8" });
  assert.deepEqual(v.cookieJar.map((c) => c.domain), [".tube.test"], "each cookie with its domain");
  await popupAsk(listeners, { type: "media-download", tab: 9, url: "https://files.test/movie.mp4" });
  const f = natives(calls).find((m) => m.url === "https://files.test/movie.mp4");
  assert.deepEqual([f.explicit, f.referrer, f.size, f.mime, f.video], [true, "https://watch.test/v/1", 52428800, "video/mp4", undefined]);
  assert.equal(f.filename, "", "a file with a name of its own is named by its address");
  await api.storage.local.set({ mediaTitleName: true });
  await popupAsk(listeners, { type: "media-download", tab: 9, url: "https://files.test/movie.mp4" });
  assert.equal(natives(calls).filter((m) => m.url === "https://files.test/movie.mp4").at(-1).filename, "A film - movie.mp4");
  // #345: an address that ends in a long hash names nothing: the tab's title does.
  const hashed = "https://files.test/SFp1NUcrZFRkRUVrUWlqM3paMitkakJIUW9JSFhOeEZkd0ZMNEc5UHpnZm5hRkY4Vis5MjlJU00xdDVqTk1vaw.mp4";
  listeners.responseStarted({ tabId: 9, frameId: 0, type: "media", url: hashed, responseHeaders: [{ name: "Content-Type", value: "video/mp4" }, { name: "Content-Length", value: "52428800" }] });
  await settle();
  await popupAsk(listeners, { type: "media-download", tab: 9, url: hashed });
  // A hash isn't worth keeping, with the switch on too: the title alone names it.
  assert.equal(natives(calls).filter((m) => m.url === hashed).at(-1).filename, "A film.mp4");
  await api.storage.local.set({ mediaTitleName: false });
  assert.equal(natives(calls).find((m) => m.url === hashed).filename, "A film.mp4");
  // #493: X's video server names each file with a short random id: the tab's title names it.
  const random = "https://video.twimg.com/ext_tw_video/1900000000000000000/pu/vid/avc1/720x1280/7HbL0kYxzF-fGjqd.mp4?tag=12";
  listeners.responseStarted({ tabId: 9, frameId: 0, type: "media", url: random, responseHeaders: [{ name: "Content-Type", value: "video/mp4" }, { name: "Content-Length", value: "52428800" }] });
  await settle();
  await popupAsk(listeners, { type: "media-download", tab: 9, url: random });
  assert.equal(natives(calls).find((m) => m.url === random).filename, "A film.mp4");
  const before = natives(calls).length;
  assert.deepEqual(await popupAsk(listeners, { type: "media-download", tab: 9, url: "https://drm.test/p/manifest.mpd" }), { ok: false, why: "drm" });
  assert.deepEqual(await popupAsk(listeners, { type: "media-download", tab: 9, url: "https://evil.test/x.mp4" }), { ok: false, why: "gone" });
  assert.equal(natives(calls).length, before, "nothing sent for a protected stream or an address the tab didn't load");
  ok("media list: downloads, never a protected one");

  // A move within the page keeps the list; another page clears it; so does closing the tab.
  listeners.tabUpdated(9, { url: "https://watch.test/v/1#t=30" });
  await settle();
  assert.equal((await popupAsk(listeners, { type: "media", tab: 9 })).media.length, 11);
  listeners.tabUpdated(9, { url: "https://watch.test/v/2" });
  await settle();
  assert.equal((await popupAsk(listeners, { type: "media", tab: 9 })).media.length, 0);
  listeners.responseStarted(res("https://files.test/movie.mp4", "media", H("video/mp4")));
  await settle();
  listeners.tabRemoved(9);
  await settle();
  assert.equal((await popupAsk(listeners, { type: "media", tab: 9 })).media.length, 0);
  ok("media list: cleared on another page, kept within one");
}
{
  // #695, #800: a video a server's program sends (`remote_control.php`, `stream.php?file=…`) is
  // listed and saved as the app's engine names it: the tab's title (or the `file=` value) and the
  // type its server said, never "remote_control.php · PHP". The popup's switch puts the title
  // before a real name.
  const tabs = { 9: { id: 9, url: "https://watch.test/v/2", title: "Movie: the title", incognito: false } };
  const { api, calls, listeners } = fakeApi({ firefox: false, frames: { 0: { href: "https://watch.test/v/2", players: [] } }, tabs });
  load(api, false);
  listeners.tabUpdated(9, { url: "https://watch.test/v/2" });
  await settle();
  const big = (type) => H(type, { "Content-Length": "176900000" });
  const script = "https://files.test/remote_control.php?id=7&t=1";
  const given = "https://files.test/baigiang/stream.php?file=FL262-1E01-4.mp4&token=x";
  const bare = "https://files.test/videoplayback?id=3";
  const real = "https://files.test/a/movie.mp4";
  for (const [u, type] of [[script, "video/mp4"], [given, "video/mp4"], [bare, "video/webm"], [real, "video/mp4"]]) {
    listeners.responseStarted(res(u, "media", big(type)));
  }
  await settle();
  const list = async () => (await popupAsk(listeners, { type: "media", tab: 9 })).media.map((m) => [m.name, m.ext]);
  assert.deepEqual(await list(), [
    ["Movie the title.mp4", "mp4"],
    ["FL262-1E01-4.mp4", "mp4"],
    ["Movie the title.webm", "webm"],
    ["movie.mp4", "mp4"],
  ]);
  const saved = async (u) => {
    await popupAsk(listeners, { type: "media-download", tab: 9, url: u });
    const m = natives(calls).filter((x) => x.url === u).at(-1);
    return [m.filename, m.title, m.mime];
  };
  assert.deepEqual(await saved(script), ["Movie the title.mp4", "Movie: the title", "video/mp4"]);
  // A name the app gives itself (the `file=` value, a real one) isn't sent: a server's own name
  // (Content-Disposition) still wins there.
  assert.deepEqual(await saved(given), ["", "Movie: the title", "video/mp4"]);
  assert.deepEqual(await saved(real), ["", "Movie: the title", "video/mp4"]);
  await api.storage.local.set({ mediaTitleName: true });
  assert.deepEqual(await list(), [
    ["Movie the title.mp4", "mp4"],
    ["Movie the title - FL262-1E01-4.mp4", "mp4"],
    ["Movie the title.webm", "webm"],
    ["Movie the title - movie.mp4", "mp4"],
  ]);
  assert.equal((await saved(given))[0], "Movie the title - FL262-1E01-4.mp4");
  assert.equal((await saved(bare))[0], "Movie the title.webm");
  // The popup's tag is the saved file's type (popup.js tagOf), its name the saved one (nameOf).
  const popup = readText("../../extension/popup.js");
  const part = (name) => popup.slice(popup.indexOf(`function ${name}(`), popup.indexOf("\n}\n", popup.indexOf(`function ${name}(`)) + 2);
  const shown = new Function(`const TYPES = {}; ${part("tagOf")}\n${part("nameOf")}\nreturn [tagOf, nameOf];`)();
  const [tagOf, nameOf] = shown;
  const listed = (await popupAsk(listeners, { type: "media", tab: 9 })).media;
  assert.deepEqual(listed.map((m) => `${nameOf(m)} · ${tagOf(m)}`), [
    "Movie the title.mp4 · MP4",
    "Movie the title - FL262-1E01-4.mp4 · MP4",
    "Movie the title.webm · WEBM",
    "Movie the title - movie.mp4 · MP4",
  ]);
  ok("media list: a script's video named by the title or its file= value, typed by its server (#695, #800)");
}
{
  // The names the extension gives a page's media are the app's engine's (crates/engine/src/naming.rs):
  // the same cases (crates/engine/tests/data/media-names.json), the same types, the same scripts.
  const ctx = { URL, URLSearchParams, console: quietConsole, setTimeout, clearTimeout, Promise, TextDecoder, fetch: fakeFetch, navigator: { userAgent: "UA" } };
  ctx.chrome = fakeApi({ firefox: false }).api;
  vm.runInNewContext(code, ctx);
  const cases = JSON.parse(readText("../../crates/engine/tests/data/media-names.json")).cases;
  for (const [u, mime, title, want] of cases) assert.equal(ctx.mediaName(u, mime, title, false).name, want, u);
  const naming = readText("../../crates/engine/src/naming.rs");
  const table = naming.slice(naming.indexOf("pub const TYPES"), naming.indexOf("];", naming.indexOf("pub const TYPES")));
  const rustTypes = Object.fromEntries([...table.matchAll(/\("([^"]+)", "([^"]+)"\)/g)].map((m) => [m[1], m[2]]));
  assert.ok(Object.keys(rustTypes).length > 30);
  const types = vm.runInNewContext("TYPE_EXTENSIONS", ctx);
  assert.deepEqual({ ...types }, rustTypes, "background.js' TYPE_EXTENSIONS is naming.rs' TYPES");
  const list = (name) => {
    const at = naming.indexOf(`pub const ${name}`);
    return [...naming.slice(at, naming.indexOf("];", at)).matchAll(/"([^"]+)"/g)].map((m) => m[1]);
  };
  assert.deepEqual([...vm.runInNewContext("SCRIPT_EXTENSIONS", ctx)], list("SCRIPT_EXTENSIONS"));
  assert.deepEqual([...vm.runInNewContext("PAGE_EXTENSIONS", ctx)], list("PAGE_EXTENSIONS"));
  const video = readText("../../extension/video.js");
  const scripts = JSON.parse(/const SCRIPT_EXTENSIONS = (\[[^\]]+\]);/.exec(video)[1]);
  assert.deepEqual(scripts, [...list("SCRIPT_EXTENSIONS"), ...list("PAGE_EXTENSIONS")], "video.js knows the same scripts and pages");
  // The button over a plain file says its type as the address says it: a script's file= value's,
  // none for a script alone (not "PHP").
  const at = video.indexOf("  function fileType(");
  const fileType = new Function(`const SCRIPT_EXTENSIONS = ${JSON.stringify(scripts)};\n${video.slice(at, video.indexOf("\n  }\n", at) + 4)}\nreturn fileType;`)();
  assert.equal(fileType("https://u.test/stream.php?file=FL262-1E01-4.mp4&token=x"), "mp4");
  assert.equal(fileType("https://u.test/remote_control.php?id=1"), "");
  assert.equal(fileType("https://u.test/v/clip.webm?x=1"), "webm");
  assert.equal(fileType("https://u.test/stream.php?file=setup.exe"), "");
  ok("media names: the extension's are the engine's (types, scripts, cases)");
}
{
  // Bounded: a page loading file after file keeps at most 60 per tab, its first stream (the one
  // the button sends) included; at most 50 tabs.
  served = {};
  const { api, listeners } = fakeApi({ firefox: true, frames: { 0: { href: "https://live.test/", players: [] } } });
  load(api, true);
  for (let tab = 100; tab < 160; tab++) listeners.responseStarted(res("https://x.test/a.mp4", "media", H("video/mp4"), { tabId: tab }));
  listeners.responseStarted(res("https://live.test/s/master.m3u8", "xmlhttprequest", H("application/vnd.apple.mpegurl")));
  for (let i = 0; i < 500; i++) listeners.responseStarted(res(`https://live.test/clip${i}.mp4`, "media", H("video/mp4")));
  await settle();
  const { media } = await popupAsk(listeners, { type: "media", tab: 9 });
  assert.equal(media.length, 60);
  assert.equal(media[0].url, "https://live.test/s/master.m3u8");
  assert.equal(media[59].url, "https://live.test/clip499.mp4");
  assert.equal((await popupAsk(listeners, { type: "media", tab: 100 })).media.length, 0, "the least recent tabs go");
  assert.equal((await popupAsk(listeners, { type: "media", tab: 159 })).media.length, 1);
  ok("media list: bounded per tab and in tabs");
}
{
  // A stream an embedded player (a frame) loaded: the frame's page goes as its page, the tab's
  // as the referrer, as the button over videos sends it.
  served = { "https://emb.test/e/master.m3u8": { body: "#EXTM3U\n#EXT-X-STREAM-INF:RESOLUTION=1280x720\nhd.m3u8\n" } };
  const frames = { 0: { href: "https://blog.test/post", players: [] }, 2: { href: "https://emb.test/player/1", players: [] } };
  const tabs = { 9: { id: 9, url: "https://blog.test/post", title: "Post" } };
  const { api, calls, listeners } = fakeApi({ firefox: false, frames, tabs });
  load(api, false);
  listeners.responseStarted(res("https://emb.test/e/master.m3u8", "xmlhttprequest", H("application/vnd.apple.mpegurl"), { frameId: 2 }));
  await settle();
  assert.equal((await popupAsk(listeners, { type: "media-download", tab: 9, url: "https://emb.test/e/master.m3u8" })).ok, true);
  const v = natives(calls).find((m) => m.video);
  assert.deepEqual([v.url, v.referrer, v.src], ["https://emb.test/player/1", "https://blog.test/post", "https://emb.test/e/master.m3u8"]);
  ok("media list: a frame's stream goes with the frame's page");
}
{
  // #900: what the browser sent with a stream (its Referer, Origin and Cookie; Chromium shows
  // them with extraHeaders only) goes with it, from the media list and from the button over its
  // player: the Referer and Origin its player sent, and the cookies it sent that no store lists
  // (a frame's own) beside the store's, for its host. A request that failed leaves nothing.
  const ask = (listeners, m, sender) => new Promise((r) => listeners.message(m, sender, (x) => r(JSON.parse(JSON.stringify(x)))));
  for (const firefox of [false, true]) {
    const frames = { 0: { href: "https://anime.test/ep/1", players: [] }, 2: { href: "https://player.video.test/embed/x", players: [] } };
    const tabs = { 9: { id: 9, url: "https://anime.test/ep/1", title: "Episode 1" } };
    const { api, calls, listeners } = fakeApi({ firefox, frames, tabs, rules: { videoLookup: true }, lookup: () => ({ ok: true, lookup: { state: "pending", token: "t1" } }) });
    load(api, firefox);
    await settle();
    assert.deepEqual(JSON.parse(JSON.stringify(listeners.sendHeadersSpec[1])), firefox ? ["requestHeaders"] : ["requestHeaders", "extraHeaders"]);
    const url = "https://cdn.video.test/manifest/x.m3u8?sec=abc";
    const headers = [
      { name: "Accept", value: "*/*" },
      { name: "Origin", value: "https://player.video.test" },
      { name: "Referer", value: "https://player.video.test/" },
      { name: "Cookie", value: "sid=1; part=p1" },
    ];
    // Another request that failed (its answer is another's), then the stream's.
    listeners.sendHeaders({ requestId: "6", tabId: 9, frameId: 0, url: "https://cdn.video.test/x.m3u8", requestHeaders: [{ name: "Origin", value: "https://wrong.test" }] });
    listeners.errorOccurred({ requestId: "6" });
    listeners.responseStarted(res("https://cdn.video.test/x.m3u8", "xmlhttprequest", H("application/vnd.apple.mpegurl"), { requestId: "6" }));
    listeners.sendHeaders({ requestId: "7", tabId: 9, frameId: 2, url, requestHeaders: headers });
    listeners.responseStarted(res(url, "xmlhttprequest", H("application/vnd.apple.mpegurl"), { frameId: 2, requestId: "7" }));
    await settle();
    assert.equal((await popupAsk(listeners, { type: "media-download", tab: 9, url })).ok, true);
    const v = natives(calls).find((m) => m.video);
    assert.deepEqual(v.fetchedWith, { referer: "https://player.video.test/", origin: "https://player.video.test" });
    assert.equal(v.cookies, "sid=1; part=p1", "the header the browser sent for it");
    assert.deepEqual(
      v.cookieJar.map((c) => [c.name, c.value, c.domain, c.hostOnly]),
      [
        ["sid", "1", ".tube.test", false],
        ["part", "p1", "cdn.video.test", true],
      ],
      "the store's, and the frame's own for its host",
    );
    await popupAsk(listeners, { type: "media-download", tab: 9, url: "https://cdn.video.test/x.m3u8" });
    const failed = natives(calls).filter((m) => m.video).at(-1);
    assert.deepEqual([failed.src, failed.fetchedWith], ["https://cdn.video.test/x.m3u8", undefined], "nothing kept from a failed request");
    // The button over the frame's player (playing from blob:): its stream, the same way.
    const sender = { id: "ours", url: "https://player.video.test/embed/x", frameId: 2, tab: { id: 9, url: "https://anime.test/ep/1", incognito: false } };
    await ask(listeners, { type: "video-lookup", src: "blob:https://player.video.test/1", title: "" }, sender);
    const m = natives(calls).find((x) => x.videoLookup);
    assert.equal(m.videoLookup.src, url);
    assert.deepEqual(m.fetchedWith, { referer: "https://player.video.test/", origin: "https://player.video.test" });
    assert.equal(m.cookieJar.length, 2);
  }
  ok("a stream goes with the Referer, Origin and cookies its player sent (#900)");
}
{
  // The browser stopped the script and started it again: what the tab loaded is read back.
  const session = {};
  const first = fakeApi({ firefox: false, session });
  load(first.api, false);
  first.listeners.responseStarted(res("https://files.test/kept.mp4", "media", H("video/mp4")));
  await new Promise((r) => setTimeout(r, 300));
  const again = fakeApi({ firefox: false, session, frames: {} });
  load(again.api, false);
  const { media } = await popupAsk(again.listeners, { type: "media", tab: 9 });
  assert.deepEqual(
    media.map((m) => m.url),
    ["https://files.test/kept.mp4"],
  );
  // The popup opened in a tab of its own (popup.html?page=) is still the popup.
  const state = await new Promise((r) =>
    again.listeners.message({ type: "media", tab: 9 }, { id: "ours", url: "chrome-extension://ours/popup.html?page=x", tab: { id: 4 } }, r),
  );
  assert.equal(JSON.parse(JSON.stringify(state)).media.length, 1);
  ok("media list: kept across the script's restarts; the popup in a tab");
}

// #171: a manifest whose address and type don't say so (text/plain, octet-stream, no .m3u8 or
// .mpd) is noted, never read on sight, and read (its first 4 KB) only when a stream is asked
// for: the popup's list for a frame whose player plays from a blob:, the button, the menu.
{
  const PL = "https://cdn.test/p/playlist?id=7";
  const PL_720 = "https://cdn.test/p/playlist?id=7&v=720";
  const DASH = "https://cdn.test/q/manifest?id=7";
  const CONFIG = "https://cdn.test/p/config?id=7";
  const FEED = "https://cdn.test/p/feed";
  const LONG = "https://cdn.test/p/log";
  served = {
    [PL]: { body: "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1280000,RESOLUTION=1280x720\nplaylist?id=7&v=720\n" },
    [PL_720]: { body: "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4.0,\nseg?i=0\n#EXT-X-ENDLIST\n" },
    // With a byte order mark and a comment before the MPD.
    [DASH]: { body: `﻿<?xml version="1.0"?>\n<!-- made by a packager -->\n${MPD(false).replace(/^<\?xml[^>]*\?>\n/, "")}` },
    [CONFIG]: { body: '{"autoplay":true}' },
    [FEED]: { body: '<?xml version="1.0"?><rss><channel><title>MPD news</title></channel></rss>' },
    // Text going on and on: only its start is read.
    [LONG]: { body: "x".repeat(200_000) },
  };
  const blob = { 0: { href: "https://watch.test/v/3", players: [{ src: "blob:https://watch.test/a", w: 0, h: 0, duration: 0, drm: false }] } };
  const tabs = { 9: { id: 9, url: "https://watch.test/v/3", title: "Clip", incognito: false } };
  const { api, calls, listeners } = fakeApi({ firefox: false, frames: blob, tabs });
  load(api, false);
  listeners.tabUpdated(9, { url: "https://watch.test/v/3" });
  await settle();
  fetched.length = 0;
  for (const d of [
    res(PL, "xmlhttprequest", H("text/plain; charset=utf-8")),
    res(PL_720, "xmlhttprequest", H("text/plain")),
    res("https://cdn.test/p/seg?i=0", "xmlhttprequest", H("video/mp2t")),
    res(DASH, "xmlhttprequest", H("application/octet-stream")),
    res(CONFIG, "xmlhttprequest", H("text/plain")),
    res(FEED, "other", H("application/xml")),
    res(LONG, "xmlhttprequest", []),
    // Not possible manifests: too large, a piece, a media element's own file, a page, JSON.
    res("https://cdn.test/p/huge", "xmlhttprequest", H("text/plain", { "Content-Length": String(3 << 20) })),
    res("https://cdn.test/p/part", "xmlhttprequest", H("application/octet-stream", { "Content-Range": "bytes 0-9/99" }), { statusCode: 206 }),
    res("https://cdn.test/p/el", "media", H("text/plain")),
    res("https://cdn.test/p/page", "xmlhttprequest", H("text/html")),
    res("https://cdn.test/p/api", "xmlhttprequest", H("application/json")),
  ]) {
    listeners.responseStarted(d);
  }
  await settle();
  assert.equal(fetched.length, 0, "nothing read on sight");
  const { media } = await popupAsk(listeners, { type: "media", tab: 9 });
  assert.deepEqual(
    media.map((m) => [m.kind, m.url]),
    [
      ["hls", PL],
      ["dash", DASH],
    ],
    "the manifests become streams (the variant inside its master); the rest stays unlisted",
  );
  assert.deepEqual(media[0].heights, [720]);
  assert.deepEqual([media[1].heights, media[1].duration], [[1080, 480], 90]);
  const sniffed = new Set(fetched.map(([u]) => u));
  for (const u of [PL, PL_720, DASH, CONFIG, FEED, LONG]) assert.ok(sniffed.has(u), `read: ${u}`);
  for (const u of ["https://cdn.test/p/huge", "https://cdn.test/p/part", "https://cdn.test/p/el", "https://cdn.test/p/page", "https://cdn.test/p/api"]) {
    assert.ok(!sniffed.has(u), `never read: ${u}`);
  }
  // 4 KB, and the stream's own read-ahead of a chunk or two.
  assert.ok(served[LONG].read <= 6 * 1024, `only the start read (${served[LONG].read} of 200000 bytes)`);
  assert.ok(fetched.every(([, c]) => c === "include"), "read with the site's cookies");
  const reads = fetched.length;
  await popupAsk(listeners, { type: "media", tab: 9 });
  assert.equal(fetched.length, reads, "each read once");
  ok("#171: manifests without .m3u8/.mpd or their type: noted, read only when asked, listed");
}
{
  // SVT Play (#377): the player first loads .../dash-full.mpd, a JSON answer naming the manifest,
  // then the manifest itself from an address without .mpd. The button must send the manifest.
  const SWITCHER = "https://switcher.test/resolve/1/dash-full.mpd";
  const REAL = "https://api.test/manifest?manifestUrl=x";
  served = { [SWITCHER]: { body: '{"location":"https://cdn.test/dash-full.mpd"}' } };
  const blob = { 0: { href: "https://watch.test/v/5", players: [{ src: "blob:https://watch.test/a", w: 0, h: 0, duration: 0, drm: false }] } };
  const tabs = { 9: { id: 9, url: "https://watch.test/v/5", title: "Clip", incognito: false } };
  const sender = { id: "ours", url: "https://watch.test/v/5", frameId: 0, tab: { id: 9, url: "https://watch.test/v/5" } };
  {
    const { api, calls, listeners } = fakeApi({ firefox: false, frames: blob, tabs });
    load(api, false);
    listeners.tabUpdated(9, { url: "https://watch.test/v/5" });
    await settle();
    fetched.length = 0;
    listeners.responseStarted(res(SWITCHER, "xmlhttprequest", H("application/json")));
    listeners.responseStarted(res(REAL, "xmlhttprequest", H("application/dash+xml")));
    await settle();
    await new Promise((r) => listeners.message({ type: "video", src: "" }, sender, r));
    assert.equal(natives(calls).find((x) => x.video).src, REAL, "the manifest, not the JSON that names it");
    assert.ok(!fetched.some(([u]) => u === SWITCHER), "a real stream known: the other isn't even read");
  }
  {
    // Alone, it is read when a stream is asked for: JSON is no stream, so nothing goes as one.
    const { api, calls, listeners } = fakeApi({ firefox: false, frames: blob, tabs });
    load(api, false);
    listeners.tabUpdated(9, { url: "https://watch.test/v/5" });
    await settle();
    fetched.length = 0;
    listeners.responseStarted(res(SWITCHER, "xmlhttprequest", H("application/json")));
    await settle();
    await new Promise((r) => listeners.message({ type: "video", src: "" }, sender, r));
    const m = natives(calls).find((x) => x.video);
    assert.equal(m.url, "https://watch.test/v/5", "the page goes");
    assert.equal(m.src, undefined, "no stream");
    assert.ok(fetched.some(([u]) => u === SWITCHER), "read before believed");
  }
  {
    // What keeps working: a .m3u8 or .mpd served as its own type, as text or as a file, is a stream
    // on sight; one served as something else is a stream too once read and found to be one.
    const OK_HLS = "https://cdn.test/a/index.m3u8";
    const TXT_MPD = "https://cdn.test/b/stream.mpd";
    const ODD = "https://cdn.test/c/odd.m3u8";
    served = { [ODD]: { body: "#EXTM3U"+String.fromCharCode(10)+"#EXT-X-ENDLIST"+String.fromCharCode(10) } };
    const { api, calls, listeners } = fakeApi({ firefox: false, frames: blob, tabs });
    load(api, false);
    listeners.tabUpdated(9, { url: "https://watch.test/v/5" });
    await settle();
    fetched.length = 0;
    listeners.responseStarted(res(OK_HLS, "xmlhttprequest", H("application/vnd.apple.mpegurl")));
    listeners.responseStarted(res(TXT_MPD, "xmlhttprequest", H("text/plain")));
    await settle();
    assert.equal(fetched.length, 0, "not read to be believed");
    assert.deepEqual((await popupAsk(listeners, { type: "media", tab: 9 })).media.map((m) => m.url).sort(), [OK_HLS, TXT_MPD].sort());
    listeners.tabUpdated(9, { url: "https://watch.test/v/6" });
    await settle();
    listeners.responseStarted(res(ODD, "xmlhttprequest", H("video/mp2t")));
    await settle();
    await new Promise((r) => listeners.message({ type: "video", src: "" }, { ...sender, url: "https://watch.test/v/6", tab: { id: 9, url: "https://watch.test/v/6" } }, r));
    assert.equal(natives(calls).filter((x) => x.video).pop().src, ODD, "read, and it is one");
  }
  ok("#377: a .mpd or .m3u8 address answering JSON is no stream: read first, the real manifest goes");
}
{
  // The popup reads nothing for a frame whose players have files of their own (no MediaSource).
  served = { "https://cdn.test/p/playlist?id=8": { body: "#EXTM3U\n#EXT-X-ENDLIST\n" } };
  const files = { 0: { href: "https://watch.test/v/4", players: [{ src: "https://files.test/movie.mp4", w: 0, h: 0, duration: 0, drm: false }] } };
  const { api, listeners } = fakeApi({ firefox: false, frames: files, tabs: { 9: { id: 9, url: "https://watch.test/v/4" } } });
  load(api, false);
  listeners.tabUpdated(9, { url: "https://watch.test/v/4" });
  await settle();
  fetched.length = 0;
  listeners.responseStarted(res("https://cdn.test/p/playlist?id=8", "xmlhttprequest", H("text/plain")));
  await settle();
  assert.deepEqual((await popupAsk(listeners, { type: "media", tab: 9 })).media, []);
  assert.equal(fetched.length, 0, "no player fed by a MediaSource: nothing read");
  ok("#171: nothing read where no player plays from a blob:");
}
{
  // The button over a player playing from a blob: and the right-click menu: no stream known, the
  // frame's possible manifests are read and the one that is goes; a frame of its own first.
  const PL = "https://emb.test/e/playlist?id=9";
  served = {
    [PL]: { body: "#EXTM3U\n#EXT-X-STREAM-INF:RESOLUTION=640x360\nplaylist?id=9&v=360\n" },
    "https://emb.test/e/settings": { body: "volume=1" },
    "https://blog.test/ads/list": { body: "#EXTM3U\n#EXTINF:5,\nad.ts\n" },
  };
  const { api, calls, listeners } = fakeApi({ firefox: false });
  load(api, false);
  // The page's own frame loaded another playlist (an ad's); the player's frame (2) its stream.
  listeners.responseStarted({ tabId: 7, frameId: 0, url: "https://blog.test/ads/list", type: "xmlhttprequest", statusCode: 200, responseHeaders: H("text/plain") });
  listeners.responseStarted({ tabId: 7, frameId: 2, url: "https://emb.test/e/settings", type: "xmlhttprequest", statusCode: 200, responseHeaders: H("text/plain") });
  listeners.responseStarted({ tabId: 7, frameId: 2, url: PL, type: "xmlhttprequest", statusCode: 200, responseHeaders: H("application/octet-stream") });
  await settle();
  fetched.length = 0;
  await listeners.menu(
    { menuItemId: "mdm-link", srcUrl: "blob:https://emb.test/1", frameId: 2, frameUrl: "https://emb.test/player", pageUrl: "https://blog.test/post" },
    { id: 7, incognito: false },
  );
  await settle();
  const m = natives(calls).find((x) => x.video);
  assert.deepEqual([m.url, m.src], ["https://emb.test/player", PL], "the menu: the frame's sniffed stream");
  assert.ok(!fetched.some(([u]) => u === "https://blog.test/ads/list"), "the frame's own found: the page's frame isn't read");
  assert.ok(calls.some((c) => c[0] === "cookies" && c[1].url === PL), "the stream's cookies");
  // The button in the page's frame (0): none of its own known yet; its frame's read, it goes.
  const sender = { id: "ours", url: "https://blog.test/post", frameId: 0, tab: { id: 7, url: "https://blog.test/post" } };
  await new Promise((r) => listeners.message({ type: "video", src: "" }, sender, r));
  assert.equal(natives(calls).filter((x) => x.video).pop().src, "https://blog.test/ads/list", "the button: its own frame's");
  ok("#171: the button and the menu read the frame's possible manifests when no stream is known");
}

// The button's logo is the extension's own icon, kept in video.js as data.
{
  const video = readText("../../extension/video.js");
  const icon = readFileSync(new URL("../../extension/icons/icon-48.png", import.meta.url)).toString("base64");
  assert.ok(video.includes(`"data:image/png;base64,${icon}"`), "video.js's LOGO is extension/icons/icon-48.png");
  ok("video button logo");
}

// video.js's videoPage, with what it calls (from it to its end mark), run against `location` and
// `document`: the address the button hands the app for a <video>.
const videoPageOn = (() => {
  const source = readText("../../extension/video.js");
  const start = source.indexOf("function videoPage(v) {");
  const end = source.indexOf("// ---- the video's own address: end.");
  assert.ok(start > 0 && end > start, "video.js has videoPage and its end mark");
  const body = `${source.slice(start, end)}; return videoPage;`;
  return (location, document) => new Function("location", "document", body)(location, document);
})();

// The address the button hands the app (video.js's videoPage): the video's own page, not the page
// that plays it. YouTube's home, a search, a channel or a playlist is a list to yt-dlp, and a watch
// page opened from a mix carries the list in its address: the button sent either, and the app
// offered the whole feed (~50 videos) instead of the one playing. #311.
{
  // A page: its address, an optional ytd-watch-flexy video-id, and the cards a player can sit in
  // (each level on the way up from the video element names the video links it contains).
  const at = (href, { flexy = null, cards = [] } = {}) => {
    const element = (links) => ({
      parentElement: null,
      querySelectorAll: () => (links || []).map((href) => ({ href })),
    });
    const chain = cards.map(element);
    for (let i = 0; i < chain.length; i += 1) chain[i].parentElement = chain[i + 1] || null;
    const document = { querySelector: (sel) => (sel === "ytd-watch-flexy" && flexy ? { getAttribute: () => flexy } : null) };
    return videoPageOn({ href }, document)(chain[0] || element([]));
  };
  // A watch page from a mix or a playlist: the video's own address, without the list.
  assert.equal(at("https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=RDdQw4w9WgXcQ&start_radio=1"), "https://www.youtube.com/watch?v=dQw4w9WgXcQ");
  assert.equal(at("https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PL1234567890abcdef&index=12"), "https://www.youtube.com/watch?v=dQw4w9WgXcQ");
  // A player that isn't the page's own: the card it sits in links to the video it is playing.
  assert.equal(at("https://www.youtube.com/", { cards: [[], ["https://www.youtube.com/watch?v=abc123XYZ_-&t=5s"]] }), "https://www.youtube.com/watch?v=abc123XYZ_-");
  // The card links to it twice (thumbnail and title): one video, still.
  assert.equal(at("https://www.youtube.com/results?search_query=rick+astley", { cards: [[], ["https://www.youtube.com/watch?v=abc123XYZ_-", "https://www.youtube.com/watch?v=abc123XYZ_-&pp=1"]] }), "https://www.youtube.com/watch?v=abc123XYZ_-");
  assert.equal(at("https://www.youtube.com/@someone", { cards: [[], [], ["https://www.youtube.com/watch?v=abc123XYZ_-"]] }), "https://www.youtube.com/watch?v=abc123XYZ_-");
  // A page that plays a video and names none: no address at all, so no button and nothing sent
  // (the page's own address — a channel's videos — would be a list of ~50 to yt-dlp).
  assert.equal(at("https://www.youtube.com/@someone/videos"), null);
  assert.equal(at("https://www.youtube.com/@someone/videos", { flexy: "feat12345" }), null);
  assert.equal(at("https://www.youtube.com/"), null);
  // A container naming two videos is the page's column, not a card: nothing is sent, rather than
  // some other card's video.
  assert.equal(at("https://www.youtube.com/@someone/videos", { cards: [[], ["https://www.youtube.com/watch?v=aaa", "https://www.youtube.com/watch?v=bbb"]] }), null);
  // Short links, embeds, and a site that isn't YouTube's (untouched).
  assert.equal(at("https://youtu.be/dQw4w9WgXcQ?t=10&list=PL999"), "https://youtu.be/dQw4w9WgXcQ");
  assert.equal(at("https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ?start=5"), "https://www.youtube.com/watch?v=dQw4w9WgXcQ");
  assert.equal(at("https://vimeo.com/123456789?list=abc"), "https://vimeo.com/123456789?list=abc");
  ok("#311: the button sends the video's own page, not the page playing it");
}

// A page that moves in place (history.pushState): the button sends the address it's at now, not
// the one the document loaded at; the page's own player names the video on YouTube; nothing goes
// when nothing names it (the page would be a list of ~50 to yt-dlp).
{
  const message = (listeners, m, sender) => new Promise((r) => listeners.message(m, sender, (x) => r(JSON.parse(JSON.stringify(x)))));
  const yt = (url, tabUrl = url, frameId = 0) => ({ id: "ours", url, frameId, tab: { id: 9, url: tabUrl, incognito: false } });
  const last = (calls) => natives(calls).filter((m) => m.video).at(-1);
  // Any site: `sender.url` is stale in the top frame, the tab's isn't. A frame keeps its own.
  {
    const { api, calls, listeners } = fakeApi({ firefox: false });
    load(api, false);
    await message(listeners, { type: "video" }, { id: "ours", url: "https://news.test/first", frameId: 0, tab: { id: 2, url: "https://news.test/second" } });
    assert.equal(last(calls).url, "https://news.test/second", "the tab's address, not the stale sender's");
    await message(listeners, { type: "video" }, { id: "ours", url: "https://player.test/embed", frameId: 4, tab: { id: 2, url: "https://news.test/second" } });
    assert.equal(last(calls).url, "https://player.test/embed", "a frame's own address");
    // m. and music. are the same site as www.
    await message(listeners, { type: "video", url: "https://www.mobile.test/watch?v=1" }, { id: "ours", url: "https://m.mobile.test/watch", frameId: 0, tab: { id: 2, url: "https://m.mobile.test/watch" } });
    assert.equal(last(calls).url, "https://www.mobile.test/watch?v=1", "another host of the same site");
  }
  // YouTube: the player's own answer, over the address and whatever the DOM named.
  {
    const { api, calls, listeners } = fakeApi({ firefox: false, playing: "dQw4w9WgXcQ" });
    load(api, false);
    const page = "https://www.youtube.com/@someone";
    assert.equal((await message(listeners, { type: "video", url: null, at: { x: 100, y: 80 } }, yt(page))).ok, true);
    assert.equal(last(calls).url, "https://www.youtube.com/watch?v=dQw4w9WgXcQ", "the channel's player, named by the player");
    assert.equal(last(calls).referrer, page);
    await message(listeners, { type: "video", url: "https://www.youtube.com/watch?v=stale", at: { x: 1, y: 1 } }, yt("https://www.youtube.com/watch?v=stale"));
    assert.equal(last(calls).url, "https://www.youtube.com/watch?v=dQw4w9WgXcQ", "the player over a DOM guess");
  }
  // The player can't be read: what the DOM named goes; with nothing named, nothing goes.
  {
    const { api, calls, listeners } = fakeApi({ firefox: false });
    load(api, false);
    await message(listeners, { type: "video", url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ", at: { x: 1, y: 1 } }, yt("https://www.youtube.com/shorts/dQw4w9WgXcQ"));
    assert.equal(last(calls).url, "https://www.youtube.com/watch?v=dQw4w9WgXcQ");
    const r = await message(listeners, { type: "video", url: null, at: { x: 1, y: 1 } }, yt("https://www.youtube.com/@someone/videos"));
    assert.deepEqual([r.ok, r.why], [false, "unnamed"]);
    assert.equal(natives(calls).filter((m) => m.video).length, 1, "the channel's list is never sent");
  }
  // The popup, on a site this copy leaves alone, and on a player that streams pieces.
  {
    const excludes = ["*://*.youtube.com/*", "*://*.youtube-nocookie.com/*", "*://youtu.be/*"];
    const { api, listeners } = fakeApi({ firefox: false, videoExcludes: excludes, frames: { 0: { href: "https://www.youtube.com/watch?v=x", players: [{ src: "blob:https://www.youtube.com/1" }] } } });
    api.tabs.get = async (id) => ({ id, url: id === 1 ? "https://www.youtube.com/watch?v=x" : "https://example.test/" });
    load(api, false);
    const popup = { id: "ours", url: api.runtime.getURL("popup.html") };
    const on = await message(listeners, { type: "media", tab: 1 }, popup);
    assert.deepEqual([on.keptOff, on.streamed, on.media], [true, true, []], "the store's copy on YouTube");
    const off = await message(listeners, { type: "media", tab: 2 }, popup);
    assert.deepEqual([off.keptOff, off.streamed], [false, true], "elsewhere it isn't kept off");
  }
  // A page's short sound isn't listed as its media (a click, a voice-search chime), a real file is.
  {
    const { api, listeners } = fakeApi({ firefox: false });
    load(api, false);
    const sound = (url, length) => listeners.responseStarted({ tabId: 3, frameId: 0, type: "media", url, statusCode: 200, responseHeaders: [{ name: "content-type", value: "audio/mpeg" }, { name: "content-length", value: String(length) }] });
    sound("https://x.test/open.mp3", 6000);
    sound("https://x.test/song.mp3", 5 << 20);
    await settle();
    const popup = { id: "ours", url: api.runtime.getURL("popup.html") };
    const list = await message(listeners, { type: "media", tab: 3 }, popup);
    assert.deepEqual(list.media.map((m) => m.url), ["https://x.test/song.mp3"]);
  }
  ok("in-place navigation and YouTube's player: the video playing now, or nothing");
}

// #1271: the player the background asks on YouTube (playingAt, run in the page) is the one at the
// point video.js sends. YouTube's main player under its home page has no box (the window's
// corner, 0,0) and still names the video watched last; a hover preview it stopped has none either
// (hidden in a container of no size). Neither is at any point, the corner included: the preview
// playing there is, and with nothing there the address video.js named (the card's link) goes.
{
  const source = code.slice(code.indexOf("function playingAt("), code.indexOf("const isYouTube"));
  const page = (...videos) => new Function("document", `${source}
return playingAt;`)({ querySelectorAll: () => videos });
  const video = (id, left, top, width, height) => ({
    getBoundingClientRect: () => ({ left, top, right: left + width, bottom: top + height, width, height }),
    closest: () => ({ getVideoData: () => ({ video_id: id }) }),
  });
  const main = video("watchedLast", 0, 0, 0, 0);
  const stopped = page(main, video("thePreview1", 0, 0, 0, 0));
  assert.equal(stopped(0, 0), "", "the corner: no player there");
  assert.equal(stopped(660, 239), "", "where the stopped preview was");
  const playing = page(main, video("thePreview1", 476, 135, 369, 208));
  assert.equal(playing(660, 239), "thePreview1", "the preview playing there");
  assert.equal(playing(0, 0), "");
  ok("#1271: YouTube's player at a point: one with no box is at none");
}

// A small DOM for the sample pages (tools/extension/feeds): elements with their attributes, open
// shadow roots (<template shadowrootmode>), `a.href`, and querySelector(All) with the simple
// selectors video.js uses (a tag, #id, [attr], [attr="v"], [attr*="v"], [attr^="v"], lists).
function samplePage(html) {
  const VOID = new Set(["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr"]);
  const ENTITY = { amp: "&", lt: "<", gt: ">", quot: '"', "#39": "'" };
  const decode = (s) => s.replace(/&(amp|lt|gt|quot|#39);/g, (_, e) => ENTITY[e]);
  let base = "about:blank";
  // Descendants in document order, not into shadow roots (as the DOM's own).
  const descendants = function* (n) {
    for (const c of n.children) {
      yield c;
      yield* descendants(c);
    }
  };
  const compound = (sel) => {
    const m = /^([a-z][\w-]*)?(?:#([\w-]+))?((?:\[[^\]]+\])*)$/i.exec(sel.trim());
    assert.ok(m, `a selector the sample DOM reads: ${sel}`);
    const attrs = [...m[3].matchAll(/\[([\w-]+)(?:([*^]?=)"([^"]*)")?\]/g)].map((a) => ({ name: a[1], op: a[2], value: a[3] }));
    return (el) =>
      (!m[1] || el.localName === m[1].toLowerCase()) &&
      (!m[2] || el.getAttribute("id") === m[2]) &&
      attrs.every(({ name, op, value }) => {
        const v = el.getAttribute(name);
        if (v === null) return false;
        return !op || (op === "=" ? v === value : op === "*=" ? v.includes(value) : v.startsWith(value));
      });
  };
  const finder = {
    querySelectorAll(sel) {
      const any = sel.split(",").map(compound);
      return [...descendants(this)].filter((el) => any.some((f) => f(el)));
    },
    querySelector(sel) {
      return this.querySelectorAll(sel)[0] || null;
    },
  };
  class Element {
    constructor(name, attributes, parent) {
      this.localName = name;
      this.attributes = attributes;
      this.children = [];
      this.parentNode = parent;
      this.shadowRoot = null;
    }
    get parentElement() {
      return this.parentNode instanceof Element ? this.parentNode : null;
    }
    getAttribute(name) {
      const a = this.attributes.find((x) => x.name === name);
      return a ? a.value : null;
    }
    get href() {
      return new URL(this.getAttribute("href"), base).href;
    }
  }
  Object.assign(Element.prototype, finder);
  const doc = { children: [], localName: "#document", ...finder };
  const stack = [doc];
  const tags = /<!--[\s\S]*?-->|<!doctype[^>]*>|<\/([\w-]+)\s*>|<([\w-]+)((?:\s+[^\s=/>]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s>]+))?)*)\s*\/?>/gi;
  for (const t of html.matchAll(tags)) {
    if (t[1]) {
      const name = t[1].toLowerCase();
      const at = stack.findLastIndex((n) => n.localName === name || (name === "template" && n.isShadow));
      if (at > 0) stack.length = at;
      continue;
    }
    if (!t[2]) continue;
    const name = t[2].toLowerCase();
    const attributes = [...(t[3] || "").matchAll(/([^\s=/>]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g)].map((a) => ({
      name: a[1].toLowerCase(),
      value: decode(a[2] ?? a[3] ?? a[4] ?? ""),
    }));
    const parent = stack[stack.length - 1];
    // A declarative shadow root: what follows, up to its </template>, is the host's shadow tree.
    if (name === "template" && attributes.some((a) => a.name === "shadowrootmode" && a.value === "open")) {
      const root = { children: [], localName: "#shadow-root", isShadow: true, host: parent, ...finder };
      parent.shadowRoot = root;
      stack.push(root);
      continue;
    }
    const el = new Element(name, attributes, parent);
    parent.children.push(el);
    if (!VOID.has(name)) stack.push(el);
  }
  // Every element, into shadow roots too (the button finds the videos there).
  const all = function* (n) {
    for (const c of n.children) {
      yield c;
      if (c.shadowRoot) yield* all(c.shadowRoot);
      yield* all(c);
    }
  };
  const page = doc.querySelector('meta[name="mdm-page"]').getAttribute("content");
  base = page;
  return { document: doc, page, videos: [...all(doc)].filter((e) => e.localName === "video") };
}

// #689, #643: a video in a feed names the post it sits in, never the feed. Each sample page (our
// own, reduced to what names a post on each site; tools/extension/feeds) says what each of its
// videos must name: an address ({origin}: the page's own), "page" (the page is the video's) or
// "none" (nothing names it: the feed is not sent; its own plain file is, when it has one).
{
  const dir = new URL("feeds/", import.meta.url);
  const pages = readdirSync(dir).filter((f) => f.endsWith(".html"));
  assert.ok(pages.length >= 12, "the sample pages");
  // Each page at its own address, and at the big sites' other hosts too.
  const OTHER = {
    "x.html": ["https://twitter.com/home", "https://mobile.x.com/home"],
    "reddit.html": ["https://sh.reddit.com/"],
    "linkedin.html": ["https://linkedin.com/feed/foryou/"],
    "facebook.html": ["https://web.facebook.com/", "https://m.facebook.com/"],
  };
  let checked = 0;
  for (const file of pages) {
    const { document, page, videos } = samplePage(readText(new URL(file, dir)));
    assert.ok(videos.length, `${file}: its videos`);
    for (const at of [page, ...(OTHER[file] || [])]) {
      const location = new URL(at);
      const videoPage = videoPageOn(location, document);
      for (const [i, v] of videos.entries()) {
        const expect = v.getAttribute("data-expect");
        const want = expect === "page" ? location.href : expect === "none" ? null : expect.replaceAll("{origin}", location.origin);
        const got = videoPage(v);
        assert.equal(got, want, `${file} at ${at}: video ${i + 1}`);
        // Never the page itself, unless it is the video's own.
        if (expect !== "page") assert.notEqual(got, location.href, `${file}: video ${i + 1} named the page`);
        checked += 1;
      }
    }
  }
  ok(`#689: a video in a feed names its post, never the feed (${pages.length} sample pages, ${checked} videos)`);
}

// #1458: Kwai's video page (our own sample of its player's shape: no real account or video) plays
// its video's plain MP4 in a <video src>, a signed `?tag=` after it, the next one ready below it.
// Each names the page (which yt-dlp can't read: it has no Kwai reader), and its own file goes with
// it as the click's download (video.js lookOf: a plain file, not a stream): MDM downloads that
// file, the page its Referer, rather than looking the file up as a page.
{
  const file = (n) => `https://aws-br-cdn.kwai.net/upic/2026/11/18/20/BMjAyNjExMTgy_b_Bsample${n}.mp4?tag=1-1795051555-s-0-sample${n}-8830deb705ea7edb`;
  const html = `<!doctype html><html><head><meta name="mdm-page" content="https://www.kwai.com/@sample/video/5250000000000000001"><title>kwai-nuxt-pwa-pc</title></head><body>
    <div class="video-swiper"><div class="video-content"><div class="video-box"><a-video-player class="container video-detail-player">
      <div class="video-tag _kwai-player_uxc1a_2"><div class="_kwai-player-video_uxc1a_11"><video src="${file(1)}"></video></div></div>
    </a-video-player></div><div class="video-box"><a-video-player class="container video-detail-player">
      <div class="video-tag _kwai-player_uxc1a_2"><div class="_kwai-player-video_uxc1a_11"><video src="${file(2)}"></video></div></div>
    </a-video-player></div></div></div></body></html>`;
  const { document, page, videos } = samplePage(html);
  const location = new URL(page);
  assert.equal(videos.length, 2);
  for (const v of videos) assert.equal(videoPageOn(location, document)(v), page, "the page names it");
  const source = readText("../../extension/video.js");
  const scripts = JSON.parse(/const SCRIPT_EXTENSIONS = (\[[^\]]+\]);/.exec(source)[1]);
  const at = source.indexOf("  function fileType(");
  const fileType = new Function(`const SCRIPT_EXTENSIONS = ${JSON.stringify(scripts)};\n${source.slice(at, source.indexOf("\n  }\n", at) + 4)}\nreturn fileType;`)();
  assert.equal(fileType(file(1)), "mp4", "its own file, an MP4");
  const stream = /const STREAM = \/(.+)\/i;/.exec(source)[1];
  assert.ok(!new RegExp(stream, "i").test(file(1)), "not a stream: no look-up, the file itself");
  // Kwai isn't one of the sites whose reader takes a post over its file (describe: `post`).
  const sites = source.indexOf("const SITES = [");
  assert.ok(sites > 0 && !/kwai/i.test(source.slice(sites, source.indexOf("\n  ];", sites))), "no Kwai reader");
  ok("#1458: Kwai's player: its page named, its own MP4 the click's download");
}

// What the resolvers build is a form yt-dlp's extractors read (their _VALID_URL, yt-dlp
// 2026.08.19), so the app's lookup finds the one video.
{
  const YTDLP = [
    /^https?:\/\/(?:www\.)?linkedin\.com\/feed\/update\/urn:li:activity:(\d+)/,
    /^https?:\/\/(?:[\w-]+\.)?facebook\.com\/(?:(?:permalink\.php|video\.php|watch\/?)\?(?:.*?)(?:v|story_fbid)=|[^/]+\/videos\/(?:[^/]+\/)?|[^/]+\/posts\/|groups\/[^/]+\/(?:permalink|posts)\/|reel\/)(pfbid[A-Za-z0-9]+|\d+)/,
    /^https?:\/\/(?:(?:www|m(?:obile)?)\.)?(?:twitter|x)\.com\/(?:(?:i\/web|[^/]+)\/status|statuses)\/(\d+)/,
    /^https?:\/\/(?:www\.)?instagram\.com(?:\/[^/?#]+)?\/(?:p|tv|reels?)\/([^/?#&]+)/,
    /^https?:\/\/(?:\w+\.)?reddit\.com\/(?:(?:r|user)\/[^/]+\/)?comments\/([^/?#&]+)/,
    /^https?:\/\/www\.tiktok\.com\/@[\w.-]+\/video\/(\d+)/,
    /^https?:\/\/vimeo\.com\/(\d+)(?:\/[\da-f]{10})?$/,
    /^https?:\/\/www\.dailymotion\.com\/video\/([^/?_&#]+)/,
    /^https?:\/\/(?:www\.)?douyin\.com\/video\/([0-9]+)/,
  ];
  const dir = new URL("feeds/", import.meta.url);
  let checked = 0;
  for (const file of readdirSync(dir).filter((f) => f.endsWith(".html") && !/^(social|article)\.html$/.test(f))) {
    const { page, videos } = samplePage(readText(new URL(file, dir)));
    for (const v of videos) {
      const expect = v.getAttribute("data-expect").replaceAll("{origin}", new URL(page).origin);
      // fb.watch's short links lead to the video's page; a LinkedIn post known by its ugcPost
      // URN is its public page, which yt-dlp's generic reader reads.
      if (expect === "none" || expect === "page" || expect.startsWith("https://fb.watch/") || /urn:li:ugcPost:/.test(expect)) continue;
      assert.ok(YTDLP.some((r) => r.test(expect)), `${file}: ${expect} is a form yt-dlp reads`);
      checked += 1;
    }
  }
  ok(`#689: the ${checked} addresses built are forms yt-dlp's extractors read`);
}

// #689: what the background does with what video.js named on a feed. A post: it goes alone (no
// stream the tab loaded, which may be another video's, nor the feed's title), the feed as its
// Referer. Nothing named: never the feed; the video's own plain file when it has one (the feed
// as its Referer), else nothing and the button says to open the post.
{
  const message = (listeners, m, sender) => new Promise((r) => listeners.message(m, sender, (x) => r(JSON.parse(JSON.stringify(x)))));
  const at = (url) => ({ id: "ours", url, frameId: 0, tab: { id: 4, url, incognito: false } });
  const videos = (calls) => natives(calls).filter((m) => m.video);
  const { api, calls, listeners } = fakeApi({ firefox: false });
  load(api, false);
  const feed = "https://www.linkedin.com/feed/";
  // A stream the feed loaded (another video's, as far as anyone knows).
  listeners.responseStarted({ tabId: 4, frameId: 0, url: "https://cdn.test/feed/master.m3u8", responseHeaders: [] });
  const post = "https://www.linkedin.com/feed/update/urn:li:activity:7390000000000000001/";
  assert.equal((await message(listeners, { type: "video", url: post, src: "", title: "Feed | LinkedIn" }, at(feed))).ok, true);
  assert.deepEqual(
    (({ url, src, title, referrer }) => ({ url, src, title, referrer }))(videos(calls).at(-1)),
    { url: post, src: undefined, title: "", referrer: feed },
    "the post alone, the feed as its Referer",
  );
  // Nothing names it, no file of its own: nothing goes, and the button says why.
  const r = await message(listeners, { type: "video", url: null, src: "", title: "Feed | LinkedIn" }, at(feed));
  assert.deepEqual([r.ok, r.why], [false, "feed"]);
  assert.equal(videos(calls).length, 1, "the feed is never sent");
  // Nothing names it, its own plain file: that file, the feed as its Referer, no title.
  await message(listeners, { type: "video", url: null, src: "https://media.test/clip.mp4", title: "Home" }, at("https://social.test/home"));
  const file = videos(calls).at(-1);
  assert.deepEqual([file.src, file.url, file.title], ["https://media.test/clip.mp4", "https://social.test/home", ""]);
  // Another host of the same site is the site's (Facebook's short links, X's old name).
  await message(listeners, { type: "video", url: "https://fb.watch/sAmPlE7x/", src: "" }, at("https://web.facebook.com/"));
  assert.equal(videos(calls).at(-1).url, "https://fb.watch/sAmPlE7x/");
  await message(listeners, { type: "video", url: "https://x.com/someone/status/1", src: "" }, at("https://twitter.com/home"));
  assert.equal(videos(calls).at(-1).url, "https://x.com/someone/status/1");
  // Another site's address named by a page is not taken: the page's own goes, as before.
  await message(listeners, { type: "video", url: "https://elsewhere.test/v/1", src: "" }, at("https://news.test/story"));
  assert.equal(videos(calls).at(-1).url, "https://news.test/story");
  ok("#689: a post alone, the video's own file, or nothing; never the feed");
}

// Shorts and live streams name their video in the path (video.js videoPage); the page is the
// video's, not a list. Their addresses as yt-dlp reads them (the same video as watch?v=).
{
  const named = (href) => videoPageOn({ href }, { querySelector: () => null })({ parentElement: null, querySelectorAll: () => [] });
  assert.equal(named("https://www.youtube.com/shorts/cxkLg22mWco"), "https://www.youtube.com/watch?v=cxkLg22mWco");
  assert.equal(named("https://www.youtube.com/shorts/cxkLg22mWco?feature=share"), "https://www.youtube.com/watch?v=cxkLg22mWco");
  assert.equal(named("https://m.youtube.com/shorts/cxkLg22mWco"), "https://www.youtube.com/watch?v=cxkLg22mWco");
  assert.equal(named("https://www.youtube.com/live/abc123XYZ_-"), "https://www.youtube.com/watch?v=abc123XYZ_-");
  // The Shorts landing page, before it moves to the first one, names none.
  assert.equal(named("https://www.youtube.com/shorts/"), null);
  ok("video.js: Shorts and live pages name their video");
}

// #758: Google Drive videos played in shared folders or direct file pages.
{
  const testDrive = (href, { parentAttrs = {}, links = [], iframes = [], src = "" } = {}) => {
    const el = {
      localName: "video",
      currentSrc: src,
      parentElement: null,
      getAttribute: () => null,
      querySelectorAll: () => [],
    };
    const parent = {
      localName: "div",
      parentElement: null,
      getAttribute: (k) => parentAttrs[k] || null,
      querySelectorAll: (sel) => {
        if (sel === "a[href]") {
          return links.map((h) => ({
            localName: "a",
            getAttribute: (k) => (k === "href" ? h : null),
            querySelector: () => null,
          }));
        }
        if (sel === "iframe, embed") {
          return iframes.map((s) => ({
            localName: "iframe",
            getAttribute: (k) => (k === "src" ? s : null),
          }));
        }
        if (sel === "[data-doc-id]") {
          return parentAttrs["data-doc-id"]
            ? [{ localName: "div", getAttribute: (k) => (k === "data-doc-id" ? parentAttrs["data-doc-id"] : null) }]
            : [];
        }
        return [];
      },
    };
    el.parentElement = parent;
    const document = { querySelector: () => null, querySelectorAll: () => [el] };
    return videoPageOn(new URL(href), document)(el);
  };

  // Direct file URL returns the file's own page.
  assert.equal(
    testDrive("https://drive.google.com/file/d/1s52ek_4YTDRt_EOkx1FS53u-vJa0c4nu/view"),
    "https://drive.google.com/file/d/1s52ek_4YTDRt_EOkx1FS53u-vJa0c4nu/view",
  );

  // Video in a shared folder with stream URL carrying driveid: picked first.
  assert.equal(
    testDrive("https://drive.google.com/drive/folders/18l2MEarbNGo94E1x0sU2mpfkRmLrVVo3", {
      src: "https://rr1---sn-4g5ednss.googlevideo.com/videoplayback?expire=123&driveid=1s52ek_4YTDRt_EOkx1FS53u-vJa0c4nu",
    }),
    "https://drive.google.com/file/d/1s52ek_4YTDRt_EOkx1FS53u-vJa0c4nu/view",
  );

  // Video in a shared folder with preview dialog holding one link: falls back to dialog's link.
  assert.equal(
    testDrive("https://drive.google.com/drive/folders/18l2MEarbNGo94E1x0sU2mpfkRmLrVVo3", {
      parentAttrs: { role: "dialog" },
      links: ["https://drive.google.com/file/d/1s52ek_4YTDRt_EOkx1FS53u-vJa0c4nu/view"],
    }),
    "https://drive.google.com/file/d/1s52ek_4YTDRt_EOkx1FS53u-vJa0c4nu/view",
  );

  // Video in a shared folder with preview dialog holding one iframe.
  assert.equal(
    testDrive("https://drive.google.com/drive/folders/18l2MEarbNGo94E1x0sU2mpfkRmLrVVo3", {
      parentAttrs: { role: "dialog" },
      iframes: ["https://drive.google.com/file/d/1s52ek_4YTDRt_EOkx1FS53u-vJa0c4nu/preview"],
    }),
    "https://drive.google.com/file/d/1s52ek_4YTDRt_EOkx1FS53u-vJa0c4nu/view",
  );

  // Video in signed-out Google Drive viewer: player container carrying data-doc-id.
  assert.equal(
    testDrive("https://drive.google.com/viewer/main?origin=https://drive.google.com&source=10", {
      parentAttrs: { "data-doc-id": "1s52ek_4YTDRt_EOkx1FS53u-vJa0c4nu" },
    }),
    "https://drive.google.com/file/d/1s52ek_4YTDRt_EOkx1FS53u-vJa0c4nu/view",
  );

  // Video in preview dialog with data-doc-id inside dialog element.
  assert.equal(
    testDrive("https://drive.google.com/drive/folders/18l2MEarbNGo94E1x0sU2mpfkRmLrVVo3", {
      parentAttrs: { role: "dialog", "data-doc-id": "1s52ek_4YTDRt_EOkx1FS53u-vJa0c4nu" },
    }),
    "https://drive.google.com/file/d/1s52ek_4YTDRt_EOkx1FS53u-vJa0c4nu/view",
  );

  // Links outside a preview dialog (in the folder list) are ignored.
  assert.equal(
    testDrive("https://drive.google.com/drive/folders/18l2MEarbNGo94E1x0sU2mpfkRmLrVVo3", {
      parentAttrs: { role: "grid" },
      links: ["https://drive.google.com/file/d/1s52ek_4YTDRt_EOkx1FS53u-vJa0c4nu/view"],
    }),
    null,
  );

  // More than one candidate in preview dialog returns null (maybe, not sure).
  assert.equal(
    testDrive("https://drive.google.com/drive/folders/18l2MEarbNGo94E1x0sU2mpfkRmLrVVo3", {
      parentAttrs: { role: "dialog" },
      links: [
        "https://drive.google.com/file/d/1s52ek_4YTDRt_EOkx1FS53u-vJa0c4nu/view",
        "https://drive.google.com/file/d/2t63fk_5ZUERu_FPlx2GT64v-wKb1d5ov/view",
      ],
    }),
    null,
  );

  // Shared folder without any video file identification returns null (never the folder itself).
  assert.equal(
    testDrive("https://drive.google.com/drive/folders/18l2MEarbNGo94E1x0sU2mpfkRmLrVVo3"),
    null,
  );

  // Test page: a folder list with two files and one playing in the preview dialog.
  const { page: gdrivePage, videos: gdriveVideos } = samplePage(readText(new URL("google-drive.html", import.meta.url)));
  for (const v of gdriveVideos) {
    const expect = v.getAttribute("data-expect");
    assert.equal(videoPageOn(new URL(gdrivePage), v.ownerDocument)(v), expect === "none" ? null : expect);
  }

  ok("#758: Google Drive videos in shared folders name the file, never the folder");
}

// #670: video.js's button against a fake DOM — whether it shows over a video while the page is
// in full screen, with the popup's option off (as installed) or on.
function videoButton({ stored = {}, location, iframes, videos, subframe = false } = {}) {
  const source = readText("../../extension/video.js");
  const handlers = {};
  // The frames asked for, by id: one cancelled since (a tick after a hide) does not run.
  const frames = new Map();
  let frameId = 0;
  let root = null;
  const node = (name = "") => {
    const style = new Map();
    const classes = new Set();
    const el = {
      localName: name,
      children: new Proxy([], {
        get(t, k) {
          if (typeof k === "string" && /^\d+$/.test(k)) return t[k] || (t[k] = node());
          return t[k];
        },
      }),
      parentNode: null,
      dataset: {},
      textContent: "",
      innerHTML: "",
      dir: "",
      title: "",
      src: "",
      offsetWidth: 0,
      offsetHeight: 0,
      activeElement: undefined,
      hidden: false,
      style: {
        setProperty: (k, v) => style.set(k, v),
        getPropertyValue: (k) => style.get(k) || "",
      },
      classList: {
        add: (c) => classes.add(c),
        remove: (c) => classes.delete(c),
        toggle: (c, on) => (on === false ? classes.delete(c) : classes.add(c)),
        contains: (c) => classes.has(c),
      },
      addEventListener(n, f) {
        (el.handlers[n] = el.handlers[n] || []).push(f);
      },
      removeEventListener() {},
      setAttribute() {},
      append(c) {
        el.appendChild(c);
      },
      appendChild(c) {
        c.parentNode = el;
        el.children.push(c);
        return c;
      },
      get lastElementChild() {
        return el.children[el.children.length - 1] || node();
      },
      replaceChildren(...kids) {
        el.children.length = 0;
        for (const k of kids) if (k && typeof k === "object") el.appendChild(k);
      },
      attachShadow: () => node("#shadow-root"),
      querySelector: () => node(),
      querySelectorAll: () => [],
      closest: () => null,
      matches: () => false,
      contains: () => true,
      remove() {
        el.parentNode = null;
      },
      focus() {},
      blur() {},
      getAttribute: () => null,
      getBoundingClientRect: () => ({ left: 0, top: 0, right: 640, bottom: 360, width: 640, height: 360 }),
    };
    el.handlers = {};
    if (name === "mdm-video-button") root = el;
    return el;
  };
  const rect = { left: 0, top: 0, right: 640, bottom: 360, width: 640, height: 360 };
  const video = {
    isConnected: true,
    muted: false,
    loop: false,
    controls: true,
    duration: 60,
    currentSrc: "",
    getBoundingClientRect: () => rect,
  };
  const fs = node("div");
  const documentElement = node("html");
  const doc = {
    title: "A video",
    createElement: (tag) => node(tag),
    getElementsByTagName: (tag) => (tag === "video" ? (videos !== undefined ? videos : [video]) : []),
    querySelector: () => null,
    querySelectorAll: (sel) => {
      if (iframes && sel.includes("iframe")) {
        return iframes.filter((el) => {
          if (sel.includes("drive-viewer-video-iframe") && el.classList.contains("drive-viewer-video-iframe")) return true;
          if (sel.includes("youtube.googleapis.com") && el.src.includes("youtube.googleapis.com")) return true;
          if (sel.includes("driveid=") && el.src.includes("driveid=")) return true;
          if (sel.includes("docid=") && el.src.includes("docid=")) return true;
          return false;
        });
      }
      return [];
    },
    documentElement,
    elementsFromPoint: () => [],
    activeElement: null,
    fullscreenElement: null,
    addEventListener(n, f) {
      (handlers[n] = handlers[n] || []).push(f);
    },
    createTreeWalker: () => ({ nextNode: () => null }),
  };
  // The storage listeners the script keeps, so a change can be fired at them as the browser does.
  const storageListeners = [];
  const api = {
    i18n: { getMessage: () => "" },
    runtime: { id: "ours", sendMessage: async (m) => (m.type === "panel" ? { on: true } : { ok: false }) },
    storage: {
      local: {
        get: async (defaults) => ({ ...defaults, ...stored }),
        set: async () => {},
      },
      onChanged: { addListener: (f) => storageListeners.push(f) },
    },
  };
  const loc = { hostname: "vid.test", href: "https://vid.test/watch", search: "", pathname: "/watch", ...location };
  const win = {
    top: null,
    location: loc,
    document: doc,
  };
  win.top = subframe ? {} : win;
  class HTMLAudioElement {}
  vm.runInNewContext(source, {
    browser: api,
    location: loc,
    window: win,
    document: doc,
    matchMedia: () => ({ matches: false, addEventListener() {} }),
    getComputedStyle: () => ({ visibility: "visible", display: "block", opacity: "1" }),
    requestAnimationFrame: (f) => (frames.set(++frameId, f), frameId),
    cancelAnimationFrame: (id) => frames.delete(id),
    addEventListener: (n, f) => ((handlers[n] = handlers[n] || []).push(f)),
    removeEventListener: () => {},
    setTimeout,
    clearTimeout,
    Promise,
    Date,
    innerHeight: 800,
    innerWidth: 1280,
    NodeFilter: { SHOW_ELEMENT: 1 },
    HTMLAudioElement,
    console: quietConsole,
  });
  // The pointer over the video, up to the button's question being answered: only its look runs
  // (its tick would ask for another frame). Each move lands a pixel to the side, as the script
  // ignores a move at the same spot (#741).
  let moved = 0;
  const move = async () => {
    moved += 1;
    for (const f of handlers.pointermove || []) f({ clientX: 320 + (moved % 2), clientY: 180 });
    assert.ok(frames.size, "the look queued");
    const [id, look] = [...frames].pop();
    frames.delete(id);
    await look();
    await settle();
  };
  return {
    display: () => root.style.getPropertyValue("display"),
    move,
    setFullscreen: (on) => {
      doc.fullscreenElement = on ? fs : null;
    },
    fullscreenchange: async () => {
      for (const f of handlers.fullscreenchange || []) await f();
      await settle();
    },
    // The popup's checkbox changing, as the browser tells every page (#670).
    setting: async (value) => {
      stored.hideVideoFullscreen = value;
      for (const f of storageListeners) f({ hideVideoFullscreen: { newValue: value } }, "local");
      await settle();
    },
    hasFramePointer: () => Boolean((handlers.pointermove || []).some((f) => f.name === "onFrameMove")),
  };
}
{
  // Off, as installed: the button shows in full screen once the pointer moves, and entering full
  // screen leaves it out of the way until then.
  const b = videoButton();
  b.setFullscreen(true);
  await b.move();
  assert.equal(b.display(), "block", "off: the button shows in full screen");
  b.setFullscreen(false);
  await b.fullscreenchange();
  await b.move();
  assert.equal(b.display(), "block", "off, not fullscreen: the button shows");
  b.setFullscreen(true);
  await b.fullscreenchange();
  assert.equal(b.display(), "none", "off: entering full screen hides it");
  await b.move();
  assert.equal(b.display(), "block", "off: the pointer's next move brings it back");
  ok("#670: with the option off, the button shows in full screen once the pointer moves");
}
{
  // On: it never shows while the page is in full screen: entering it hides a placed button,
  // leaving it shows the button again.
  const b = videoButton({ stored: { hideVideoFullscreen: true } });
  await b.move();
  assert.equal(b.display(), "block", "on, not fullscreen: the button shows");
  b.setFullscreen(true);
  await b.fullscreenchange();
  assert.equal(b.display(), "none", "on: entering full screen hides it");
  await b.move();
  assert.equal(b.display(), "none", "on: it stays hidden while fullscreen");
  b.setFullscreen(false);
  await b.fullscreenchange();
  assert.equal(b.display(), "block", "on: leaving full screen shows it again");
  ok("#670: with the option on, the button hides in full screen");
}
{
  // The setting changed while the video was already in full screen: it hides at once, and comes
  // back at once when turned off again, without waiting for the page to leave full screen.
  const b = videoButton();
  b.setFullscreen(true);
  await b.move();
  assert.equal(b.display(), "block", "off in full screen: the button shows");
  await b.setting(true);
  assert.equal(b.display(), "none", "turned on in full screen: the button hides at once");
  await b.setting(false);
  await b.move();
  assert.equal(b.display(), "block", "turned off in full screen: the button comes back");
  ok("#670: turning the option on and off while in full screen");
}
{
  // #758: on docs.google.com / drive.google.com, non-video preview frames (Docs, Sheets) must not get the button.
  const docFrame = {
    localName: "iframe",
    id: "doc-preview",
    src: "https://docs.google.com/document/d/1s52ek_4YTDRt_EOkx1FS53u-vJa0c4nu/preview",
    classList: { contains: () => false },
    isConnected: true,
    muted: false,
    loop: false,
    controls: false,
    duration: 0,
    getBoundingClientRect: () => ({ left: 0, top: 0, right: 640, bottom: 360, width: 640, height: 360 }),
  };
  const bDoc = videoButton({
    location: { hostname: "docs.google.com", href: "https://docs.google.com/document/d/1s52ek_4YTDRt_EOkx1FS53u-vJa0c4nu/preview" },
    videos: [],
    iframes: [docFrame],
  });
  await bDoc.move();
  assert.notEqual(bDoc.display(), "block", "a non-video preview frame on docs.google.com does not get the button");

  // A Google Drive video player frame gets the button.
  const videoFrame = {
    localName: "iframe",
    src: "https://youtube.googleapis.com/embed/abc?driveid=1s52ek_4YTDRt_EOkx1FS53u-vJa0c4nu",
    classList: { contains: (c) => c === "drive-viewer-video-iframe" },
    isConnected: true,
    muted: false,
    loop: false,
    controls: false,
    duration: 0,
    getBoundingClientRect: () => ({ left: 0, top: 0, right: 640, bottom: 360, width: 640, height: 360 }),
  };
  const bVideo = videoButton({
    location: { hostname: "drive.google.com", href: "https://drive.google.com/drive/folders/18l2MEarbNGo94E1x0sU2mpfkRmLrVVo3" },
    videos: [],
    iframes: [videoFrame],
  });
  await bVideo.move();
  assert.equal(bVideo.display(), "block", "the video player frame on drive.google.com gets the button");
  ok("#758: non-video preview frame never gets the button, video player frame does");

  // #758: only the player's own subframe sets isDriveFrame, never docs/sheets subframes
  const playerYt = videoButton({
    location: { hostname: "youtube.googleapis.com", href: "https://youtube.googleapis.com/embed/abc?driveid=1s52ek", search: "?driveid=1s52ek", pathname: "/embed/abc" },
    subframe: true,
  });
  assert.equal(playerYt.hasFramePointer(), true, "youtube embed with driveid registers frame pointer");

  const playerYtDoc = videoButton({
    location: { hostname: "youtube.googleapis.com", href: "https://youtube.googleapis.com/embed/abc?docid=1s52ek", search: "?docid=1s52ek", pathname: "/embed/abc" },
    subframe: true,
  });
  assert.equal(playerYtDoc.hasFramePointer(), true, "youtube embed with docid registers frame pointer");

  const playerPreview = videoButton({
    location: { hostname: "drive.google.com", href: "https://drive.google.com/file/d/1s52ek/preview", search: "", pathname: "/file/d/1s52ek/preview" },
    subframe: true,
  });
  assert.equal(playerPreview.hasFramePointer(), true, "Drive /file/d/.../preview registers frame pointer");

  const docsSubframe = videoButton({
    location: { hostname: "docs.google.com", href: "https://docs.google.com/document/d/1s52ek/edit", search: "", pathname: "/document/d/1s52ek/edit" },
    subframe: true,
  });
  assert.equal(docsSubframe.hasFramePointer(), false, "Docs subframe does not register frame pointer");

  const sheetsSubframe = videoButton({
    location: { hostname: "docs.google.com", href: "https://docs.google.com/spreadsheets/d/1s52ek/edit", search: "", pathname: "/spreadsheets/d/1s52ek/edit" },
    subframe: true,
  });
  assert.equal(sheetsSubframe.hasFramePointer(), false, "Sheets subframe does not register frame pointer");

  const folderSubframe = videoButton({
    location: { hostname: "drive.google.com", href: "https://drive.google.com/drive/folders/18l2MEarb", search: "", pathname: "/drive/folders/18l2MEarb" },
    subframe: true,
  });
  assert.equal(folderSubframe.hasFramePointer(), false, "Folder subframe does not register frame pointer");
  ok("#758: pointer bridge only triggers on player subframe, not Docs or Sheets");
}

// The qualities' menu (video.js; its plain functions run here): the questions' pace and their
// minute, a row's text, and what a page can never make it do.
{
  const source = readText("../../extension/video.js");
  // A function's own source (async with its keyword), by its braces.
  const fn = (name) => {
    let start = source.indexOf(`function ${name}(`);
    assert.ok(start > 0, `video.js has ${name}`);
    if (source.slice(start - 6, start) === "async ") start -= 6;
    let depth = 0;
    // The body's brace: after the parameters (which may destructure).
    for (let i = source.indexOf(") {", start) + 2; i < source.length; i += 1) {
      if (source[i] === "{") depth += 1;
      if (source[i] === "}" && --depth === 0) return source.slice(start, i + 1);
    }
    throw new Error(`${name}: no end`);
  };
  const make = (names, value) => new Function(`${names.map(fn).join("\n")}\nreturn ${value};`)();
  // 100 ms before the first question, then the old backoff's target (1 s x 1.25^n, 2 s at most)
  // less what the last answer took (50 ms at least): an older app (60 ms) is asked as seldom as
  // before, a newer one (holding the answer ~1.5 s) is asked again almost at once early on.
  const pollDelay = make(["pollDelay"], "pollDelay");
  assert.equal(pollDelay(0, 0), 100);
  assert.equal(pollDelay(0, 5000), 100);
  assert.equal(pollDelay(1, 60), 1190);
  assert.equal(pollDelay(1, 1500), 50);
  assert.equal(pollDelay(2, 1500), 50);
  assert.equal(pollDelay(2, 999), 564);
  assert.equal(pollDelay(9, 0), 2000);
  assert.equal(pollDelay(9, 1500), 50);
  assert.equal(pollDelay(9, 99999), 50);
  assert.equal(pollDelay(3, undefined), 1953);
  assert.equal(pollDelay(3, NaN), 1953);
  assert.equal(pollDelay(3, -20), 1953);
  // The qualities in tabs: sound-only ones apart when the video has both kinds; else one list. Every
  // number is the place in the app's list.
  const groupChoices = make(["groupChoices"], "groupChoices");
  const grp = (list, max) => JSON.parse(JSON.stringify(groupChoices(list, max)));
  const mixed = [{ audio: false }, { audio: false }, { audio: true }, { audio: false }, { audio: true }];
  assert.deepEqual(grp(mixed), { tabbed: true, video: [0, 1, 3], audio: [2, 4], all: [0, 1, 2, 3, 4], start: "video" });
  assert.deepEqual(grp([{ audio: true }, { audio: false }]), { tabbed: true, video: [1], audio: [0], all: [0, 1], start: "video" });
  assert.deepEqual(grp([{ label: "720p" }, { label: "360p" }]), { tabbed: false, video: [0, 1], audio: [], all: [0, 1], start: "video" }, "an older app: no audio flag, one list");
  assert.equal(grp([{ audio: "yes" }, { audio: 1 }, null, "x"]).tabbed, false, "only true counts as audio");
  assert.deepEqual(grp([{ audio: true }, { audio: true }]), { tabbed: false, video: [], audio: [0, 1], all: [0, 1], start: "audio" }, "only audio: one list");
  assert.deepEqual(grp([{ audio: false }, { audio: false }]), { tabbed: false, video: [0, 1], audio: [], all: [0, 1], start: "video" }, "only video: one list");
  assert.deepEqual(grp([]), { tabbed: false, video: [], audio: [], all: [], start: "video" });
  assert.deepEqual(grp(undefined), { tabbed: false, video: [], audio: [], all: [], start: "video" });
  assert.deepEqual(grp(mixed, 3), { tabbed: true, video: [0, 1], audio: [2], all: [0, 1, 2], start: "video" }, "the cap keeps the app's order");
  // A row: the app's name for the quality as it is (set as text), its size in the browser's language.
  const { sizeText, rowText } = make(["sizeText", "choiceSize", "rowText"], "{ sizeText, rowText }");
  assert.equal(sizeText(187500000, "en"), "178.8 MB");
  assert.equal(sizeText(3 * 2 ** 30, "en"), "3 GB");
  assert.equal(sizeText(null, "en"), "");
  assert.equal(sizeText(undefined, "en"), "");
  assert.deepEqual({ ...rowText({ label: "<img src=x onerror=alert(1)> 720p", ext: "mp4", size: null }, "en") }, { name: "<img src=x onerror=alert(1)> 720p", size: "" });
  assert.deepEqual({ ...rowText(null, "en") }, { name: "", size: "" });
  // Following a lookup: the next question only after the last answer, on a clock of its own here.
  const followLookup = make(["pollDelay", "followLookup"], "followLookup");
  const follow = async (first, answers, { alive = () => true, took = 0 } = {}) => {
    let clock = 0;
    let open = 0;
    const asked = [];
    const result = await followLookup(first, {
      ask: async (token, ms) => {
        assert.equal(open++, 0, "one question at a time");
        asked.push({ token, at: clock, ms });
        await Promise.resolve();
        clock += took;
        open -= 1;
        return answers.length ? answers.shift() : { lookup: { state: "pending" } };
      },
      alive: () => alive(asked),
      sleep: async (ms) => {
        clock += ms;
      },
      now: () => clock,
      end: 60000,
    });
    return { result: result && { ...result }, asked, clock };
  };
  const pending = { state: "pending", token: "t1" };
  const ready = { state: "ready", token: "t1", choices: [{ label: "720p MP4", ext: "mp4", size: 1 }] };
  let f = await follow(pending, [{ lookup: pending }, { lookup: ready }]);
  assert.deepEqual(f.result, ready);
  assert.deepEqual(f.asked.map((a) => [a.token, a.at]), [["t1", 100], ["t1", 1350]], "100 ms, then the old backoff (an app that answers at once)");
  // Still looking after a minute: given up then, never asked after it.
  f = await follow(pending, []);
  assert.deepEqual(f.result, { state: "late" });
  assert.equal(f.clock, 60000);
  assert.ok(f.asked.every((a) => a.at < 60000 && a.ms > 0 && a.token === "t1"), "the token from the first answer");
  // An older app (answers in ~60 ms): no more questions than the old schedule (1 s x 1.25^n, 2 s at
  // most: asks at 1 s, 2.25 s, ...) made in a minute.
  let old = 0;
  for (let at = 0, n = 0; (at += Math.min(2000, 1000 * 1.25 ** n)) < 60000; n += 1) old += 1;
  f = await follow(pending, [], { took: 60 });
  assert.deepEqual(f.result, { state: "late" });
  assert.equal(f.asked[0].at, 100);
  assert.ok(f.asked.length <= old, `old host: ${f.asked.length} questions in a minute, the old schedule's ${old}`);
  // A newer app (holds the answer ~1.5 s while it looks): one at a time, asked again almost at once
  // early on, and the answer reaches the page soon after it is ready. A host that is ready at `r`
  // answers its question in flight 100 ms later (else after 1.5 s).
  const latency = async (r) => {
    let clock = 0;
    let done = null;
    let ready = { state: "ready", token: "t1", choices: [] };
    const res = await followLookup(pending, {
      ask: async () => {
        const start = clock;
        const end = Math.min(start + 1500, Math.max(start, r) + 100);
        clock = end;
        return { lookup: end >= r ? ((done = end), ready) : pending };
      },
      alive: () => true,
      sleep: async (ms) => {
        clock += ms;
      },
      now: () => clock,
      end: 60000,
    });
    assert.equal(res.state, "ready");
    return done - r;
  };
  for (const r of [0, 300, 1000, 1700, 2500, 3000]) assert.ok((await latency(r)) <= (r ? 150 : 250), `new host, ready at ${r} ms: answered ${await latency(r)} ms later`);
  for (const r of [5000, 12000, 30000]) assert.ok((await latency(r)) <= 150, `new host, ready at ${r} ms: answered ${await latency(r)} ms later`);
  f = await follow(pending, [], { took: 1500 });
  assert.deepEqual(f.result, { state: "late" });
  assert.ok(f.asked.length > 20 && f.asked.length <= 40, `new host: ${f.asked.length} questions in a minute`);
  // In between (an answer after 600 ms): the rest of the second.
  f = await follow(pending, [{ lookup: pending }, { lookup: pending }, { lookup: ready }], { took: 600 });
  assert.deepEqual(f.asked.map((a) => a.at), [100, 1350, 2913]);
  assert.deepEqual(f.result, ready);
  // Dropped (the menu closed, the video or the page changed): nothing more, no answer shown.
  f = await follow(pending, [{ lookup: pending }, { lookup: ready }], { alive: (asked) => asked.length < 1 });
  assert.deepEqual([f.result, f.asked.length], [null, 1]);
  // The background's other answers.
  assert.deepEqual((await follow(pending, [{ late: true }])).result, { state: "late" });
  assert.deepEqual((await follow(pending, [{ fallback: true }])).result, { state: "use-window", token: "t1" }, "another MDM chosen meanwhile");
  assert.deepEqual((await follow(pending, [{ ok: false }])).result, { state: "gone" });
  assert.equal((await follow(pending, [{ ok: false, gone: true }])).result, null, "the extension gone");
  f = await follow({ state: "pending" }, []);
  assert.deepEqual([f.result, f.asked.length], [{ state: "gone" }, 0], "no token, no questions");
  assert.deepEqual((await follow({ state: "use-window" }, [])).result, { state: "use-window", token: "" });
  // What each answer comes to (replyState, then lookupAction): the background's answer to the
  // click, the lookup's end, and the answer to a choice.
  const { replyState, lookupAction, firstWindow, pointerTarget } = make(
    ["replyState", "lookupAction", "firstWindow", "pointerTarget"],
    "{ replyState, lookupAction, firstWindow, pointerTarget }",
  );
  const key = (k) => k;
  const act = (r, picked) => {
    const s = replyState(r);
    return s && { ...lookupAction(s, key, picked) };
  };
  const look = (lookup) => act({ lookup });
  assert.deepEqual(look(pending), { act: "follow" });
  assert.deepEqual(look(ready), { act: "offer" });
  assert.deepEqual(look({ state: "ready", token: "t1", choices: [] }), { act: "say", text: "video_no_choices" });
  assert.deepEqual(look({ state: "failed", reason: "no-choices" }), { act: "say", text: "video_no_choices" });
  assert.deepEqual(look({ state: "failed", reason: "no-ytdlp" }), { act: "say", text: "video_no_ytdlp" });
  assert.deepEqual(look({ state: "failed", reason: "error", message: "HTTP 403" }), { act: "say", text: "video_failed_lookup", detail: "HTTP 403" });
  assert.deepEqual(look({ state: "failed", reason: "error", message: { html: "<b>" } }), { act: "say", text: "video_failed_lookup", detail: "" });
  assert.deepEqual(look({ state: "list" }), { act: "say", text: "video_playlist" });
  assert.deepEqual(look({ state: "busy" }), { act: "say", text: "video_busy" });
  assert.deepEqual(look({ state: "late" }), { act: "say", text: "video_too_long" });
  assert.deepEqual(act({ late: true }), { act: "say", text: "video_too_long" }, "no answer within the minute");
  assert.deepEqual(look({ state: "off" }), { act: "flash", kind: "off", text: "video_off" });
  assert.deepEqual(act({ ok: false, why: "unnamed" }), { act: "flash", kind: "warn", text: "video_unnamed" });
  assert.deepEqual(act({ ok: false, why: "feed" }), { act: "flash", kind: "warn", text: "video_open_post" });
  // The click as it always was (the Download video window): no lookups here, the app forgot the
  // lookup, an older or a newer answer, a host that failed.
  const windows = [{ fallback: true }, { ok: false }, { ok: false, why: "not_running" }, { lookup: null }, { lookup: "ready" }, {}, null];
  for (const s of ["use-window", "gone", "a-newer-state", "", 7]) windows.push({ lookup: { state: s } });
  for (const r of windows) assert.deepEqual(act(r), { act: "window" }, JSON.stringify(r));
  assert.equal(act({ ok: false, gone: true }), null, "the extension gone: nothing");
  // A choice: downloading, or handed to the app ("sent", which didn't say in time what became of
  // it: it will download it, so no window), or queued; anything else, the window.
  assert.deepEqual(act({ lookup: { state: "started" } }, true), { act: "flash", kind: "ok", text: "video_added" });
  assert.deepEqual(act({ lookup: { state: "sent" } }, true), { act: "flash", kind: "ok", text: "video_added" });
  assert.deepEqual(act({ lookup: { state: "queued" } }, true), { act: "flash", kind: "ok", text: "video_queued" });
  for (const s of ["use-window", "gone", "pending", "ready", "busy", "off", "failed", "list", "late"]) {
    assert.deepEqual(act({ lookup: { state: s, token: "t1", choices: ready.choices } }, true), { act: "window" }, `a choice answered ${s}`);
  }
  assert.deepEqual(act({ fallback: true }, true), { act: "window" });
  assert.equal(act({ ok: false, gone: true }, true), null);
  // #852: the app's word on what the look-up lacked, shown with the qualities: its text, and a
  // link to MDM's own help page only (never another address a page could slip in).
  const { noticeOf } = make(["noticeOf"], "{ noticeOf }");
  const help = "https://misterdownloadmanager.com/de/help/video.html#javascript-runtime";
  const notice = { text: "YouTube needs a JavaScript runtime", link: "How to install one", url: help };
  assert.deepEqual(noticeOf({ ...ready, notice }), notice);
  assert.deepEqual(noticeOf({ state: "failed", reason: "no-choices", notice }), notice);
  for (const url of ["javascript:alert(1)", "https://misterdownloadmanager.com.evil.test/", "http://misterdownloadmanager.com/help/", 7]) {
    assert.deepEqual(noticeOf({ ...ready, notice: { ...notice, url } }), { text: notice.text, link: "", url: "" }, String(url));
  }
  for (const n of [undefined, null, "words", { text: "" }, { text: 7 }, { link: "x", url: help }]) {
    assert.equal(noticeOf({ ...ready, notice: n }), null, JSON.stringify(n));
  }
  assert.equal(noticeOf(null), null);
  assert.equal(noticeOf({ ...ready, notice: { ...notice, text: "x".repeat(900) } }).text.length, 400);
  assert.ok(fn("fillMenu").includes("tell(noticeOf(look))"), "fillMenu shows it");
  // One window per click, whichever way it comes to it: a lookup's end, then the choice's answer,
  // then "Open in…" (and a second click gets its own).
  const click = {};
  assert.deepEqual([firstWindow(click), firstWindow(click), firstWindow(click)], [true, false, false]);
  assert.equal(firstWindow({}), true, "the next click");
  assert.equal(firstWindow(null), false);
  // …and every way there goes through it: the window's message is sent from useWindow alone,
  // after firstWindow, and each answer is read by lookupAction.
  assert.equal(source.split('send({ type: "video",').length, 2, "one place sends the window's message");
  assert.ok(/if \(!firstWindow\(j\)\) return;[^]*send\(\{ type: "video",/.test(fn("useWindow")), "useWindow: once per click");
  for (const name of ["download", "pick", "fillMenu"]) assert.ok(fn(name).includes("lookupAction("), `${name} reads answers with lookupAction`);
  assert.ok(fn("pick").includes("lookupAction(s, t, true)"), "pick: a choice's answer");
  // While the qualities' menu is open, the pointer over another video (a feed's next one, under
  // the menu) leaves the button on its own; over its own, or with no menu, it follows.
  const mine = { id: "mine" };
  const next = { id: "next" };
  assert.equal(pointerTarget(next, mine, true), null);
  assert.equal(pointerTarget(mine, mine, true), mine);
  assert.equal(pointerTarget(next, mine, false), next);
  assert.equal(pointerTarget(next, null, false), next);
  assert.equal(pointerTarget(null, mine, false), null);
  assert.ok(fn("look").includes("pointerTarget(videoAt(pointer.x, pointer.y), video, menuOpen)"), "look: kept on its video while its menu is open");
  // #1582: where the page is asked what lies under a spot the button would take (onControl).
  // Bilibili's player keeps a feedback button (54 x 24) at its top right, 12 px from its edge, in
  // view while the pointer moves over the player: on a 1550 px wide player the button's default
  // spot (155 x 32, 8 px in) covers its glyph, yet the spot's middle and four corners all miss
  // it. Every point the page is asked at lies inside the spot, and any control of 14 x 14 px or
  // more under any part of it lies under one of them.
  const probes = make(["probes"], "probes");
  const inside = (p, r) => p[0] >= r.left && p[0] <= r.right && p[1] >= r.top && p[1] <= r.bottom;
  const hits = (pts, r) => pts.some((p) => inside(p, r));
  const spotAt = { left: 1387, top: 8, right: 1542, bottom: 40 };
  const feedback = { left: 1484, top: 18, right: 1538, bottom: 42 };
  const fivePoints = [[1464.5, 24], [1391, 12], [1538, 12], [1391, 36], [1538, 36]];
  assert.ok(!fivePoints.some((p) => p[0] > feedback.left && p[0] < feedback.right && p[1] > feedback.top && p[1] < feedback.bottom), "the middle and corners alone miss Bilibili's feedback button (the miss #1582 saw)");
  assert.ok(hits(probes(spotAt), feedback), "the points find Bilibili's feedback button under the button's end");
  for (const box of [spotAt, { left: 8, top: 8, right: 44, bottom: 40 }, { left: 100, top: 300, right: 399, bottom: 334 }, { left: 5, top: 5, right: 5, bottom: 5 }]) {
    const pts = probes(box);
    assert.ok(pts.length >= 6 && pts.length <= 3 * 33, `a bounded number of points (${pts.length})`);
    assert.ok(pts.every((p) => Number.isFinite(p[0]) && Number.isFinite(p[1]) && inside(p, box)), "every point inside the spot");
    for (let x = box.left - 13; x <= box.right; x += 1) {
      for (let y = box.top - 13; y <= box.bottom; y += 1) {
        const c = { left: x, top: y, right: x + 14, bottom: y + 14 };
        // Overlapping the spot by 14 px both ways (a smaller box can't fit inside it): found.
        const over = Math.min(c.right, box.right) - Math.max(c.left, box.left) >= 14 && Math.min(c.bottom, box.bottom) - Math.max(c.top, box.top) >= 14;
        if (over) assert.ok(hits(pts, c), `a 14 px control at ${x},${y} under the spot ${JSON.stringify(box)}`);
      }
    }
  }
  assert.ok(fn("onControl").includes("for (const [x, y] of probes(b))"), "onControl asks at every point of probes");
  // #1271: where the background asks the page's player what the click's video is (the window's
  // message): its middle while it has a box on the screen; a hover preview the page stopped as
  // the pointer went to the button (taken out, or hidden in a container of no size) is at the
  // point the click was taken at, never the window's corner (YouTube's main player there names
  // the video watched last).
  const middle = make(["middle"], "middle");
  const shown = (left, top, width, height, isConnected = true) => ({ isConnected, getBoundingClientRect: () => ({ left, top, width, height }) });
  const was = { x: 660, y: 239 };
  assert.deepEqual(middle(shown(100, 50, 640, 360), was), { x: 420, y: 230 }, "a video on the screen: its middle now");
  assert.deepEqual(middle(shown(0, 0, 0, 0), was), was, "a stopped preview, hidden");
  assert.deepEqual(middle(shown(476, 135, 369, 0), was), was, "no height");
  assert.deepEqual(middle(shown(476, 135, 369, 208, false), was), was, "taken out of the page");
  assert.equal(middle(shown(0, 0, 0, 0), undefined), undefined, "nowhere known: no point");
  assert.ok(fn("useWindow").includes("at: middle(j.video, j.asked.at)"), "useWindow: the point the click was taken at, for a video gone");
  assert.ok(/const d = held \? held\.d : describe\(/.test(fn("download")), "download: a held video's own (#503), its point with it");
  // What a page can never make it do: the menu's rows and keys take only the user's own clicks
  // and keys; rows are text (markup only for the fixed parts); a pick sends the token and the
  // row's number alone; nothing is asked on a timer of its own, nor before a click.
  assert.ok(/shadow\.addEventListener\("keydown", \(e\) => \{\s*if \(!e\.isTrusted\) return;/.test(source), "keys: the user's own");
  assert.ok(/menu\.addEventListener\("click", \(e\) => \{\s*if \(!e\.isTrusted\) return;/.test(source), "rows: the user's own clicks");
  assert.ok(/acts\.addEventListener\("click", async \(e\) => \{\s*if \(!e\.isTrusted\) return;/.test(source), "a status's buttons: the user's own clicks");
  // The page's own controls: a key the open menu used doesn't reach the page, and the button (and
  // so its menu) stays where it is while the menu is open.
  const keys = source.slice(source.indexOf('shadow.addEventListener("keydown"'), source.indexOf("// The main part: a click downloads"));
  assert.equal(keys.match(/e\.stopPropagation\(\)/g).length, 5, "Escape (the menu, the button), Enter and Space, the arrows, ↓ on the button: not the page's too");
  assert.ok(fn("spot").includes("if (menuOpen && auto.v === video && auto.at) return auto.at;"), "spot: still while its menu is open");
  assert.equal(source.match(/innerHTML/g).length, 1, "markup only for the fixed parts");
  assert.ok(!/insertAdjacentHTML|outerHTML|setInterval/.test(source));
  assert.ok(/send\(\{ type: "video-pick", token: l\.token, choice: i \}\)/.test(fn("pick")), "a pick: the token and the row's number");
  assert.equal(source.split('"video-lookup"').length, 2, "one lookup message");
  // #703: asked as the button shows, once it stayed a moment over the same video (previews the
  // pointer only crosses aren't), one per video for the page's life; a plain file never.
  assert.ok(fn("startLook").includes('"video-lookup"'), "sent by startLook alone");
  assert.ok(/if \(lookingFor === v\) return;[^]*setTimeout\(\(\) => \{[^]*if \(video === v && v\.isConnected && pill\.classList\.contains\("shown"\)\) startLook\(describeNow\(v\)\);\s*\}, LOOK_DELAY_MS\)/.test(fn("scheduleLook")), "scheduleLook: after LOOK_DELAY_MS, the same video still shown, not put off by each move");
  assert.ok(fn("startLook").includes("if (usable(kept) && !(byUser && kept.state === \"busy\")) return kept;"), "one look-up per video, kept");
  assert.ok(fn("startLook").includes("!STREAM.test(c.d.src)"), "none for a plain file");
  // #1053: the pointer resting on the button starts the look-up (REST_MS), a pass over it doesn't;
  // it is sent once, and the click (or its menu) shares it rather than asking again. Run here on a
  // clock of the test's own, with startLook, lookOf and usable as video.js has them.
  {
    const constant = (name) => {
      const m = new RegExp(`const ${name} = [^]*?;\\n`).exec(source);
      assert.ok(m, `video.js has ${name}`);
      return m[0];
    };
    assert.ok(/const REST_MS = (\d+);/.test(source) && +/const REST_MS = (\d+);/.exec(source)[1] >= 150 && +/const REST_MS = (\d+);/.exec(source)[1] <= 250, "REST_MS: 150 to 250 ms");
    const page = new Function(
      "send",
      `const LOOKUP_MS = 60000;
      ${constant("LOOK_RETRY_MS")}${constant("STREAM")}${constant("REST_MS")}${constant("usable")}
      const looks = new Map();
      const location = { href: "https://videos.test/watch?v=1" };
      const landedAt = location.href;
      const document = { title: "A talk", querySelector: () => null };
      const current = null;
      const menuOpen = false;
      const render = () => {};
      const fillMenu = () => {};
      const within = (p) => p;
      // #1092: what the tab played, never asked for here.
      const playedLook = async () => null;
      ${["pollDelay", "followLookup", "replyState", "playedWanted", "lookOf", "startLook", "restWait"].map(fn).join("\n")}
      return { REST_MS, looks, usable, lookOf, startLook, restWait };`,
    );
    const asked = [];
    let answer = { lookup: { state: "pending", token: "t1" } };
    const w = page((m) => {
      asked.push(m);
      return Promise.resolve(answer);
    });
    // The page's timers, on the test's clock.
    let now = 0;
    let timers = [];
    let next = 1;
    const set = (f, ms) => {
      timers.push({ id: next, at: now + ms, f });
      return next++;
    };
    const clear = (id) => (timers = timers.filter((x) => x.id !== id));
    const to = (t) => {
      for (;;) {
        const due = timers.filter((x) => x.at <= t).sort((a, b) => a.at - b.at)[0];
        if (!due) break;
        clear(due.id);
        now = due.at;
        due.f();
      }
      now = t;
    };
    const v = { id: "video" };
    const c = { v, d: { url: "https://videos.test/watch?v=1", src: "", at: null, drm: false }, key: "https://videos.test/watch?v=1 " };
    const resting = w.restWait(w.REST_MS, (x) => x === v && w.startLook(c), set, clear);
    // A pass: over the button and off it again before REST_MS.
    resting.rest(v);
    to(w.REST_MS - 50);
    resting.leave();
    to(5000);
    assert.equal(asked.length, 0, "a pass over the button asks nothing");
    // Resting: nothing before REST_MS (the pointer's moves on it don't start the wait over), then
    // one look-up.
    resting.rest(v);
    to(5000 + w.REST_MS - 100);
    resting.rest(v);
    to(5000 + w.REST_MS - 1);
    assert.equal(asked.length, 0, "not before REST_MS");
    to(5000 + w.REST_MS);
    assert.equal(asked.length, 1, "after REST_MS: the look-up");
    assert.equal(asked[0].type, "video-lookup");
    to(60000);
    assert.equal(asked.length, 1, "once");
    // The click, as download() takes it: the rest's look-up, still running, and no second one.
    let l = w.lookOf(c);
    assert.ok(w.usable(l) && l.state === "pending", "the click finds it running");
    if (!w.usable(l) || l.state === "busy") l = w.startLook(c, true);
    assert.equal(l, w.looks.get(c.key));
    assert.equal(w.startLook(c, true), l, "the menu's arrow too");
    // Off the button and back: rested again, the same look-up.
    resting.leave();
    resting.rest(v);
    to(60000 + w.REST_MS);
    assert.equal(asked.length, 1, "one look-up per video");
    assert.ok(/if \(!usable\(l\) \|\| l\.state === "busy"\) l = startLook\(c, true\);/.test(fn("download")), "download: shares the kept look-up");
    assert.ok(fn("openMenu").includes("if (video) startLook(describeNow(video), true);"), "openMenu: shares it too");
    // An early look-up the app was too busy for (other videos' running): kept for the rests, which
    // don't ask again, but the click asks again.
    const c2 = { v, d: { url: "https://videos.test/watch?v=2", src: "", at: null, drm: false }, key: "https://videos.test/watch?v=2 " };
    answer = { lookup: { state: "busy" } };
    const early = w.startLook(c2);
    await early.done;
    assert.equal(early.state, "busy");
    assert.equal(w.startLook(c2), early, "a rest keeps it");
    answer = { lookup: { state: "pending", token: "t2" } };
    const clicked = w.startLook(c2, true);
    assert.notEqual(clicked, early, "the click asks again");
    assert.equal(asked.length, 3);
    // A private window: neither the button showing nor resting on it asks; the click and the menu do.
    assert.ok(source.includes("const PRIVATE = Boolean(ext.extension?.inIncognitoContext);"), "PRIVATE: the browser's word");
    assert.ok(fn("show").includes("if (!PRIVATE && !usable(looks.get(c.key))) scheduleLook(v);"), "show: not in a private window");
    assert.ok(/restWait\(\s*REST_MS,\s*\(v\) => \{\s*if \(!PRIVATE && video === v && v\.isConnected && root\.matches\(":hover"\)\) startLook\(describeNow\(v\)\);/.test(source), "resting: not in a private window, the same video, still on the button");
    assert.ok(/pill\.addEventListener\("pointerenter", \(\) => \{\s*if \(video && !PRIVATE\) resting\.rest\(video\);\s*\}\);\s*pill\.addEventListener\("pointerleave", \(\) => resting\.leave\(\)\);/.test(source), "the pointer on the button rests, off it leaves");
    assert.ok(fn("hide").includes("resting.leave();") && fn("teardown").includes("resting.leave();"), "hidden or torn down: no rest's look-up after");
    assert.ok(!/PRIVATE/.test(fn("openMenu")) && !/PRIVATE/.test(fn("download")), "the click and the menu look up in a private window too");
  }
  // #741: only the pointer really moving shows it (a repeated move, as a browser sends when the
  // page changes under a still pointer on pause or play, doesn't), nor the focus a click gives.
  assert.ok(fn("onMove").startsWith("function onMove(e) {\n    if (e.clientX === pointer.x && e.clientY === pointer.y) return;"), "onMove: a move in place is no move");
  assert.ok(source.includes('if (!t.matches(":focus-visible")) return;'), "focusin: the keyboard's focus only");
  assert.ok(/const IDLE_MS = 1500;/.test(source), "fades 1.5 s after the pointer rests");
  // Its texts: English's, kept as the fallback too.
  const english = JSON.parse(readText("../../extension/_locales/en/messages.json"));
  for (const key of ["video_looking", "video_all_qualities", "video_no_choices", "video_playlist", "video_no_ytdlp", "video_too_long", "video_busy", "video_queued", "video_failed_lookup",
    "video_off", "video_drm", "video_cant_save", "video_open_post", "video_open_post_button", "video_drag_dots", "video_tip_best", "video_sending", "video_added", "video_show",
    "video_no_answer", "video_get_mdm", "video_retry", "video_turn_on", "video_download_best", "video_best", "video_original", "video_compatible"]) {
    assert.ok(english[key], key);
    assert.ok(source.includes(`${key}: ${JSON.stringify(english[key].message)}`), `video.js FALLBACK: ${key}`);
    assert.ok(source.includes(`t("${key}")`), `video.js uses ${key}`);
  }
  {
    const w = make(["sizeText", "choiceSize", "qualityName", "qualityMeta", "qualityWords", "bestOf", "chipFor", "tipFor", "durationText", "headerLine", "moveIn", "fitsWords", "compactFor"],
      "{ choiceSize, qualityName, qualityMeta, qualityWords, bestOf, chipFor, tipFor, durationText, headerLine, moveIn, fitsWords, compactFor }");
    const tt = (k, ...a) => (a.length ? `${k}(${a.join(",")})` : k);
    const hd = { label: "1080p 60fps MP4", ext: "mp4", size: 412000000, audio: false, height: 1080, fps: 60 };
    const k4 = { label: "2160p MP4", ext: "mp4", height: 2160 };
    const sound = { label: "Audio only original M4A", ext: "m4a", size: 9000000, audio: true, kbps: 128 };
    const mp3 = { label: "Audio only MP3 320 kbps", ext: "mp3", audio: true };
    const older = { label: "720p MP4", ext: "mp4", size: 1 };
    assert.deepEqual([hd, k4, { height: 4320 }, { height: 480 }, sound, older, null].map((c) => w.qualityName(c, tt)), ["1080p", "4K", "8K", "480p", "video_tab_audio", "720p MP4", ""]);
    assert.equal(w.qualityName({ file: true, ext: "webm" }, tt), "WEBM");
    assert.equal(w.qualityMeta(hd, tt), "MP4 · 60 fps");
    assert.equal(w.qualityMeta(sound, tt), "128 kbps");
    assert.equal(w.qualityMeta(mp3, tt), "", "a conversion says it in its label");
    assert.equal(w.qualityMeta(older, tt), "", "an older app's label says it all");
    // #755, #1465: the file type and the codec (every merged video is an MP4, whatever its
    // picture), the frame rate and range; each line of one height its own.
    const av1 = { label: "2160p MP4 · AV1 · 60 fps · HDR", ext: "mp4", height: 2160, fps: 60, codec: "AV1", range: "HDR", size: 305554961 };
    const vp9 = { label: "2160p MP4 · VP9 · 60 fps · SDR", ext: "mp4", height: 2160, fps: 60, codec: "VP9", range: "SDR", size: 292630287 };
    assert.equal(w.qualityMeta(av1, tt), "MP4 · AV1 · 60 fps · HDR");
    assert.equal(w.qualityMeta(vp9, tt), "MP4 · VP9 · 60 fps · SDR");
    // #1465: a quality's whole name is the app's label, as its window and video-info say it.
    for (const c of [av1, vp9]) assert.equal(w.qualityWords(c, tt), c.label.replace(/^2160p/, "4K"));
    const fhd = { label: "1080p MP4 · AV1 · 30 fps", ext: "mp4", height: 1080, fps: 30, codec: "AV1" };
    assert.equal(w.qualityWords(fhd, tt), fhd.label);
    assert.equal(w.qualityWords(av1, tt, true), "4K MP4 · AV1", "Download best's line: the name, type and codec");
    assert.equal(w.qualityMeta(av1, tt, true), "MP4 · AV1");
    assert.equal(w.qualityWords({ height: 720, ext: "mp4" }, tt), "720p MP4", "no codec said: none guessed");
    assert.equal(w.qualityWords(sound, tt), "video_tab_audio");
    assert.equal(w.qualityWords(older, tt), "720p MP4", "an older app's label as it is");
    assert.equal(w.qualityWords({ file: true, ext: "webm" }, tt), "WEBM", "a plain file's type once");
    assert.equal(w.qualityWords({ file: true, height: 720, ext: "mp4" }, tt), "720p MP4");
    assert.equal(w.qualityMeta({ height: 360, ext: "webm", codec: "VP9", fps: 30 }, tt), "WEBM · VP9 · 30 fps", "a type other than MP4 is said");
    assert.equal(w.qualityMeta({ height: 720, ext: "mp4", codec: 7, range: { x: 1 } }, tt), "MP4", "only text is shown");
    assert.equal(w.qualityMeta({ height: 720, ext: "mp4", codec: "H.264".repeat(9) }, tt).split(" · ")[1].length, 24, "a codec's name is cut");
    // #1439: the most compatible choice (H.264 + AAC) says so in place of its codec.
    const compatible = { label: "Most compatible (H.264 + AAC) · 1080p", ext: "mp4", height: 1080, fps: 60, codec: "H.264", compatible: true };
    assert.ok(source.includes('if (c && c.compatible === true) b.title = String(c.label ?? "").slice(0, 200);'), "its line's tip is the app's whole name");
    assert.equal(w.qualityName(compatible, tt), "1080p");
    assert.equal(w.qualityMeta(compatible, tt), "video_compatible");
    // #801: an estimate is marked; an exact size isn't; an older app's has no mark.
    assert.equal(w.choiceSize({ size: 557842432, approx: true }, "en"), "~532 MB");
    assert.equal(w.choiceSize({ size: 557842432 }, "en"), "532 MB");
    assert.equal(w.choiceSize({ size: null, approx: true }, "en"), "");
    assert.equal(w.choiceSize(null, "en"), "");
    // The list's sound lines: the type, then the bitrate, and "original" for the sound as the site
    // sends it (with or without a bitrate); a conversion's VBR.
    const lw = make(["qualityName", "qualityMeta", "lineWords"], "lineWords");
    assert.deepEqual({ ...lw({ ext: "m4a", audio: true, kbps: 128, original: true }, tt) }, { q: "M4A", f: "128 kbps · video_original" });
    assert.deepEqual({ ...lw({ ext: "mp4", audio: true, original: true }, tt) }, { q: "MP4", f: "video_original" });
    assert.deepEqual({ ...lw({ ext: "mp3", audio: true, kbps: 320 }, tt) }, { q: "MP3", f: "320 kbps" });
    assert.deepEqual({ ...lw({ ext: "mp3", audio: true, label: "Audio only MP3 VBR" }, tt) }, { q: "MP3", f: "VBR" });
    // The best: the app's first video choice (its order is best first), else its first.
    const ready = { state: "ready", choices: [sound, hd, older] };
    assert.equal(w.bestOf(ready), hd);
    assert.equal(w.bestOf({ state: "ready", choices: [sound] }), sound);
    assert.equal(w.bestOf({ state: "pending" }), null);
    assert.deepEqual(w.chipFor({ state: "pending" }, tt), { skeleton: true });
    assert.deepEqual(w.chipFor(ready, tt), { text: "1080p", hd: true });
    assert.deepEqual(w.chipFor({ state: "ready", choices: [{ height: 480, ext: "mp4" }] }, tt), { text: "480p", hd: false });
    assert.equal(w.chipFor({ state: "ready", choices: [older] }, tt), null, "no height: no chip");
    assert.deepEqual(w.chipFor({ file: true, height: 720, ext: "mp4" }, tt), { text: "720p", hd: true });
    assert.equal(w.chipFor({ state: "failed" }, tt), null);
    assert.equal(w.tipFor(ready, tt, "en"), "video_tip(1080p MP4 · 60 fps · 392.9 MB)");
    assert.equal(w.tipFor({ state: "ready", choices: [{ ...hd, approx: true }] }, tt, "en"), "video_tip(1080p MP4 · 60 fps · ~392.9 MB)");
    assert.equal(w.tipFor({ state: "ready", choices: [fhd] }, tt, "en"), "video_tip(1080p MP4 · AV1 · 30 fps)", "#1465: the codec too");
    assert.equal(w.tipFor({ state: "pending" }, tt, "en"), "video_tip_best");
    assert.equal(w.tipFor(null, tt, "en"), "video_download");
    assert.deepEqual([596, 3723, 0, null, "x"].map(w.durationText), ["9:56", "1:02:03", "", "", ""]);
    assert.equal(w.headerLine("Big Buck Bunny", 596, "youtube.com"), "Big Buck Bunny · 9:56 · youtube.com");
    assert.equal(w.headerLine("", NaN, "a.test"), "a.test");
    assert.deepEqual([[-1, 4, "ArrowDown"], [3, 4, "ArrowDown"], [0, 4, "ArrowUp"], [-1, 4, "ArrowUp"], [2, 4, "Home"], [1, 4, "End"], [1, 0, "ArrowDown"]].map(([i, n, k]) => w.moveIn(i, n, k)), [0, 0, 3, 3, 0, 3, -1]);
    assert.equal(w.fitsWords(640, 240, 14), true);
    assert.equal(w.fitsWords(359, 100, 14), false, "under 360 px: the round button only");
    assert.equal(w.fitsWords(400, 380, 14), false, "a long language that wouldn't fit");
    // #1278: Options → Download panels → Button size. Full (the default): the words where they
    // fit, the round button on a small video or for a long language; Compact: round on every video.
    const english = 240;
    const german = 380; // the same button's width with longer words
    assert.equal(w.compactFor(false, 1280, english, 14), false, "Full on a large video: the words");
    assert.equal(w.compactFor(false, 1280, german, 14), false, "Full, a long language on a large video: the words");
    assert.equal(w.compactFor(false, 320, english, 14), true, "Full on a small video: round");
    assert.equal(w.compactFor(false, 400, german, 14), true, "Full, a long language that wouldn't fit: round");
    for (const [width, full] of [[1920, english], [1280, german], [640, english], [400, german], [200, english]]) {
      assert.equal(w.compactFor(true, width, full, 14), true, `Compact: round at ${width} px whatever the language`);
    }
    assert.equal(w.compactFor(undefined, 1280, english, 14), false, "not said (an older app): Full");
    // The answer to `panel` sets it, and position() decides from it.
    const ask = fn("ask");
    assert.ok(ask.includes("compactAlways = Boolean(r && r.compact);"), "ask: the size comes with the app's answer");
    assert.ok(fn("position").includes('pill.classList.toggle("compact", compactFor(compactAlways, vr.width, fullWidth, INSET));'), "position: compact from the setting and the video's width");
  }
  ok("video.js: the qualities' menu follows a lookup one question at a time for a minute; rows as text, the user's own clicks and keys; its words, #703's early look-up, #741's moves");
}

// No other download manager's name in what ships (the clean-room rule, CLAUDE.md).
{
  for (const file of readdirSync(new URL("../../extension/", import.meta.url))) {
    if (!/\.(js|html|css|json)$/.test(file)) continue;
    const text = readText(`../../extension/${file}`);
    assert.ok(!/\bIDM\b|Internet Download Manager|notes\//.test(text), `${file} names IDM or the private notes`);
  }
  ok("nothing of IDM's in the extension");
}

// The right-click command in the browser's language, and every locale complete: the same keys
// as English, which both manifests' __MSG_…__ names exist in.
{
  const { api, calls, listeners } = fakeApi({ firefox: false });
  load(api, false);
  await listeners.installed({ reason: "update" });
  const item = calls.find((c) => c[0] === "menu-item")[1];
  assert.equal(item.title, "Mit Mister Download Manager herunterladen");
  const dir = new URL("../../extension/_locales/", import.meta.url);
  const english = JSON.parse(readText(new URL("en/messages.json", dir)));
  for (const locale of readdirSync(dir)) {
    const m = JSON.parse(readText(new URL(`${locale}/messages.json`, dir)));
    assert.deepEqual(Object.keys(m).sort(), Object.keys(english).sort(), `${locale} has every text`);
  }
  for (const name of ["manifest.json", "manifest.firefox.json"]) {
    const text = readText(`../../extension/${name}`);
    for (const [, key] of text.matchAll(/__MSG_(\w+)__/g)) assert.ok(english[key], `${name}: ${key}`);
  }
  // Every key the scripts use exists in English.
  for (const file of ["background.js", "popup.js", "popup.html", "video.js"]) {
    const text = readText(`../../extension/${file}`);
    for (const [, key] of text.matchAll(/(?:t\(|data-i18n=)"((?:menu|status|popup|video)_\w+)"/g)) assert.ok(english[key], `${file}: ${key}`);
  }
  // The popup's (#1496): every key named in them, however it is passed on, and those its row
  // buttons make by adding "_named".
  for (const file of ["popup.html", "popup.js", "popup-ui.js", "popup-downloads.js"]) {
    const text = readText(`../../extension/${file}`);
    for (const [, key] of text.matchAll(/"((?:popup|status)_[a-z0-9_]+)"/g)) assert.ok(english[key], `${file}: ${key}`);
    for (const [, key] of text.matchAll(/data-i18n(?:-label|-placeholder)?="(\w+)"/g)) assert.ok(english[key], `${file}: ${key}`);
  }
  for (const b of ["popup_act_folder", "popup_act_pause", "popup_act_resume", "popup_act_more"]) assert.ok(english[`${b}_named`], `${b}_named`);
  for (const why of ["not_installed", "forbidden", "not_running", "host_failed", "too_large", "update_app"]) {
    assert.ok(english[`status_${why}`], `status_${why}`);
  }
  ok("translations complete");
}

// #42: the browser's proxy goes with the download, so the app uses the same one: an opt-in, the
// popup's switch. Chromium can't make the "proxy" permission optional, so its extension has it
// from the install ("required") and the switch is its own setting (`proxyOn`, off as installed).
// What Chromium's proxy.settings.get says, handed over for each address: {url: proxy or PAC}.
async function chromeProxy(proxy, urls, { on = true, levelOfControl } = {}) {
  const { api, calls, listeners } = fakeApi({ firefox: false, proxy, proxyPerm: "required", proxyOn: on });
  if (levelOfControl) api.proxy.settings.get = async () => ({ value: proxy, levelOfControl });
  load(api, false);
  let id = 600;
  for (const url of urls) listeners.determining({ id: id++, url, filename: "f.zip" }, () => {});
  await settle();
  const sent = natives(calls);
  assert.equal(sent.length, urls.length, "every download handed over");
  return Object.fromEntries(sent.map((m) => [m.url, m.proxy || (m.proxyPac ? `PAC ${m.proxyPac}` : null)]));
}
{
  // A VPN extension's fixed SOCKS5 proxy: remote DNS (socks5h), as Chromium does.
  const proxy = { mode: "fixed_servers", rules: { singleProxy: { scheme: "socks5", host: "vpn.example", port: 1080 }, bypassList: ["<local>"] } };
  const { api, calls, listeners } = fakeApi({ firefox: false, proxy, proxyPerm: "required", proxyOn: true });
  load(api, false);
  listeners.determining({ id: 31, url: "https://files.example/p.zip", filename: "p.zip" }, () => {});
  await settle();
  const m = natives(calls)[0];
  assert.equal(m.proxy, "socks5h://vpn.example:1080");
  assert.equal(m.proxyLogin, undefined);
  assert.ok(!calls.some((c) => c[0] === "contains-proxy" || c[0] === "request"), "no permission call: Chrome's is required");
  ok("Chrome, switched on: the extension's SOCKS5 proxy goes with the download");
}
{
  // One server per scheme: http and https each their own, the fallback for the one not given;
  // a scheme and a port left out are Chromium's defaults (http; 80, 443, 1080).
  const per = { mode: "fixed_servers", rules: { proxyForHttp: { scheme: "http", host: "h.example", port: 3128 }, proxyForHttps: { scheme: "https", host: "s.example" } } };
  assert.deepEqual(await chromeProxy(per, ["http://x/a.zip", "https://x/b.zip"]), { "http://x/a.zip": "http://h.example:3128", "https://x/b.zip": "https://s.example:443" });
  const fallback = { mode: "fixed_servers", rules: { proxyForHttp: { host: "h.example" }, fallbackProxy: { scheme: "socks4", host: "f.example" } } };
  assert.deepEqual(await chromeProxy(fallback, ["http://x/c.zip", "https://x/d.zip"]), { "http://x/c.zip": "http://h.example:80", "https://x/d.zip": "socks4://f.example:1080" });
  const onlyHttp = { mode: "fixed_servers", rules: { proxyForHttp: { scheme: "http", host: "h.example", port: 8080 } } };
  assert.deepEqual(await chromeProxy(onlyHttp, ["http://x/e.zip", "https://x/f.zip"]), { "http://x/e.zip": "http://h.example:8080", "https://x/f.zip": null });
  const socks = { mode: "fixed_servers", rules: { singleProxy: { scheme: "socks5", host: "10.8.0.1" } } };
  assert.deepEqual(await chromeProxy(socks, ["https://x/g.zip"]), { "https://x/g.zip": "socks5h://10.8.0.1:1080" });
  const quic = { mode: "fixed_servers", rules: { singleProxy: { scheme: "quic", host: "q.example", port: 443 } } };
  assert.deepEqual(await chromeProxy(quic, ["https://x/h.zip"]), { "https://x/h.zip": null }, "a QUIC proxy, which the app can't use: nothing");
  ok("Chrome: a fixed server for each scheme, the fallback, Chromium's default ports");
}
{
  // The bypass list, read as Chromium reads it: those addresses go without the proxy.
  const P = "http://p.example:8080";
  const rules = (bypassList) => ({ mode: "fixed_servers", rules: { singleProxy: { scheme: "http", host: "p.example", port: 8080 }, bypassList } });
  const list = ["*.intranet.example", ".corp.example", "<local>", "10.0.0.0/8", "files.example:8080", "https://secure.example", "*mirror.example"];
  const urls = [
    "https://files.intranet.example/a.zip", // *.intranet.example
    "https://intranet.example/b.zip", // not *.intranet.example's
    "https://a.corp.example/c.zip", // .corp.example is *.corp.example
    "https://corp.example/d.zip", // not .corp.example's
    "http://nas/e.zip", // <local>: no dot
    "http://10.1.2.3/f.zip", // 10.0.0.0/8
    "http://11.1.2.3/g.zip", // not in it
    "http://files.example:8080/h.zip", // its port
    "http://files.example/i.zip", // another port
    "https://secure.example/j.zip", // its scheme
    "http://secure.example/k.zip", // another scheme
    "https://dl.mirror.example/l.zip", // *mirror.example
    "https://other.example/m.zip",
  ];
  const got = await chromeProxy(rules(list), urls);
  assert.deepEqual(
    urls.map((u) => got[u]),
    [null, P, null, P, null, null, P, null, P, null, P, null, P],
  );
  // Chromium never sends loopback to a proxy, unless the list has "<-loopback>".
  const local = ["http://localhost:8000/n.zip", "http://127.0.0.1/o.zip", "http://[::1]/p.zip", "http://app.localhost/q.zip"];
  const plain = await chromeProxy(rules([]), [...local, "http://[fe80::1]/r.zip", "https://feature.example/s.zip", "https://127.example/t.zip"]);
  assert.deepEqual(local.map((u) => plain[u]), [null, null, null, null], "loopback: direct");
  assert.equal(plain["http://[fe80::1]/r.zip"], null, "link-local: direct");
  assert.deepEqual([plain["https://feature.example/s.zip"], plain["https://127.example/t.zip"]], [P, P], "names that only look local");
  const forced = await chromeProxy(rules(["<-loopback>"]), local);
  assert.deepEqual(local.map((u) => forced[u]), [P, P, P, P], "<-loopback>: through the proxy");
  ok("Chrome: the bypass list (patterns, ranges, ports, schemes, <local>, loopback and <-loopback>)");
}
{
  // A proxy that asked for a login (the VPN extension answers it): flagged for the app.
  const proxy = { mode: "fixed_servers", rules: { proxyForHttps: { scheme: "https", host: "gw.vpn.example" } } };
  const { api, calls, listeners } = fakeApi({ firefox: false, proxy, proxyPerm: "required", proxyOn: true });
  load(api, false);
  await settle();
  listeners.authRequired({ isProxy: true, challenger: { host: "gw.vpn.example", port: 443 } });
  listeners.determining({ id: 33, url: "https://x/q.zip", filename: "q.zip" }, () => {});
  await settle();
  const m = natives(calls)[0];
  assert.deepEqual([m.proxy, m.proxyLogin], ["https://gw.vpn.example:443", true]);
  ok("Chrome: a proxy that asks for a login is flagged");
}
{
  // A PAC script: inline (its data) or by its address (fetched without cookies); one that can't
  // be fetched, or isn't a PAC script, sends nothing.
  const data = 'function FindProxyForURL(u, h) { return "PROXY p:3128"; }';
  assert.deepEqual(await chromeProxy({ mode: "pac_script", pacScript: { data } }, ["https://x/r.zip"]), { "https://x/r.zip": `PAC ${data}` });
  served = { "http://wpad.example/vpn.pac": { body: data }, "http://wpad.example/page.html": { body: "<html>" } };
  const before = fetched.length;
  assert.deepEqual(await chromeProxy({ mode: "pac_script", pacScript: { url: "http://wpad.example/vpn.pac", mandatory: false } }, ["https://x/s.zip"]), { "https://x/s.zip": `PAC ${data}` });
  assert.deepEqual(fetched.slice(before).filter(([u]) => u.includes("wpad")), [["http://wpad.example/vpn.pac", "omit"]]);
  assert.deepEqual(await chromeProxy({ mode: "pac_script", pacScript: { url: "http://wpad.example/gone.pac" } }, ["https://x/t.zip"]), { "https://x/t.zip": null });
  assert.deepEqual(await chromeProxy({ mode: "pac_script", pacScript: { url: "http://wpad.example/page.html" } }, ["https://x/u.zip"]), { "https://x/u.zip": null });
  served = {};
  ok("Chrome: a PAC script, inline or by its address");
}
{
  // Direct, the system's, auto-detect: nothing (the app goes by its own Options). A policy's or
  // the command line's proxy (not_controllable) is the browser's too.
  for (const mode of ["direct", "system", "auto_detect"]) {
    assert.deepEqual(await chromeProxy({ mode }, ["https://x/v.zip"]), { "https://x/v.zip": null }, mode);
  }
  const policy = { mode: "fixed_servers", rules: { singleProxy: { scheme: "http", host: "corp-proxy", port: 8080 } } };
  assert.deepEqual(await chromeProxy(policy, ["https://x/w.zip"], { levelOfControl: "not_controllable" }), { "https://x/w.zip": "http://corp-proxy:8080" });
  ok("Chrome: direct, system and auto-detect send nothing; a policy's proxy goes");
}
{
  // As installed (the switch off): nothing read or sent, a proxy login seen included; the popup
  // switches it on (storage), then off: it follows at once. No permission call, nothing in the
  // console.
  const before = complaints.length;
  const proxy = { mode: "fixed_servers", rules: { singleProxy: { scheme: "http", host: "10.0.0.9", port: 3128 } } };
  const { api, calls, listeners, store } = fakeApi({ firefox: false, proxy, proxyPerm: "required" });
  let read = 0;
  api.proxy.settings.get = async () => (read++, { value: proxy, levelOfControl: "controlled_by_other_extensions" });
  load(api, false);
  await settle();
  listeners.authRequired({ isProxy: true, challenger: { host: "10.0.0.9", port: 3128 } });
  listeners.determining({ id: 42, url: "https://x/off.zip", filename: "off.zip" }, () => {});
  await settle();
  let m = natives(calls)[0];
  assert.deepEqual([m.proxy, m.proxyPac, m.proxyLogin], [undefined, undefined, undefined]);
  assert.equal(read, 0, "the browser's proxy settings never read while off");
  await api.storage.local.set({ proxyOn: true });
  await settle();
  listeners.determining({ id: 43, url: "https://x/on.zip", filename: "on.zip" }, () => {});
  await settle();
  m = natives(calls)[1];
  assert.deepEqual([m.proxy, m.proxyLogin], ["http://10.0.0.9:3128", undefined], "on: the proxy goes; the login seen while off was not kept");
  listeners.authRequired({ isProxy: true, challenger: { host: "10.0.0.9", port: 3128 } });
  await api.storage.local.set({ proxyOn: false });
  await settle();
  assert.equal(store.proxyOn, false);
  listeners.determining({ id: 44, url: "https://x/off2.zip", filename: "off2.zip" }, () => {});
  await settle();
  m = natives(calls)[2];
  assert.deepEqual([m.proxy, m.proxyPac, m.proxyLogin], [undefined, undefined, undefined], "off again: nothing");
  await api.storage.local.set({ proxyOn: true });
  listeners.determining({ id: 45, url: "https://x/on2.zip", filename: "on2.zip" }, () => {});
  await settle();
  m = natives(calls)[3];
  assert.deepEqual([m.proxy, m.proxyLogin], ["http://10.0.0.9:3128", undefined], "on again: the login seen before was forgotten when off");
  assert.equal(read, 2, "read only while on");
  assert.ok(!calls.some((c) => c[0] === "contains-proxy" || c[0] === "request" || c[0] === "remove"), "no permission call");
  assert.equal(complaints.length, before, `the console: ${JSON.stringify(complaints.slice(before))}`);
  ok("Chrome: off as installed (nothing read or sent), on and off from the popup's switch, nothing in the console");
}
{
  // Firefox, switched on (the optional permission granted): the proxy its own request for the
  // file took (proxy.onRequest extensions too).
  const { api, calls, listeners } = fakeApi({ firefox: true, proxyGranted: true });
  load(api, true);
  await settle();
  listeners.responseStarted({ url: "https://x/s.zip", proxyInfo: { type: "socks", host: "10.8.0.1", port: 1080, proxyDNS: true, username: "u" } });
  listeners.created({ id: 35, url: "https://x/s.zip", filename: "C:\\dl\\s.zip" });
  await settle();
  const m = natives(calls)[0];
  assert.deepEqual([m.proxy, m.proxyLogin], ["socks5h://10.8.0.1:1080", true]);
  ok("Firefox: the request's own proxy goes, its login flagged");
}
{
  // Firefox, not switched on (as installed, or the request refused): as before #42, nothing read
  // and nothing sent, the browser's own proxy settings included; no complaint in the console.
  const before = complaints.length;
  const settings = { proxyType: "manual", http: "10.0.0.9:3128", httpProxyAll: true };
  const { api, calls, listeners } = fakeApi({ firefox: true, proxy: settings, proxyAnswer: false });
  let read = 0;
  api.proxy.settings.get = async () => (read++, { value: settings });
  load(api, true);
  await settle();
  listeners.responseStarted({ url: "https://x/u.zip", proxyInfo: { type: "http", host: "10.0.0.9", port: 3128 } });
  listeners.authRequired({ isProxy: true, challenger: { host: "10.0.0.9", port: 3128 } });
  listeners.created({ id: 37, url: "https://x/u.zip", filename: "C:\\dl\\u.zip" });
  await settle();
  const m = natives(calls)[0];
  assert.deepEqual([m.proxy, m.proxyPac, m.proxyLogin], [undefined, undefined, undefined]);
  assert.equal(read, 0, "the browser's proxy settings never read");
  // Refused: still nothing.
  assert.equal(await api.permissions.request({ permissions: ["proxy"] }), false);
  listeners.created({ id: 38, url: "https://x/u.zip", filename: "C:\\dl\\u2.zip" });
  await settle();
  assert.equal(natives(calls)[1].proxy, undefined);
  assert.equal(complaints.length, before, `the console: ${JSON.stringify(complaints.slice(before))}`);
  ok("Firefox, not switched on or refused: no proxy read or sent, nothing in the console");
}
{
  // Firefox: switched on (granted while it runs), then off (removed): it follows at once, and
  // what it saw while on is forgotten.
  const { api, calls, listeners } = fakeApi({ firefox: true });
  load(api, true);
  await settle();
  assert.equal(await api.permissions.request({ permissions: ["proxy"] }), true);
  await settle();
  listeners.responseStarted({ url: "https://x/v.zip", proxyInfo: { type: "http", host: "10.0.0.9", port: 3128 } });
  listeners.created({ id: 39, url: "https://x/v.zip", filename: "C:\\dl\\v.zip" });
  await settle();
  assert.equal(natives(calls)[0].proxy, "http://10.0.0.9:3128", "granted: the request's proxy goes");
  listeners.responseStarted({ url: "https://x/w.zip", proxyInfo: { type: "http", host: "10.0.0.9", port: 3128 } });
  await api.permissions.remove({ permissions: ["proxy"] });
  await settle();
  listeners.created({ id: 40, url: "https://x/w.zip", filename: "C:\\dl\\w.zip" });
  await settle();
  assert.equal(natives(calls)[1].proxy, undefined, "removed: nothing, not even what it saw before");
  ok("Firefox: switched on, then off: the proxy goes only while on");
}
{
  // A Chromium manifest without the "proxy" permission (0.2.4's, the API absent), even with the
  // switch's setting there: as before #42, and the extension never asks about the permission;
  // nothing in the console.
  const before = complaints.length;
  const { api, calls, listeners } = fakeApi({ firefox: false, proxyOn: true });
  load(api, false);
  listeners.authRequired && listeners.authRequired({ isProxy: true, challenger: { host: "p", port: 1 } });
  listeners.determining({ id: 41, url: "https://x/y.zip", filename: "y.zip" }, () => {});
  await settle();
  const m = natives(calls)[0];
  assert.deepEqual([m.proxy, m.proxyPac, m.proxyLogin], [undefined, undefined, undefined]);
  assert.ok(!calls.some((c) => c[0] === "contains-proxy" || c[0] === "request"), "no permission call");
  assert.equal(listeners.authRequired, undefined, "no proxy-login listener");
  assert.equal(complaints.length, before, `the console: ${JSON.stringify(complaints.slice(before))}`);
  ok("Chromium without the proxy permission: no reading, no permission call, nothing in the console");
}
{
  // No proxy in use (switched on): nothing extra.
  const { api, calls, listeners } = fakeApi({ firefox: false, proxyPerm: "required", proxyOn: true });
  load(api, false);
  listeners.determining({ id: 36, url: "https://x/t.zip", filename: "t.zip" }, () => {});
  await settle();
  const m = natives(calls)[0];
  assert.deepEqual([m.proxy, m.proxyPac, m.proxyLogin], [undefined, undefined, undefined]);
  ok("no proxy: nothing extra goes");
}

// ---- #208: why a download stayed with the browser, and older apps --------------------------

const popupState = (listeners) =>
  new Promise((r) => listeners.message({ type: "state" }, { id: "ours" }, (v) => r(JSON.parse(JSON.stringify(v)))));

// The host refuses by Options (its rules changed since): the rule it names is kept for the popup
// (the host logged it; no notice); a download taken later clears it.
{
  const { api, calls, listeners, store } = fakeApi({ firefox: false, reply: { ok: false, why: "options", declined: "browser", error: "left to the browser by Options (Browser)" } });
  load(api, false);
  let kept = false;
  listeners.determining({ id: 60, url: "https://x/a.zip", filename: "a.zip" }, () => (kept = true));
  await settle();
  assert.ok(kept);
  assert.deepEqual(notices(calls), []);
  const s = await popupState(listeners);
  assert.deepEqual([s.declined.why, s.declined.name, s.declined.site, s.connected], ["browser", "a.zip", "x", true]);
  ok("#208: the host's refusal is kept for the popup, by its rule");
}
{
  const { api, listeners, store } = fakeApi({ firefox: false });
  load(api, false);
  store.declined = { why: "file-type", name: "p.png", site: "x", at: 1 };
  listeners.determining({ id: 61, url: "https://x/b.zip", filename: "b.zip" }, () => {});
  await settle();
  assert.equal(store.declined, undefined, "a download taken clears it");
  assert.equal((await popupState(listeners)).declined, null);
  ok("#208: a download taken clears the last refusal");
}

// An app before #208 (0.2.4, 0.3.0): its rules say only the types (0.2.4) or no reason for
// `capture` (0.3.0), its refusal only in the error text, and it takes a notice for an address it
// can't download. Downloads go as before, the popup still names the rule, the status stays.
{
  const old024 = { ok: true, types: "ZIP EXE" };
  const notice024 = { ok: false, error: "not an http(s) or ftp(s) URL" };
  const { api, calls, listeners } = fakeApi({ firefox: false, rulesReply: old024, noticeReply: notice024 });
  load(api, false);
  listeners.determining({ id: 62, url: "https://x/a.zip", filename: "a.zip" }, () => {});
  await settle();
  assert.equal(natives(calls).length, 1, "a listed type is offered");
  listeners.determining({ id: 63, url: "https://x/p.png", filename: "p.png" }, () => {});
  await settle();
  assert.deepEqual(notices(calls).map((n) => n.why), ["file-type"]);
  const s = await popupState(listeners);
  assert.ok(s.connected && !s.why, `the older host's answer to a notice changes no status: ${JSON.stringify(s)}`);
  assert.equal(s.declined.why, "file-type");
  ok("#208: a 0.2.4 app: downloads as before, a notice it can't read is harmless");
}
{
  const { api, listeners } = fakeApi({ firefox: true, reply: { ok: false, error: "left to the browser by Options (Small)" } });
  load(api, true);
  listeners.created({ id: 64, url: "https://x/c.zip", filename: "C:\\dl\\c.zip" });
  await settle();
  assert.equal((await popupState(listeners)).declined.why, "small");
  const off = fakeApi({ firefox: false, rules: { capture: false } });
  load(off.api, false);
  off.listeners.determining({ id: 65, url: "https://x/d.zip", filename: "d.zip" }, () => {});
  await settle();
  assert.deepEqual(notices(off.calls).map((n) => n.why), ["browser"], "0.3.0: capture off, no reason: the browser's box");
  const integration = fakeApi({ firefox: false, rules: { capture: false, captureOff: "integration-off" } });
  load(integration.api, false);
  integration.listeners.determining({ id: 66, url: "https://x/e.zip", filename: "e.zip" }, () => {});
  await settle();
  assert.deepEqual(natives(integration.calls), []);
  assert.deepEqual(notices(integration.calls).map((n) => n.why), ["integration-off"]);
  ok("#208: an older host's refusal read from its words; capture off by the reason given");
}

// A private window's: neither its name nor its site is kept or sent.
{
  const { api, calls, listeners, store } = fakeApi({ firefox: false });
  load(api, false);
  listeners.determining({ id: 67, url: "https://secret.test/p.png", filename: "p.png", incognito: true }, () => {});
  await settle();
  assert.deepEqual([store.declined.why, store.declined.name, store.declined.site], ["file-type", "", ""]);
  const sent = calls.find((c) => c[0] === "native" && c[2].declined)[2];
  assert.ok(sent.private && !JSON.stringify(sent).includes("secret"));
  ok("#208: a private window's refusal names nothing");
}

// #1058: the app's Torrent settings leave torrents to the browser (its own torrent client): a
// .torrent (a tracker's download.php named by Content-Disposition, or a .torrent address) stays
// with the browser and says why; other types go as before. On, or a host before it: taken.
for (const firefox of [false, true]) {
  const fire = (listeners, id, url, filename, mime) =>
    firefox ? listeners.created({ id, url, filename: `C:\\dl\\${filename}`, mime }) : listeners.determining({ id, url, filename, mime }, () => {});
  const types = "ZIP TORRENT";
  {
    const { api, calls, listeners } = fakeApi({ firefox, types, rules: { torrents: false } });
    load(api, firefox);
    fire(listeners, 70, "https://tracker.test/download.php?id=42", "Show.S01.torrent", "application/x-bittorrent");
    await settle();
    fire(listeners, 71, "https://tracker.test/dl/set.torrent", "set.torrent", "");
    await settle();
    assert.deepEqual(natives(calls).filter((m) => m.url), [], "no torrent offered");
    assert.deepEqual(notices(calls).map((n) => n.why), ["torrents", "torrents"]);
    assert.equal((await popupState(listeners)).declined.why, "torrents");
    fire(listeners, 72, "https://tracker.test/a.zip", "a.zip", "application/zip");
    await settle();
    assert.deepEqual(natives(calls).filter((m) => m.url).map((m) => m.filename), ["a.zip"], "other types as before");
  }
  for (const rules of [{ torrents: true }, {}]) {
    const { api, calls, listeners } = fakeApi({ firefox, types, rules });
    load(api, firefox);
    fire(listeners, 73, "https://tracker.test/download.php?id=42", "Show.S01.torrent", "application/x-bittorrent");
    await settle();
    assert.deepEqual(natives(calls).filter((m) => m.url).map((m) => m.filename), ["Show.S01.torrent"], JSON.stringify(rules));
    assert.deepEqual(notices(calls), []);
  }
}
{
  // Safari's page.js holds no .torrent link then (its stored rules say so).
  const page = readText("../../extension/page.js");
  assert.ok(/noTorrents = Boolean\(rules && rules\.torrents === false\)/.test(page) && page.includes("!(noTorrents && /^torrent$/i.test(type))"));
}
ok("#1058: a torrent stays with the browser while the app's Torrent settings leave torrents alone");

// The codes agree end to end: the app's (mdm_ipc::capture::Declined::code), the extension's own
// ("site-access"), and a popup text for each.
{
  const rust = readText("../../crates/ipc/src/capture.rs");
  const body = /pub fn code\(self\)[\s\S]*?\n    \}\n/.exec(rust)[0];
  const codes = [...body.matchAll(/=> "([a-z-]+)"/g)].map((m) => m[1]);
  assert.deepEqual(codes, ["integration-off", "browser", "file-type", "site", "address", "small", "torrents"]);
  const en = JSON.parse(readText("../../extension/_locales/en/messages.json"));
  for (const c of [...codes, "site-access"]) assert.ok(en[`declined_${c.replace(/-/g, "_")}`], `a popup text for ${c}`);
  assert.ok(en.popup_declined && en.popup_declined_unnamed);
  const bg = code;
  for (const name of ["IntegrationOff", "Browser", "FileType", "Site", "Address", "Small"]) assert.ok(bg.includes(`${name}: "`), `older hosts' ${name}`);
  ok("#208: refusal codes, the app's and the popup's, agree");
}

// #208: Firefox (and Waterfox, which always asks first) creates a download only after its "What
// should … do with this file?" dialog, having downloaded the whole file behind it. The answer to
// the page load is caught before that instead: offered with its name (Content-Disposition), size,
// type and the page that linked it, then cancelled (no dialog, no browser download); the
// download it would still become is left alone, never asked twice.
const answer = (url, headers, more = {}) => ({
  requestId: "r1",
  url,
  method: "GET",
  type: "main_frame",
  tabId: 3,
  statusCode: 200,
  responseHeaders: Object.entries(headers).map(([name, value]) => ({ name, value })),
  ...more,
});
// The listener's answer, as a plain object (it is made in the sandbox).
const held = async (listeners, d) => JSON.parse(JSON.stringify(await listeners.headersReceived(d)));
const ask = (listeners) => (m, sender) => new Promise((r) => listeners.message(m, sender, r));
{
  const { api, calls, listeners } = fakeApi({ firefox: true });
  load(api, true);
  await settle();
  listeners.beforeSendHeaders({ requestId: "r1", method: "GET", url: "https://dl.test/get?id=1", requestHeaders: [{ name: "Referer", value: "https://site.test/page" }] });
  const res = await held(listeners, 
    answer(
      "https://dl.test/get?id=1",
      {
        "Content-Type": "application/octet-stream",
        "Content-Disposition": "attachment; filename=\"fallback.zip\"; filename*=UTF-8''r%C3%A9sum%C3%A9%20v2.zip",
        "Content-Length": "4096",
      },
      { cookieStoreId: "firefox-container-3" },
    ),
  );
  assert.deepEqual(res, { cancel: true });
  const m = natives(calls)[0];
  assert.deepEqual(
    [m.url, m.filename, m.size, m.mime, m.referrer, m.cookies],
    ["https://dl.test/get?id=1", "résumé v2.zip", 4096, "application/octet-stream", "https://site.test/page", "sid=1"],
  );
  assert.deepEqual(calls.find((c) => c[0] === "cookies")[1], { url: "https://dl.test/get?id=1", storeId: "firefox-container-3" });
  // Were the browser to create the download all the same: left alone, not asked twice.
  listeners.created({ id: 30, url: "https://dl.test/get?id=1", filename: "C:\\dl\\résumé v2.zip" });
  await settle();
  assert.equal(natives(calls).length, 1);
  assert.deepEqual(steps(calls), []);
  ok("#208 Firefox: a download caught from its answer, before the browser's dialog");
}
// #607: the page a download was started from goes whole with it (the app's Refresh download
// address opens it), where the Referer is often the site's address only. Firefox names it for the
// load (originUrl), through its redirects; never the file's own address.
{
  const { api, calls, listeners } = fakeApi({ firefox: true });
  load(api, true);
  await settle();
  const page = "https://site.test/files/report.html?lang=en";
  listeners.beforeRequest({ requestId: "q1", method: "GET", url: "https://site.test/get?id=7", tabId: 3, originUrl: page });
  listeners.beforeRequest({ requestId: "q1", method: "GET", url: "https://cdn.test/r/7.zip", tabId: 3, originUrl: page });
  listeners.beforeRedirect({ url: "https://site.test/get?id=7", redirectUrl: "https://cdn.test/r/7.zip" });
  listeners.beforeSendHeaders({ requestId: "q1", method: "GET", url: "https://cdn.test/r/7.zip", requestHeaders: [{ name: "Referer", value: "https://site.test/" }] });
  await held(listeners, answer("https://cdn.test/r/7.zip", { "Content-Type": "application/zip", "Content-Disposition": "attachment" }, { requestId: "q1" }));
  const m = natives(calls)[0];
  assert.deepEqual([m.url, m.referrer, m.pageUrl], ["https://site.test/get?id=7", "https://site.test/", page]);
  // Typed in the address bar: no page started it, the tab shows none, nothing is said.
  listeners.beforeRequest({ requestId: "q2", method: "GET", url: "https://cdn.test/typed.zip", tabId: 3 });
  await held(listeners, answer("https://cdn.test/typed.zip", { "Content-Type": "application/zip", "Content-Disposition": "attachment" }, { requestId: "q2" }));
  assert.equal(natives(calls)[1].pageUrl, undefined);
  ok("#607 Firefox: the page a download was started from goes with it, whole");
}
// #607 Chromium: the tab the download's load was in still shows the page; a link opened in a tab
// of its own, the tab that opened it; a download no load names (a script's, <a download>), the
// active tab of the window last used, from the same kind of window only.
{
  const page = "https://site.test/list?page=2";
  const tabs = { 4: { id: 4, url: page }, 5: { id: 5, url: "about:blank", openerTabId: 4 } };
  const { api, calls, listeners } = fakeApi({ tabs, activeTab: { id: 6, url: "https://other.test/news", incognito: false } });
  load(api, false);
  await settle();
  listeners.beforeRequest({ requestId: "c1", method: "GET", url: "https://cdn.test/a.zip", tabId: 4, initiator: "https://site.test" });
  listeners.determining({ id: 41, url: "https://cdn.test/a.zip", referrer: "https://site.test/", filename: "a.zip" }, () => {});
  await settle();
  listeners.beforeRequest({ requestId: "c2", method: "GET", url: "https://cdn.test/b.zip", tabId: 5 });
  listeners.determining({ id: 42, url: "https://cdn.test/b.zip", filename: "b.zip" }, () => {});
  await settle();
  listeners.determining({ id: 43, url: "https://cdn.test/c.zip", filename: "c.zip" }, () => {});
  await settle();
  listeners.determining({ id: 44, url: "https://cdn.test/d.zip", filename: "d.zip", incognito: true }, () => {});
  await settle();
  const sent = natives(calls).map((m) => [m.url, m.pageUrl]);
  assert.deepEqual(sent, [
    ["https://cdn.test/a.zip", page],
    ["https://cdn.test/b.zip", page],
    ["https://cdn.test/c.zip", "https://other.test/news"],
    ["https://cdn.test/d.zip", undefined],
  ]);
  ok("#607 Chromium: the page is the download's tab, its opener, else the active tab");
}
// A file host's Download button (a premium Rapidgator file): the file page is on the site's
// /file/… address, the download comes from another of its servers, the Referer is the site's
// address only. Chromium: by the time the answer is a download, the tab opened for the button's
// link is closed by the browser (or the tab has gone on to another page): the page is the one
// its tab (or its opener) showed as the load started, never the Referer's bare site.
{
  const page = "https://rapidgator.test/file/0a1b2c/report.zip.html";
  const file = "https://s118.rapidgator.test/download/7f3e-uuid/report.zip";
  const tabs = { 4: { id: 4, url: page }, 5: { id: 5, url: "", openerTabId: 4 }, 6: { id: 6, url: page } };
  const { api, calls, listeners } = fakeApi({ tabs, activeTab: { id: 9, url: "https://other.test/news", incognito: false } });
  load(api, false);
  await settle();
  // Opened in a tab of its own, which the browser closes as the download starts.
  listeners.beforeRequest({ requestId: "r1", method: "GET", url: file, tabId: 5, type: "main_frame", initiator: "https://rapidgator.test" });
  await settle();
  Object.defineProperty(tabs, 5, { get: () => Promise.reject(new Error("No tab with id: 5")) });
  listeners.determining({ id: 51, url: file, referrer: "https://rapidgator.test/", filename: "report.zip" }, () => {});
  await settle();
  // Through a redirect in the same tab, which has moved on by then (the site's home).
  const start = "https://rapidgator.test/download/premium/0a1b2c";
  const file2 = "https://s118.rapidgator.test/download/8a4f-uuid/report2.zip";
  listeners.beforeRequest({ requestId: "r2", method: "GET", url: start, tabId: 6, type: "main_frame", initiator: "https://rapidgator.test" });
  listeners.beforeRequest({ requestId: "r2", method: "GET", url: file2, tabId: 6, type: "main_frame", initiator: "https://rapidgator.test" });
  await settle();
  tabs[6] = { id: 6, url: "https://rapidgator.test/" };
  listeners.determining({ id: 52, url: start, finalUrl: file2, referrer: "https://rapidgator.test/", filename: "report2.zip" }, () => {});
  await settle();
  const sent = natives(calls).map((m) => [m.finalUrl || m.url, m.referrer, m.pageUrl]);
  assert.deepEqual(sent, [
    [file, "https://rapidgator.test/", page],
    [file2, "https://rapidgator.test/", page],
  ]);
  ok("#607 Chromium: a file host's button, its tab closed or moved on: the file page, not the site");
}
// Firefox: the same button's load names its page (originUrl), the one in a tab of its own too.
{
  const page = "https://rapidgator.test/file/0a1b2c/report.zip.html";
  const file = "https://s118.rapidgator.test/download/7f3e-uuid/report.zip";
  const { api, calls, listeners } = fakeApi({ firefox: true, activeTab: { id: 9, url: "https://other.test/news", incognito: false } });
  load(api, true);
  await settle();
  listeners.beforeRequest({ requestId: "f1", method: "GET", url: file, tabId: 5, type: "main_frame", originUrl: page });
  listeners.beforeSendHeaders({ requestId: "f1", method: "GET", url: file, requestHeaders: [{ name: "Referer", value: "https://rapidgator.test/" }] });
  await held(listeners, answer(file, { "Content-Type": "application/octet-stream", "Content-Disposition": "attachment; filename=report.zip" }, { requestId: "f1", tabId: 5 }));
  const m = natives(calls)[0];
  assert.deepEqual([m.url, m.referrer, m.pageUrl], [file, "https://rapidgator.test/", page]);
  ok("#607 Firefox: a file host's button on another of its servers: the file page, not the site");
}
// #447: Firefox hands a header's value over byte for byte, each byte one character. A name sent as
// raw UTF-8 is read as UTF-8; bytes that aren't valid UTF-8 (ISO-8859-1) and a real percent sign stay.
{
  const asBytes = (text) => String.fromCharCode(...new TextEncoder().encode(text));
  for (const [value, want] of [
    [`attachment; filename="${asBytes("تقرير.pdf")}"`, "تقرير.pdf"],
    [`attachment; filename="${asBytes("été à Paris.pdf")}"`, "été à Paris.pdf"],
    [`attachment; filename="${asBytes("日本語.pdf")}"`, "日本語.pdf"],
    ["attachment; filename=\"plain report.pdf\"", "plain report.pdf"],
    // ISO-8859-1 bytes (é alone is not valid UTF-8).
    ["attachment; filename=\"caf\u00e9.pdf\"", "café.pdf"],
    // Percent-encoded UTF-8 in the plain name, as Firefox and the app read it; ASCII too (#1427).
    ["attachment; filename=\"%D8%AA%D9%82%D8%B1%D9%8A%D8%B1.pdf\"", "تقرير.pdf"],
    ["attachment; filename=\"100%25.pdf\"", "100%.pdf"],
    ["attachment; filename=\"50%.pdf\"", "50%.pdf"],
    // RFC 8187 keeps winning, ASCII as it comes.
    ["attachment; filename=\"fallback.pdf\"; filename*=UTF-8''%D8%AA%D9%82%D8%B1%D9%8A%D8%B1.pdf", "تقرير.pdf"],
  ]) {
    const { api, calls, listeners } = fakeApi({ firefox: true });
    load(api, true);
    await settle();
    await held(listeners, answer("https://dl.test/get?id=2", { "Content-Type": "application/octet-stream", "Content-Disposition": value }));
    assert.equal(natives(calls)[0].filename, want, value);
  }
  ok("#447 Firefox: a raw UTF-8 file name in a header reaches the app readable, other names as they were");
}
// #1427: a link that only redirects (302) to the file, whose own answer alone names it: the 302 is
// no download, the file's answer is, under its name read as Chrome and Firefox read it (both
// checked on a lab server) and as the app does (naming.rs `decode_plain`): a plain filename's
// percent-encoding decoded, ASCII too, and RFC 2047's encoded words.
{
  for (const [value, want] of [
    ["attachment; filename=\"fallback.bin\"; filename*=UTF-8''My%20Report%20%E2%80%93%20v2.pdf", "My Report – v2.pdf"],
    ["attachment; filename*=UTF-8''My%20Report%20%E2%80%93%20v2.pdf", "My Report – v2.pdf"],
    ["attachment; filename=\"My%20Report.pdf\"", "My Report.pdf"],
    ["attachment; filename=My%20Report%20v3.pdf", "My Report v3.pdf"],
    ["attachment; filename=\"100%25 sure.zip\"", "100% sure.zip"],
    ["attachment; filename=\"50%off sale.zip\"", "50%off sale.zip"],
    ["attachment; filename=\"caf%E9 menu.zip\"", "caf%E9 menu.zip"],
    ["attachment; filename=\"=?UTF-8?B?5oql5ZGKIHY1LnBkZg==?=\"", "报告 v5.pdf"],
    ["attachment; filename=\"=?utf-8?q?My_Report_=E2=80=93_v6.pdf?=\"", "My Report – v6.pdf"],
    ["attachment; filename=\"Report =?UTF-8?B?5oql5ZGK?= v7.pdf\"", "Report 报告 v7.pdf"],
    ["attachment; filename=\"=?UTF-8?Q?My_?= =?UTF-8?Q?Report.pdf?=\"", "My Report.pdf"],
    ["attachment; filename=\"=?ISO-8859-1?Q?caf=E9.zip?=\"", "café.zip"],
    ["attachment; filename*=UTF-8''100%2525%20double.zip", "100%25 double.zip"],
    ["attachment; filename=\"a%2Fb report.pdf\"", "a_b report.pdf"],
    ["attachment; filename*=UTF-8''a%2Fb%5Cc.pdf", "a_b_c.pdf"],
  ]) {
    const { api, calls, listeners } = fakeApi({ firefox: true });
    load(api, true);
    await settle();
    const first = "https://site.test/download/42";
    const file = "https://cdn.test/files/7f3e/x.bin";
    listeners.beforeRequest({ requestId: "c1", method: "GET", url: first, tabId: 3, type: "main_frame" });
    // The 302 itself: no name, no download.
    assert.deepEqual(await held(listeners, answer(first, { Location: file }, { requestId: "c1", statusCode: 302 })), {});
    listeners.beforeRedirect({ url: first, redirectUrl: file });
    listeners.beforeRequest({ requestId: "c1", method: "GET", url: file, tabId: 3, type: "main_frame" });
    const res = await held(listeners, answer(file, { "Content-Type": "application/octet-stream", "Content-Disposition": value }, { requestId: "c1" }));
    assert.deepEqual(res, { cancel: true }, value);
    const sent = natives(calls);
    assert.equal(sent.length, 1, value);
    assert.deepEqual([sent[0].url, sent[0].finalUrl, sent[0].filename], [first, file, want], value);
  }
  ok("#1427 Firefox: a link that redirects to its file, named by the file's own answer as browsers read it");
}
// A download taken from its answer (#208) is cancelled, so its response never starts: the proxy a
// VPN add-on chose for it (proxy.onRequest, in the answer's proxyInfo) still goes with it.
{
  const { api, calls, listeners } = fakeApi({ firefox: true, proxyGranted: true });
  load(api, true);
  await settle();
  const res = await held(
    listeners,
    answer("https://dl.test/vpn.zip", { "Content-Type": "application/zip", "Content-Disposition": "attachment" }, { proxyInfo: { type: "socks", host: "10.8.0.1", port: 1080, proxyDNS: true } }),
  );
  assert.deepEqual(res, { cancel: true });
  assert.deepEqual([natives(calls)[0].proxy, natives(calls)[0].proxyLogin], ["socks5h://10.8.0.1:1080", undefined]);
  ok("#208 Firefox: a download taken from its answer keeps its request's proxy (a VPN add-on's)");
}
// Firefox's own proxy settings never cover loopback addresses (allow_hijacking_localhost is off):
// nothing is sent for them, the proxy for anything else.
{
  const settings = { proxyType: "manual", http: "10.0.0.9:3128", httpProxyAll: true, passthrough: "" };
  const { api, calls, listeners } = fakeApi({ firefox: true, proxyGranted: true, proxy: settings });
  api.proxy.settings.get = async () => ({ value: settings });
  load(api, true);
  await settle();
  const urls = ["http://127.0.0.1:8231/a.zip", "http://localhost/b.zip", "http://[::1]/c.zip", "http://127.9.9.9/d.zip", "https://x.test/e.zip"];
  urls.forEach((url, i) => listeners.created({ id: 60 + i, url, filename: `C:\dl\f${i}.zip` }));
  await settle();
  assert.deepEqual(natives(calls).map((m) => m.proxy), [undefined, undefined, undefined, undefined, "http://10.0.0.9:3128"]);
  ok("Firefox: loopback addresses skip its proxy settings' proxy, as Firefox itself does");
}
// A type the app doesn't take: the browser goes on (its dialog shows), MDM's log hears why once,
// and the download after the dialog isn't paused or asked again.
{
  const { api, calls, listeners } = fakeApi({ firefox: true });
  load(api, true);
  await settle();
  const res = await held(listeners, answer("https://dl.test/data.xyz", { "Content-Type": "application/octet-stream", "Content-Disposition": "attachment" }));
  assert.deepEqual(res, {});
  listeners.created({ id: 31, url: "https://dl.test/data.xyz", filename: "C:\\dl\\data.xyz" });
  await settle();
  assert.deepEqual(natives(calls), []);
  assert.deepEqual(notices(calls), [{ why: "file-type", name: "data.xyz", site: "dl.test" }]);
  assert.deepEqual(steps(calls), []);
  ok("#208 Firefox: a type not taken goes on to the browser, said once");
}
// Pages, pictures, feeds, add-ons, redirects, pieces and errors are never held; an inline PDF
// opens when Options don't take the files the browser would open; a type the browser can't show
// is a download even without "attachment".
{
  const { api, calls, listeners } = fakeApi({ firefox: true, rules: { opened: false } });
  load(api, true);
  await settle();
  for (const [url, headers, more] of [
    ["https://x.test/", { "Content-Type": "text/html; charset=utf-8" }],
    ["https://x.test/p.png", { "Content-Type": "image/png" }],
    ["https://x.test/doc.pdf", { "Content-Type": "application/pdf" }],
    ["https://x.test/a.json", { "Content-Type": "application/json" }],
    ["https://x.test/feed", { "Content-Type": "application/rss+xml" }],
    ["https://x.test/addon.xpi", { "Content-Type": "application/x-xpinstall" }],
    ["https://x.test/old.zip", { "Content-Type": "application/zip", Location: "https://x.test/new.zip" }, { statusCode: 302 }],
    ["https://x.test/part.zip", { "Content-Type": "application/zip" }, { statusCode: 206 }],
    ["https://x.test/gone.zip", { "Content-Type": "application/zip" }, { statusCode: 404 }],
    ["https://x.test/untyped", {}],
  ]) {
    assert.deepEqual(await held(listeners, answer(url, headers, more)), {}, url);
  }
  assert.deepEqual(natives(calls), []);
  assert.deepEqual(notices(calls), []);
  assert.deepEqual(await held(listeners, answer("https://x.test/dl/setup.exe", { "Content-Type": "application/x-msdownload", "Content-Length": "10" })), { cancel: true });
  assert.equal(natives(calls)[0].filename, "setup.exe");
  ok("#208 Firefox: pages, pictures, redirects, inline PDFs untouched; an unshowable type taken");
}
// A PDF the browser would show is taken when Options say so; the app saying no lets the browser
// go on.
{
  const taken = fakeApi({ firefox: true });
  load(taken.api, true);
  await settle();
  assert.deepEqual(await held(taken.listeners, answer("https://x.test/doc.pdf", { "Content-Type": "application/pdf" })), { cancel: true });
  const refused = fakeApi({ firefox: true, reply: { ok: false } });
  load(refused.api, true);
  await settle();
  assert.deepEqual(await held(refused.listeners, answer("https://x.test/a.zip", { "Content-Type": "application/zip" })), {});
  assert.equal(natives(refused.calls).length, 1);
  ok("#208 Firefox: a PDF taken when Options say so; the app's no lets the browser go on");
}
// #1087: a PDF a page shows in a frame, an <embed> or an <object> (a course's, from a CDN, at a
// signed address) is taken as one the tab goes to is, with the page as its referrer; the page,
// in a tab just opened, stays. One the page's script fetches is never held: its listener isn't
// asked for those.
{
  const { api, calls, listeners } = fakeApi({ firefox: true });
  load(api, true);
  await settle();
  assert.deepEqual([...listeners.headersReceivedSpec[0].types], ["main_frame", "sub_frame", "object"]);
  assert.deepEqual([...listeners.beforeSendHeadersSpec[0].types], ["main_frame", "sub_frame", "object"]);
  const pdf = { "Content-Type": "application/pdf", "Content-Length": "448712" };
  const signed = "https://d1.cloudfront.test/pro/course/content/01M5.pdf?Expires=1893456000&Signature=a~b__&Key-Pair-Id=K2";
  listeners.tabCreated({ id: 14 });
  for (const [i, type] of ["sub_frame", "object"].entries()) {
    const requestId = `f${i}`;
    listeners.beforeSendHeaders({ requestId, method: "GET", url: signed, type, requestHeaders: [{ name: "Referer", value: "https://course.test/lesson/7" }] });
    const res = await held(listeners, answer(signed, pdf, { requestId, type, tabId: 14, frameId: 5 }));
    assert.deepEqual(res, { cancel: true }, type);
    const m = natives(calls)[i];
    assert.deepEqual([m.url, m.filename, m.size, m.referrer], [signed, "01M5.pdf", 448712, "https://course.test/lesson/7"], type);
  }
  await settle();
  assert.ok(!calls.some((c) => c[0] === "closeTab"), "the page's tab stays");
  ok("#1087 Firefox: a PDF in a frame, an <embed> or an <object> taken, its page stays");
}
// Alt held, Options' excluded sites, a file under the minimum size, a form that can't be posted
// again, paused: the browser keeps it, nothing asked.
{
  const { api, calls, listeners } = fakeApi({ firefox: true, rules: { excludedSites: "*.skip.test", minKb: 100 } });
  load(api, true);
  await settle();
  const send = ask(listeners);
  const zip = { "Content-Type": "application/zip" };
  await send({ type: "bypass", url: "https://x.test/alt.zip" }, { id: "ours", url: "https://x.test/", tab: { url: "https://x.test/" } });
  assert.deepEqual(await held(listeners, answer("https://x.test/alt.zip", zip)), {});
  assert.deepEqual(await held(listeners, answer("https://dl.skip.test/a.zip", zip)), {});
  assert.deepEqual(await held(listeners, answer("https://x.test/small.zip", { ...zip, "Content-Length": "2048" })), {});
  listeners.beforeRequest({ method: "POST", url: "https://x.test/upload", requestId: "p1", requestBody: { raw: [{ file: "C:\\f" }] } });
  const upload = answer("https://x.test/upload", { ...zip, "Content-Disposition": "attachment; filename=r.zip" }, { method: "POST", requestId: "p1" });
  assert.deepEqual(await held(listeners, upload), {});
  assert.deepEqual(natives(calls), []);
  await send({ type: "pause", paused: true }, { id: "ours", url: "chrome-extension://ours/popup.html" });
  assert.deepEqual(await held(listeners, answer("https://x.test/paused.zip", zip)), {});
  assert.deepEqual(natives(calls), []);
  ok("#208 Firefox: Alt, excluded sites, small files, file uploads and pause stay the browser's");
}
// #332: Alt (Option on a Mac) held on a link, in the order each browser's events come. page.js
// says it as the button goes down (pointerdown) and again at the click; the download may come
// before the click's word, under the link's address, its redirect's end, or another address a
// page's script went to. Each is the browser's; one click leaves one such download to it, for ten
// seconds; the link's own address, and a download its own page starts, for a minute (#1494: a
// bot check's page runs its script first).
{
  const clock = { t: Date.now() };
  class FakeDate extends Date {
    static now() {
      return clock.t;
    }
  }
  const page = { id: "ours", url: "https://site.test/files", frameId: 0, tab: { id: 3, url: "https://site.test/files" } };
  // Chrome and Edge on a Mac (and everywhere): Option+click downloads the link itself, at the
  // click; only the press was said by then.
  {
    const { api, calls, listeners } = fakeApi({ firefox: false });
    load(api, false, { Date: FakeDate });
    const send = (m, sender) => new Promise((r) => listeners.message(m, sender, r));
    const kept = async (item) => {
      let suggested = false;
      listeners.determining(item, () => (suggested = true));
      await settle();
      return suggested;
    };
    await send({ type: "bypass", url: "https://site.test/get?id=7" }, page);
    assert.ok(await kept({ id: 60, url: "https://site.test/get?id=7", finalUrl: "https://cdn.test/a.zip", referrer: "https://site.test/files", filename: "a.zip" }), "redirected: the link's own address");
    // The click's word comes after: still the same click, nothing more is left to the browser.
    await send({ type: "bypass", url: "https://site.test/get?id=7" }, page);
    assert.ok(!(await kept({ id: 61, url: "https://site.test/b.zip", referrer: "https://site.test/files", filename: "b.zip" })), "a later plain download from the page");
    assert.equal(natives(calls).length, 1);
    // A link the page's script took (the click's own download prevented): the download it
    // started instead, under another address, sent from the page (the referrer cut to the site
    // too, the default across sites); once.
    calls.length = 0;
    await send({ type: "bypass", url: "https://site.test/dl/9" }, page);
    assert.ok(await kept({ id: 62, url: "https://files.test/token/c.zip", referrer: "https://site.test/", filename: "c.zip" }), "a script's download");
    assert.ok(!(await kept({ id: 63, url: "https://files.test/token/d.zip", referrer: "https://site.test/", filename: "d.zip" })), "only one");
    assert.equal(natives(calls).length, 1);
    // Another site's download meanwhile is not the click's.
    calls.length = 0;
    await send({ type: "bypass", url: "https://site.test/dl/10" }, page);
    assert.ok(!(await kept({ id: 64, url: "https://other.test/e.zip", referrer: "https://other.test/page", filename: "e.zip" })), "another site's");
    // Ten seconds on, a download from the page the link was on is MDM's again; the link's own
    // address stays the browser's for a minute (#1494), then it is forgotten too.
    clock.t += 10001;
    assert.ok(!(await kept({ id: 65, url: "https://files.test/token/f.zip", referrer: "https://site.test/files", filename: "f.zip" })), "the page's, later");
    assert.ok(await kept({ id: 66, url: "https://site.test/dl/10", referrer: "https://site.test/files", filename: "g.zip" }), "the link itself, later");
    clock.t += 50000;
    assert.ok(!(await kept({ id: 67, url: "https://site.test/dl/10", referrer: "https://site.test/files", filename: "h.zip" })), "forgotten");
    assert.equal(natives(calls).length, 3);
    // #1494: a link to a page behind a bot check ("Just a moment..."). page.js followed it: its
    // page opened in the tab, its script ran (half a minute, a box ticked) and loaded the address
    // again, which is the file now: the browser's. Or the check sent to the file elsewhere, from
    // its page (its referrer: the link's address with the check's token added): the browser's,
    // once. A download from the page the link was on that late is MDM's.
    calls.length = 0;
    await send({ type: "bypass", url: "https://site.test/get?id=8" }, page);
    clock.t += 30000;
    assert.ok(await kept({ id: 68, url: "https://site.test/get?id=8", referrer: "https://site.test/files", filename: "i.zip" }), "the link, after its check");
    await send({ type: "bypass", url: "https://site.test/get?id=9" }, page);
    clock.t += 30000;
    assert.ok(!(await kept({ id: 69, url: "https://files.test/j.zip", referrer: "https://site.test/files", filename: "j.zip" })), "the page the link was on, half a minute on");
    assert.ok(!(await kept({ id: 70, url: "https://files.test/k.zip", referrer: "https://site.test/get?id=10&__cf_chl_rt_tk=abc", filename: "k.zip" })), "another link's page");
    assert.ok(await kept({ id: 71, url: "https://files.test/l.zip", referrer: "https://site.test/get?id=9&__cf_chl_rt_tk=abc", filename: "l.zip" }), "the link's page, after its check");
    assert.ok(!(await kept({ id: 72, url: "https://files.test/m.zip", referrer: "https://site.test/get?id=9&__cf_chl_rt_tk=abc", filename: "m.zip" })), "once");
    assert.equal(natives(calls).length, 3);
  }
  // Firefox (on a Mac too): page.js follows the link once the background has it (the browser
  // alone does nothing with an Alt+click); the page load's answer is the download. Straight, by a
  // redirect, or a page whose script goes to the file in the same tab.
  {
    const { api, calls, listeners } = fakeApi({ firefox: true });
    load(api, true, { Date: FakeDate });
    await settle();
    const send = ask(listeners);
    const zip = { "Content-Type": "application/zip" };
    await send({ type: "bypass", url: "https://site.test/a.zip" }, page);
    assert.deepEqual(await held(listeners, answer("https://site.test/a.zip", zip)), {}, "straight");
    await send({ type: "bypass", url: "https://site.test/go?f=b" }, page);
    listeners.beforeRedirect({ url: "https://site.test/go?f=b", redirectUrl: "https://cdn.test/b.zip", type: "main_frame", tabId: 3 });
    assert.deepEqual(await held(listeners, answer("https://cdn.test/b.zip", zip, { requestId: "r2" })), {}, "redirected");
    await send({ type: "bypass", url: "https://site.test/thanks?f=c" }, page);
    assert.deepEqual(await held(listeners, answer("https://cdn.test/c.zip", zip, { requestId: "r3" })), {}, "a script's, same tab");
    assert.deepEqual(natives(calls), []);
    // That click's leave is used: the next download in the tab is MDM's.
    assert.deepEqual(await held(listeners, answer("https://cdn.test/d.zip", zip, { requestId: "r4" })), { cancel: true });
    assert.equal(natives(calls).length, 1);
    // The download Firefox itself makes of it (a save with browser.altClickSave, a script's
    // downloads.download): by its address.
    calls.length = 0;
    await send({ type: "bypass", url: "https://site.test/e.zip" }, page);
    listeners.created({ id: 70, url: "https://site.test/e.zip", filename: "/dl/e.zip", referrer: "https://site.test/files" });
    await settle();
    assert.deepEqual(natives(calls), []);
    assert.deepEqual(steps(calls), []);
  }
  // Safari: Option+click downloads the link itself (nothing to see here); a plain click on another
  // link of that page right after is still offered to the app.
  {
    const { api, calls, listeners } = fakeApi({ firefox: true, safari: true });
    load(api, true, { Date: FakeDate });
    const send = (m) => new Promise((r) => listeners.message(m, page, (v) => r(JSON.parse(JSON.stringify(v)))));
    await send({ type: "bypass", url: "https://site.test/a.zip" });
    assert.deepEqual(await send({ type: "link", url: "https://site.test/b.zip" }), { taken: true });
    assert.deepEqual(await send({ type: "link", url: "https://site.test/a.zip" }), { taken: false }, "the Alt-clicked link itself");
    assert.equal(natives(calls).length, 1);
  }
  ok("#332: Alt / Option+click: the link, its redirect, a script's download after it, once; Chrome, Firefox, Safari orders");
}
// A private window's download goes as one; a tab opened only for it closes with it.
{
  const { api, calls, listeners } = fakeApi({ firefox: true });
  load(api, true);
  await settle();
  listeners.tabCreated({ id: 12 });
  const res = await held(listeners, answer("https://x.test/p.zip", { "Content-Type": "application/zip" }, { incognito: true, tabId: 12 }));
  assert.deepEqual(res, { cancel: true });
  await settle();
  assert.equal(natives(calls)[0].private, true);
  assert.ok(calls.some((c) => c[0] === "closeTab" && c[1] === 12));
  ok("#208 Firefox: private stays private; a tab opened for the download closes");
}
// Chromium has its own pause (onDeterminingFilename): no answer is held there.
{
  const { api, listeners } = fakeApi({ firefox: false });
  load(api, false);
  await settle();
  assert.equal(listeners.headersReceived, undefined);
  ok("#208 Chromium: answers aren't held");
}
// #1345: Telegram Web's downloads come from its service worker, under addresses that are
// nothing outside the page (a 302 to the app): the browser keeps them, from the K and A apps.
for (const [id, url] of [
  [93, "https://web.telegram.org/k/d/1284042609"],
  [94, "https://web.telegram.org/a/progressive/document5123456789"],
]) {
  const { api, calls, listeners } = fakeApi({ firefox: false });
  load(api, false);
  await settle();
  let suggested;
  listeners.determining(
    {
      id,
      url,
      finalUrl: url,
      referrer: "https://web.telegram.org/k/",
      filename: "Episode.mp4",
      totalBytes: 376_637_440,
      mime: "video/mp4",
    },
    (value) => (suggested = value),
  );
  await settle();
  assert.ok(suggested, `the browser keeps Telegram Web's download (${url})`);
  assert.equal(natives(calls).length, 0, "nothing offered to MDM");
}
ok("#1345 Chromium: Telegram Web's service-worker downloads stay in the browser");
// #603: only a tokenized Colab runtime-proxy transfer stays with the browser. A normal
// download linked from a Colab notebook is still offered to MDM.
{
  const { api, calls, listeners } = fakeApi({ firefox: false });
  load(api, false);
  await settle();
  let suggested;
  listeners.determining(
    {
      id: 91,
      url: "https://us-west1-colab.googleusercontent.com/files/out.zip?colab-runtime-proxy-token=secret",
      finalUrl: "https://us-west1-colab.googleusercontent.com/files/out.zip?colab-runtime-proxy-token=secret",
      referrer: "https://colab.research.google.com/drive/abc",
      filename: "out.zip",
      totalBytes: 1024,
      mime: "application/zip",
    },
    (value) => (suggested = value),
  );
  await settle();
  assert.ok(suggested, "the browser keeps the authenticated runtime-proxy download");
  assert.equal(natives(calls).length, 0);
  ok("#603 Chromium: tokenized Colab runtime-proxy download stays in the browser");
}
{
  const { api, calls, listeners } = fakeApi({ firefox: false });
  load(api, false);
  await settle();
  listeners.determining(
    {
      id: 92,
      url: "https://github.com/example/data/archive/refs/heads/main.zip",
      finalUrl: "https://github.com/example/data/archive/refs/heads/main.zip",
      referrer: "https://colab.research.google.com/drive/abc",
      filename: "data.zip",
      totalBytes: 4096,
      mime: "application/zip",
    },
    () => {},
  );
  await settle();
  assert.equal(natives(calls).length, 1, "ordinary notebook links remain capturable");
  ok("#603 Chromium: a GitHub file linked from Colab is still handed to MDM");
}
{
  const { api, calls, listeners } = fakeApi({ firefox: true });
  load(api, true);
  await settle();
  const url = "https://us-west1.prod.r.colab.dev/files/out.zip?colab-runtime-proxy-token=secret";
  listeners.beforeSendHeaders({
    requestId: "r-colab",
    method: "GET",
    type: "main_frame",
    url,
    originUrl: "https://colab.research.google.com/drive/abc",
    requestHeaders: [],
  });
  const response = await held(
    listeners,
    answer(
      url,
      {
        "Content-Type": "application/zip",
        "Content-Disposition": "attachment; filename=out.zip",
        "Content-Length": "1024",
      },
      { requestId: "r-colab" },
    ),
  );
  assert.deepEqual(response, {});
  assert.equal(natives(calls).length, 0, "Firefox leaves the authenticated transfer to the browser");
  ok("#603 Firefox: tokenized Colab runtime-proxy download stays in the browser");
}

// A token alone is not enough: unrelated hosts must still be captured.
{
  const { api, calls, listeners } = fakeApi({ firefox: false });
  load(api, false);
  await settle();
  listeners.determining(
    {
      id: 93,
      url: "https://downloads.example/data.zip?colab-runtime-proxy-token=not-colab",
      finalUrl: "https://downloads.example/data.zip?colab-runtime-proxy-token=not-colab",
      referrer: "https://colab.research.google.com/drive/abc",
      filename: "data.zip",
      totalBytes: 4096,
      mime: "application/zip",
    },
    () => {},
  );
  await settle();
  assert.equal(natives(calls).length, 1, "a token on a non-proxy host must not bypass MDM");
  ok("#603 Chromium: tokenized non-Colab URLs remain capturable");
}
{
  const { api, calls, listeners } = fakeApi({ firefox: false });
  load(api, false);
  await settle();
  listeners.determining(
    {
      id: 94,
      url: "https://prod.r.colab.dev/files/out.zip",
      finalUrl: "https://prod.r.colab.dev/files/out.zip",
      referrer: "https://colab.research.google.com/drive/abc",
      filename: "out.zip",
      totalBytes: 4096,
      mime: "application/zip",
    },
    () => {},
  );
  await settle();
  assert.equal(natives(calls).length, 1, "a proxy host without its token must not bypass MDM");
  ok("#603 Chromium: an un-tokenized Colab host remains capturable");
}

// The manifests (#42): "proxy" optional in Firefox's, required in Chrome's (it can't be optional
// there); the popup's switch: the permission in Firefox, the extension's own setting in Chrome.
{
  const chrome = JSON.parse(readText("../../extension/manifest.json"));
  const firefox = JSON.parse(readText("../../extension/manifest.firefox.json"));
  assert.ok((chrome.permissions || []).includes("proxy") && !(chrome.optional_permissions || []).includes("proxy"), "Chrome: proxy required");
  assert.ok(!(firefox.permissions || []).includes("proxy"), "Firefox: proxy not required");
  assert.deepEqual(firefox.optional_permissions, ["proxy"], "Firefox: proxy optional");
  const popup = readText("../../extension/popup.js");
  assert.ok(/optional_permissions \|\| \[\]\)\.includes\("proxy"\)/.test(popup), "Firefox's switch where proxy is optional");
  assert.ok(/permissions\.request\(PROXY\)/.test(popup) && /permissions\.remove\(PROXY\)/.test(popup), "asked for and taken back on the switch");
  assert.ok(/storage\.local\s*\.get\(\{ proxyOn: false \}\)/.test(popup) && /storage\.local\.set\(\{ proxyOn: box\.checked \}\)/.test(popup), "Chrome's switch: its own setting");
  assert.ok(/const PROXY_SWITCH = "proxyOn"/.test(code), "the same setting in background.js");
  const en = JSON.parse(readText("../../extension/_locales/en/messages.json"));
  assert.ok(en.popup_proxy && en.popup_proxy_title && en.popup_proxy_title_chromium, "the switch's texts");
  ok("manifests: proxy optional in Firefox, required in Chrome; the popup's switch in each");
}
// Another MDM (#157): the file types caught for it are the app's defaults, and every message
// the pairing can end with is there.
{
  const capture = readText("../../crates/ipc/src/capture.rs");
  const app = /pub const DEFAULT_AUTO_TYPES: &str = "([^"]+)"/.exec(capture)[1];
  const ours = /const DEFAULT_TYPES =\s*"([^"]+)"/.exec(code)[1];
  assert.equal(ours, app, "DEFAULT_TYPES is the app's DEFAULT_AUTO_TYPES");
  const english = JSON.parse(readText("../../extension/_locales/en/messages.json"));
  for (const why of ["address", "unreachable", "wrong_code", "no_code", "too_soon", "failed", "device_name"]) {
    assert.ok(english[`remote_${why}`], `remote_${why}`);
  }
  assert.ok(english.status_remote, "status_remote");
  ok("another MDM: the app's file types, every pairing message");
}
// Another MDM (#157): multiple servers, migration from a single remote, switching, forget, and 401 pruning.
{
  const { api, listeners, store } = fakeApi({ firefox: false });
  load(api, false);
  const send = (m) => new Promise((r) => listeners.message(m, { id: "ours" }, (v) => r(JSON.parse(JSON.stringify(v)))));

  // 1. Migration: upgrading from a single legacy `remote` in storage.
  store.remote = { url: "https://server-a.test:8463/", token: "token-a" };
  delete store.remotes;
  const state1 = await send({ type: "state" });
  assert.equal(state1.remote, "https://server-a.test:8463/");
  assert.deepEqual(state1.remotes, ["https://server-a.test:8463/"]);
  assert.deepEqual(store.remotes, [{ url: "https://server-a.test:8463/", token: "token-a" }], "migrated list saved on read");

  // Switching to "This computer" (remote-off) sets active remote to null, but leaves the server in remotes.
  assert.deepEqual(await send({ type: "remote-off" }), { ok: true });
  assert.equal(store.remote, null);
  assert.deepEqual(store.remotes, [{ url: "https://server-a.test:8463/", token: "token-a" }]);
  const state2 = await send({ type: "state" });
  assert.equal(state2.remote, null);
  assert.deepEqual(state2.remotes, ["https://server-a.test:8463/"]);

  // 2. Pair a second server: both are remembered, and the new one becomes active.
  served["https://server-b.test:8463/api/pair"] = { status: 200, body: JSON.stringify({ token: "token-b" }) };
  const paired = await send({ type: "remote-pair", url: "https://server-b.test:8463/", code: "12345678" });
  assert.deepEqual(paired, { ok: true });
  assert.equal(store.remote.url, "https://server-b.test:8463/");
  assert.deepEqual(store.remotes.map((r) => r.url), ["https://server-a.test:8463/", "https://server-b.test:8463/"]);

  // 3. remote-select: switch back to server A.
  assert.deepEqual(await send({ type: "remote-select", url: "https://server-a.test:8463/" }), { ok: true });
  assert.equal(store.remote.url, "https://server-a.test:8463/");
  // An address not in the list is refused.
  assert.deepEqual(await send({ type: "remote-select", url: "https://unknown.test:8463/" }), { ok: false, why: "not-found" });
  assert.equal(store.remote.url, "https://server-a.test:8463/");

  // 4. remote-forget: the token goes only to its own server.
  served["https://server-a.test:8463/api/forget"] = { status: 200, body: JSON.stringify({ ok: true }) };
  served["https://server-b.test:8463/api/forget"] = { status: 200, body: JSON.stringify({ ok: true }) };
  const beforeFetches = fetched.length;
  assert.deepEqual(await send({ type: "remote-forget", url: "https://server-a.test:8463/" }), { ok: true });
  assert.equal(store.remote, null, "active server A cleared on forget");
  assert.deepEqual(store.remotes.map((r) => r.url), ["https://server-b.test:8463/"]);
  const forgetCalls = fetched.slice(beforeFetches).filter(([u]) => u.includes("api/forget"));
  assert.equal(forgetCalls.length, 1);
  assert.ok(forgetCalls[0][0].startsWith("https://server-a.test:8463/"), "token went only to server A");
  assert.equal(served["https://server-a.test:8463/api/forget"].calls?.[0]?.headers?.Authorization, "Bearer token-a");
  assert.equal(served["https://server-b.test:8463/api/forget"].calls, undefined, "server B never received forget");

  // 5. 401 pruning: a 401 from server B removes B and resets active B to null.
  store.remotes = [
    { url: "https://server-a.test:8463/", token: "token-a" },
    { url: "https://server-b.test:8463/", token: "token-b" },
  ];
  store.remote = { url: "https://server-b.test:8463/", token: "token-b" };
  served["https://server-b.test:8463/api/add"] = { status: 401, body: JSON.stringify({ error: "unauthorized" }) };
  listeners.determining({ id: 81, url: "https://dl.test/file.zip", filename: "file.zip" }, () => {});
  await settle();
  assert.deepEqual(store.remotes.map((r) => r.url), ["https://server-a.test:8463/"], "server B pruned on 401");
  assert.equal(store.remote, null, "active server B reset on 401");

  // 6. Race: a late 401 from server A while user already switched to B prunes A but keeps active B.
  store.remotes = [
    { url: "https://server-a.test:8463/", token: "token-a" },
    { url: "https://server-b.test:8463/", token: "token-b" },
  ];
  store.remote = { url: "https://server-a.test:8463/", token: "token-a" };
  let serverACalled;
  const serverACalledPromise = new Promise((resolve) => { serverACalled = resolve; });
  let finishLate401;
  served["https://server-a.test:8463/api/add"] = () => new Promise((resolve) => {
    serverACalled();
    finishLate401 = () => resolve({ status: 401, body: JSON.stringify({ error: "unauthorized" }) });
  });
  listeners.determining({ id: 82, url: "https://dl.test/file2.zip", filename: "file2.zip" }, () => {});
  await serverACalledPromise;
  assert.deepEqual(await send({ type: "remote-select", url: "https://server-b.test:8463/" }), { ok: true });
  assert.equal(store.remote?.url, "https://server-b.test:8463/");
  finishLate401();
  await settle();
  assert.deepEqual(store.remotes.map((r) => r.url), ["https://server-b.test:8463/"], "server A pruned on late 401");
  assert.equal(store.remote?.url, "https://server-b.test:8463/", "active server B kept after late 401 from A");

  // 7. Token mismatch: a late 401 with old token does not prune re-paired server A.
  store.remotes = [{ url: "https://server-a.test:8463/", token: "token-a-old" }];
  store.remote = { url: "https://server-a.test:8463/", token: "token-a-old" };
  let oldCalled;
  const oldCalledPromise = new Promise((resolve) => { oldCalled = resolve; });
  let finishOld401;
  served["https://server-a.test:8463/api/add"] = () => new Promise((resolve) => {
    oldCalled();
    finishOld401 = () => resolve({ status: 401, body: JSON.stringify({ error: "unauthorized" }) });
  });
  listeners.determining({ id: 83, url: "https://dl.test/file3.zip", filename: "file3.zip" }, () => {});
  await oldCalledPromise;
  store.remotes = [{ url: "https://server-a.test:8463/", token: "token-a-new" }];
  store.remote = { url: "https://server-a.test:8463/", token: "token-a-new" };
  finishOld401();
  await settle();
  assert.deepEqual(store.remotes, [{ url: "https://server-a.test:8463/", token: "token-a-new" }], "re-paired server A kept");
  assert.deepEqual(store.remote, { url: "https://server-a.test:8463/", token: "token-a-new" });

  ok("another MDM: multiple servers, migration, switching, forget, and 401 pruning");
}
// Another MDM chosen (#157): only a download becomes one there (its api/add), each once; every
// other message is this computer's MDM's or nobody's, and no remote call. The video button's
// "may it show here" went to api/add too: pointing at videos added the page to the other MDM's
// list every few seconds. With and without an MDM on this computer to ask.
for (const here of [true, false]) {
  const clock = { t: Date.now() };
  class FakeDate extends Date {
    static now() {
      return clock.t;
    }
  }
  const film = "https://tube.test/watch?v=1";
  const tab = { id: 5, index: 2, url: film, title: "A film - Tube", incognito: false };
  const { api, calls, listeners, store } = fakeApi({ firefox: false, host: here, tabs: { 5: tab } });
  load(api, false, { Date: FakeDate });
  await settle();
  const R = "https://server.test:8463/";
  store.remotes = [{ url: R, token: "k" }];
  store.remote = { url: R, token: "k" };
  served = {
    [`${R}api/add`]: { status: 200, body: JSON.stringify({ ok: true }) },
    [`${R}api/act`]: { status: 200, body: JSON.stringify({ done: [4] }) },
  };
  // What reached the other MDM since `from`: [path, what was sent].
  const remoteCalls = (from) =>
    fetched.slice(from).filter(([u]) => u.startsWith(R)).map(([u]) => {
      const path = u.slice(R.length);
      const opts = ((served[u] && served[u].calls) || []).shift();
      const body = opts ? JSON.parse(opts.body) : null;
      return [path, path === "api/add" ? body.url : body];
    });
  const page = { id: "ours", frameId: 0, url: film, tab };
  const popup = { id: "ours", url: "chrome-extension://ours/popup.html" };
  const send = (m, sender) => new Promise((r) => listeners.message(m, sender, (v) => r(JSON.parse(JSON.stringify(v ?? null)))));
  const table = [
    // The video button: its question, its menu's look-ups, its other buttons; none a download.
    ["panel", () => send({ type: "panel" }, page), []],
    ["video-lookup", () => send({ type: "video-lookup", src: "https://cdn.test/v/clip.mp4", url: film }, page), []],
    ["video-poll", () => send({ type: "video-poll", token: "t1" }, page), []],
    ["video-pick", () => send({ type: "video-pick", token: "t1", choice: 0 }, page), []],
    ["video-played", () => send({ type: "video-played", played: { duration: 60 } }, page), []],
    ["show-download", () => send({ type: "show-download", address: "https://cdn.test/v/clip.mp4" }, page), []],
    ["show-site", () => send({ type: "show-site" }, page), []],
    ["options", () => send({ type: "options" }, page), []],
    ["get-mdm", () => send({ type: "get-mdm" }, page), []],
    ["open-post", () => send({ type: "open-post", url: "https://tube.test/p/1" }, page), []],
    ["rules", () => send({ type: "rules" }, page), []],
    ["bypass", () => send({ type: "bypass", url: "https://x.test/a.zip" }, page), []],
    ["drive-frame-pointer", () => send({ type: "drive-frame-pointer" }, page), []],
    // The popup's: its state, its switches, its media list; a download's button goes to the
    // other MDM's own api/act.
    ["state", () => send({ type: "state" }, popup), []],
    ["pause", () => send({ type: "pause", paused: false }, popup), []],
    ["exclude-site", () => send({ type: "exclude-site", host: "tube.test" }, popup), []],
    ["include-site", () => send({ type: "include-site", host: "tube.test" }, popup), []],
    ["site-excluded", () => send({ type: "site-excluded", host: "tube.test" }, popup), []],
    ["open-app", () => send({ type: "open-app" }, popup), []],
    ["open-options", () => send({ type: "open-options" }, popup), []],
    ["media", () => send({ type: "media", tab: 5 }, popup), []],
    ["download-act", () => send({ type: "download-act", act: "pause", id: 4 }, popup), [["api/act", { action: "pause", ids: [4] }]]],
    // Downloads: the button's click (Download video), the browser's own download, the right-click
    // menu's link, its player and a stream's manifest. Each one api/add, of its address.
    ["video", () => send({ type: "video", src: "https://cdn.test/v/clip.mp4", url: film, title: "A film" }, page), [["api/add", film]]],
    ["download", () => (listeners.determining({ id: 91, url: "https://dl.test/f.zip", filename: "f.zip" }, () => {}), settle()), [["api/add", "https://dl.test/f.zip"]]],
    ["menu link", () => listeners.menu({ menuItemId: "mdm-link", linkUrl: "https://dl.test/g.zip", pageUrl: film }, tab), [["api/add", "https://dl.test/g.zip"]]],
    ["menu player", () => listeners.menu({ menuItemId: "mdm-link", mediaType: "video", srcUrl: "blob:https://tube.test/1", frameId: 0, pageUrl: film }, tab), [["api/add", film]]],
    ["menu stream", () => listeners.menu({ menuItemId: "mdm-link", linkUrl: "https://cdn.test/s/a.m3u8", pageUrl: film }, tab), [["api/add", film]]],
  ];
  for (const [kind, act, want] of table) {
    const from = fetched.length;
    await act();
    await settle();
    assert.deepEqual(remoteCalls(from), want, `${kind}, ${here ? "an MDM here too" : "no MDM here"}`);
  }
  // Regression: pointing at videos for a while (the page asks again every few seconds, each
  // answer kept PANEL_KEEP_MS) never adds anything to the other MDM; the button shows, as this
  // computer's Options say, or as they are at first when there's no MDM here to ask.
  clock.t += 6000;
  const from = fetched.length;
  const asked = natives(calls).filter((m) => m.panel).length;
  for (let i = 0; i < 5; i++) {
    assert.deepEqual(await send({ type: "panel" }, page), { on: true, compact: false });
    clock.t += 6000;
  }
  assert.deepEqual(remoteCalls(from), [], "the panel question never reaches the other MDM");
  assert.equal(natives(calls).filter((m) => m.panel).length - asked, here ? 5 : 0, "asked of this computer's MDM, when there is one");
  // A site this computer's Options keep the button off: off with another MDM chosen too.
  if (here) {
    const off = fakeApi({ firefox: false, panelOn: false });
    load(off.api, false);
    await settle();
    Object.assign(off.store, { remotes: [{ url: R, token: "k" }], remote: { url: R, token: "k" } });
    assert.deepEqual(await new Promise((r) => off.listeners.message({ type: "panel" }, page, (v) => r(JSON.parse(JSON.stringify(v))))), { on: false, compact: false });
  }
  // Only the kinds a download is may become one there, and nothing else calls the remote's
  // api/add.
  assert.ok(/const REMOTE_ADD = new Set\(\["file", "video"\]\);/.test(code), "the whitelist");
  assert.equal((code.match(/"api\/add"/g) || []).length, 1, "api/add sent from one place");
  assert.equal((code.match(/remoteSend\(/g) || []).length, 2, "remoteSend called from `download` only");
  ok(`another MDM: only downloads reach its api/add, the button's question never (${here ? "an MDM here too" : "no MDM here"})`);
}
// Back to this computer: the same downloads and questions as before, all to its MDM.
{
  const film = "https://tube.test/watch?v=1";
  const tab = { id: 5, index: 2, url: film, title: "A film - Tube", incognito: false };
  const { api, calls, listeners } = fakeApi({ firefox: false, tabs: { 5: tab } });
  load(api, false);
  await settle();
  const from = fetched.length;
  const page = { id: "ours", frameId: 0, url: film, tab };
  const send = (m) => new Promise((r) => listeners.message(m, page, r));
  await send({ type: "panel" });
  await send({ type: "video", src: "https://cdn.test/v/clip.mp4", url: film, title: "A film" });
  listeners.determining({ id: 92, url: "https://dl.test/f.zip", filename: "f.zip" }, () => {});
  await settle();
  const sent = natives(calls);
  assert.deepEqual(
    sent.map((m) => [m.panel ? "panel" : m.video ? "video" : "file", m.url]),
    [["panel", film], ["video", film], ["file", "https://dl.test/f.zip"]],
  );
  assert.deepEqual(fetched.slice(from).filter(([u]) => /\/api\//.test(u)), [], "nothing to any remote control");
  ok("this computer: the button's question, Download video and a download go to its MDM");
}
// Safari has no downloads API: the extension still starts, and a link page.js holds (a file type
// Options lists) is offered to the app as a browser's download is; the app taking it is told to
// page.js, which lets the click go on when it isn't.
{
  const { api, calls, listeners } = fakeApi({ firefox: true, safari: true });
  load(api, true);
  const send = (m, sender) => new Promise((r) => listeners.message(m, sender, (v) => r(JSON.parse(JSON.stringify(v)))));
  const sender = { id: "ours", frameId: 0, url: "https://page.test/", tab: { id: 3, url: "https://page.test/dl", incognito: false } };
  assert.deepEqual(await send({ type: "link", url: "https://x.test/files/a.zip" }, sender), { taken: true });
  const m = natives(calls)[0];
  assert.deepEqual([m.url, m.filename, m.referrer, m.cookies], ["https://x.test/files/a.zip", "a.zip", "https://page.test/dl", "sid=1"]);
  assert.ok(!("explicit" in m), "Options' rules apply, as for the browser's own downloads");
  // Not a listed type: the click goes on, and MDM's log hears why.
  calls.length = 0;
  assert.deepEqual(await send({ type: "link", url: "https://x.test/page.html" }, sender), { taken: false });
  await settle();
  assert.deepEqual(natives(calls), []);
  assert.deepEqual(notices(calls), [{ why: "file-type", name: "page.html", site: "x.test" }]);
  // A `download` name (page.js sends it) decides the type and names the file.
  calls.length = 0;
  assert.deepEqual(await send({ type: "link", url: "https://x.test/dl?id=4", name: "b.iso" }, sender), { taken: true });
  assert.equal(natives(calls)[0].filename, "b.iso");
  // An address's own name reaches the app readable, not percent-encoded.
  calls.length = 0;
  assert.deepEqual(await send({ type: "link", url: "https://x.test/f/My%20Report.zip?x=1" }, sender), { taken: true });
  assert.equal(natives(calls)[0].filename, "My Report.zip");
  calls.length = 0;
  assert.deepEqual(await send({ type: "link", url: "https://x.test/f/bad%ZZ.zip" }, sender), { taken: true });
  assert.equal(natives(calls)[0].filename, "bad%ZZ.zip", "not percent-encoding: left as it is");
  // page.js's "rules" at a page's start: fresh ones aren't asked for again.
  calls.length = 0;
  assert.deepEqual(await send({ type: "rules" }, sender), { ok: true });
  await settle();
  assert.deepEqual(natives(calls), []);
  // A PDF is the browser's own to open unless Options say to take such files too.
  const closed = fakeApi({ firefox: true, safari: true, rules: { opened: false } });
  load(closed.api, true);
  const closedAsk = (m) => new Promise((r) => closed.listeners.message(m, sender, (v) => r(JSON.parse(JSON.stringify(v)))));
  assert.deepEqual(await closedAsk({ type: "link", url: "https://x.test/d.pdf" }, sender), { taken: false });
  assert.deepEqual(await closedAsk({ type: "link", url: "https://x.test/d.zip" }, sender), { taken: true });
  // The app says no: the click goes on.
  const refused = fakeApi({ firefox: true, safari: true, reply: { ok: false } });
  load(refused.api, true);
  const ask = (m, sender) => new Promise((r) => refused.listeners.message(m, sender, (v) => r(JSON.parse(JSON.stringify(v)))));
  assert.deepEqual(await ask({ type: "link", url: "https://x.test/b.iso" }, sender), { taken: false });
  // Alt held: the browser's own (page.js sends `bypass`).
  await ask({ type: "bypass", url: "https://x.test/c.zip" }, sender);
  assert.deepEqual(await ask({ type: "link", url: "https://x.test/c.zip" }, sender), { taken: false });
  ok("Safari: no downloads API; a held link is offered to the app, else the click goes on");
}

// page.js, in a page: Safari holds a plain click on a link to a listed file type, tells the app,
// and (when it isn't taken) clicks again; Alt and other types are never held.
{
  const pageCode = readText("../../extension/page.js");
  const run = async ({ safari, firefox, taken, types = "ZIP ISO R0*", capture = true }) => {
    const sent = [];
    const handlers = {};
    class Element {
      constructor(href, attrs = {}) {
        Object.assign(this, { href, attrs });
        this.clicks = 0;
      }
      closest() {
        return this;
      }
      getAttribute(n) {
        return this.attrs[n] || "";
      }
      click() {
        this.dispatchEvent(new MouseEvent("click", {}));
      }
      dispatchEvent(ev) {
        this.clicks++;
        this.again = ev;
        const plain = { type: "click", button: 0, isTrusted: false, target: this, preventDefault() {} };
        handlers.click(plain);
        handlers["click:bubble"](plain);
      }
    }
    class MouseEvent {
      constructor(type, init) {
        Object.assign(this, { type }, init);
      }
    }
    const stored = { rules: { types, capture } };
    const api = {
      runtime: {
        id: "ours",
        getManifest: () => (safari ? { browser_specific_settings: { safari: {} } } : {}),
        getURL: (p) => (firefox ? "moz-extension://5d0c/" : safari ? "safari-web-extension://5d0c/" : "chrome-extension://ours/") + p,
        sendMessage: async (m) => (sent.push(JSON.parse(JSON.stringify(m))), m.type === "link" ? { taken } : { ok: true }),
      },
      storage: { local: { get: async () => stored }, onChanged: { addListener() {}, removeListener() {} } },
    };
    vm.runInNewContext(pageCode, {
      browser: api,
      Element,
      MouseEvent,
      URL,
      Promise,
      // The capture listeners by the event's name, the bubbling ones as "name:bubble".
      addEventListener: (n, f, capture) => (handlers[capture ? n : `${n}:bubble`] = f),
      removeEventListener() {},
    });
    await settle();
    // A person's click: the button pressed, then the click going down to the link (capture) and
    // back up to the window (`page`: what the page's own listener does with it on the way).
    const click = (href, extra = {}, attrs, page = () => {}) => {
      const a = new Element(href, attrs);
      const e = { type: "click", button: 0, isTrusted: true, altKey: false, target: a, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; }, ...extra };
      handlers.pointerdown({ ...e, type: "pointerdown" });
      handlers.click(e);
      page(e);
      handlers["click:bubble"](e);
      return { a, e };
    };
    return { click, sent, settle };
  };
  let t = await run({ safari: true, taken: true });
  let { a, e } = t.click("https://x.test/f/a.zip");
  await t.settle();
  assert.ok(e.defaultPrevented && a.clicks === 0);
  // At the page's start it has the rules refreshed when old (Safari's background sees no downloads).
  assert.deepEqual(t.sent, [{ type: "rules" }, { type: "link", url: "https://x.test/f/a.zip" }]);
  // R0* is a wildcard; the type is the last name's, the query is not part of it.
  ({ e } = t.click("https://x.test/f/a.r07?x=1.html"));
  assert.ok(e.defaultPrevented);
  ({ e } = t.click("https://x.test/f/page.html"));
  assert.ok(!e.defaultPrevented, "not a listed type");
  // Alt (Option): never held; Safari's own Option+click (a save of the link) is stopped and the
  // link followed as a plain click once the background has it, not held either (#1494).
  const links = () => t.sent.filter((m) => m.type === "link").length;
  const before = links();
  ({ a, e } = t.click("https://x.test/f/a.zip", { altKey: true }));
  assert.ok(e.defaultPrevented && a.clicks === 0, "Alt: the browser's, not held");
  await t.settle();
  assert.ok(a.clicks === 1 && !a.again.altKey, "Safari: followed as a plain click");
  assert.equal(links(), before, "the click followed isn't held");
  assert.ok(t.sent.slice(-2).every((m) => m.type === "bypass" && m.url === "https://x.test/f/a.zip"));
  ({ e } = t.click("https://x.test/dl?id=4", {}, { download: "b.iso" }));
  assert.ok(e.defaultPrevented, "a download attribute names the type");
  assert.deepEqual(t.sent.at(-1), { type: "link", url: "https://x.test/dl?id=4", name: "b.iso" }, "and is sent with it");
  ({ e } = t.click("https://x.test/f/a.zip", { button: 1 }));
  assert.ok(!e.defaultPrevented, "only the primary button");
  // The app doesn't take it: the same link is clicked again, untrusted, and let through.
  t = await run({ safari: true, taken: false });
  ({ a, e } = t.click("https://x.test/f/a.zip"));
  await t.settle();
  assert.ok(e.defaultPrevented && a.clicks === 1, "clicked again");
  // Cmd held (a new tab): the click let through holds it too.
  ({ a } = t.click("https://x.test/f/a.zip", { metaKey: true, shiftKey: false, ctrlKey: false }));
  await t.settle();
  assert.ok(a.clicks === 1 && a.again.metaKey === true && a.again.bubbles, "Cmd kept");
  assert.equal(t.sent.filter((m) => m.type === "link").length, 2, "the clicks let through are not held");
  // Capture off in Options, or another browser: nothing held.
  t = await run({ safari: true, taken: true, capture: false });
  assert.ok(!t.click("https://x.test/f/a.zip").e.defaultPrevented);
  t = await run({ safari: false, taken: true });
  assert.ok(!t.click("https://x.test/f/a.zip").e.defaultPrevented, "Chrome and Firefox take it from the downloads API");
  // #332, #1494: Alt (Option on a Mac) on a link, in every browser. Chromium would save the link
  // without opening it at the click (a bot check's page, "Just a moment...", saved instead of
  // run), Firefox does nothing with it (or saves it, with browser.altClickSave): the browser's
  // own handling is stopped, and once the background has it the link is followed as a plain
  // click (one click, not trusted, without Alt), so its page opens in the tab and its check runs.
  for (const firefox of [false, true]) {
    const who = firefox ? "Firefox" : "Chromium";
    t = await run({ firefox, taken: true });
    ({ a, e } = t.click("https://x.test/go?f=a", { altKey: true }));
    assert.ok(e.defaultPrevented && a.clicks === 0, `${who}: not before the background has it`);
    await t.settle();
    assert.ok(a.clicks === 1 && !a.again.altKey && a.again.isTrusted !== true, `${who}: followed`);
    assert.deepEqual(t.sent.slice(0, 2), [{ type: "bypass", url: "https://x.test/go?f=a" }, { type: "bypass", url: "https://x.test/go?f=a" }], `${who}: at the press, and the click`);
    assert.ok(t.sent.every((m) => m.type === "bypass" && m.url === "https://x.test/go?f=a"));
    // A link with a `download` name: followed the same way (the browser then downloads it under
    // that name, as a plain click does).
    ({ a, e } = t.click("https://x.test/dl?id=5", { altKey: true }, { download: "c.iso" }));
    await t.settle();
    assert.ok(e.defaultPrevented && a.clicks === 1 && !a.again.altKey, `${who}: a download link`);
    // A link the page's own script took, Alt with Shift, Cmd or Ctrl (a new window, a new tab),
    // the middle button: left as they are (said, so their download stays the browser's).
    ({ a, e } = t.click("https://x.test/go?f=b", { altKey: true }, undefined, (ev) => ev.preventDefault()));
    await t.settle();
    assert.equal(a.clicks, 0, `${who}: the page's script`);
    for (const keys of [{ shiftKey: true }, { metaKey: true }, { ctrlKey: true }, { button: 1 }]) {
      ({ a, e } = t.click("https://x.test/go?f=c", { altKey: true, ...keys }));
      await t.settle();
      assert.ok(!e.defaultPrevented && a.clicks === 0, `${who}: ${JSON.stringify(keys)}`);
      assert.deepEqual(t.sent.at(-1), { type: "bypass", url: "https://x.test/go?f=c" });
    }
    // Without Alt nothing changes.
    const said = t.sent.length;
    ({ a, e } = t.click("https://x.test/f/a.zip"));
    await t.settle();
    assert.ok(!e.defaultPrevented && a.clicks === 0 && t.sent.length === said, `${who}: a plain click`);
  }
  ok("Safari: page.js holds a click on a listed file type, never Alt or other types; #332, #1494: Alt said at the press, the link followed in every browser");
}

// #1092: what the tab played, offered when the page's look-up finds nothing (a feed, a post
// yt-dlp can't read, a site it doesn't know). A file's header (MP4) says its length and its
// tracks; the video under the button is the one whose length is its own, never a feed's next
// video, a picture or a sound alone, nor a guess between several.
const MP4 = (() => {
  const cat = (...parts) => {
    const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
    let at = 0;
    for (const p of parts) {
      out.set(p, at);
      at += p.length;
    }
    return out;
  };
  const u32 = (n) => new Uint8Array([(n >>> 24) & 255, (n >>> 16) & 255, (n >>> 8) & 255, n & 255]);
  const text = (s) => new TextEncoder().encode(s);
  const box = (type, ...parts) => {
    const body = cat(...parts);
    return cat(u32(8 + body.length), text(type), body);
  };
  const zeros = (n) => new Uint8Array(n);
  // A movie of `seconds` (timescale 1000; 0: unknown, as a fragmented file's mvhd may say), its
  // tracks ("vide" with a picture `height`, "soun"), `pad` bytes of sample tables in each track
  // (a long file's header), the movie after its data when `atEnd`, a fragmented one's length in
  // its mehd (`fragments`).
  return ({ seconds, tracks = ["soun", "vide"], height = 720, pad = 100, atEnd = false, fragments = 0, data = 2000 }) => {
    const mvhd = box("mvhd", zeros(4), zeros(8), u32(1000), u32(Math.round(seconds * 1000)), zeros(80));
    const trak = (kind) =>
      box(
        "trak",
        box("tkhd", zeros(4), zeros(72), u32(kind === "vide" ? (height * 2) << 15 : 0), u32(kind === "vide" ? height << 16 : 0)),
        box("mdia", box("mdhd", zeros(24)), box("hdlr", zeros(8), text(kind), zeros(13)), box("minf", zeros(pad))),
      );
    const mvex = fragments ? [box("mvex", box("mehd", zeros(4), u32(fragments * 1000)))] : [];
    const moov = box("moov", mvhd, ...tracks.map(trak), ...mvex);
    const ftyp = box("ftyp", text("isomiso2avc1mp41"));
    const mdat = box("mdat", zeros(data));
    return atEnd ? cat(ftyp, mdat, moov) : cat(ftyp, moov, mdat);
  };
})();
{
  const ctx = load(fakeApi({ firefox: false }).api, false);
  // The header, read from bytes in memory as the background reads a file's (a few small reads).
  const head = async (bytes) => {
    const reads = [];
    const info = await ctx.mp4Head(async (start, len) => {
      reads.push(start);
      return start < bytes.length ? bytes.slice(start, start + len) : null;
    });
    return { info: info && JSON.parse(JSON.stringify(info)), reads };
  };
  let h = await head(MP4({ seconds: 10.007 }));
  assert.deepEqual(h.info, { duration: 10.007, video: true, audio: true, height: 720 });
  assert.equal(h.reads.length, 1, "a short file's header in one read");
  // A long file: its first track's tables are large, the second track is read where it starts.
  h = await head(MP4({ seconds: 1501.8, height: 1080, pad: 300_000 }));
  assert.deepEqual(h.info, { duration: 1501.8, video: true, audio: true, height: 1080 });
  assert.ok(h.reads.length >= 2 && h.reads.length <= 6, `${h.reads.length} reads`);
  // The movie after its data (not made for the web): found past it.
  h = await head(MP4({ seconds: 33, atEnd: true, data: 5_000_000 }));
  assert.deepEqual([h.info.duration, h.info.video, h.info.audio], [33, true, true]);
  // One of a video's tracks alone (Douyin's long videos: a picture and a sound, apart).
  assert.deepEqual((await head(MP4({ seconds: 796.5, tracks: ["vide"] }))).info, { duration: 796.5, video: true, audio: false, height: 720 });
  assert.deepEqual((await head(MP4({ seconds: 796.5, tracks: ["soun"] }))).info, { duration: 796.5, video: false, audio: true, height: 0 });
  // A fragmented file: its length in the mehd.
  assert.equal((await head(MP4({ seconds: 0, fragments: 42 }))).info.duration, 42);
  // Tracks past the reads it may make: not known (never a picture alone by mistake).
  h = await head(MP4({ seconds: 60, tracks: ["soun", "soun", "soun", "soun", "soun", "soun", "vide"], pad: 50_000 }));
  assert.deepEqual([h.info.duration, h.info.video, h.info.audio], [60, null, null]);
  // Not an MP4: a WebM, a page, nothing.
  assert.equal((await head(new Uint8Array([0x1a, 0x45, 0xdf, 0xa3, 0x9f, 0x42, 0x86, 0x81, 1, 0x42, 0xf7, 0x81]))).info, null);
  assert.equal((await head(new TextEncoder().encode("<!doctype html><title>x</title>"))).info, null);
  assert.equal((await head(new Uint8Array(0))).info, null);
  ok("#1092: an MP4's header read for its length, its tracks and its picture's height, in a few small reads");

  // The choice (playedChoice), each candidate as playedCandidates makes it.
  const file = (url, duration, more = {}) => ({
    url,
    kind: "video",
    stream: false,
    at: 0,
    size: 2e6,
    duration,
    live: false,
    tracks: { video: true, audio: true },
    height: 720,
    drm: false,
    ...more,
  });
  const stream = (url, duration, more = {}) => ({ ...file(url, duration), kind: "hls", stream: true, size: 0, tracks: null, ...more });
  const choose = (cands, v) => {
    const r = JSON.parse(JSON.stringify(ctx.playedChoice(cands, { duration: 0, live: false, height: 720, audio: false, began: 0, nextBegan: 0, alone: false, ...v })));
    return r.pick ? r.pick.url : r.several ? `several ${r.several}` : "none";
  };
  // Douyin's feed: the video playing (10 s), the next one preloaded (5.6 s), the page's own clip.
  const feed = [file("https://v.test/clip-a/", 10.007), file("https://v.test/clip-b/", 5.617), file("https://s.test/bg.mp4", 2.6, { size: 199164 })];
  assert.equal(choose(feed, { duration: 10.167 }), "https://v.test/clip-a/", "its length (the player says a little more)");
  assert.equal(choose(feed, { duration: 5.757 }), "https://v.test/clip-b/");
  assert.equal(choose(feed, { duration: 31 }), "none", "nothing of its length");
  // A picture and a sound apart: neither is the video.
  const split = [
    file("https://v.test/a/media-video-hvc1/", 796.5, { tracks: { video: true, audio: false } }),
    file("https://v.test/a/media-audio-und-mp4a/", 796.5, { tracks: { video: false, audio: true }, height: 0 }),
  ];
  assert.equal(choose(split, { duration: 796.5 }), "none");
  assert.equal(choose([...split, ...feed], { duration: 10 }), "https://v.test/clip-a/");
  // Two of its length: the one with the player's picture height, else said, never guessed.
  const two = [file("https://v.test/hd.mp4", 30, { height: 1080 }), file("https://v.test/sd.mp4", 30, { height: 480 })];
  assert.equal(choose(two, { duration: 30, height: 480 }), "https://v.test/sd.mp4");
  assert.equal(choose(two, { duration: 30, height: 720 }), "several 2");
  // A stream of its length is the video, its files beside it its pieces; a protected one never.
  assert.equal(choose([stream("https://v.test/m.m3u8", 12), file("https://v.test/init.mp4", 12)], { duration: 12 }), "https://v.test/m.m3u8");
  assert.equal(choose([stream("https://v.test/m.m3u8", 12, { drm: true })], { duration: 12 }), "none");
  // A live video: a live stream; never a file.
  const live = stream("https://v.test/live.m3u8", 0, { live: true });
  assert.equal(choose([live, file("https://v.test/x.mp4", 30)], { live: true }), "https://v.test/live.m3u8");
  assert.equal(choose([live], { duration: 30 }), "none");
  // A sound player: a sound.
  const m4a = file("https://v.test/x.m4a", 30, { kind: "audio", tracks: { video: false, audio: true } });
  assert.equal(choose([file("https://v.test/x.mp4", 30), m4a], { duration: 30, audio: true }), "https://v.test/x.m4a");
  // Lengths not known (a WebM, a manifest the extension can't read): the frame's only video's,
  // or what came while this video loaded before the next one began to; else said.
  const webm = (url, at) => file(url, 0, { at, tracks: null });
  assert.equal(choose([webm("https://v.test/a.webm", 5000)], { duration: 30, alone: true }), "https://v.test/a.webm");
  const loaded = [webm("https://v.test/a.webm", 5000), webm("https://v.test/b.webm", 9000)];
  assert.equal(choose(loaded, { duration: 30, began: 4000, nextBegan: 8000 }), "https://v.test/a.webm");
  assert.equal(choose(loaded, { duration: 30, began: 8000 }), "https://v.test/b.webm");
  assert.equal(choose(loaded, { duration: 30 }), "several 2", "when it began isn't known");
  assert.equal(choose(loaded, { duration: 30, began: 1000, nextBegan: 2000 }), "several 2");
  // A known length beats an unknown one.
  assert.equal(choose([...loaded, file("https://v.test/c.mp4", 30)], { duration: 30, alone: true }), "https://v.test/c.mp4");
  ok("#1092: what the tab played is offered for the video of its length, never the next one, a track alone or a guess");
}

// #1092, from the button's question to the app: a feed's player fetched files in ranges (kept
// apart, never in the media list), the button asks what the tab played for its video, the file of
// its length goes to the app as the video's own (its page as Referer, its title, its type); a
// stream goes to the look-up instead of the feed. An address the tab didn't load is never taken.
{
  const A = "https://v3.douyinvod.test/video/tos/cn/a/?mime_type=video_mp4&sig=1";
  const B = "https://v3.douyinvod.test/video/tos/cn/b/?mime_type=video_mp4&sig=2";
  const V = "https://v3.douyinvod.test/video/tos/cn/c/media-video-hvc1/?sig=3";
  const S = "https://v3.douyinvod.test/video/tos/cn/c/media-audio-und-mp4a/?sig=4";
  const M = "https://cdn.test/f/master.m3u8";
  const M720 = "https://cdn.test/f/720p/index.m3u8";
  served = {
    [A]: { bytes: MP4({ seconds: 10.007, data: 300_000 }) },
    [B]: { bytes: MP4({ seconds: 5.617, data: 200_000 }) },
    [V]: { bytes: MP4({ seconds: 796.5, tracks: ["vide"] }) },
    [S]: { bytes: MP4({ seconds: 796.5, tracks: ["soun"] }) },
    [M]: { body: "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1280000,RESOLUTION=1280x720\n720p/index.m3u8\n" },
    [M720]: { body: "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4.0,\na.ts\n#EXTINF:4.0,\nb.ts\n#EXTINF:4.0,\nc.ts\n#EXT-X-ENDLIST\n" },
  };
  const page = "https://www.douyin.test/?recommend=1";
  const tabs = { 4: { id: 4, url: page, title: "抖音-记录美好生活", incognito: false } };
  const { api, calls, listeners } = fakeApi({
    firefox: false,
    tabs,
    rules: { videoLookup: true },
    lookup: () => ({ ok: true, lookup: { state: "pending", token: "t9" } }),
  });
  load(api, false);
  listeners.tabUpdated(4, { url: page });
  await settle();
  const ranged = (url, total) => res(url, "xmlhttprequest", H("video/mp4", { "Content-Range": `bytes 0-1023/${total}` }), { statusCode: 206, tabId: 4 });
  for (const d of [
    ranged(A, 2304964),
    ranged(B, 1981590),
    ranged(V, 76408983),
    ranged(S, 19271821),
    // Too small to be a video's file; a piece named by its address.
    ranged("https://v3.douyinvod.test/tiny.mp4", 40000),
    res("https://v3.douyinvod.test/x.mp4?range=0-99", "xmlhttprequest", H("video/mp4", { "Content-Range": "bytes 0-99/900000" }), { statusCode: 206, tabId: 4 }),
  ]) {
    listeners.responseStarted(d);
  }
  await settle();
  assert.deepEqual((await popupAsk(listeners, { type: "media", tab: 4 })).media, [], "never in the media list");
  const message = (m, frameId = 0) =>
    new Promise((r) => listeners.message(m, { id: "ours", url: page, frameId, tab: tabs[4] }, (x) => r(JSON.parse(JSON.stringify(x)))));
  fetched.length = 0;
  const answer = await message({ type: "video-played", duration: 10.167, height: 720, began: 0, nextBegan: 0, alone: false });
  assert.deepEqual(answer, { played: { url: A, stream: false, ext: "mp4", size: 2304964, height: 720, duration: 10.007 } });
  assert.ok(
    fetched.every(([, c, r]) => c === "include" && /^bytes=\d+-\d+$/.test(r)),
    "each header read in ranges, with the site's cookies",
  );
  assert.ok(fetched.length <= 8, `${fetched.length} reads`);
  assert.deepEqual(served[A].ranges, ["bytes=0-4095"]);
  // Asked again: each header read once.
  const reads = fetched.length;
  assert.deepEqual(await message({ type: "video-played", duration: 5.7, height: 720 }), {
    played: { url: B, stream: false, ext: "mp4", size: 1981590, height: 720, duration: 5.617 },
  });
  assert.equal(fetched.length, reads);
  // The feed moving to another address of the site (Douyin's /jingxuan to /?recommend=1, after its
  // player fetched the first video): its files stay; another site's page: they go.
  listeners.tabUpdated(4, { url: "https://www.douyin.test/jingxuan?from=feed" });
  await settle();
  assert.equal((await message({ type: "video-played", duration: 10.167, height: 720 })).played.url, A);
  listeners.tabUpdated(4, { url: page });
  await settle();
  // The long video, a picture and a sound apart: nothing (its post, if any, is the way).
  assert.deepEqual(await message({ type: "video-played", duration: 796.5, height: 1080 }), {});
  // Another frame played none of these.
  assert.deepEqual(await message({ type: "video-played", duration: 10.167, height: 720 }, 3), {});
  // The click: the file as the video's own, the feed as its page and Referer, the page's title,
  // the type the server said.
  assert.equal((await message({ type: "video", url: null, src: A, played: A, direct: true, title: "抖音-记录美好生活" })).ok, true);
  const sent = natives(calls).filter((x) => x.video).at(-1);
  assert.deepEqual([sent.url, sent.src, sent.direct, sent.mime, sent.title, sent.referrer], [page, A, true, "video/mp4", "抖音-记录美好生活", ""]);
  // A post's (Douyin's /video/<id>, #707 names it): the same file, with its title.
  const post = "https://www.douyin.test/video/7677883232318323279";
  await message({ type: "video", url: post, src: A, played: A, direct: true, title: "抖音" });
  const fromPost = natives(calls).filter((x) => x.video).at(-1);
  assert.deepEqual([fromPost.src, fromPost.direct, fromPost.title], [A, true, "抖音"]);
  // A stream the frame played: its master stands for its variants and goes to the look-up.
  listeners.responseStarted(res(M, "xmlhttprequest", H("application/vnd.apple.mpegurl"), { tabId: 4, frameId: 2 }));
  listeners.responseStarted(res(M720, "xmlhttprequest", H("application/vnd.apple.mpegurl"), { tabId: 4, frameId: 2 }));
  await settle();
  assert.deepEqual(await message({ type: "video-played", duration: 12, height: 720 }, 2), {
    played: { url: M, stream: true, ext: "", size: 0, height: 720, duration: 12 },
  });
  const looked = await message({ type: "video-lookup", url: null, src: "", played: M, title: "Feed" }, 2);
  assert.deepEqual(looked, { lookup: { state: "pending", token: "t9" }, show: M });
  // Named after the tab, as a frame's player is (#1075), not after its frame's own title.
  assert.deepEqual(natives(calls).filter((x) => x.videoLookup).at(-1).videoLookup, { page, src: M, title: "抖音-记录美好生活" });
  // Without it (or with an address the tab didn't load, or another frame's): the feed, as before.
  for (const [played, frame] of [
    [undefined, 2],
    ["https://evil.test/x.m3u8", 2],
    [M, 0],
  ]) {
    assert.deepEqual(await message({ type: "video-lookup", url: null, src: "", played, title: "Feed" }, frame), { ok: false, why: "feed" }, String(played));
  }
  listeners.tabUpdated(4, { url: "https://elsewhere.test/" });
  await settle();
  assert.deepEqual(await message({ type: "video-played", duration: 10.167, height: 720 }), {}, "another site: its files gone");
  ok("#1092: a feed's file of the video's length goes as its own (kept while the tab stays on its site), its stream to the look-up; ranged files never listed");
}

// #1092: Douyin names its posts (a slide's data-e2e-vid, ?modal_id=), and the button asks what the
// tab played only after a look-up found nothing.
{
  const source = readText("../../extension/video.js");
  const start = source.indexOf("  function playedWanted(");
  const playedWanted = new Function(`${source.slice(start, source.indexOf("\n  }\n", start) + 4)}\nreturn playedWanted;`)();
  for (const s of [{ state: "feed" }, { state: "failed", reason: "error" }, { state: "failed", reason: "no-ytdlp" }, { state: "ready", choices: [] }, { state: "failed", reason: "no-choices" }]) {
    assert.equal(playedWanted(s), true, JSON.stringify(s));
  }
  for (const s of [{ state: "ready", choices: [{ label: "720p" }] }, { state: "list" }, { state: "busy" }, { state: "off" }, { state: "late" }, { state: "unnamed" }, { state: "use-window" }, { state: "gone" }, null]) {
    assert.equal(playedWanted(s), false, JSON.stringify(s));
  }
  // Douyin's own pages, and one opened over a profile's (videoPage).
  const named = (href) =>
    videoPageOn(new URL(href), { querySelector: () => null, querySelectorAll: () => [] })({ parentElement: null, getAttribute: () => null, querySelectorAll: () => [] });
  assert.equal(named("https://www.douyin.com/video/7692732464107719962"), "https://www.douyin.com/video/7692732464107719962");
  assert.equal(
    named("https://www.douyin.com/user/self?modal_id=7704568510928004170&showSubTab=video&showTab=favorite_collection"),
    "https://www.douyin.com/video/7704568510928004170",
  );
  assert.equal(named("https://www.douyin.com/?recommend=1"), null);
  assert.equal(named("https://www.douyin.com/jingxuan?modal_id=12"), null);
  ok("#1092: Douyin's posts named; what the tab played asked only after a look-up found nothing");
}

// ---- #1496: the popup's Downloads tab and the toolbar button's count ------------------------

// The popup's shared pieces (popup-ui.js) in English: what a row says, which buttons it has, a
// name cut in its middle.
{
  const en = JSON.parse(readText("../../extension/_locales/en/messages.json"));
  const getMessage = (key, subs = []) => {
    const m = en[key];
    if (!m) return "";
    let text = m.message;
    for (const [name, p] of Object.entries(m.placeholders || {})) text = text.replaceAll(`$${name}$`, subs[Number(p.content.slice(1)) - 1] ?? "");
    return text;
  };
  const ctx = { chrome: { i18n: { getMessage, getUILanguage: () => "en-US" } }, console: quietConsole, URL };
  vm.runInNewContext(readText("../../extension/popup-ui.js"), ctx);
  const MB = 2 ** 20;
  const now = Date.UTC(2026, 10, 19, 12, 0, 0);
  const st = (d) => JSON.parse(JSON.stringify(ctx.statusOf(d, now, "en-US")));
  assert.deepEqual(st({ status: "Downloading", size: 48 * MB, downloaded: 12 * MB, speed: 2 * MB, left: 42 }), {
    text: "12 MB of 48 MB · 2 MB/s · 0:42 left",
    tone: "",
    bar: 0.25,
    busy: false,
  });
  // No size: how much came, a bar that can't measure; another MDM's row without `left`: from its speed.
  assert.deepEqual(st({ status: "Downloading", downloaded: 5 * MB, speed: MB }), { text: "5 MB · 1 MB/s", tone: "", bar: null, busy: true });
  assert.equal(st({ status: "Downloading", size: 10 * MB, downloaded: 4 * MB, speed: MB }).text, "4 MB of 10 MB · 1 MB/s · 0:06 left");
  assert.equal(st({ status: "Downloading", downloaded: 0, speed: 0 }).text, "Starting...");
  assert.equal(st({ status: "Stopped", size: 40 * MB, downloaded: 10 * MB }).text, "Paused · 25% · 10 MB of 40 MB");
  assert.equal(st({ status: "Queued", size: 2 ** 30, downloaded: 0 }).text, "Waiting · 1 GB");
  const done = (ago) => st({ status: "Complete", size: 48.7 * MB, downloaded: 48.7 * MB, exists: true, finished: (now - ago) / 1000 });
  assert.deepEqual(done(10e3), { text: "48.7 MB · Done", tone: "ok" });
  assert.deepEqual(done(2 * 3600e3), { text: "48.7 MB · 2 hours ago", tone: "" });
  assert.equal(done(26 * 3600e3).text, "48.7 MB · yesterday");
  assert.deepEqual(st({ status: "Complete", size: 1, exists: false }), { text: "File deleted or moved", tone: "muted" });
  assert.deepEqual(st({ status: "Error", error: "the server refused the connection" }), { text: "Failed: the server refused the connection", tone: "bad" });
  assert.deepEqual(st({ status: "Error" }), { text: "Failed", tone: "bad" });
  // The buttons: this computer's file, another MDM's (nothing that needs the file here), running, gone, failed.
  const on = (a) => Object.entries(a).filter(([, v]) => v).map(([k]) => k).sort();
  const d = { status: "Complete", exists: true, url: "https://files.example.com/a.zip", redownload: true };
  assert.deepEqual(on(ctx.actionsOf(d, { local: true, share: true })), ["again", "copyFile", "copyLink", "del", "folder", "open", "remove", "share"]);
  assert.deepEqual(on(ctx.actionsOf(d, { local: true, share: false })), ["again", "copyFile", "copyLink", "del", "folder", "open", "remove"], "no share sheet (Linux)");
  assert.deepEqual(on(ctx.actionsOf(d, { local: false, share: true })), ["copyLink", "remove"], "another MDM's: nothing that needs the file here");
  assert.deepEqual(on(ctx.actionsOf({ ...d, status: "Downloading", redownload: false }, { local: true })), ["copyLink", "folder", "pause"]);
  assert.deepEqual(on(ctx.actionsOf({ ...d, exists: false }, { local: true, share: true })), ["again", "copyLink", "folder", "remove"]);
  assert.deepEqual(on(ctx.actionsOf({ ...d, status: "Error", redownload: false }, { local: false })), ["copyLink", "remove", "resume"]);
  assert.ok(!ctx.actionsOf({ ...d, url: "javascript:alert(1)" }).copyLink, "only a web, FTP or magnet address is copied");
  // A long name keeps its start and its extension.
  const fits = (n) => (text) => Array.from(text).length <= n;
  const long = "HushGram-automated-release-workflow (4).zip";
  const cut = ctx.middleCut(long, fits(24));
  assert.ok(Array.from(cut).length <= 24 && cut.startsWith("HushGram") && cut.endsWith("(4).zip") && cut.includes("…"), cut);
  assert.equal(ctx.middleCut("short.zip", fits(24)), "short.zip");
  assert.ok(ctx.middleCut("abcdefghijklmnopqrstuvwxyz", fits(10)).length <= 10);
  // Sizes and speeds in the browser's language.
  assert.equal(ctx.sizeText(1536, "en-US"), "1.5 kB");
  assert.equal(ctx.speedText(3 * MB, "de"), "3 MB/s");
  assert.equal(ctx.clockText(3725), "1:02:05");
  assert.equal(ctx.actWhyText("missing"), "Its file is no longer where it was saved (moved or deleted).");
  assert.equal(ctx.actWhyText("odd"), "That didn't work. Try again.");
  ok("#1496 the popup's rows: status lines, buttons, cut names");
}

// Another MDM's list cut to the popup's view as the app cuts its own (mdm_ipc RecentAnswer::of,
// its test recent_view_filters_counts_and_limits: the same downloads, the same answers).
{
  const { api } = fakeApi({ firefox: false });
  const ctx = load(api, false);
  const row = (id, file, status) => ({ id, file, url: `https://files.example.com/${file}`, status });
  const all = [
    row(1, "Report final.pdf", "Complete"),
    row(2, "report draft.docx", "Error"),
    row(3, "holiday.mp4", "Downloading"),
    row(4, "report-2.zip", "Downloading"),
    row(5, "notes.txt", "Stopped"),
    row(6, "report 3.pdf", "Complete"),
  ];
  const view = (filter, search, limit) => JSON.parse(JSON.stringify(ctx.recentView(all, { filter, search, limit }, 9)));
  const ids = (v) => v.downloads.map((x) => x.id);
  const every = view("all", "", 50);
  assert.deepEqual(ids(every), [6, 5, 4, 3, 2, 1]);
  assert.deepEqual(every.counts, { all: 6, running: 2, done: 2, failed: 1 });
  assert.deepEqual([every.running, every.speed], [2, 9]);
  const reports = view("done", "REPORT", 50);
  assert.deepEqual(ids(reports), [6, 1]);
  assert.deepEqual(reports.counts, { all: 4, running: 1, done: 2, failed: 1 });
  assert.equal(reports.running, 2);
  assert.deepEqual(ids(view("all", "report files.example", 50)), [6, 4, 2, 1]);
  assert.deepEqual(ids(view("all", "report mp4", 50)), []);
  assert.deepEqual(ids(view("running", "", 50)), [4, 3]);
  assert.deepEqual(ids(view("failed", "", 50)), [2]);
  assert.deepEqual(ids(view("paused", "", 2)), [6, 5]);
  assert.deepEqual(ids(view("all", "", 0)), []);
  const many = Array.from({ length: 500 }, (_, i) => row(i + 1, "a.bin", "Complete"));
  const capped = ctx.recentView(many, { filter: "all", search: "", limit: 10000 });
  assert.deepEqual([capped.downloads.length, capped.downloads[0].id], [200, 500]);
  // The same cases as the app's own test.
  const rust = readText("../../crates/ipc/src/api.rs");
  assert.ok(rust.includes('row(4, "report-2.zip", "Downloading")') && rust.includes('ask("done", "REPORT", 50)'), "the app's test has the same cases");
  ok("#1496 another MDM's list: the same view as the app's");
}

// A popup's port (as popup-downloads.js opens it): what it is sent, and what it can do.
function popupPort(sender = { id: "ours", url: "chrome-extension://ours/popup.html" }) {
  const posted = [];
  const heard = [];
  const ended = [];
  return {
    posted,
    port: {
      name: "recent",
      sender,
      onMessage: { addListener: (f) => heard.push(f) },
      onDisconnect: { addListener: (f) => ended.push(f) },
      postMessage: (m) => posted.push(JSON.parse(JSON.stringify(m))),
    },
    send: (m) => heard.forEach((f) => f(m)),
    close: () => ended.forEach((f) => f()),
  };
}
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const until = async (what, ms, check) => {
  const end = Date.now() + ms;
  for (;;) {
    const got = await check();
    if (got) return got;
    if (Date.now() > end) throw new Error(`waited for ${what}`);
    await wait(20);
  }
};

// This computer's MDM: one native port for the list (not a helper per question), the file type
// icons sent once, the toolbar button's count while something runs, the buttons' messages; a
// page's script can't ask; the port closes once nothing runs and the popup is closed.
{
  let running = 2;
  const list = (m) => ({
    downloads: m.recent.limit
      ? [{ id: 3, file: "a.zip", url: "https://files.example.com/a.zip", path: "C:\\dl\\a.zip", status: "Complete", size: 5, downloaded: 5, speed: 0, exists: true, icon: "zip" }]
      : [],
    counts: { all: 1, running: 0, done: 1, failed: 0 },
    running,
    speed: 0,
    icons: m.recent.have.includes("zip") ? {} : { zip: "data:image/png;base64,iVBORw0KGgo=" },
    share: true,
  });
  const reply = (m) => (m.recent ? { ok: true, recent: list(m) } : m.downloadAct ? { ok: true, act: { done: true } } : { ok: true });
  const { api, calls, listeners } = fakeApi({ firefox: false, reply, nativePort: true });
  load(api, false);
  await settle();
  const badges = () => calls.filter((c) => c[0] === "badge").map((c) => c[1]);
  const recents = () => calls.filter((c) => c[0] === "native" && c[2].recent).map((c) => c[2].recent);
  // A download handed over: the count is watched.
  listeners.determining({ id: 70, url: "https://x/big.zip", filename: "big.zip" }, () => {});
  await until("the count", 3000, () => badges().includes("2"));
  assert.equal(recents()[0].limit, 0, "the badge asks for counts only");
  // The popup's port: the list, the icons once.
  const p = popupPort();
  listeners.connect(p.port);
  p.send({ type: "ask", ask: { filter: "done", search: "a", limit: 60, px: 48 } });
  await until("the popup's list", 3000, () => p.posted.length);
  const first = p.posted[0];
  assert.deepEqual([first.type, first.ok, first.remote, first.list.downloads[0].file, first.px], ["list", true, null, "a.zip", 48]);
  assert.equal(first.icons.zip, "data:image/png;base64,iVBORw0KGgo=");
  const asked = recents().at(-1);
  assert.deepEqual([asked.filter, asked.search, asked.limit, asked.px], ["done", "a", 60, 48]);
  await until("the second answer", 3000, () => p.posted.length >= 2);
  assert.deepEqual(p.posted[1].icons, {}, "an icon is sent once");
  assert.deepEqual(recents().at(-1).have, ["zip"], "the app is told which icons are here");
  assert.equal(calls.filter((c) => c[0] === "port-open").length, 1, "one helper for all the questions");
  // A page's script can't open that port.
  const page = popupPort({ id: "ours", url: "https://evil.test/", tab: { id: 4 } });
  listeners.connect(page.port);
  page.send({ type: "ask", ask: { limit: 60 } });
  // The buttons.
  assert.deepEqual(await popupAsk(listeners, { type: "download-act", act: "folder", id: 3 }), { ok: true, why: null });
  assert.deepEqual(calls.filter((c) => c[0] === "native" && c[2].downloadAct).map((c) => c[2].downloadAct), [{ act: "folder", id: 3 }]);
  await wait(1100);
  assert.equal(page.posted.length, 0, "never answered");
  // Nothing runs any more, and the popup closes: the count goes, the helper's port closes.
  running = 0;
  p.close();
  await until("the count gone", 6000, () => badges().at(-1) === "");
  await until("the port closed", 6000, () => calls.some((c) => c[0] === "port-closed"));
  ok("#1496 this computer's list: one port, icons once, the count, the buttons; never for a page");
}

// MDM not running, an older helper (which refuses the question), a button that can't: said so,
// and the browser still counts as connected.
{
  const replies = [{ ok: false, appState: "off" }, { ok: false, why: "refused", error: "not an http(s) or ftp(s) URL" }];
  const reply = (m) => (m.recent ? replies[0] : m.downloadAct ? { ok: false, act: { done: false, why: "missing" } } : { ok: true });
  const { api, store, listeners } = fakeApi({ firefox: false, reply, nativePort: true });
  load(api, false);
  await settle();
  const p = popupPort();
  listeners.connect(p.port);
  await until("the popup's word", 3000, () => p.posted.length);
  assert.deepEqual([p.posted[0].ok, p.posted[0].appState, p.posted[0].why], [false, "off", null]);
  replies.shift();
  await until("an older helper's word", 3000, () => p.posted.some((x) => x.appState === "old"));
  assert.equal(store.status && store.status.why, null, "still connected");
  assert.deepEqual(await popupAsk(listeners, { type: "download-act", act: "copy-file", id: 3 }), { ok: false, why: "missing" });
  p.close();
  ok("#1496 MDM not running, an older helper, a button refused: said, still connected");
}

// Another MDM (#157): its remote control's list cut to the view, no icons, nothing that needs
// the file here; pause, resume and remove go to its API.
{
  const { api, listeners } = fakeApi({ firefox: false, nativePort: true });
  await api.storage.local.set({ remote: { url: "https://srv.test:8463/", token: "tok" }, remotes: [{ url: "https://srv.test:8463/", token: "tok" }] });
  served["https://srv.test:8463/api/list"] = {
    body: JSON.stringify({
      downloads: [
        { id: 1, file: "old.iso", url: "https://files.example.com/old.iso", path: "", status: "Complete", size: 9, downloaded: 9, speed: 0 },
        { id: 2, file: "new.zip", url: "https://files.example.com/new.zip", path: "", status: "Downloading", size: 9, downloaded: 3, speed: 3 },
      ],
      speed: 3,
    }),
  };
  let acted = null;
  served["https://srv.test:8463/api/act"] = (opts) => {
    acted = JSON.parse(opts.body);
    return { body: JSON.stringify({ done: acted.ids, skipped: [] }) };
  };
  load(api, false);
  await settle();
  const p = popupPort();
  listeners.connect(p.port);
  p.send({ type: "ask", ask: { filter: "all", search: "", limit: 60, px: 24 } });
  await until("the server's list", 3000, () => p.posted.length);
  const got = p.posted.at(-1);
  assert.deepEqual([got.ok, got.remote, got.list.downloads.map((d) => d.id), got.list.share], [true, "https://srv.test:8463/", [2, 1], false]);
  const listCall = fetched.filter((f) => f[0] === "https://srv.test:8463/api/list");
  assert.ok(listCall.length && listCall.every((f) => f[1] === "omit"), "no cookies to the server");
  assert.deepEqual(await popupAsk(listeners, { type: "download-act", act: "pause", id: 2 }), { ok: true });
  assert.deepEqual(acted, { action: "pause", ids: [2] });
  assert.deepEqual(await popupAsk(listeners, { type: "download-act", act: "resume", id: 2 }), { ok: true });
  assert.deepEqual(acted, { action: "start", ids: [2] });
  assert.deepEqual(await popupAsk(listeners, { type: "download-act", act: "open", id: 1 }), { ok: false, why: "unsupported" });
  assert.deepEqual(await popupAsk(listeners, { type: "download-act", act: "delete", id: 1 }), { ok: false, why: "unsupported" });
  p.close();
  delete served["https://srv.test:8463/api/list"];
  delete served["https://srv.test:8463/api/act"];
  ok("#1496 another MDM's list: its view, no icons, only what its remote control does");
}

// The permissions don't grow for the list: the popup copies a link and asks the app for the rest.
{
  const before = { chrome: ["downloads", "contextMenus", "nativeMessaging", "cookies", "storage", "webRequest", "scripting", "declarativeNetRequestWithHostAccess", "proxy"], firefox: ["downloads", "contextMenus", "nativeMessaging", "cookies", "storage", "webRequest", "webRequestBlocking", "scripting"] };
  assert.deepEqual(JSON.parse(readText("../../extension/manifest.json")).permissions, before.chrome);
  assert.deepEqual(JSON.parse(readText("../../extension/manifest.firefox.json")).permissions, before.firefox);
  const pack = readText("./pack.ps1");
  for (const f of ["popup-ui.js", "popup-downloads.js"]) assert.ok(pack.includes(`"${f}"`), `pack.ps1 copies ${f}`);
  ok("#1496 no new permissions; the popup's new scripts are packed");
}
assert.deepEqual(complaints, [], "nothing written to the console as an error or warning");

console.log(`extension: ${passed} checks passed`);
