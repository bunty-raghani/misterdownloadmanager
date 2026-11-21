// Mister Download Manager — site behaviour: light/dark, the download for your system, the
// split button's menu, the "What do you download?" chips and the rotating hello.
(function () {
  "use strict";
  var root = document.documentElement;
  // Words for people in the page's language: a translated page carries its own (MDM_I18N,
  // tools/website/sitei18n.py); English otherwise. Every such string goes through t().
  var I18N = window.MDM_I18N || {};
  function t(s) { return I18N[s] || s; }
  // Visitors who asked for less motion get every moving part at rest: the race at its finish,
  // the tour still, the hello on its first word.
  var reduce = window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  function each(selector, fn) {
    var els = document.querySelectorAll(selector);
    for (var i = 0; i < els.length; i++) fn(els[i]);
  }

  // ---- The crew's names ------------------------------------------------------------------------
  // A contributor's login shows over their photo on hover and focus (styles.css .face-name); at
  // the start or end of a row it slides back inside the list rather than run out of the sheet.
  function nudge(e) {
    var face = e.target && e.target.closest ? e.target.closest(".face") : null;
    var name = face && face.querySelector(".face-name");
    var list = face && face.closest(".faces");
    if (!name || !list) return;
    name.style.setProperty("--nudge", "0px");
    var r = name.getBoundingClientRect();
    var box = list.getBoundingClientRect();
    var shift = r.left < box.left ? box.left - r.left : r.right > box.right ? box.right - r.right : 0;
    name.style.setProperty("--nudge", shift + "px");
  }
  document.addEventListener("pointerover", nudge);
  document.addEventListener("focusin", nudge);

  // ---- Your language ---------------------------------------------------------------------------
  // An English page offers its copy in the visitor's language once (the first language the
  // browser asks for, unless that is English), in that language's words (the language menu's
  // data-offer/-yes/-no, sitei18n.py). Picking a language, or No, is remembered.
  (function () {
    var links = document.querySelectorAll(".lang-pick a[hreflang]");
    if (!links.length) return;
    function remember(code) { try { localStorage.setItem("mdm-lang", code); } catch (e) { /* asked again */ } }
    for (var i = 0; i < links.length; i++) {
      links[i].addEventListener("click", function () { remember(this.getAttribute("hreflang")); });
    }
    var chosen = null;
    try { chosen = localStorage.getItem("mdm-lang"); } catch (e) { /* none */ }
    if (chosen || (root.getAttribute("lang") || "en").toLowerCase() !== "en") return;
    var wanted = navigator.languages && navigator.languages.length ? navigator.languages : [navigator.language || ""];
    var match = null;
    for (var w = 0; w < wanted.length && !match; w++) {
      var tag = String(wanted[w]).toLowerCase();
      var base = tag.split("-")[0];
      if (base === "en") return;
      // Chinese: only the simplified script's places; Portuguese: Brazil's copy for every place.
      if (base === "zh" && /-(tw|hk|mo|hant)/.test(tag)) continue;
      for (var k = 0; k < links.length; k++) {
        var h = links[k].getAttribute("hreflang").toLowerCase();
        if (h !== "en" && h.split("-")[0] === base) { match = links[k]; break; }
      }
    }
    if (!match || !match.getAttribute("data-offer")) return;
    var bar = document.createElement("div");
    bar.className = "lang-offer";
    bar.setAttribute("role", "region");
    bar.setAttribute("lang", match.getAttribute("hreflang"));
    if (/^(ar|fa)/.test(match.getAttribute("hreflang"))) bar.setAttribute("dir", "rtl");
    var p = document.createElement("p");
    p.textContent = match.getAttribute("data-offer");
    var yes = document.createElement("a");
    yes.className = "btn btn-primary";
    yes.href = match.getAttribute("href");
    yes.className = "btn btn-line";
    yes.textContent = match.getAttribute("data-yes") + " · " + match.querySelector("span").textContent;
    yes.addEventListener("click", function () { remember(match.getAttribute("hreflang")); });
    var no = document.createElement("button");
    no.type = "button";
    no.className = "btn btn-ghost";
    no.style.boxShadow = "none";
    no.textContent = match.getAttribute("data-no");
    no.addEventListener("click", function () { remember("en"); bar.parentNode.removeChild(bar); });
    bar.appendChild(p);
    bar.appendChild(yes);
    bar.appendChild(no);
    document.body.appendChild(bar);
  })();

  // ---- The language picker: a dialog over the page ------------------------------------------
  // The header's button opens it (without scripts it is a link to the languages page). It is a
  // Vitrine modal (website/vitrine/, on the browser's own dialog): the focus stays
  // inside and starts on the current language, Escape, a click on the backdrop, its close button
  // or Back close it, the page behind doesn't scroll, and the focus goes back to the button.
  each("[data-lang-open]", function (btn) {
    var dialog = btn.parentNode.querySelector(".lang-dialog");
    if (!dialog || !window.Vitrine || !Vitrine.modal || typeof dialog.showModal !== "function") return;
    btn.setAttribute("role", "button");
    function open(e) {
      e.preventDefault();
      if (!Vitrine.modal.open(dialog, btn)) return;
      var cur = dialog.querySelector("[aria-current]");
      if (cur) cur.focus();
    }
    btn.addEventListener("click", open);
    btn.addEventListener("keydown", function (e) { if (e.key === " ") open(e); });
    dialog.querySelector("[data-lang-close]").setAttribute("data-modal-close", "");
  });

  // ---- Light / dark ------------------------------------------------------------------------
  // The page follows the system until the visitor picks; the pick is remembered.
  var media = window.matchMedia ? window.matchMedia("(prefers-color-scheme: dark)") : null;
  function current() {
    var t = root.getAttribute("data-theme");
    if (t === "light" || t === "dark") return t;
    return media && media.matches ? "dark" : "light";
  }
  each(".theme", function (button) {
    button.addEventListener("click", function () {
      var next = current() === "dark" ? "light" : "dark";
      root.setAttribute("data-theme", next);
      try { localStorage.setItem("mdm-theme", next); } catch (e) { /* not remembered */ }
    });
  });

  // ---- The split button's menu: one open at a time, closed by a click elsewhere or Escape ----
  each(".split-more, .menu", function (d) {
    d.addEventListener("toggle", function () {
      if (d.open) each(".split-more, .menu", function (o) { if (o !== d) o.open = false; });
    });
  });
  document.addEventListener("click", function (e) {
    each(".split-more[open], .menu[open]", function (d) { if (!d.contains(e.target)) d.open = false; });
  });
  document.addEventListener("keydown", function (e) {
    if (e.key === "Escape") each(".split-more[open], .menu[open]", function (d) { d.open = false; d.querySelector("summary").focus(); });
  });

  // ---- The benchmark, replayed in real time, as tracking -------------------------------------
  // Medians from docs/bench/run5/REPORT.md: startup (command to first request), transfer and
  // finish, in milliseconds. Each lane is one rule on a time axis shared by both: dashed while
  // the app gets going, solid while the file comes in, stamped where it is delivered. Its scan
  // events (request sent, receiving at what rate, delivered) appear as they are reached.
  var RACES = [
    { label: "faster", note: t("1 GiB over a fast link. IDM spends 3 s before its first request reaches the server."),
      mdm: [14, 438, 556], idm: [3076, 1831, 545], e2e: [1009, 5456], ratio: 5.41, bytes: 1073741824 },
    { label: "faster", note: t("A 256 KiB file. MDM is done in 30 ms; IDM is still starting."),
      mdm: [14, 7, 8], idm: [3090, 25, 32], e2e: [30, 3141], ratio: 105.9, bytes: 262144 },
    { label: "faster", note: t("A server that allows two connections. MDM learns it and keeps to two; IDM opened 480."),
      mdm: [14, 12546, 9], idm: [3069, 12733, 48], e2e: [12568, 15864], ratio: 1.26, bytes: 52428800 }
  ];
  // mdmRace(box, races): sets up one race box; the benchmark page calls it with every scenario
  // from bench/results.json and fills in the tabs itself (data-race-src).
  window.mdmRace = function (box, RACES) {
    var lanes = { mdm: box.querySelector('[data-lane="mdm"]'), idm: box.querySelector('[data-lane="idm"]') };
    var tabList = box.querySelector(".race-tabs");
    if (!tabList.children.length) {
      RACES.forEach(function (r, k) {
        var b = document.createElement("button");
        b.type = "button"; b.setAttribute("role", "tab"); b.setAttribute("data-scenario", k);
        b.setAttribute("aria-selected", k ? "false" : "true");
        b.textContent = r.tab;
        tabList.appendChild(b);
      });
    }
    var tabs = box.querySelectorAll("[data-scenario]");
    var ratioEl = box.querySelector("[data-ratio]"), noteEl = box.querySelector("[data-note]");
    var frame = 0, current = 0;
    function total(ph) { return ph[0] + ph[1] + ph[2]; }
    // The phases' medians don't add up exactly to the end-to-end medians: scale them so the
    // finish times are the report's.
    RACES.forEach(function (r) {
      ["mdm", "idm"].forEach(function (k, n) {
        var f = r.e2e[n] / total(r[k]);
        r[k] = r[k].map(function (v) { return v * f; });
      });
    });
    function fmt(ms) { return ms < 1000 ? Math.round(ms) + " ms" : (ms / 1000).toFixed(2) + " s"; }
    function times(x) { return (x >= 10 ? Math.round(x) : x < 2 ? x.toFixed(2) : x.toFixed(1)) + "\u00d7"; }
    // The rate the file came in at while it was coming in, as the app shows it.
    function rate(bytes, ms) {
      var mb = bytes / 1048576 / (ms / 1000);
      return mb >= 100 ? Math.round(mb) + " MB/s" : mb >= 1 ? mb.toFixed(1) + " MB/s" : Math.round(mb * 1024) + " KB/s";
    }
    // One stretch of the rule, from a to b ms, on an axis of `axis` ms.
    function stretch(el, a, b, axis) {
      el.style.left = (a / axis * 100) + "%";
      el.style.width = (Math.max(0, b - a) / axis * 100) + "%";
    }
    // ph: the lane's phases (startup, transfer, finish); not `t`, which is the page's words.
    function paint(lane, ph, now, axis, bytes) {
      var start = ph[0], xfer = ph[1], all = total(ph), at = Math.min(now, all), done = now >= all;
      var segs = lane.querySelectorAll(".lane-track .seg");
      stretch(segs[0], 0, Math.min(at, start), axis);
      stretch(segs[1], start, Math.max(start, Math.min(at, start + xfer)), axis);
      stretch(segs[2], start + xfer, Math.max(start + xfer, at), axis);
      lane.querySelector(".lane-stamp").style.setProperty("--at", all / axis);
      lane.classList.toggle("done", done);
      lane.querySelector("[data-time]").textContent = fmt(at);
      lane.querySelector("[data-state]").textContent = done ? t("Done") : now < start ? t("Starting\u2026") : now < start + xfer ? t("Downloading") : t("Finishing");
      var ev = lane.querySelectorAll(".lane-events li");
      var words = [t("Request sent") + " \u00b7 " + fmt(start),
                   t("Receiving") + (bytes ? " \u00b7 " + rate(bytes, xfer) : ""),
                   t("Delivered") + " \u00b7 " + fmt(all)];
      var reached = [now >= start, now >= start, done];
      for (var i = 0; i < ev.length; i++) {
        ev[i].textContent = words[i];
        ev[i].classList.toggle("on", reached[i]);
      }
    }
    function run(k) {
      cancelAnimationFrame(frame);
      current = k;
      var r = RACES[k];
      for (var i = 0; i < tabs.length; i++) tabs[i].setAttribute("aria-selected", i === k ? "true" : "false");
      var end = Math.max(total(r.mdm), total(r.idm)), first = Math.min(total(r.mdm), total(r.idm));
      // The report's ratio of medians (docs/bench/run5/REPORT.md).
      var ratio = r.ratio;
      noteEl.textContent = r.note;
      // Less motion: the finish at once.
      var t0 = reduce ? performance.now() - end : performance.now();
      (function tick(now) {
        var ms = now - t0;
        paint(lanes.mdm, r.mdm, ms, end, r.bytes);
        paint(lanes.idm, r.idm, ms, end, r.bytes);
        // No ratio until the first delivery; then it grows while the other is still on its way.
        box.classList.toggle("pending", ms < first);
        ratioEl.textContent = ms < first ? "\u2013" : times(ms >= end ? ratio : Math.min(ratio, ms / first));
        if (ms < end) frame = requestAnimationFrame(tick);
      })(t0);
    }
    for (var i = 0; i < tabs.length; i++) {
      tabs[i].addEventListener("click", function () { run(+this.getAttribute("data-scenario")); });
    }
    box.querySelector("[data-race-replay]").addEventListener("click", function () { run(current); });
    // Start when it comes into view; at once where the browser can't tell.
    if ("IntersectionObserver" in window) {
      var seen = new IntersectionObserver(function (es) {
        if (es[0].isIntersecting) { seen.disconnect(); run(0); }
      }, { threshold: 0.4 });
      seen.observe(box);
    } else run(0);
  };
  each("[data-race]:not([data-race-src])", function (box) { window.mdmRace(box, RACES); });

  // ---- Hello in the app's languages: each word written in, its chip lit -----------------------
  var stage = document.querySelector("[data-hello]");
  if (stage && !reduce) {
    var words = [["Hello", "en"], ["مرحبا", "ar"], ["Bonjour", "fr"], ["Hallo", "de"], ["Hola", "es"],
      ["こんにちは", "ja"], ["Olá", "pt"], ["Привет", "ru"], ["नमस्ते", "hi"], ["Merhaba", "tr"], ["你好", "zh"]];
    var chips = document.querySelectorAll("[data-hello-chips] [lang]");
    var n = 0;
    setInterval(function () {
      var old = stage.querySelector('[data-pos="current"]');
      if (old) {
        old.setAttribute("data-pos", "past");
        setTimeout(function () { if (old.parentNode) old.parentNode.removeChild(old); }, 450);
      }
      n = (n + 1) % words.length;
      var word = document.createElement("span");
      word.className = "hello-word";
      word.setAttribute("data-pos", "current");
      word.setAttribute("lang", words[n][1]);
      word.textContent = words[n][0];
      stage.appendChild(word);
      for (var i = 0; i < chips.length; i++) {
        if (chips[i].getAttribute("lang") === words[n][1]) chips[i].setAttribute("data-on", "");
        else chips[i].removeAttribute("data-on");
      }
    }, 2600);
  }

  // ---- Themes: every tile in its light or its dark side, and the prompt to copy -----------------
  // The tiles follow the site's light or dark at first; the switch shows them all in the other.
  (function () {
    var grid = document.querySelector("[data-variant-grid]");
    var bar = document.querySelector("[data-variant-switch]");
    if (!grid || !bar) return;
    var buttons = bar.querySelectorAll("button[data-variant]");
    function current() {
      var set = root.getAttribute("data-theme");
      if (set === "light" || set === "dark") return set;
      return window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
    }
    function show(v) {
      grid.setAttribute("data-variant", v);
      for (var i = 0; i < buttons.length; i++) {
        buttons[i].setAttribute("aria-pressed", String(buttons[i].getAttribute("data-variant") === v));
      }
    }
    for (var i = 0; i < buttons.length; i++) {
      buttons[i].addEventListener("click", function () { show(this.getAttribute("data-variant")); });
    }
    show(current());
    bar.hidden = false;
  })();

  each("[data-copy]", function (btn) {
    var src = document.getElementById(btn.getAttribute("data-copy"));
    var label = btn.querySelector("[data-copy-label]");
    if (!src || !label || !navigator.clipboard) return;
    var idle = label.textContent, timer = 0;
    btn.hidden = false;
    btn.addEventListener("click", function () {
      navigator.clipboard.writeText(src.textContent.trim() + "\n").then(function () {
        label.textContent = t("Copied");
      }, function () {
        label.textContent = t("Select the text and copy it");
      }).then(function () {
        clearTimeout(timer);
        timer = setTimeout(function () { label.textContent = idle; }, 2200);
      });
    });
  });

  // ---- Lightbox: any screenshot, big ----------------------------------------------------------
  // Vitrine's (anefzaoui/vitrine, vendored in website/vitrine/). Pictures in [data-lightbox] groups and
  // every screenshot in a help article open over the page, flying from where they are, at their
  // real size (the width and height the page gives them, which tools/website/screens.py and
  // build.py keep true) fitted to the screen, never larger. A click, a double tap or the wheel
  // zooms (drag to look around), arrows, keys and swipes go through the group, Escape, Back or
  // the backdrop close it. The day or night version shown on the page is the one opened. Moving
  // through the tour's pictures moves the tour along, so closing lands on the one last seen.
  if (window.Vitrine && Vitrine.lightbox) {
    Vitrine.setStrings({
      viewer: t("Image viewer"), close: t("Close"), prev: t("Previous image"), next: t("Next image"),
      zoomIn: t("Zoom in"), zoomOut: t("Zoom out"), counter: t("{n} of {total}"),
      hint: t("Click to zoom · drag to move")
    });
    Vitrine.lightbox.init({
      auto: ".prose",
      skip: ".lb-skip, [data-lightbox-skip]",
      minWidth: 121,
      size: function (img) {
        var w = +img.getAttribute("width"), h = +img.getAttribute("height");
        return w && h ? [w, h] : null;
      },
      // "Title · what it shows" when the caption leads with a bold title.
      caption: function (img) {
        var fig = img.closest("figure");
        var cap = fig && fig.querySelector("figcaption");
        if (!cap) return null;
        var all = cap.textContent.replace(/\s+/g, " ").trim();
        var b = cap.querySelector("b");
        var head = b ? b.textContent.trim() : "";
        return head && all.indexOf(head) === 0 && all.length > head.length ? head + " · " + all.slice(head.length).trim() : all;
      },
      onChange: function (img) {
        var panel = img.closest(".tour-panel");
        if (panel) panel.dispatchEvent(new CustomEvent("tour-show", { bubbles: true }));
      }
    });
  }

  // ---- The tour: the app one window at a time -----------------------------------------------
  // A tablist of features beside a stage whose panels share one spot and crossfade. While it is
  // on screen it moves on every DWELL ms by itself, the chosen tab's line filling up to show when
  // (--p, from the time spent, so a pause holds it exactly where it was). A pointer over the tour
  // or the focus inside it holds it; choosing a tab (click, Enter, arrows, a swipe) or opening a
  // picture stops it for good, as someone reading shouldn't have it change under them. Panels
  // left behind stay laid out, only faded, inert and hidden from screen readers: the stage keeps
  // its height, and the lightbox still goes through every picture. Never on its own with reduced
  // motion (`reduce`, the hello's).
  each("[data-tour]", function (tour) {
    var DWELL = 6000;
    var list = tour.querySelector('[role="tablist"]');
    var stage = tour.querySelector(".tour-stage");
    var tabs = list.querySelectorAll('[role="tab"]');
    var panels = [];
    for (var i = 0; i < tabs.length; i++) panels.push(document.getElementById(tabs[i].getAttribute("aria-controls")));
    var at = 0, auto = !reduce, seen = !("IntersectionObserver" in window), over = false, inside = false;
    var spent = 0, last = 0, frame = 0;

    function select(k) {
      at = (k + tabs.length) % tabs.length;
      for (var i = 0; i < tabs.length; i++) {
        var on = i === at;
        tabs[i].setAttribute("aria-selected", on ? "true" : "false");
        tabs[i].tabIndex = on ? 0 : -1;
        tabs[i].style.setProperty("--p", "0");
        if (on) { panels[i].setAttribute("data-on", ""); panels[i].removeAttribute("aria-hidden"); }
        else { panels[i].removeAttribute("data-on"); panels[i].setAttribute("aria-hidden", "true"); }
        panels[i].inert = !on;
      }
      spent = 0;
      // On a phone the tabs are a row of chips that scrolls: bring the chosen one into it (the
      // row only, never the page).
      if (list.scrollWidth > list.clientWidth + 1) {
        var r = list.getBoundingClientRect(), t = tabs[at].getBoundingClientRect();
        var by = t.left + t.width / 2 - (r.left + r.width / 2);
        if (list.scrollBy) list.scrollBy({ left: by, behavior: reduce ? "auto" : "smooth" });
        else list.scrollLeft += by;
      }
    }
    function running() { return auto && seen && !over && !inside && !document.hidden; }
    function tick(now) {
      frame = 0;
      if (!running()) { last = 0; return; }
      if (last) spent += now - last;
      last = now;
      if (spent >= DWELL) select(at + 1);
      tabs[at].style.setProperty("--p", Math.min(1, spent / DWELL).toFixed(4));
      frame = requestAnimationFrame(tick);
    }
    function wake() { if (running() && !frame) { last = 0; frame = requestAnimationFrame(tick); } }
    function stop() {
      auto = false;
      tour.classList.remove("is-auto");
      if (frame) cancelAnimationFrame(frame);
      frame = 0;
    }
    function choose(k, focus) {
      stop();
      select(k);
      if (focus) tabs[at].focus();
    }

    for (var j = 0; j < tabs.length; j++) {
      (function (k) { tabs[k].addEventListener("click", function () { choose(k); }); })(j);
    }
    // Arrows go through the tabs whichever way they're laid out (a column, or a row on a phone).
    list.addEventListener("keydown", function (e) {
      var k = { ArrowDown: at + 1, ArrowRight: at + 1, ArrowUp: at - 1, ArrowLeft: at - 1, Home: 0, End: tabs.length - 1 }[e.key];
      if (k === undefined || e.altKey || e.ctrlKey || e.metaKey) return;
      e.preventDefault();
      choose(k, true);
    });
    var narrow = window.matchMedia ? window.matchMedia("(max-width: 820px)") : null;
    function orient() { list.setAttribute("aria-orientation", narrow && narrow.matches ? "horizontal" : "vertical"); }
    orient();
    if (narrow && narrow.addEventListener) narrow.addEventListener("change", orient);

    // Held while a mouse is over it (a touch has no hover to end) or the focus is in it.
    tour.addEventListener("pointerenter", function (e) { if (e.pointerType !== "touch") { over = true; } });
    tour.addEventListener("pointerleave", function (e) { if (e.pointerType !== "touch") { over = false; wake(); } });
    tour.addEventListener("focusin", function () { inside = true; });
    tour.addEventListener("focusout", function (e) { inside = !!(e.relatedTarget && tour.contains(e.relatedTarget)); wake(); });
    document.addEventListener("visibilitychange", function () { last = 0; wake(); });
    // A picture opened in the lightbox: the tour stays on it, so closing comes back to it; going
    // through the pictures there takes the tour along (main.js, the lightbox's onChange).
    stage.addEventListener("click", stop);
    stage.addEventListener("tour-show", function (e) {
      var k = panels.indexOf(e.target);
      if (k >= 0 && k !== at) choose(k);
    });
    stage.addEventListener("keydown", function (e) { if (e.key === "Enter" || e.key === " ") stop(); });
    // A sideways swipe on the stage goes to the next or previous feature (touch-action: pan-y
    // leaves the vertical one to the page).
    var swipe = null;
    stage.addEventListener("pointerdown", function (e) {
      swipe = e.pointerType === "touch" ? { x: e.clientX, y: e.clientY, id: e.pointerId } : null;
    });
    stage.addEventListener("pointerup", function (e) {
      if (!swipe || swipe.id !== e.pointerId) return;
      var dx = e.clientX - swipe.x, dy = e.clientY - swipe.y;
      swipe = null;
      if (Math.abs(dx) > 50 && Math.abs(dx) > Math.abs(dy) * 1.5) choose(at + (dx < 0 ? 1 : -1));
    });
    stage.addEventListener("pointercancel", function () { swipe = null; });

    select(0);
    if (auto) tour.classList.add("is-auto");
    // Only while it's on screen: nobody misses a feature that went by out of sight.
    if (!seen) {
      new IntersectionObserver(function (es) {
        seen = es[es.length - 1].isIntersecting;
        if (seen) wake();
      }, { threshold: 0.35 }).observe(tour);
    }
    wake();
  });

  // ---- Your system -------------------------------------------------------------------------
  // Each system's card (.os-card[data-os]) carries its main file and label; the download
  // buttons take the visitor's, and the cards mark it. Phones and tablets keep Windows and see
  // that it's a desktop app.
  function detect() {
    var uad = navigator.userAgentData;
    var platform = ((uad && uad.platform) || navigator.platform || "").toLowerCase();
    var ua = (navigator.userAgent || "").toLowerCase();
    if (/android|iphone|ipad|ipod|mobile/.test(ua) || (uad && uad.mobile)) return "mobile";
    // iPadOS reports a Mac; a touch screen gives it away.
    if (platform.indexOf("mac") === 0 && navigator.maxTouchPoints > 1) return "mobile";
    if (platform.indexOf("win") === 0 || ua.indexOf("windows") >= 0) return "windows";
    if (platform.indexOf("mac") === 0 || ua.indexOf("mac os") >= 0) return "macos";
    if (/linux|x11|cros|bsd/.test(platform + " " + ua)) return "linux";
    return "windows";
  }

  var os = detect();
  if (os === "mobile") {
    each("[data-mobile-note]", function (el) { el.hidden = false; });
    return;
  }
  var card = document.querySelector('.os-card[data-os="' + os + '"]');
  each(".os-card", function (el) { el.classList.toggle("current", el === card); });
  if (!card) return;
  each("[data-os-cta]", function (el) { el.setAttribute("href", card.getAttribute("data-href")); });
  each("[data-os-cta-icon]", function (el) { el.setAttribute("href", "#os-" + os); });
  each("[data-os-label]", function (el) { el.textContent = card.getAttribute("data-label"); });
  // The detected system's card comes first.
  if (card.parentNode && card.parentNode.firstElementChild !== card) {
    card.parentNode.insertBefore(card, card.parentNode.firstElementChild);
  }
})();
