# Command line and automation

[← Back to docs index](README.md)

MDM can be driven without its window in three ways:

- **The app's own command line**, the same switches as IDM's: hand a download to the running app from a script, a batch file or another program.
- **`misterdownloadmanager-cli`**, a separate command-line tool: download files, videos and torrents in a terminal, with progress, without the app.
- **The automation API**: other programs list the running app's downloads (as JSON) and pause, start, remove or delete them, over a local, token-protected channel. AI assistants use it through MDM's MCP server: see [AI assistants](ai-assistants.md).

To run the whole app without a window, on a home server or a NAS, see [Run MDM on a server](server.md) (`misterdownloadmanager --headless`); to control it from a phone, [Control MDM from your phone](remote-control.md).

The examples use Windows' paths; on macOS and Linux the programs have the same names without `.exe`.

## The app's command line

```
misterdownloadmanager.exe /d URL [/p folder] [/f name] [/a] [/n] [/s]
```

| Switch | What it does |
|---|---|
| `/d URL` | The address to download. |
| `/p folder` | Save it in this folder (else the category's folder from **Options → Save to**). |
| `/f name` | Save it under this name. |
| `/a` | Add it to the main download queue without starting it. |
| `/n` | Silent: no **Download File Info** window, the download starts (or is queued, with `/a`) at once. |
| `/s` | Start the main download queue. |
| `/exit` | Save and close the running app (the installer uses it before an upgrade). |
| `/startup` | Start in the notification area, the window hidden: what **Launch Mister Download Manager on startup** starts MDM with when you sign in. Where no notification area shows MDM's icon (some Linux desktops), MDM starts minimized. A running MDM stays as it is. |
| *a `.torrent` file or `magnet:` link* | Open it in **Add torrent** (what Windows passes when MDM opens torrents). |

Switches can start with `/` or `-`, in any case. If MDM is running, the command goes to it; if not, MDM starts and carries it out. For example:

```
misterdownloadmanager.exe /d https://example.com/files/report.pdf /p D:\Reports /n
misterdownloadmanager.exe /d https://example.com/big.iso /a
misterdownloadmanager.exe /s
```

## misterdownloadmanager-cli

It comes with every download of MDM, next to the app (the installer puts it in MDM's folder). It works on its own: the app needn't run, except for `status` and `api`.

| Command | What it does |
|---|---|
| `get URL [-o folder\|file] [-c connections] [-H "Header: value"]... [--checksum hash] [--dns choice] [--adapter name]` | Download a file with up to 8 connections (`-c`, 1 to 128), showing progress. `Ctrl+C` pauses it; `resume` continues. `--checksum` checks the finished file: hex MD5, SHA-1, SHA-256, SHA-512 or CRC32, optionally named (`sha256:…`). `--dns` picks the DNS: `system` (the default); a service and how to ask it, `cloudflare-https`, `quad9-tls`, `google-plain`, `adguard-https`, `nextdns-tls`; DNS servers' addresses, `9.9.9.9,149.112.112.112`; or a DNS-over-HTTPS address. It never falls back to the system's DNS. `--adapter` sends every connection through one network adapter, by its name (`Ethernet`, `wg0`) or one of its addresses; when it isn't connected, nothing is sent and the download fails. A link from a file host that one of your debrid accounts (**Options → Accounts**) supports goes through that account, as in the app, and `get` says `through Real-Debrid`; `--direct` downloads the address itself ([Debrid service accounts](debrid.md)). |
| `resume file.mdm.json [-c connections] [-H ...] [--direct]` | Continue a paused or broken download from the `.mdm.json` file next to it. Through a debrid account, as `get`: the service gives a new link when the old one expired; `--direct` continues from the address itself. |
| `install-gallery-dl` | Install or update gallery-dl, for image galleries: its checksum list is checked against its author's signing key, the program against that list (Windows and Linux x86-64; elsewhere install it yourself). |
| `gallery-get URL [-o folder] [--limit N] [--adapter name]` | Download a gallery's pictures (a Wikimedia Commons category, a Flickr album, ...) into a folder named after it inside `-o`; `--limit` takes at most that many, the first the site lists. Started again, it fetches only the new ones. MDM's own downloader brings the pictures gallery-dl lists, 4 at a time. `--adapter` sends the connections, MDM's and gallery-dl's, through one network adapter, as for `get`. |
| `video-info URL` | What video a page holds (YouTube, Vimeo, ...), and the qualities to choose from. |
| `playlist-info URL [--from N] [--count N]` | A playlist's or channel's videos, one line each as they're listed: its place, length, title and address. |
| `playlist-get URL [-f selector] [-o folder] [-c pieces] [--from N] [--count N]` | Download a playlist's or channel's videos one after another, into a folder named after the list inside `-o`. `-f` is one quality for all, as for `video-get` (by default each video's best; `bv*[height<=720]+ba/b[height<=720]` = up to 720p; `ba` = the original sound; `mp3:320`, `opus:`, `flac:` as for `video-get`). `-c` as for `video-get`. |
| `video-get URL [-f selector] [-o folder] [-c pieces] [--subs langs] [--auto-subs] [--embed-subs] [--adapter name]` | Download a video; `-f` takes a quality's selector from `video-info` (`b` = the best single file; `ba` = the sound alone as the site has it, not converted; `mp3:320`, `mp3:256`, `mp3:192` or `mp3:128` = the sound as an MP3 at that bitrate, `mp3:0` at its best variable one; `opus:` = as Opus; `flac:` = as FLAC, lossless but no better than the site's sound; conversions need ffmpeg). `-c`: MDM's own downloader brings each stream yt-dlp finds over this many connections (8 by default, 1 to 128), a stream in pieces (HLS, DASH) this many pieces at once. `--subs` saves subtitles beside it: language codes from `video-info` (`en,fr`), or `all` for every one the uploader gave; `--auto-subs` takes the site's automatic captions instead; `--embed-subs` puts them into the video (needs ffmpeg, MP4 or MKV). With ffmpeg, the video's chapters go into the file too, as Download video's **Embed chapters** does. `--adapter` sends the downloads' connections, and yt-dlp's (and ffmpeg's), through one network adapter, as for `get`; when it isn't connected, nothing starts. |
| `install-ytdlp`, `install-ffmpeg` | Install or update the helpers videos need: yt-dlp (checked against its signing key) and ffmpeg (Windows; on macOS and Linux MDM uses the system's). |
| `torrent-info SOURCE` | A torrent's name, files (numbered) and size; `SOURCE` is a magnet link or a `.torrent` file. |
| `torrent-get SOURCE [-o folder] [--files 0,2] [--use-existing]` | Download a torrent, or only some of its files. If some of its files are already in the folder, it stops and names them; `--use-existing` uses them instead (what matches is kept, the rest downloaded). |
| `torrent-create PATH -o file.torrent` | Make a `.torrent` of a file or folder (peers find it through the DHT). |
| `status` | The running app's downloads, as JSON, with their ids (the automation API must be on). |
| `pause ID...`, `start ID...` | Pause running downloads, or start (resume) stopped, queued or failed ones, in the running app. |
| `remove ID...`, `delete ID...` | Take downloads off the running app's list: `remove` keeps their files, `delete` deletes them too. A running download is paused first (`pause`). |
| `api on`, `api off` | Turn the running app's automation API on or off. |

`misterdownloadmanager-cli help <command>` shows every option. A command that fails (the app isn't running, it answers with an error, a download breaks) exits with code 1. For example:

```
misterdownloadmanager-cli get https://example.com/big.iso -o D:\Downloads -c 16 --checksum sha256:9f86d0…
misterdownloadmanager-cli video-get "https://www.youtube.com/watch?v=aqz-KE-bpKQ" -f "b[height=720]"
misterdownloadmanager-cli torrent-get "magnet:?xt=urn:btih:…" --files 0
```

### JSON for scripts and programs

With `--json` (before or after the command), every command prints JSON instead of text, one object per line, for scripts and AI agents to read:

| Command | Prints |
|---|---|
| `get`, `resume` | `{"event": "through", "service"}` first when the download goes through a debrid account, then a `{"event": "progress", "downloaded", "size", "speed", "connections", "segments"}` line every half second, then `{"event": "done", "path", "size", "seconds"}`, or `{"event": "paused", "state"}` after `Ctrl+C` (`state` is the file `resume` takes). |
| `video-get` | `{"event": "progress", "downloaded", "size", "speed"}` lines, then `{"event": "done", "path", "subtitles", "subtitle_problems"}`: the subtitle files saved beside it, and the languages the site refused (`{"code", "name", "reason", "beside"}`; `beside`: they came on a second try, beside the video instead of in it). |
| `torrent-get` | `{"event": "progress", "phase", "downloaded", "size", "speed", "peers", "peers_seen"}` lines, then `{"event": "done", "path", "size"}`, or `{"event": "stopped", "downloaded", "seconds"}` with `--seconds`. |
| `video-info` | `{"title", "choices": [{"label", "format", "size", "approx", "height", "fps", "codec", "range"}], "subtitles": [{"code", "name", "automatic"}]}`; a choice's `format` is what `video-get -f` takes (the very formats it was sized from, as `401+251`, then the nearest should those be gone), `approx` says its `size` is an estimate, `codec` is `AV1`, `VP9`, `H.264`…, `range` is `HDR` (or `SDR` beside an HDR choice); a subtitle's `code` is what `--subs` takes. |
| `torrent-info` | `{"name", "info_hash", "size", "files": [{"index", "size", "path"}]}`. |
| `torrent-create` | `{"info_hash", "path"}`. |
| `install-ytdlp`, `install-ffmpeg` | `{"tool", "version", "path"}`. |
| `status`, `pause`, `start`, `remove`, `delete`, `api` | The app's answer, which is JSON already (see below). |

Sizes are in bytes (`null` while unknown), speeds in bytes per second. A failure prints `{"error": "…"}` and exits with code 1. For example:

```
misterdownloadmanager-cli --json get https://example.com/big.iso -o D:\Downloads
{"event":"progress","downloaded":4194304,"size":1073741824,"speed":52428800,"connections":8,"segments":8}
…
{"event":"done","path":"D:\\Downloads\\big.iso","size":1073741824,"seconds":20.4}
```

## The automation API

Other programs can ask the running app what it is downloading, and pause, start, remove or delete downloads. The API is **off by default**; turn it on with `misterdownloadmanager-cli api on` (and off with `api off`).

### How a program talks to MDM

1. While it runs, MDM listens on a port of `127.0.0.1` (this computer only) and writes the port and a secret token to `ipc.json` in its data folder:
   - Windows: `%APPDATA%\misterdownloadmanager\ipc.json`
   - macOS: `~/Library/Application Support/misterdownloadmanager/ipc.json`
   - Linux: `~/.local/share/misterdownloadmanager/ipc.json`
   - A [portable copy](portable.md): `data\ipc.json` next to the program.

   The token changes at every start; only programs that can read your data folder (yours) can use it.
2. Connect to that port and send one line of JSON: the token and the command's fields, e.g. `{"token": "<token>", "query": "list"}`.
3. MDM answers with one line: `ok` for a command, or a JSON object for a question.

### What it understands

| Fields (besides `token`) | Answer |
|---|---|
| `{"query": "list"}` | Every download and the total speed: `{"downloads": [{"id", "file", "url", "path", "status", "size", "downloaded", "speed", "added", "category", "error"}], "speed": bytes per second, "queues": [{"name", "running", "ids"}], "categories": [{"key", "name"}], "free": bytes}`. `status` is `Queued`, `Downloading`, `Stopped`, `Complete` or `Error`; `size` is `null` while unknown; `error` (why it failed) is there only for a download in `Error`; `category` is a category's `key`; `free` is the free space where downloads go (`null` when unknown). Only while the API is on. |
| `{"action": "pause", "ids": [3, 4]}` | Acts on downloads by id: `"pause"`, `"start"`, `"remove"` (file kept) or `"delete"` (file deleted too). Answers `{"done": [3], "skipped": [{"id": 4, "why": "not running"}]}`; a running download must be paused before it is removed or deleted. Only while the API is on. |
| `{"action": "start_queue", "queue": "Main download queue"}` | Starts (or with `"stop_queue"`, stops) the queue of that name (a name from the list's `queues`). Answers `{"done": [], "skipped": []}`, or `{"error": …, "code": "unknown-queue"}`. Only while the API is on. |
| `{"api": true}` / `{"api": false}` | Turns the API on or off: `{"api": true}`. |
| `{"url": "https://…", "dir": "D:\\Files", "name": "a.zip", "silent": true}` | Adds a download, as the command line's `/d /p /f /n` do (`"later": true` is `/a`); `ok`. `"category"` (a key from the list's `categories`) puts it in that category's folder instead of the one its type picks; `"queue"` (a queue's name) is the queue it waits in with `"later"`. Also with the API off, like the command line. |
| `{"start_queue": true}` | Starts the main download queue; `ok`. |

An unknown question or action is answered `{"error": "…"}`, as is a question while the API is off (then with `"code": "api-off"`, for programs to check: the error text may change). A wrong token is answered `denied`.

Every request and answer is described, field by field, in a JSON Schema: [automation-api.schema.json](automation-api.schema.json) (also at `https://misterdownloadmanager.com/automation-api.schema.json`). It is made from the code the app answers with, so it stays exact; programs can check what they send and read against it, or generate their types from it.

### Examples

PowerShell (Windows):

```powershell
$ep = Get-Content "$env:APPDATA\misterdownloadmanager\ipc.json" | ConvertFrom-Json
$client = New-Object Net.Sockets.TcpClient("127.0.0.1", $ep.port)
$stream = $client.GetStream()
$writer = New-Object IO.StreamWriter($stream); $reader = New-Object IO.StreamReader($stream)
$writer.WriteLine((@{ token = $ep.token; query = "list" } | ConvertTo-Json -Compress))
$writer.Flush()
$reader.ReadLine() | ConvertFrom-Json | Select-Object -ExpandProperty downloads | Format-Table file, status, downloaded, size
$client.Close()
```

Python:

```python
import json, os, socket

ipc = os.path.join(os.environ["APPDATA"], "misterdownloadmanager", "ipc.json")  # see above elsewhere
ep = json.load(open(ipc))
with socket.create_connection(("127.0.0.1", ep["port"])) as s:
    s.sendall((json.dumps({"token": ep["token"], "query": "list"}) + "\n").encode())
    answer = json.loads(s.makefile().readline())
for d in answer["downloads"]:
    print(d["file"], d["status"], d["downloaded"], d["size"])
```

To add a download instead, send `{"token": "…", "url": "https://example.com/a.zip", "silent": true}`.
