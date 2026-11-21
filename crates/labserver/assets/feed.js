// labserver's feed players (/feed, #1092): each <video data-secs> gets a MediaSource (a blob:
// address, as a feed's player gives it) of that length, then fetches what it plays as such a
// player does: a whole MP4 file in ranges (data-file; data-preload: only its start, the next video
// of a feed), or an HLS stream (data-hls: its master, the first variant's and the sound's
// playlists, every segment). It decodes nothing. The players start one after the other, 3 s
// apart, as a feed loads the next post while one plays; <body data-feed> is "done" once all have
// loaded what they load (or "error", with the reason in data-feed-error).
"use strict";
(() => {
  const videos = [...document.querySelectorAll("video[data-secs]")];
  const fetchOk = async (url, init) => {
    const r = await fetch(url, init);
    if (!r.ok) throw new Error(`${url}: ${r.status}`);
    return r;
  };
  const lines = (s) => s.split(/\r?\n/).map((l) => l.trim()).filter(Boolean);

  async function file(v) {
    const url = new URL(v.dataset.file, location.href);
    const first = await fetchOk(url, { headers: { Range: "bytes=0-65535" } });
    const total = Number((/\/(\d+)\s*$/.exec(first.headers.get("content-range") || "") || [])[1]);
    await first.arrayBuffer();
    if (v.dataset.preload || !(total > 65536)) return;
    await (await fetchOk(url, { headers: { Range: `bytes=65536-${total - 1}` } })).arrayBuffer();
  }

  async function hls(v) {
    const master = new URL(v.dataset.hls, location.href);
    const l = lines(await (await fetchOk(master)).text());
    const variant = l.find((_, i) => i > 0 && l[i - 1].startsWith("#EXT-X-STREAM-INF"));
    const audio = /URI="([^"]+)"/.exec(l.find((x) => x.startsWith("#EXT-X-MEDIA:TYPE=AUDIO")) || "");
    for (const p of [variant, audio && audio[1]].filter(Boolean)) {
      const playlist = new URL(p, master);
      for (const seg of lines(await (await fetchOk(playlist)).text()).filter((x) => !x.startsWith("#"))) {
        await (await fetchOk(new URL(seg, playlist))).arrayBuffer();
      }
    }
  }

  async function play(v) {
    const source = new MediaSource();
    v.src = URL.createObjectURL(source);
    await new Promise((open) => source.addEventListener("sourceopen", open, { once: true }));
    try {
      source.duration = Number(v.dataset.secs);
    } catch {
      // A browser that wants data first: the length stays unknown, as it may on a real feed.
    }
    await (v.dataset.hls ? hls(v) : file(v));
  }

  const ends = videos.map((v, i) => new Promise((wait) => setTimeout(wait, 1500 + 3000 * i)).then(() => play(v)));
  Promise.all(ends).then(
    () => (document.body.dataset.feed = "done"),
    (e) => {
      document.body.dataset.feed = "error";
      document.body.dataset.feedError = String(e && e.message);
    },
  );
})();
