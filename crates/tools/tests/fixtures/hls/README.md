# HLS parity corpus

What `crates/tools/src/manifest.rs` reads of an HLS master playlist must be what yt-dlp lists for
it (`streams_are_read_as_yt_dlp_lists_them`). Each folder is one stream, served on this computer
only, and yt-dlp's own answer for it:

- `master.m3u8`: the master playlist, as served;
- `media.m3u8`: the playlist yt-dlp asked for after it (its "Checking m3u8 live status", which
  reads the duration), as the servers' request logs showed;
- `ytdlp.json`: yt-dlp's answer, trimmed to what `resolve::parse` reads: `title`, `duration`,
  `direct`, `language`; `subtitles` and `automatic_captions` with each file's `name` only (none
  here); and of each format `format_id`, `ext`, `height`, `fps`, `vcodec`, `acodec`, `filesize`,
  `filesize_approx`, `tbr`, `protocol` (each kept only where yt-dlp gave it, nulls included).
  Formats keep yt-dlp's order.

yt-dlp **2026.10.26** (the Windows `yt-dlp.exe` MDM installs), on 2026-11-09. Each case ran

    yt-dlp --ignore-config --no-update --proxy "" -J --no-playlist <address>

(`group` also had `-v`, for its log; it changes nothing in the answer). The addresses:

| Case | Address | Then read |
|---|---|---|
| `lab` | `http://127.0.0.1:8093/hls/master.m3u8` (the lab server's stream) | `/hls/audio/index.m3u8` |
| `group` | `http://127.0.0.1:8094/group.m3u8` | `/a/en.m3u8` |
| `videoonly` | `http://127.0.0.1:8094/videoonly.m3u8` | `/a/en.m3u8` |
| `repeated` | `http://127.0.0.1:8094/repeated.m3u8` | `/v/v720.m3u8` |
| `nocodecs` | `http://127.0.0.1:8094/nocodecs.m3u8` | `/a/en.m3u8` |
| `aes` | `http://127.0.0.1:8094/aes.m3u8` | `/v/aes.m3u8` |
| `portrait` | `http://127.0.0.1:8094/my%20clip.m3u8` | `/v/v720.m3u8` |
| `order` | `http://127.0.0.1:8094/order.m3u8` | `/a/en.m3u8` |
| `redirect` | `http://127.0.0.1:8093/redirect?to=http%3A%2F%2F127.0.0.1%3A8094%2Frepeated.m3u8` | `/v/v720.m3u8` |

Port 8093 is `mdm-labserver`; 8094 served the streams `capture.py` lists (`STREAMS`), then with
Python's `http.server`, as `application/vnd.apple.mpegurl`.

To make the corpus again (it rewrites these folders; nothing is asked of any other host):

    cargo run -p mdm-labserver -- --bind 127.0.0.1:8093
    python -I crates/tools/tests/fixtures/hls/capture.py --ytdlp <yt-dlp 2026.10.26> [--out DIR]

With yt-dlp 2026.10.26 it writes these files byte for byte. A newer yt-dlp may answer otherwise:
then the test says where MDM's reading and yt-dlp's part.
