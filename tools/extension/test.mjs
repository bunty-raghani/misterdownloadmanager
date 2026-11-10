// Runs extension/background.js against fake Firefox and Chrome APIs (no browser needed):
//   node tools/extension/test.mjs
import { readFileSync, readdirSync } from "node:fs";
import vm from "node:vm";
import assert from "node:assert/strict";

const code = readFileSync(new URL("../../extension/background.js", import.meta.url), "utf8");
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
      addListener: (f) => {
        fns.push(f);
        listeners[name] = (...a) => fns.reduce((r, g) => g(...a) ?? r, undefined);
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
        if (m.panel) return { ok: typeof panelOn === "function" ? panelOn(m) : panelOn !== false };
        // A notice of a download left alone (#208): an older host, which knows none, takes it
        // for an address it can't download (`noticeReply`).
        if (m.declined) return noticeReply || { ok: true };
        return reply;
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
      id: "ours",
      getURL: (p) => `chrome-extension://ours/${p}`,
    },
    tabs: {
      onCreated: event("tabCreated"),
      onRemoved: event("tabRemoved"),
      onUpdated: event("tabUpdated"),
      get: async (id) => tabs[id] || { id, url: "about:blank", status: "complete" },
      remove: async (id) => calls.push(["closeTab", id]),
      query: async () => [],
      create: async (t) => calls.push(["openTab", t.url]),
    },
    webRequest: {
      onBeforeRequest: event("beforeRequest"),
      onBeforeSendHeaders: event("beforeSendHeaders"),
      onHeadersReceived: event("headersReceived"),
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
        return func ? [{ result: ["https://x/1.zip", "https://x/2.zip"] }] : [];
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
  fetched.push([String(url), opts && opts.credentials]);
  const r = served[url];
  if (!r) throw new TypeError("Failed to fetch");
  // Its body as a stream too, 1 KB a chunk: `r.read` counts what was pulled (#171 reads 4 KB).
  const bytes = new TextEncoder().encode(r.body);
  r.read = 0;
  const body = new ReadableStream({
    pull(c) {
      if (r.read >= bytes.length) return c.close();
      c.enqueue(bytes.slice(r.read, r.read + 1024));
      r.read += 1024;
    },
  });
  return { ok: (r.status || 200) < 300, status: r.status || 200, headers: { get: () => null }, body, text: async () => r.body };
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
    setTimeout,
    clearTimeout,
    Promise,
  };
  if (firefox) ctx.browser = api;
  else ctx.chrome = api;
  Object.assign(ctx, extra);
  vm.runInNewContext(code, ctx);
}

