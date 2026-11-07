// Mister Download Manager's download button over videos (a content script, in every page and
// frame). Our own design: a small pill at the video's edge that appears while the pointer is
// over the video (or it has the keyboard's focus) and fades when the pointer rests or leaves.
// Clicking it opens the app's Download video window for this page (an embedded player's own
// address, which yt-dlp understands), with the video's own address when it has a plain one. It can be dragged along the video's edge, and stays where
// it was put on that site. Its menu hides it on this video, keeps it off the site (the app's
// Options → General → Download panels lists those sites), or opens those settings. The app
// decides where it may show at all (the native host: integration on, this browser captured,
// the button on, the site not excluded).
(() => {
  // One button per page: a copy cut off from the extension (after an update) gives way to the
  // new one.
  const prev = globalThis.__mdmVideoButton;
  if (prev && typeof prev === "object" && prev.alive()) return;
  if (prev && typeof prev.teardown === "function") prev.teardown();
  const ext = globalThis.browser ?? globalThis.chrome;

  // A video worth a button: big enough to be content, not a short muted looping clip (an
  // animated picture or a background). A sound player (<audio> with controls) gets one too.
  const MIN_W = 200;
  const MIN_H = 112;
  const MIN_AUDIO_H = 24;
  const CLIP_S = 15;
  // Shown this long after the pointer last moved over the video, kept while it is on the button.
  const IDLE_MS = 2600;
  const INSET = 10;
  const DRAG_PX = 4;
  // Our app icon (extension/icons/icon-48.png, as data: no extension file is exposed to pages).
  const LOGO = "data:image/png;base64,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";

  const t = (key, ...subs) => ext.i18n.getMessage(key, subs) || FALLBACK[key] || key;
  const FALLBACK = {
    video_download: "Download video",
    video_more: "More options",
    video_hide_here: "Hide on this video",
    video_settings: "Button settings...",
    video_sent: "Sent to Mister Download Manager",
    video_failed: "Mister Download Manager isn't installed or didn't start",
    video_off: "The download button is off on this site",
    video_drm: "Protected video: it can't be saved",
    video_drag: "Drag to move it along the video's edge",
  };
  // To the background. After the extension was updated or removed, this page's copy of the script
  // is cut off from it (calls throw at once): then it takes itself away.
  const send = (m) => {
    try {
      if (!ext.runtime?.id) throw new Error("gone");
      return ext.runtime.sendMessage(m).catch(() => ({ ok: false }));
    } catch {
      teardown();
      return Promise.resolve({ ok: false, gone: true });
    }
  };
  const host = location.hostname;
  const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;

  // "Hide on this video": the video's address (players reuse one element for the next video),
  // else the element.
  const dismissed = new WeakSet();
  const dismissedSrc = new Set();
  const isDismissed = (v) => dismissed.has(v) || (v.currentSrc && dismissedSrc.has(v.currentSrc));
  let allowed = null; // asked once per page: may it show here?
  let place = { x: 1, y: 0 }; // where on the edge: fractions of the room, top right by default
  let video = null; // the video it is over
  let lastMove = 0;
  let pointer = { x: -1, y: -1 };
  let menuOpen = false;
  let dragging = null;
  let busy = false;
  let flashing = false;
  let raf = 0;
  let scan = 0;
  // Kept on screen while the keyboard is on it (not after a mouse click: that leaves focus too),
  // or on the video's player (its own controls).
  const keyboardOn = () =>
    Boolean(shadow.activeElement && shadow.activeElement.matches(":focus-visible")) || playerFocused();
  let focusedPlayer = null;
  const playerFocused = () =>
    Boolean(video && focusedPlayer && document.activeElement === focusedPlayer && focusedPlayer.contains(video));

  // ---- the button: a closed shadow tree the page's styles and scripts can't reach ----------
  const root = document.createElement("mdm-video-button");
  // Each with !important: a page's own rules can't hide or move it.
  for (const [k, v] of Object.entries({
    all: "initial", position: "fixed", "z-index": "2147483647", left: "0", top: "0", display: "none",
    contain: "layout style", opacity: "1", visibility: "visible", "pointer-events": "auto",
  })) root.style.setProperty(k, v, "important");
  const shadow = root.attachShadow({ mode: "closed" });
  const CSS = `
:host { all: initial; }
* { box-sizing: border-box; }
/* A light, glossy panel that stands out over a dark video, with our icon: the classic download
   button's feel, made modern (rounded, crisp type, soft shadow and motion). */
.pill {
  display: flex; align-items: stretch; height: 34px; border-radius: 9px;
  background: linear-gradient(#ffffff, #f1f4f8 48%, #e3e8ef 52%, #eef2f6);
  color: #17324d; border: 1px solid rgba(23, 50, 77, .28);
  box-shadow: 0 4px 14px rgba(0, 0, 0, .32), 0 1px 0 rgba(255, 255, 255, .9) inset;
  font: 600 13px/1 "Segoe UI", system-ui, -apple-system, Roboto, sans-serif;
  opacity: 0; transform: translateY(-3px) scale(.98);
  transition: opacity .16s ease-out, transform .16s ease-out, filter .12s;
  user-select: none; -webkit-user-select: none; touch-action: none; white-space: nowrap;
}
.pill.shown { opacity: 1; transform: none; }
.pill.dragging { cursor: grabbing; box-shadow: 0 10px 28px rgba(0, 0, 0, .45); transition: none; }
.pill.dragging .go { cursor: grabbing; }
.pill.ok { background: linear-gradient(#f2fff6, #d4f3df); border-color: #2f9a5b; color: #135c33; }
.pill.err { background: linear-gradient(#fff4f4, #f6d6d6); border-color: #c24848; color: #7d1f1f; }
button {
  /* all: unset resets box-sizing too, past the rule above: without it a menu item's 100 % width
     plus its padding ran past the menu's edge (#81). */
  all: unset; box-sizing: border-box; display: flex; align-items: center; gap: 8px;
  color: inherit; cursor: pointer; font: inherit;
}
.go { padding: 0 12px 0 8px; border-radius: 8px 0 0 8px; cursor: grab; }
.more { padding: 0 9px; border-left: 1px solid rgba(23, 50, 77, .18); border-radius: 0 8px 8px 0; }
.go:hover, .more:hover { background: linear-gradient(rgba(214, 232, 255, .9), rgba(182, 212, 250, .9)); }
.go:active, .more:active { background: rgba(160, 196, 242, .9); }
button:focus-visible { outline: 2px solid #2a78d6; outline-offset: -2px; }
.go img { width: 22px; height: 22px; flex: none; filter: drop-shadow(0 1px 1px rgba(0, 0, 0, .25)); }
.more svg { width: 12px; height: 12px; color: #3b5876; }
.compact .label { display: none; }
.compact .go { padding: 0 6px; border-radius: 8px 0 0 8px; }
.menu {
  position: absolute; min-width: 210px; padding: 4px; border-radius: 9px;
  background: #fbfcfe; color: #17324d; border: 1px solid rgba(23, 50, 77, .22);
  box-shadow: 0 12px 32px rgba(0, 0, 0, .35);
  font: 400 13px/1.2 "Segoe UI", system-ui, -apple-system, Roboto, sans-serif;
  display: none;
}
.menu.open { display: block; }
.menu button { display: block; width: 100%; padding: 8px 12px; border-radius: 6px; cursor: default; }
.menu button:hover, .menu button:focus-visible { background: #dceafb; outline: none; }
.menu hr { border: 0; border-top: 1px solid rgba(23, 50, 77, .12); margin: 4px 2px; }
@media (prefers-reduced-motion: reduce) { .pill { transition: none; transform: none; } }
`;
  // A constructed sheet: a page whose Content-Security-Policy refuses inline styles can't strip
  // it (Firefox applies the page's policy to what a content script adds).
  try {
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(CSS);
    shadow.adoptedStyleSheets = [sheet];
  } catch {
    const style = document.createElement("style");
    style.textContent = CSS;
    shadow.appendChild(style);
  }
  shadow.innerHTML += `
<div class="pill" part="pill">
  <button class="go" type="button">
    <img alt="" draggable="false">
    <span class="label"></span>
  </button>
  <button class="more" type="button" aria-haspopup="menu" aria-expanded="false">
    <svg viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.6"
      stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 4.5 6 7.5l3-3"/></svg>
  </button>
</div>
<div class="menu" role="menu">
  <button role="menuitem" data-act="download"></button>
  <hr>
  <button role="menuitem" data-act="hide"></button>
  <button role="menuitem" data-act="never"></button>
  <hr>
  <button role="menuitem" data-act="settings"></button>
</div>`;
  shadow.querySelector(".go img").src = LOGO;
  const pill = shadow.querySelector(".pill");
  const go = shadow.querySelector(".go");
  const label = shadow.querySelector(".label");
  const more = shadow.querySelector(".more");
  const menu = shadow.querySelector(".menu");
  const item = (act) => menu.querySelector(`[data-act="${act}"]`);
  pill.dir = ext.i18n.getMessage("@@bidi_dir") || "ltr";
  menu.dir = pill.dir;
  const idleLabel = t("video_download");
  label.textContent = idleLabel;
  go.title = `${idleLabel} — ${t("video_drag")}`;
  go.setAttribute("aria-label", idleLabel);
  more.title = t("video_more");
  more.setAttribute("aria-label", t("video_more"));
  item("download").textContent = idleLabel;
  item("hide").textContent = t("video_hide_here");
  item("never").textContent = ext.i18n.getMessage("video_never_site", [host]) || `Never on ${host}`;
  item("settings").textContent = t("video_settings");

  // ---- which video ----------------------------------------------------------------------------
  const videos = document.getElementsByTagName("video");
  const audios = document.getElementsByTagName("audio");

  // Videos inside open shadow roots (players built as web components), looked for again every
  // few seconds at most.
  let shadowed = [];
  let shadowedAt = 0;
  function shadowVideos() {
    if (Date.now() - shadowedAt < 3000) return shadowed;
    shadowedAt = Date.now();
    const found = [];
    const walk = (root, depth) => {
      if (depth > 4) return;
      const w = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT);
      for (let n = w.nextNode(), seen = 0; n && seen < 20000; n = w.nextNode(), seen++) {
        if (n.shadowRoot) {
          found.push(...n.shadowRoot.querySelectorAll("video"));
          walk(n.shadowRoot, depth + 1);
        }
      }
    };
    walk(document.documentElement, 0);
    shadowed = found;
    return found;
  }

  function eligible(v) {
    if (!v.isConnected || isDismissed(v)) return false;
    const r = v.getBoundingClientRect();
    const audio = v instanceof HTMLAudioElement;
    if (audio ? !v.controls || r.width < MIN_W || r.height < MIN_AUDIO_H : r.width < MIN_W || r.height < MIN_H) {
      return false;
    }
    // A short muted loop without controls: a moving picture, not something to save.
    if (v.muted && v.loop && !v.controls && !(v.duration > CLIP_S)) return false;
    const s = getComputedStyle(v);
    return s.visibility !== "hidden" && s.display !== "none" && Number(s.opacity) > 0.05;
  }

  // The topmost eligible video under (x, y): players lay their controls over the video, so the
  // video itself rarely gets the pointer's events.
  function videoAt(x, y) {
    let best = null;
    for (const v of [...videos, ...audios, ...shadowVideos()]) {
      const r = v.getBoundingClientRect();
      if (x < r.left || x > r.right || y < r.top || y > r.bottom || !eligible(v)) continue;
      if (!best || r.width * r.height < best.r.width * best.r.height) best = { v, r };
    }
    return best && best.v;
  }

  // ---- placing it -------------------------------------------------------------------------
  // `place` is where it sits in the room the video leaves around it: x and y from 0 to 1, one of
  // them 0 or 1 (on an edge).
  function rects() {
    const r = video.getBoundingClientRect();
    const w = pill.offsetWidth || 150;
    const h = pill.offsetHeight || 32;
    return { r, w, h, roomX: Math.max(0, r.width - w - 2 * INSET), roomY: Math.max(0, r.height - h - 2 * INSET) };
  }

  function position() {
    if (!video) return;
    const { r, w, roomX, roomY } = rects();
    // A narrow video gets the icon only.
    pill.classList.toggle("compact", r.width < 360);
    const at = dragging ? dragging.at : place;
    const left = r.left + INSET + at.x * roomX;
    // In the part of the video on screen: one scrolled half out keeps its button in view.
    const h = pill.offsetHeight || 32;
    const visTop = Math.max(r.top, 0) + INSET;
    const visBottom = Math.min(r.bottom, innerHeight) - INSET - h;
    const top = Math.min(Math.max(r.top + INSET + at.y * roomY, visTop), Math.max(visTop, visBottom));
    root.style.setProperty("transform", `translate(${Math.round(left)}px, ${Math.round(top)}px)`, "important");
    if (menuOpen) placeMenu(top, left, w);
  }

  // The menu opens below the button, or above it near the bottom of the window; aligned with
  // whichever of its sides is nearer the video's middle.
  function placeMenu(top, left, w) {
    const h = menu.offsetHeight;
    const below = top + 36 + h < innerHeight;
    menu.style.top = below ? "36px" : `${-h - 4}px`;
    const { r } = rects();
    const rightHalf = left + w / 2 > r.left + r.width / 2;
    menu.style.left = rightHalf ? "" : "0";
    menu.style.right = rightHalf ? "0" : "";
  }

  // Snap a free spot (fractions, after a drag) to the nearest edge.
  function snap(at) {
    const d = { left: at.x, right: 1 - at.x, top: at.y, bottom: 1 - at.y };
    const edge = Object.keys(d).reduce((a, b) => (d[a] <= d[b] ? a : b));
    // Along the edge it stays where it was dropped, but close to a corner it goes into it.
    const c = (n) => (n < 0.08 ? 0 : n > 0.92 ? 1 : n);
    if (edge === "left") return { x: 0, y: c(at.y) };
    if (edge === "right") return { x: 1, y: c(at.y) };
    if (edge === "top") return { x: c(at.x), y: 0 };
    return { x: c(at.x), y: 1 };
  }

  // ---- showing and hiding ---------------------------------------------------------------------
  function attach() {
    // Over the video in fullscreen: inside the fullscreen element (a fullscreen <video> itself
    // can't hold it: then it hides).
    const fs = document.fullscreenElement;
    const parent = fs && fs !== video && fs.contains(video) ? fs : document.documentElement;
    if (root.parentNode !== parent) parent.appendChild(root);
    return !(fs && (fs === video || !fs.contains(video)));
  }

  function show(v) {
    if (video !== v) {
      video = v;
      closeMenu();
    }
    if (!attach()) return hide(true);
    root.style.setProperty("display", "block", "important");
    position();
    if (!pill.classList.contains("shown")) requestAnimationFrame(() => pill.classList.add("shown"));
    if (!raf) raf = requestAnimationFrame(tick);
  }

  // `now`: at once and whatever has the focus (dismissed, the video gone).
  function hide(now) {
    if (!now && (menuOpen || dragging || keyboardOn())) return;
    if (now) {
      closeMenu();
      shadow.activeElement?.blur();
    }
    pill.classList.remove("shown");
    cancelAnimationFrame(raf);
    raf = 0;
    const was = video;
    setTimeout(() => {
      if (!pill.classList.contains("shown") && video === was) root.style.setProperty("display", "none", "important");
    }, now || reduced ? 0 : 180);
  }

  // While shown: follow the video (scrolling, layout) and fade when the pointer rests.
  function tick() {
    raf = 0;
    if (!video || !video.isConnected || !eligible(video)) {
      video = null;
      return hide(true);
    }
    const r = video.getBoundingClientRect();
    const over =
      pointer.x >= r.left && pointer.x <= r.right && pointer.y >= r.top && pointer.y <= r.bottom;
    const onButton = root.matches(":hover");
    if (!onButton && !menuOpen && !dragging && !keyboardOn() && (!over || Date.now() - lastMove > IDLE_MS)) {
      return hide();
    }
    position();
    raf = requestAnimationFrame(tick);
  }

  // At most one look per frame, however fast the pointer moves.
  function onMove(e) {
    pointer = { x: e.clientX, y: e.clientY };
    lastMove = Date.now();
    if (!scan) scan = requestAnimationFrame(look);
  }

  function look() {
    scan = 0;
    if (dragging || allowed === false) return;
    const v = videoAt(pointer.x, pointer.y);
    if (!v) return;
    if (allowed === null) return void ask().then(() => allowed && look());
    show(v);
  }

  async function ask() {
    if (allowed !== null) return;
    allowed = false; // until answered: one question per page
    const r = await send({ type: "panel" });
    allowed = Boolean(r && r.on);
    if (allowed) {
      try {
        const { positions } = await ext.storage.local.get({ positions: {} });
        if (positions[host]) place = snap(positions[host]);
      } catch {
        // Default spot.
      }
    }
  }

  // ---- acting ---------------------------------------------------------------------------------
  async function download() {
    if (busy || !video) return;
    busy = true;
    try {
      closeMenu();
      // Encrypted (DRM): no tool can save it; say so rather than send it.
      if (video.mediaKeys) return flash("err", t("video_drm"));
      const src = /^https?:/i.test(video.currentSrc || "") ? video.currentSrc : "";
      const title = (document.querySelector('meta[property="og:title"]')?.content || document.title || "").trim();
      const r = await send({ type: "video", src, title, page: location.href });
      if (r.gone) return;
      flash(r.ok ? "ok" : "err", t(r.ok ? "video_sent" : r.why === "options" ? "video_off" : "video_failed"));
    } finally {
      busy = false;
    }
  }

  // A short confirmation on the button itself, then back to normal.
  function flash(kind, text) {
    flashing = true;
    pill.classList.remove("ok", "err");
    pill.classList.add(kind);
    label.textContent = text;
    lastMove = Date.now();
    position();
    setTimeout(() => {
      flashing = false;
      pill.classList.remove(kind);
      label.textContent = idleLabel;
      if (video) position();
    }, 2200);
  }

  function openMenu() {
    menuOpen = true;
    menu.classList.add("open");
    more.setAttribute("aria-expanded", "true");
    position();
    menu.querySelector("button").focus();
  }

  function closeMenu() {
    if (!menuOpen) return;
    menuOpen = false;
    menu.classList.remove("open");
    more.setAttribute("aria-expanded", "false");
  }

  menu.addEventListener("click", (e) => {
    if (!e.isTrusted) return;
    const act = e.target.closest("[data-act]")?.dataset.act;
    if (act) run(act);
  });

  async function run(act) {
    if (act === "download") return download();
    closeMenu();
    if (act === "hide") {
      dismissed.add(video);
      if (video.currentSrc) dismissedSrc.add(video.currentSrc);
      video = null;
      return hide(true);
    }
    if (act === "never") {
      allowed = false;
      video = null;
      hide(true);
      return send({ type: "hide-site" });
    }
    if (act === "settings") send({ type: "options" });
  }

  more.addEventListener("click", () => (menuOpen ? closeMenu() : openMenu()));

  // Keys: Escape closes the menu, arrows move through it.
  shadow.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && menuOpen) {
      closeMenu();
      more.focus();
      e.stopPropagation();
    } else if (menuOpen && (e.key === "Enter" || e.key === " ") && menu.contains(shadow.activeElement)) {
      // The focused command, whatever the browser does with a key on a button.
      run(shadow.activeElement.dataset.act);
      e.preventDefault();
    } else if (menuOpen && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
      const items = [...menu.querySelectorAll("button")];
      const i = items.indexOf(shadow.activeElement);
      items[(i + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length].focus();
      e.preventDefault();
    }
  });

  // The main part: a click downloads, a drag moves it (and is not a click).
  // Only the user's own clicks and keys (never events a page script makes up).
  go.addEventListener("pointerdown", (e) => {
    if (e.button !== 0 || !video || !e.isTrusted) return;
    const { r, roomX, roomY } = rects();
    dragging = {
      id: e.pointerId,
      startX: e.clientX,
      startY: e.clientY,
      // Where in the button it was taken.
      dx: e.clientX - (r.left + INSET + place.x * roomX),
      dy: e.clientY - (r.top + INSET + place.y * roomY),
      at: place,
      moved: false,
    };
    go.setPointerCapture(e.pointerId);
    e.preventDefault();
  });
  go.addEventListener("pointermove", (e) => {
    if (!dragging || e.pointerId !== dragging.id) return;
    if (!dragging.moved && Math.hypot(e.clientX - dragging.startX, e.clientY - dragging.startY) < DRAG_PX) return;
    if (!dragging.moved) {
      dragging.moved = true;
      closeMenu();
      pill.classList.add("dragging");
    }
    const { r, roomX, roomY } = rects();
    const c = (n) => Math.min(1, Math.max(0, n));
    dragging.at = {
      x: roomX ? c((e.clientX - dragging.dx - r.left - INSET) / roomX) : 0,
      y: roomY ? c((e.clientY - dragging.dy - r.top - INSET) / roomY) : 0,
    };
    position();
  });
  const endDrag = (e) => {
    if (!dragging || e.pointerId !== dragging.id) return;
    const d = dragging;
    dragging = null;
    pill.classList.remove("dragging");
    if (!d.moved) {
      if (e.type === "pointerup") download();
      return;
    }
    place = snap(d.at);
    position();
    lastMove = Date.now();
    // Remembered per site, not in a private window (it would keep the site's name), and the
    // 200 sites used last only.
    if (ext.extension?.inIncognitoContext) return;
    try {
      ext.storage.local
        .get({ positions: {} })
        .then(({ positions }) => {
          delete positions[host];
          positions[host] = place;
          const sites = Object.keys(positions);
          for (const s of sites.slice(0, Math.max(0, sites.length - 200))) delete positions[s];
          return ext.storage.local.set({ positions });
        })
        .catch(() => {});
    } catch {
      teardown();
    }
  };
  go.addEventListener("pointerup", endDrag);
  go.addEventListener("pointercancel", endDrag);
  // Keyboard: Enter / Space (a real button's click without a pointer).
  go.addEventListener("click", (e) => {
    if (e.detail === 0 && e.isTrusted) download();
  });

  // ---- wiring -----------------------------------------------------------------------------------
  // The extension updated or removed under this page: the button and its listeners go (a fresh
  // copy of this script comes with the new extension).
  function teardown() {
    cancelAnimationFrame(raf);
    cancelAnimationFrame(scan);
    removeEventListener("pointermove", onMove, true);
    removeEventListener("keydown", onTab, true);
    root.remove();
    allowed = false;
    video = null;
  }
  globalThis.__mdmVideoButton = { alive: () => Boolean(ext.runtime?.id), teardown };
  addEventListener("pointermove", onMove, { capture: true, passive: true });
  addEventListener("pointerdown", (e) => {
    if (menuOpen && !e.composedPath().includes(root)) closeMenu();
  }, { capture: true, passive: true });
  // A video with the keyboard's focus, or a player around one (its controls), gets the button
  // too, kept while the focus stays there; Tab goes from there to the button.
  addEventListener("focusin", (e) => {
    if (!(e.target instanceof Element) || e.composedPath().includes(root)) return;
    const t = e.target;
    const v =
      t instanceof HTMLMediaElement ? t : [...t.querySelectorAll("video, audio")].find(eligible) || null;
    if (!v || !eligible(v)) return;
    focusedPlayer = t;
    ask().then(() => {
      if (!allowed) return;
      lastMove = Date.now();
      show(v);
    });
  }, true);
  // Tab on the focused player moves into the button first; Tab from the button goes back to
  // the player and on from there (its own controls next), as if the button sat at its start.
  let tabOn = 0;
  function onTab(e) {
    if (e.key !== "Tab" || e.shiftKey) return;
    // The Tab that just left the button: on from the player, as the browser goes.
    if (Date.now() - tabOn < 100) {
      tabOn = 0;
      return;
    }
    if (video && playerFocused() && root.style.getPropertyValue("display") === "block") {
      go.focus();
      e.preventDefault();
    }
  }
  addEventListener("keydown", onTab, true);
  go.addEventListener("keydown", (e) => {
    if (e.key === "Tab" && !e.shiftKey && focusedPlayer && focusedPlayer.isConnected) {
      tabOn = Date.now();
      focusedPlayer.focus();
    }
  });
  document.addEventListener("fullscreenchange", () => video && show(video));
  addEventListener("blur", () => hide());
})();
