// The popup's Downloads tab (#1496): MDM's newest downloads, live while the popup is open, each
// with the system's icon for its file type, what it is doing, and what can be done with it.
// background.js gets the list (this computer's MDM through its helper, or another MDM's remote
// control) and does what the buttons ask; popup-ui.js has the shared pieces.
"use strict";

const Downloads = (() => {
  const $ = (id) => document.getElementById(id);
  // The most rows asked for: the newest; the rest are in MDM's own list.
  const LIMIT = 60;
  // The icons' size in CSS pixels (popup.css .dl-icon).
  const ICON = 24;
  const params = new URLSearchParams(location.search);
  // Opened in a tab of its own (the tests, the pictures): it stays open after a button.
  const keepOpen = params.has("page") || params.has("w") || params.has("keep");
  const s = {
    port: null,
    filter: "all",
    search: "",
    answered: false,
    list: null,
    remote: null,
    appState: null,
    why: null,
    share: false,
    icons: new Map(),
    current: null,
    shown: false,
  };
  const rows = new Map();

  // ---- the list from background.js -------------------------------------------------------------

  function connect() {
    try {
      s.port = popupExt.runtime.connect({ name: "recent" });
    } catch {
      s.port = null;
      return;
    }
    s.port.onMessage.addListener(onList);
    // The background part was stopped and started again: asked again.
    s.port.onDisconnect.addListener(() => {
      s.port = null;
      setTimeout(connect, 400);
    });
    ask();
  }

  function ask() {
    if (!s.port) return;
    const px = Math.min(64, Math.max(16, Math.round(ICON * (window.devicePixelRatio || 1))));
    s.port.postMessage({ type: "ask", ask: { filter: s.filter, search: s.search, limit: LIMIT, px } });
  }

  function onList(m) {
    if (!m || m.type !== "list") return;
    for (const [k, v] of Object.entries(m.icons || {})) s.icons.set(k, v);
    // MDM busy a moment (it answers within 3 s, or says so): the list as it was stays.
    if (!m.ok && m.appState === "busy" && s.list) return;
    s.answered = true;
    s.remote = m.remote || null;
    if (m.ok && m.list) {
      s.list = m.list;
      s.share = Boolean(m.list.share) && !s.remote;
      s.appState = s.why = null;
    } else {
      s.list = null;
      s.appState = m.appState || null;
      s.why = m.why || null;
    }
    render();
  }

  // ---- drawing ---------------------------------------------------------------------------------

  function render() {
    const list = s.list;
    const counts = (list && list.counts) || { all: 0, running: 0, done: 0, failed: 0 };
    for (const chip of document.querySelectorAll("#dl-filters .chip")) {
      const n = counts[chip.dataset.filter];
      chip.querySelector(".n").textContent = list && n ? String(n) : "";
    }
    const running = list ? Number(list.running) || 0 : 0;
    const pill = $("tab-running");
    pill.hidden = !running;
    pill.textContent = running ? String(running) : "";
    pill.title = running ? tx("popup_dl_running_count", running) : "";
    // Another MDM's downloads: what needs the file here isn't offered, and the bar says why.
    const host = s.remote ? hostOf(s.remote) : "";
    $("dl-remote").hidden = !s.remote;
    $("dl-remote-text").textContent = s.remote ? tx("popup_dl_remote", host) : "";
    // The search and the filters while there is a list to search; Clear finished for this
    // computer's; the foot's button opens MDM, or the other MDM's own page.
    document.querySelector(".dl-tools").hidden = !list;
    $("dl-clear").disabled = !list || !counts.done;
    $("dl-clear").hidden = Boolean(s.remote) || !list;
    $("dl-open").textContent = tx(s.remote ? "popup_dl_open_remote" : "popup_open");
    const downloads = list ? list.downloads || [] : [];
    drawRows(downloads);
    drawEmpty(list, downloads);
    const total = list ? counts[s.filter] || 0 : 0;
    const more = $("dl-more");
    more.hidden = !list || total <= downloads.length;
    more.textContent = more.hidden ? "" : tx("popup_dl_more", downloads.length, total);
  }

  function hostOf(url) {
    try {
      return new URL(url).host;
    } catch {
      return String(url);
    }
  }

  // Why the list is empty, and what to do: not answered yet, MDM not running, too old, not
  // reached, nothing yet, nothing that matches.
  function drawEmpty(list, downloads) {
    const box = $("dl-empty");
    const button = $("dl-empty-act");
    let look = null;
    if (!s.answered) look = { ico: "download", title: "popup_dl_loading", text: "", busy: true };
    else if (s.remote && !list) look = { ico: "server", title: "popup_list_remote_unreachable", text: "popup_list_remote_unreachable_text", sub: [hostOf(s.remote)] };
    else if (s.appState === "off") look = { ico: "download", title: "popup_list_off", text: "popup_list_off_text", act: "popup_list_start", run: startApp };
    else if (s.appState === "old") look = { ico: "alert", title: "popup_list_old", text: "popup_list_old_text" };
    else if (s.appState === "busy") look = { ico: "download", title: "popup_list_busy", text: "" };
    else if (!list) look = { ico: "alert", title: "popup_list_unreached", text: s.why ? `status_${String(s.why).replace(/-/g, "_")}` : "", act: "popup_fix", run: () => openHelp(s.why) };
    else if (!downloads.length && (s.search || s.filter !== "all")) look = { ico: "search", title: "popup_dl_no_match", text: s.search ? "popup_dl_no_match_text" : "", sub: [s.search], act: "popup_dl_show_all", run: showAll };
    else if (!downloads.length) look = { ico: "download", title: "popup_dl_none", text: "popup_dl_none_text" };
    box.hidden = !look;
    box.classList.toggle("busy", Boolean(look && look.busy));
    if (!look) return;
    const ico = $("dl-empty-ico");
    ico.replaceChildren(icon(look.ico, 28));
    $("dl-empty-title").textContent = tx(look.title, ...(look.sub || []));
    $("dl-empty-text").textContent = look.text ? tx(look.text, ...(look.sub || [])) : "";
    $("dl-empty-text").hidden = !look.text;
    button.hidden = !look.act;
    if (look.act) {
      button.textContent = tx(look.act);
      button.onclick = look.run;
    }
  }

  // The rows for `downloads`, newest first: each row kept by its download's id (its focus, its
  // pointer and its cut name stay as it updates), new ones made, gone ones taken away.
  function drawRows(downloads) {
    const ul = $("dl-list");
    const keep = new Set(downloads.map((d) => d.id));
    for (const [id, r] of rows) {
      if (!keep.has(id)) {
        if (r.li.contains(document.activeElement)) s.current = null;
        if (cutOnResize && r.name) cutOnResize.unobserve(r.name);
        r.li.remove();
        rows.delete(id);
      }
    }
    let before = ul.firstChild;
    for (const d of downloads) {
      let r = rows.get(d.id);
      if (!r) {
        r = makeRow(d);
        rows.set(d.id, r);
      }
      updateRow(r, d);
      if (r.li !== before) ul.insertBefore(r.li, before);
      before = r.li.nextSibling;
    }
    settleFocus();
  }

  function makeRow(d) {
    const li = document.createElement("li");
    li.className = "dl";
    li.dataset.id = String(d.id);
    const pic = document.createElement("span");
    pic.className = "dl-icon";
    const main = document.createElement("div");
    main.className = "dl-main";
    const status = document.createElement("span");
    status.className = "dl-status";
    status.id = `dl-st-${d.id}`;
    const bar = document.createElement("span");
    bar.className = "dl-bar";
    bar.setAttribute("role", "progressbar");
    bar.setAttribute("aria-valuemin", "0");
    bar.setAttribute("aria-valuemax", "100");
    const fill = document.createElement("span");
    bar.append(fill);
    main.append(status, bar);
    const quick = document.createElement("div");
    quick.className = "dl-quick";
    const folder = smallButton("folder", "popup_act_folder");
    const pause = smallButton("pause", "popup_act_pause");
    const resume = smallButton("play", "popup_act_resume");
    quick.append(folder, pause, resume);
    const more = smallButton("more", "popup_act_more");
    more.classList.add("dl-more");
    more.setAttribute("aria-haspopup", "menu");
    more.setAttribute("aria-expanded", "false");
    li.append(pic, main, quick, more);
    const r = { li, pic, main, name: null, status, bar, fill, folder, pause, resume, more, d, iconKey: undefined };
    folder.onclick = () => act(r.d, "folder");
    pause.onclick = () => act(r.d, "pause");
    resume.onclick = () => act(r.d, "resume");
    more.onclick = () => openMore(r);
    return r;
  }

  function smallButton(ico, label) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "icon-btn small";
    b.append(icon(ico));
    b.dataset.label = label;
    return b;
  }

  function updateRow(r, d) {
    r.d = d;
    const local = !s.remote;
    const a = actionsOf(d, { local, share: s.share });
    const st = statusOf(d);
    const state = stateOf(d);
    r.li.dataset.state = state;
    // The name: a button that opens the file when it can be, else plain text.
    const tag = a.open ? "BUTTON" : "SPAN";
    if (!r.name || r.name.tagName !== tag) {
      const el = document.createElement(tag.toLowerCase());
      el.className = "dl-name";
      if (tag === "BUTTON") {
        el.type = "button";
        el.onclick = () => act(r.d, "open");
      }
      if (r.name) {
        if (cutOnResize) cutOnResize.unobserve(r.name);
        r.name.replaceWith(el);
      } else r.main.prepend(el);
      r.name = el;
    }
    setName(r.name, d.file || d.url || "");
    r.name.dir = "auto";
    if (a.open) {
      r.name.setAttribute("aria-label", tx("popup_act_open_named", d.file));
      r.name.setAttribute("aria-describedby", r.status.id);
    }
    // Its picture: the system's icon for its type, else a drawn one.
    const src = d.icon ? s.icons.get(d.icon) : null;
    const key = src ? `img:${d.icon}` : `ico:${d.kind === "torrent" && d.icon === "folder" ? "folder" : "file"}`;
    if (r.iconKey !== key) {
      r.iconKey = key;
      if (src) {
        const img = document.createElement("img");
        img.src = src;
        img.width = img.height = ICON;
        img.alt = "";
        img.decoding = "async";
        r.pic.replaceChildren(img);
      } else r.pic.replaceChildren(icon(key.slice(4), ICON));
    }
    r.status.textContent = st.text;
    r.status.title = st.text;
    r.status.className = `dl-status${st.tone ? ` ${st.tone}` : ""}`;
    // The bar: while it runs or is paused part way.
    const showBar = st.bar !== undefined && (state === "running" || (state === "paused" && st.bar !== null));
    r.bar.hidden = !showBar;
    r.bar.classList.toggle("busy", Boolean(st.busy));
    r.bar.classList.toggle("paused", state === "paused");
    if (showBar) {
      const pct = st.bar === null ? null : Math.round(st.bar * 1000) / 10;
      r.fill.style.width = pct === null ? "" : `${pct}%`;
      if (pct === null) r.bar.removeAttribute("aria-valuenow");
      else r.bar.setAttribute("aria-valuenow", String(Math.round(pct)));
      r.bar.setAttribute("aria-label", d.file || "");
    }
    // Its buttons, each named for its download (a screen reader hears which one).
    r.folder.hidden = !a.folder;
    r.pause.hidden = !a.pause;
    r.resume.hidden = !a.resume;
    for (const b of [r.folder, r.pause, r.resume, r.more]) {
      const label = tx(b.dataset.label === "popup_act_more" ? "popup_act_more_named" : `${b.dataset.label}_named`, d.file || "");
      b.setAttribute("aria-label", label);
      b.title = tx(b.dataset.label);
    }
    r.li.classList.toggle("has-quick", !r.folder.hidden || !r.pause.hidden || !r.resume.hidden);
  }

  // ---- moving through the list ---------------------------------------------------------------

  function buttonsOf(li) {
    return [...li.querySelectorAll("button")].filter((b) => !b.hidden);
  }

  // One row is the list's stop for Tab (the one focused last, else the first): its buttons only.
  function settleFocus() {
    const lis = [...rows.values()].map((r) => r.li).filter((li) => li.isConnected);
    let current = lis.find((li) => li.dataset.id === s.current);
    if (!current) current = lis[0];
    for (const li of lis) {
      for (const b of li.querySelectorAll("button")) b.tabIndex = li === current ? 0 : -1;
    }
  }

  function listKeys(e) {
    const li = e.target.closest("li.dl");
    if (!li) return;
    const lis = [...$("dl-list").children];
    const at = lis.indexOf(li);
    const buttons = buttonsOf(li);
    const col = Math.max(0, buttons.indexOf(e.target));
    const rtl = getComputedStyle(li).direction === "rtl";
    const go = (to, column = col) => {
      const target = lis[to];
      if (!target) return;
      e.preventDefault();
      s.current = target.dataset.id;
      settleFocus();
      const bs = buttonsOf(target);
      (bs[Math.min(column, bs.length - 1)] || bs[0]).focus();
    };
    const r = rows.get(Number(li.dataset.id));
    switch (e.key) {
      case "ArrowDown":
        return go(Math.min(lis.length - 1, at + 1));
      case "ArrowUp":
        if (at === 0) {
          e.preventDefault();
          return document.querySelector('#dl-filters .chip[tabindex="0"]').focus();
        }
        return go(at - 1);
      case "Home":
        return go(0, 0);
      case "End":
        return go(lis.length - 1, 0);
      case "ArrowRight":
      case "ArrowLeft": {
        const forward = (e.key === "ArrowRight") !== rtl;
        const to = buttons[col + (forward ? 1 : -1)];
        if (to) {
          e.preventDefault();
          to.focus();
        }
        return;
      }
      case "ContextMenu":
        e.preventDefault();
        return r && openMore(r);
      case "F10":
        if (e.shiftKey && r) {
          e.preventDefault();
          openMore(r);
        }
        return;
      case "Delete":
        if (r && actionsOf(r.d, { local: !s.remote, share: s.share }).del) {
          e.preventDefault();
          act(r.d, "del");
        }
        return;
      default:
    }
  }

  // ---- what the buttons do ---------------------------------------------------------------------

  function openMore(r) {
    const d = r.d;
    const a = actionsOf(d, { local: !s.remote, share: s.share });
    const item = (on, id, label, ico, extra = {}) => (on ? [{ id, label: tx(label), icon: ico, run: () => act(r.d, id), ...extra }] : []);
    const items = [
      ...item(a.open, "open", "popup_act_open", "open"),
      ...item(a.folder, "folder", "popup_act_folder", "folder"),
      ...item(a.copyFile, "copyFile", "popup_act_copy_file", "copy"),
      ...item(a.share, "share", "popup_act_share", "share"),
      ...item(a.copyLink, "copyLink", "popup_act_copy_link", "link"),
      "-",
      ...item(a.pause, "pause", "popup_act_pause", "pause"),
      ...item(a.resume, "resume", "popup_act_resume", "play"),
      ...item(a.again, "again", "popup_act_again", "again"),
      "-",
      ...item(a.remove, "remove", "popup_act_remove", "remove"),
      ...item(a.del, "del", "popup_act_delete", "trash", { danger: true }),
    ];
    showMenu(r.more, items, tx("popup_act_more_named", d.file || ""));
  }

  // The popup closes once the file, its folder or the share sheet opens (in front of it).
  function closeSoon() {
    if (!keepOpen) setTimeout(() => window.close(), 150);
  }

  async function act(d, what) {
    if (what === "copyLink") {
      const ok = await copyText(d.url);
      sayBriefly(tx(ok ? "popup_said_link" : "popup_act_failed"), ok ? "ok" : "bad");
      return;
    }
    if (what === "del") {
      const yes = await confirmAsk({
        title: tx("popup_ask_delete_title"),
        text: tx("popup_ask_delete", d.file || ""),
        ok: tx("popup_ask_delete_ok"),
        danger: true,
      });
      if (!yes) return;
    }
    const name = { open: "open", folder: "folder", copyFile: "copy-file", share: "share", pause: "pause", resume: "resume", again: "redownload", remove: "remove", del: "delete" }[what];
    const reply = await popupExt.runtime.sendMessage({ type: "download-act", act: name, id: d.id }).catch(() => ({ ok: false }));
    if (!reply || !reply.ok) {
      sayBriefly(actWhyText(reply && reply.why), "bad");
      return;
    }
    const said = { copyFile: "popup_said_file", remove: "popup_said_removed", del: "popup_said_deleted", again: "popup_said_again" }[what];
    if (said) sayBriefly(tx(said, d.file || ""), "ok");
    if (what === "open" || what === "folder" || what === "share") closeSoon();
  }

  async function clearDone() {
    const n = (s.list && s.list.counts && s.list.counts.done) || 0;
    if (!n) return;
    const yes = await confirmAsk({ title: tx("popup_ask_clear_title"), text: tx("popup_ask_clear", n), ok: tx("popup_ask_clear_ok") });
    if (!yes) return;
    const reply = await popupExt.runtime.sendMessage({ type: "download-act", act: "clear-done", id: 0 }).catch(() => ({ ok: false }));
    sayBriefly(reply && reply.ok ? tx("popup_said_cleared") : actWhyText(reply && reply.why), reply && reply.ok ? "ok" : "bad");
  }

  async function startApp() {
    $("dl-empty-act").disabled = true;
    await popupExt.runtime.sendMessage({ type: "open-app" }).catch(() => null);
    $("dl-empty-act").disabled = false;
  }

  function openHelp(why) {
    popupExt.tabs.create({ url: `https://misterdownloadmanager.com/help/browser-help.html#${why || "not-installed"}` });
  }

  function showAll() {
    const input = $("dl-search");
    input.value = "";
    s.search = "";
    setFilter("all");
    input.focus();
  }

  function setFilter(f) {
    s.filter = f;
    for (const chip of document.querySelectorAll("#dl-filters .chip")) {
      const on = chip.dataset.filter === f;
      chip.setAttribute("aria-checked", String(on));
      chip.tabIndex = on ? 0 : -1;
    }
    ask();
  }

  // ---- start ------------------------------------------------------------------------------------

  function start() {
    if (s.shown) return;
    s.shown = true;
    const search = $("dl-search");
    let typing = null;
    search.addEventListener("input", () => {
      clearTimeout(typing);
      typing = setTimeout(() => {
        s.search = search.value.trim();
        ask();
      }, 180);
    });
    search.addEventListener("keydown", (e) => {
      if (e.key === "Escape" && search.value) {
        e.preventDefault();
        e.stopPropagation();
        search.value = "";
        s.search = "";
        ask();
      } else if (e.key === "ArrowDown") {
        const first = $("dl-list").querySelector("li.dl button:not([hidden])");
        if (first) {
          e.preventDefault();
          first.focus();
        }
      }
    });
    const chips = () => [...document.querySelectorAll("#dl-filters .chip")];
    roving($("dl-filters"), chips, { axis: "x", onMove: (el) => setFilter(el.dataset.filter) });
    for (const chip of chips()) chip.addEventListener("click", () => setFilter(chip.dataset.filter));
    $("dl-filters").addEventListener("keydown", (e) => {
      if (e.key === "ArrowDown") {
        const first = $("dl-list").querySelector("li.dl button:not([hidden])");
        if (first) {
          e.preventDefault();
          first.focus();
        }
      }
    });
    const list = $("dl-list");
    list.addEventListener("keydown", listKeys);
    list.addEventListener("focusin", (e) => {
      const li = e.target.closest("li.dl");
      if (li && s.current !== li.dataset.id) {
        s.current = li.dataset.id;
        settleFocus();
      }
    });
    $("dl-clear").addEventListener("click", clearDone);
    $("dl-open").addEventListener("click", async () => {
      if (s.remote) {
        popupExt.tabs.create({ url: s.remote });
        return;
      }
      await popupExt.runtime.sendMessage({ type: "open-app" }).catch(() => null);
      closeSoon();
    });
    connect();
  }

  return { start, ask };
})();