const settle = () => new Promise((r) => setTimeout(r, 30));
// The messages to the host about downloads, and apart, the notices of downloads the extension
// left to the browser by Options (#208).
const natives = (calls) => calls.filter((c) => c[0] === "native" && !c[2].declined).map((c) => c[2]);
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
  // Options' excluded sites keep opening in the browser: *. with its subdomains, a plain host
  // by itself (an allow rule).
  const ex = fakeApi({ firefox: false, types: "PDF", rules: { excludedSites: "*.docs.test files.test:8080" } });
  load(ex.api, false);
  await settle();
  const added = ex.calls.filter((c) => c[0] === "dnr").pop()[1].addRules;
  assert.deepEqual(added[0].condition.excludedRequestDomains, ["docs.test"]);
  assert.equal(added[1].action.type, "allow");
  assert.equal(added[1].condition.regexFilter, "^[a-z]+://files\\.test(:[0-9]+)?/", "the host alone, any port");
  const off = fakeApi({ firefox: false, rules: { opened: false } });
  load(off.api, false);
  await settle();
  assert.deepEqual(off.calls.filter((c) => c[0] === "dnr").pop()[1].addRules, []);
  ok("opened types: the session rule follows the app's list");
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
  assert.deepEqual(await ask({ type: "panel" }), { on: true });
  assert.deepEqual(await ask({ type: "panel" }, "https://tube.test/embed/2"), { on: true });
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
  assert.deepEqual(await ask({ type: "panel" }, "https://tube.test/embed/3"), { on: false }, "off at once");
  assert.deepEqual(await ask({ type: "panel" }, "https://tube.test:8443/embed/4"), { on: false }, "whatever the port");
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
  await send({ type: "video", src: "" });
  assert.equal(natives(calls).find((m) => m.video).src, "https://cdn.test/v/master.m3u8?t=1");
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
    { id: 7, incognito: false },
  );
  await settle();
  const m = natives(calls).find((x) => x.video);
  assert.deepEqual(
    [m.url, m.src, m.referrer, m.explicit],
    ["https://player.test/embed", "https://cdn.test/s/master.m3u8", "https://blog.test/post", true],
    "the frame's stream, not the page alone",
  );
  assert.ok(calls.some((c) => c[0] === "cookies" && c[1].url === "https://cdn.test/s/master.m3u8"), "the stream's cookies");
  await listeners.menu({ menuItemId: "mdm-link", linkUrl: "https://cdn.test/s/other.m3u8", pageUrl: "https://blog.test/post" }, { id: 7, incognito: false });
  await settle();
  assert.equal(natives(calls).filter((x) => x.video)[1].src, "https://cdn.test/s/other.m3u8");
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
  assert.deepEqual(await ask(embed), { on: true });
  const asked = () => natives(calls).filter((m) => m.panel);
  assert.equal(asked()[0].referrer, "https://blog.test/post", "the tab's address with an embedded player's question");
  assert.deepEqual(await ask(top), { on: true });
  assert.equal(asked()[1].referrer, undefined, "none for the tab's own page");
  appSays = false;
  clock.t += 1000;
  assert.deepEqual(await ask(top), { on: true }, "kept a moment (the page's frames ask together)");
  assert.equal(asked().length, 2);
  clock.t += 5000;
  assert.deepEqual(await ask(top), { on: false }, "switched off in Options: the next question has it");
  assert.deepEqual(await ask(embed), { on: false });
  appSays = true;
  clock.t += 6000;
  assert.deepEqual(await ask(embed), { on: true }, "and on again");
  // "Never on this site" from the embed: off at once, in its tab and in any other.
  await new Promise((r) => listeners.message({ type: "hide-site" }, embed, r));
  assert.deepEqual(await ask(embed), { on: false });
  assert.deepEqual(await ask({ ...embed, tab: { url: "https://news.test/" } }), { on: false }, "whatever the tab");
  ok("#532: the video button follows Options within seconds; an embed asks with its tab's address");
}
{
  const { api, listeners } = fakeApi({ firefox: true, panelOn: false });
  load(api, true);
  const on = await new Promise((r) =>
    listeners.message({ type: "panel" }, { id: "ours", url: "https://x.test/", tab: {} }, (v) => r(JSON.parse(JSON.stringify(v)))),
  );
  assert.deepEqual(on, { on: false }, "where the app says no");
  ok("video button messages");
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
  const video = readFileSync(new URL("../../extension/video.js", import.meta.url), "utf8");
  const icon = readFileSync(new URL("../../extension/icons/icon-48.png", import.meta.url)).toString("base64");
  assert.ok(video.includes(`"data:image/png;base64,${icon}"`), "video.js's LOGO is extension/icons/icon-48.png");
  ok("video button logo");
}

