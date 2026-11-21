# YouTube's refusals, in yt-dlp's words

What `crates/tools/src/youtube.rs` reads to tell a refusal of the signed-in path (#686), which a
look-up without the browser's cookies gets past, from a video that needs a sign-in, which keeps
them. Each file is yt-dlp's stderr as it wrote it.

Run on 2026-11-14 with yt-dlp **2026.10.26** (the Windows `yt-dlp.exe` MDM installs), without
any cookies and signed in nowhere:

    yt-dlp --ignore-config --no-cache-dir -J --no-playlist https://www.youtube.com/watch?v=<id>

- `private.stderr`: a private video (`yZIXLfi8CZQ`, yt-dlp's own test of one);
- `age.stderr`: an age-restricted one (`Tq92D6wQ1mg`, also from yt-dlp's tests);
- `members.stderr`: a channel members' video (`2a1cufNNiyQ`, the first entry of the channel's
  `UUMO` members-only list, yt-dlp's test of `subscriber_only`);
- `stream-403.stderr`: a public video (`jNQXAC9IVRw`) looked up, then downloaded with
  `--load-info-json` from a copy of its answer whose stream addresses had their `expire` moved
  into the past: googlevideo.com refused the stream as it refuses the reporter's, in the very
  words of #686;
- `cookies-rotated.stderr`: the same public video looked up with a cookie file of made-up
  values (no account's): yt-dlp's warning when YouTube doesn't take the sign-in cookies, said
  once per request (one line kept). The look-up itself worked.

Not seen from this computer (YouTube answers them to some networks and sessions), so written
as yt-dlp's source puts them together (`extractor/youtube/_video.py`: YouTube's
reason, then `_youtube_login_hint`; and `networking`'s `HTTPError`), for the same public video:

- `bot-check.stderr`: YouTube's "Sign in to confirm you’re not a bot" (its curly apostrophe);
- `api-403.stderr`: YouTube's player API refusing the look-up.

Run on 2026-11-20 with yt-dlp **2025.06.30** (its Windows folder build), a release old enough that
YouTube no longer serves the players it asks as, for the same public video (#1544: what an
out-of-date yt-dlp says):

    yt-dlp --ignore-config --no-cache-dir --no-update -f "bv*+ba/b" -s --no-playlist https://www.youtube.com/watch?v=jNQXAC9IVRw

- `outdated-client.stderr`: its last two lines.
