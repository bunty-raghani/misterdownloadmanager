// Checks what tools/extension/pack.ps1 built (target/extension) before it is uploaded:
//   node tools/extension/check-pack.mjs
// Each package carries the app's version and every file its manifest names; the unpacked copy's
// key gives the ID the native host serves (crates/ipc/src/extension.rs); the Chrome Web Store's
// copy has no key; Firefox's has its add-on ID; Safari's (a folder, tools/safari builds it into the
// app) asks for nothing Safari lacks.
import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import assert from "node:assert/strict";

const root = new URL("../../", import.meta.url);
const out = new URL("target/extension/", root);
const cargo = readFileSync(new URL("Cargo.toml", root), "utf8");
const version = cargo.match(/^version\s*=\s*"([^"]+)"/m)[1];
const ipc = readFileSync(new URL("crates/ipc/src/extension.rs", root), "utf8");
const unpackedId = ipc.match(/UNPACKED_ID: &str = "([a-p]{32})"/)[1];
const firefoxId = ipc.match(/FIREFOX_ID: &str = "([^"]+)"/)[1];

// A Chromium extension's ID: the first 32 hex digits of SHA-256 of its public key, 0-f as a-p.
const idOf = (key) =>
  [...createHash("sha256").update(Buffer.from(key, "base64")).digest("hex").slice(0, 32)]
    .map((c) => String.fromCharCode(97 + parseInt(c, 16)))
    .join("");

for (const kind of ["chromium", "chrome-store", "firefox", "safari"]) {
  const dir = new URL(`${kind}/`, out);
  const m = JSON.parse(readFileSync(new URL("manifest.json", dir), "utf8"));
  assert.equal(m.version, version, `${kind}: the app's version`);
  const files = [
    m.background.service_worker,
    ...(m.background.scripts || []),
    m.action && m.action.default_popup,
    ...Object.values(m.icons),
    ...m.content_scripts.flatMap((c) => c.js),
  ].filter(Boolean);
  for (const f of files) assert.ok(existsSync(new URL(f, dir)), `${kind}: ${f} is there`);
  assert.ok(existsSync(new URL("_locales/en/messages.json", dir)), `${kind}: English texts`);
  if (kind === "chromium") assert.equal(idOf(m.key), unpackedId, "the key gives the ID the host serves");
  if (kind === "chrome-store") {
    assert.equal(m.key, undefined, "the store signs it with its own key");
    const video = m.content_scripts.find((c) => c.js.includes("video.js"));
    assert.ok(video && !(video.exclude_matches || []).length, "the video button runs everywhere, YouTube included");
  }
  if (kind === "firefox") assert.equal(m.browser_specific_settings.gecko.id, firefoxId);
  if (kind === "safari") {
    // page.js tells Safari by this; Safari has no downloads or proxy API to ask permission for.
    assert.ok(m.browser_specific_settings.safari, "page.js knows Safari by its manifest");
    for (const p of ["downloads", "proxy", "webRequestBlocking", "declarativeNetRequestWithHostAccess"]) {
      assert.ok(!(m.permissions || []).includes(p), `Safari: no ${p}`);
    }
    assert.ok(m.permissions.includes("nativeMessaging"), "Safari: the app extension is reached by native messaging");
  }
}
for (const zip of ["mdm-chromium.zip", "mdm-firefox.xpi", "mdm-chrome-store.zip"]) {
  assert.ok(existsSync(new URL(zip, out)), zip);
}
console.log(`packages ok (version ${version}, unpacked ID ${unpackedId})`);