// The address the button hands the app (video.js's videoPage): the video's own page, not the page
// that plays it. YouTube's home, a search, a channel or a playlist is a list to yt-dlp, and a watch
// page opened from a mix carries the list in its address: the button sent either, and the app
// offered the whole feed (~50 videos) instead of the one playing. #311.
{
  const source = readFileSync(new URL("../../extension/video.js", import.meta.url), "utf8");
  const start = source.indexOf("function videoPage(v) {");
  assert.ok(start > 0, "video.js has videoPage");
  // The function's own braces, so a test never runs a half of it.
  let depth = 0;
  let end = start;
  for (let i = source.indexOf("{", start); i < source.length; i += 1) {
    if (source[i] === "{") depth += 1;
    if (source[i] === "}") {
      depth -= 1;
      if (depth === 0) {
        end = i + 1;
        break;
      }
    }
  }
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
    const fn = new Function("location", "document", `${source.slice(start, end)}; return videoPage;`)({ href }, document);
    return fn(chain[0] || element([]));
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

// Shorts and live streams name their video in the path (video.js videoPage); the page is the
// video's, not a list. Their addresses as yt-dlp reads them (the same video as watch?v=).
{
  const source = readFileSync(new URL("../../extension/video.js", import.meta.url), "utf8");
  const start = source.indexOf("function videoPage(v) {");
  let depth = 0;
  let end = start;
  for (let i = source.indexOf("{", start); i < source.length; i += 1) {
    if (source[i] === "{") depth += 1;
    if (source[i] === "}" && --depth === 0) {
      end = i + 1;
      break;
    }
  }
  const named = (href) =>
    new Function("location", "document", `${source.slice(start, end)}; return videoPage;`)({ href }, { querySelector: () => null })({ parentElement: null, querySelectorAll: () => [] });
  assert.equal(named("https://www.youtube.com/shorts/cxkLg22mWco"), "https://www.youtube.com/watch?v=cxkLg22mWco");
  assert.equal(named("https://www.youtube.com/shorts/cxkLg22mWco?feature=share"), "https://www.youtube.com/watch?v=cxkLg22mWco");
  assert.equal(named("https://m.youtube.com/shorts/cxkLg22mWco"), "https://www.youtube.com/watch?v=cxkLg22mWco");
  assert.equal(named("https://www.youtube.com/live/abc123XYZ_-"), "https://www.youtube.com/watch?v=abc123XYZ_-");
  // The Shorts landing page, before it moves to the first one, names none.
  assert.equal(named("https://www.youtube.com/shorts/"), null);
  ok("video.js: Shorts and live pages name their video");
}

// No other download manager's name in what ships (the clean-room rule, CLAUDE.md).
{
  for (const file of readdirSync(new URL("../../extension/", import.meta.url))) {
    if (!/\.(js|html|css|json)$/.test(file)) continue;
    const text = readFileSync(new URL(`../../extension/${file}`, import.meta.url), "utf8");
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
  const english = JSON.parse(readFileSync(new URL("en/messages.json", dir), "utf8"));
  for (const locale of readdirSync(dir)) {
    const m = JSON.parse(readFileSync(new URL(`${locale}/messages.json`, dir), "utf8"));
    assert.deepEqual(Object.keys(m).sort(), Object.keys(english).sort(), `${locale} has every text`);
  }
  for (const name of ["manifest.json", "manifest.firefox.json"]) {
    const text = readFileSync(new URL(`../../extension/${name}`, import.meta.url), "utf8");
    for (const [, key] of text.matchAll(/__MSG_(\w+)__/g)) assert.ok(english[key], `${name}: ${key}`);
  }
  // Every key the scripts use exists in English.
  for (const file of ["background.js", "popup.js", "popup.html", "video.js"]) {
    const text = readFileSync(new URL(`../../extension/${file}`, import.meta.url), "utf8");
    for (const [, key] of text.matchAll(/(?:t\(|data-i18n=)"((?:menu|status|popup|video)_\w+)"/g)) assert.ok(english[key], `${file}: ${key}`);
  }
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

// The codes agree end to end: the app's (mdm_ipc::capture::Declined::code), the extension's own
// ("site-access"), and a popup text for each.
{
  const rust = readFileSync(new URL("../../crates/ipc/src/capture.rs", import.meta.url), "utf8");
  const body = /pub fn code\(self\)[\s\S]*?\n    \}\n/.exec(rust)[0];
  const codes = [...body.matchAll(/=> "([a-z-]+)"/g)].map((m) => m[1]);
  assert.deepEqual(codes, ["integration-off", "browser", "file-type", "site", "address", "small"]);
  const en = JSON.parse(readFileSync(new URL("../../extension/_locales/en/messages.json", import.meta.url), "utf8"));
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
    // Percent-encoded UTF-8 in the plain name, as Firefox and the app read it.
    ["attachment; filename=\"%D8%AA%D9%82%D8%B1%D9%8A%D8%B1.pdf\"", "تقرير.pdf"],
    ["attachment; filename=\"100%25.pdf\"", "100%25.pdf"],
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
// The manifests (#42): "proxy" optional in Firefox's, required in Chrome's (it can't be optional
// there); the popup's switch: the permission in Firefox, the extension's own setting in Chrome.
{
  const chrome = JSON.parse(readFileSync(new URL("../../extension/manifest.json", import.meta.url), "utf8"));
  const firefox = JSON.parse(readFileSync(new URL("../../extension/manifest.firefox.json", import.meta.url), "utf8"));
  assert.ok((chrome.permissions || []).includes("proxy") && !(chrome.optional_permissions || []).includes("proxy"), "Chrome: proxy required");
  assert.ok(!(firefox.permissions || []).includes("proxy"), "Firefox: proxy not required");
  assert.deepEqual(firefox.optional_permissions, ["proxy"], "Firefox: proxy optional");
  const popup = readFileSync(new URL("../../extension/popup.js", import.meta.url), "utf8");
  assert.ok(/optional_permissions \|\| \[\]\)\.includes\("proxy"\)/.test(popup), "Firefox's switch where proxy is optional");
  assert.ok(/permissions\.request\(PROXY\)/.test(popup) && /permissions\.remove\(PROXY\)/.test(popup), "asked for and taken back on the switch");
  assert.ok(/storage\.local\s*\.get\(\{ proxyOn: false \}\)/.test(popup) && /storage\.local\.set\(\{ proxyOn: box\.checked \}\)/.test(popup), "Chrome's switch: its own setting");
  assert.ok(/const PROXY_SWITCH = "proxyOn"/.test(code), "the same setting in background.js");
  const en = JSON.parse(readFileSync(new URL("../../extension/_locales/en/messages.json", import.meta.url), "utf8"));
  assert.ok(en.popup_proxy && en.popup_proxy_title && en.popup_proxy_title_chromium, "the switch's texts");
  ok("manifests: proxy optional in Firefox, required in Chrome; the popup's switch in each");
}
// Another MDM (#157): the file types caught for it are the app's defaults, and every message
// the pairing can end with is there.
{
  const capture = readFileSync(new URL("../../crates/ipc/src/capture.rs", import.meta.url), "utf8");
  const app = /pub const DEFAULT_AUTO_TYPES: &str = "([^"]+)"/.exec(capture)[1];
  const ours = /const DEFAULT_TYPES =\s*"([^"]+)"/.exec(code)[1];
  assert.equal(ours, app, "DEFAULT_TYPES is the app's DEFAULT_AUTO_TYPES");
  const english = JSON.parse(readFileSync(new URL("../../extension/_locales/en/messages.json", import.meta.url), "utf8"));
  for (const why of ["address", "unreachable", "wrong_code", "no_code", "too_soon", "failed", "device_name"]) {
    assert.ok(english[`remote_${why}`], `remote_${why}`);
  }
  assert.ok(english.status_remote, "status_remote");
  ok("another MDM: the app's file types, every pairing message");
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
  const pageCode = readFileSync(new URL("../../extension/page.js", import.meta.url), "utf8");
  const run = async ({ safari, taken, types = "ZIP ISO R0*", capture = true }) => {
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
        handlers.click({ type: "click", button: 0, isTrusted: false, target: this, preventDefault() {} });
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
      addEventListener: (n, f) => (handlers[n] = f),
      removeEventListener() {},
    });
    await settle();
    const click = (href, extra = {}, attrs) => {
      const a = new Element(href, attrs);
      const e = { type: "click", button: 0, isTrusted: true, altKey: false, target: a, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; }, ...extra };
      handlers.click(e);
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
  ({ e } = t.click("https://x.test/f/a.zip", { altKey: true }));
  assert.ok(!e.defaultPrevented, "Alt: the browser's");
  assert.deepEqual(t.sent.at(-1), { type: "bypass", url: "https://x.test/f/a.zip" });
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
  ok("Safari: page.js holds a click on a listed file type, never Alt or other types");
}
assert.deepEqual(complaints, [], "nothing written to the console as an error or warning");

console.log(`extension: ${passed} checks passed`);
