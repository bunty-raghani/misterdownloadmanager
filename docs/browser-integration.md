# Browser integration

[← Back to docs index](README.md)

With the MDM extension, clicking a download link in your browser hands the download to MDM. MDM gets the address together with the page it came from (the referrer), your cookies for the site and the browser's User-Agent, so downloads that need you to be signed in still work. It also gets the address the browser's own download had reached: when a site refuses MDM the link itself (a one-time link the browser has already used, or a site that lets only browsers ask for its links), MDM goes on from there once, without your cookies if it is another site. When a site's bot check (Cloudflare's, for one) or its refusal of programs (FileHippo's *412 Precondition Failed*) turns MDM's own connection away, MDM asks once more connecting the way your browser does (the same handshake, settings and order of headers), with your browser's User-Agent and its cookies for that site only, and downloads on it with as many connections as any download, each asking for its part of the file (on one connection when the site refuses parts there); your proxy, network adapter and speed limits apply as usual. The pieces of a video stream (HLS, DASH) go the same way when the site's check refuses them. Some checks (SourceForge's) refuse anything that calls itself a browser without connecting exactly like one, and let a program that says what it is through: when that connection is refused too, MDM asks once more under its own name (`MisterDownloadManager/` and its version, or the User-Agent you typed in **Options → Downloads**), with the same cookies and referrer, and remembers the site until MDM closes, so its next downloads, and a paused one when it resumes, go that way at once. It solves no challenge: a site that still wants one answered (some Cloudflare checks ask it of every visitor, for the site's pages too) refuses again, and MDM says *this site lets only browsers through*; Alt+click the link to leave the download to the browser, which opens the link, passes the check and keeps the file.

The extension works in **Google Chrome**, **Microsoft Edge**, **Brave**, **Opera** and **Vivaldi**, in **Firefox** 140 or later, and in **Safari** 17 or later on a Mac (see [Safari](#safari-macos)).

Browsers built on them work too: Chromium and Helium take the extension as Chrome does, Brave Origin as Brave does, Zen and LibreWolf as Firefox does. On Windows, Zen has a bug of its own (see [The browser and MDM aren't connected](browser-help.md#not-installed)).

## Set it up

Browser integration has two parts: a small helper program that the browser talks to, and the extension itself.

1. Install MDM with the setup and keep **Set up browser integration (Chrome, Edge, Brave, Firefox)** ticked. This registers the helper. (The portable copy can't do this; see [Portable mode](portable.md).) On Linux and macOS MDM registers it itself, each time it starts, for the browsers installed then.
2. Add the extension to each browser you use, as below: Firefox from its add-ons store, the others from the Chrome Web Store. Until a browser's extension has reached MDM, MDM's **Add Mister Download Manager to your browser** window opens at each start: it lists the browsers it found, and **Add the extension** opens the store's page in that browser. **Install it by hand** opens the steps for loading it without the store; **Don't ask again** stops the window.

![Add Mister Download Manager to your browser: each browser found, with a button to its store page](images/help/browser-offer.webp)
*MDM's own window opens each browser's store page for you.*

### Firefox

Firefox 140 or later is needed. Install the add-on from its page on addons.mozilla.org, [Mister Download Manager](https://addons.mozilla.org/firefox/addon/mister-download-manager/): click **Add to Firefox**. Firefox keeps it updated. Zen and LibreWolf install it from the same page. (MDM's **Add Mister Download Manager to your browser** window opens that page for you.)

### Chrome, Edge, Brave, Opera and Vivaldi

Install the extension from its page in the Chrome Web Store, [Mister Download Manager](https://chromewebstore.google.com/detail/mister-download-manager/ejoleppdfgjdndddcjoehopknmlglojp): click **Add to Chrome**. In Edge the button is **Get**: Edge first asks to **Allow extensions from other stores** (click **Allow**), then **Add extension**. The browser keeps it updated. MDM's **Add Mister Download Manager to your browser** window opens that page in each browser for you, and Chrome may offer it by itself at its next start once MDM is installed.

To load it by hand instead (a browser that can't use the store, or a build of your own): its files are in the `extension` folder where MDM is installed: `%LOCALAPPDATA%\Programs\Mister Download Manager\extension` if you installed it for yourself, or `Program Files\Mister Download Manager\extension` if you installed it for everyone. Every release also has them as their own download, `...-extension-chromium.zip` (unzip it first).

1. Open the extensions page: `chrome://extensions`, `edge://extensions` or `brave://extensions`.
2. Turn on **Developer mode**.
3. Click **Load unpacked** and choose the `extension\chromium` folder.

**Mister Download Manager** appears in the list. Leave it turned on. Pin its button to the toolbar to see at a glance whether the browser reaches MDM.

### Helium

[Helium](https://helium.computer/) installs the extension from the same page in the Chrome Web Store: click **Add to Chrome**. Helium fetches extensions through its own services, so this works only while they're allowed. If Helium says **Extension downloads are disabled**, open `helium://settings/privacy/services` (**Settings → Privacy and security → Helium services**), turn on **Allow connecting to Helium services** and **Proxy extension downloads and updates**, and click **Add to Chrome** again. With those off, load the extension by hand instead, as above (`chrome://extensions` works in Helium too).

MDM's setup registers the helper where Helium looks for it, on Windows, Linux and macOS. On Linux and macOS MDM also offers the extension to Helium, through Helium's own services, so with them allowed it arrives by itself at Helium's next start (on macOS Helium asks first: **New extension added** in its menu, where you turn it on). The Linux packages (.deb, .rpm) leave that to the window below, writing nothing in your home folder. On Windows Helium can't be offered the extension this way: MDM's **Add Mister Download Manager to your browser** window opens the store's page in Helium when Helium was installed with its setup; Helium from its zip isn't found there, so open the store's page in it yourself. Helium presents itself as Google Chrome, on purpose, so in **Options → General → Capture downloads from the following browsers** it is **Google Chrome**.

### Safari (macOS)

The Mac app carries the Safari extension: there is nothing to download. Safari 17 or later and macOS 14 or later are needed.

1. Drag MDM to Applications and start it once. MDM's **Add Mister Download Manager to your browser** window lists Safari: **How to Add** shows the steps below.
2. In Safari open **Settings → Extensions** and tick **Mister Download Manager**. Click **Always Allow on Every Website**: the extension needs to read the site to pass your sign-in on.
3. While the app is not notarized by Apple (the experimental builds are not), Safari lists the extension only with unsigned extensions allowed: **Settings → Advanced** → **Show features for web developers**, then **Settings → Developer** → **Allow unsigned extensions**. Safari forgets that setting each time it quits.

Safari lets an extension neither see nor stop the browser's downloads, so what works differs from the other browsers:

- **Works as in the other browsers:** the right-click items, the button over videos, **Media on this page**, the toolbar button, **Alt**-click, MDM starting by itself, and every rule in Options.
- **A click on a link to a file type MDM takes** goes to MDM. A download the page starts by itself (a script, or a link with no file name in its address that the server answers with a file) stays with Safari: use right-click → **Download with Mister Download Manager** for those.
- **Not available:** sharing the browser's proxy or VPN, and a download from a form.

### From the plain zip

The `-x64.zip` copy has no installer, but you can register the helper yourself. Open a command prompt in MDM's folder and run:

```
misterdownloadmanager-nmhost.exe --register
```

This is for your Windows account only. `--unregister` undoes it. Then load the extension as above, from the zip's `extension` folder.

## What it takes

When you click a download, the extension checks MDM's Options:

- **Options → General → Use advanced browser integration** must be on. Turn it off to send nothing to MDM.
- **Options → General → Capture downloads from the following browsers** must have your browser ticked.
- **Options → File types → Automatically start downloading the following file types** must list the file's type. The list starts with the usual ones: ZIP, RAR, 7Z, EXE, MSI, ISO, DMG, DEB, RPM, APPIMAGE, MP3, MP4, MKV, WEBM, PDF, AI models such as GGUF and SAFETENSORS, 3D files such as GLB and STL, and many more. Anything else can be added to the list.
- **Options → File types → Don't start downloading automatically from the following sites** must not list the site. Names are separated by spaces; a name covers its subdomains (`example.com` covers `www.example.com`, as `*.update.microsoft.com` covers `dl.update.microsoft.com`; `*` matches anything, as in `*.example.*`), and the files the site's pages link to on another server (a CDN), told by the page the download came from (its referrer). **Edit list ...** below it lists whole addresses to leave alone, one per line (`https://example.com/private/*`).
- **Options → File types → Don't take files smaller than** leaves small files (a receipt, an icon) to the browser. 0 takes any size.
- A `.torrent` file goes to MDM only while **Tasks → Torrent settings → Open magnet links and .torrent files with Mister Download Manager** is on. Off, the browser keeps it and opens it in your own torrent program. See [Torrents](torrents.md#add-a-torrent).
- The extension must be allowed to read the site. When you limit it to some sites (or **On click**), it can't pass your sign-in on, so it leaves those sites' downloads to the browser.

![Options, General: Use advanced browser integration and the list of browsers MDM takes downloads from](images/help/options-integration.webp)
*1 must be on, and 2 your browser ticked.*

When the browser keeps a download, the extension's button says which of these left it there, with the file's name, and MDM's log has a line for each one (`logs\mdm.log` in [MDM's data folder](crash-reports.md#where-reports-are-kept)).

Both the address you clicked and the one its redirects led to are checked, so a site you excluded stays excluded even when it sends you to another server. Everything else stays with the browser. Changes in Options reach the browser within a minute; you don't need to restart it.

When MDM takes a download, it gets the browser's file name, size and type with it, so **Download File Info** shows them at once without asking the server again. A download from a form (a button that posts and gets a file back) is posted again by MDM with the form's data; a form that sends a file stays with the browser.

When MDM takes a download, the browser's own copy is cancelled and removed from the browser's download list. In Firefox and the browsers built on it (Waterfox, Zen, LibreWolf), MDM takes it as soon as the server answers, before the browser asks *What should … do with this file?*: for a file MDM takes, that question never shows (Waterfox asks it for every new type of file). Files MDM doesn't take get the question as before. **Download File Info** opens in MDM (unless you turned it off in **Options → Downloads**). Cancelling there doesn't hand the download back to the browser.

<!-- steps -->

1. ![Download File Info in MDM for a file clicked in the browser](images/help/catch-file-info.webp)
   *Click a download link in the browser: Download File Info opens in MDM.*
2. ![The progress window of a download coming in over several connections](images/help/catch-progress.webp)
   *Start Download: the progress window shows it coming in.*
3. ![Download complete, with Open, Open with..., Open folder and Close](images/help/catch-complete.webp)
   *Download complete: Open the file, or Open folder.*

Downloads from a private window use that window's cookies, never the normal ones, and aren't kept in MDM's list after it closes, unless you tick **Options → General → Keep private-window downloads in the list**: then they are saved like any other and survive a restart (their cookies are never saved).

Files MDM downloads from the browser are marked as coming from the internet, as the browser marks its own: Windows checks a program with SmartScreen before it runs and Office opens a document in Protected View. Like the browser, MDM leaves the mark out when Windows' Group Policy **Do not preserve zone information in file attachments** is on; [Options → Save to](options.md#save-to) can turn it off for MDM alone.

## Files the browser would open

A PDF, a song or a video that the browser would show or play itself goes to MDM too, when its type is in **Options → File types** (not on the excluded sites). That's one you open from a link or the address bar, and one a page shows in a frame, as many course and document sites show their PDFs. To turn this off, untick **Also take files the browser would open** in **Options → General → Keys...**. This needs a Chromium browser (Chrome, Edge, Brave, Opera, Vivaldi) version 128 or later, or Firefox.

A page with a PDF viewer of its own (its own page numbers, zoom and buttons, while the address bar keeps the page's address) reads the file itself and draws it. MDM leaves that file to the page: if MDM took it, the page would get nothing and say the document failed to load. To download such a file, open its address in a tab of its own, or use the page's own download button if it has one.

## The toolbar button

The extension's button shows whether the browser reaches MDM. A red **!** means it doesn't; click the button to see why, and **How to fix it** opens the matching section of [The browser and MDM aren't connected](browser-help.md). While MDM downloads, the button shows how many downloads are running, in blue; **II** means taking downloads is paused. Its popup says under MDM's name whether it is connected, and to which version; the gear opens MDM's **Options**. It has three tabs:

- **Downloads**: MDM's newest downloads (see [Recent downloads](#recent-downloads), below).
- **This page**: **Media on this page**, what the page has loaded that MDM can download (see below), and **Never take downloads from** the current site, a switch: on, the site goes on MDM's excluded sites and the popup says **Downloads from** the site **stay with the browser**, whenever you open it there; off takes it off the list again. A site that a pattern of your own in Options excludes (`*.example.com`) shows the switch on, and greyed: Options holds it.
- **Settings**: **Take downloads from this browser** (off pauses: every download stays with the browser until you turn it on again), the browser's proxy (see below), **Hide the download button in full screen**, **Send downloads to** (this computer or another MDM, see [Remote control](remote-control.md)), **Options...** and **Open Mister Download Manager**.

### Recent downloads

**Downloads** lists MDM's newest downloads, newest first, and keeps them up to date while the popup is open: each with your system's own icon for its file type (as Explorer, Finder or your file manager shows it), its size and what it is doing. A running download shows a progress bar, its speed and the time left; a finished one when it finished; a failed one why; a finished one whose file was deleted or moved since says so. The search box finds downloads by any words of their name or address, and **All**, **Running**, **Done** and **Failed** show only those (each with how many there are). The popup shows the newest 60 that match; MDM's own window lists them all.

Point at a download (or reach it with the keyboard) for **Show in folder** and **Pause** or **Resume**. Click a finished file's name to open it. Its **⋯** menu has the rest:

- **Open**, **Show in folder**: as in MDM; the program or folder opens in front of the browser.
- **Copy file**: the file itself goes on the clipboard, as your file manager's Copy puts it. Paste it into a folder, a chat, an e-mail, or a web page's upload box (browsers take pasted files).
- **Share...**: your system's share sheet with the file (Windows 10 and 11, macOS), at any size. Linux has no standard share sheet, so Share isn't offered there: use **Copy file** or **Show in folder**.
- **Copy link**: the download's address.
- **Download again**: from the start (a file; videos and torrents have their own way in MDM).
- **Remove from list** (the file stays) and **Delete file...**, which asks first: the file is deleted from your computer, which can't be undone.

**Clear finished** takes every finished download off the list; their files stay where they are. When MDM isn't running, the tab says so, with a button to start it.

A popup can't drag a file out of the browser into another program or a web page (browsers don't let an add-on's popup carry a real file on a drag), so **Copy file** and **Show in folder** do that job.

When downloads go to another MDM (a server), the list is that server's: **Pause**, **Resume**, **Remove from list** and **Copy link** work, and the buttons that need the file on this computer (open it, its folder, copy, share, delete) aren't shown; a line at the top says so.

On Linux, **Copy file** works in X11 sessions and in Wayland sessions with XWayland (GNOME, KDE and most others have it): MDM keeps the file on the clipboard until something else is copied.

## A VPN or proxy extension

A VPN or proxy extension is a proxy for the browser only. Switch on **Send downloads through this
browser's proxy or VPN extension** in the extension's toolbar popup (it's off as installed), and
MDM downloads what it takes from the browser through the same proxy (with **Options → Proxy /
Socks → Use proxy/socks from a browser's request**, on by default; see
[Options](options.md#proxy--socks)). Switched off, the extension reads nothing of the browser's
proxy.

- **Firefox** asks you once, at the switch, to allow the extension to read the browser's proxy;
  switching it off takes that permission back. MDM gets the proxy the browser's own request for
  the file took, including the ones a VPN extension picks for each request.
- **Chrome, Edge, Brave, Opera and Vivaldi** can't ask for that permission at the switch, so the
  extension has it from the install (Chrome counts it under the access to every site the extension
  already asks for, so it adds no line to the install prompt and the update asks for nothing new).
  The switch asks nothing. MDM gets the browser's effective proxy, whoever set it (a VPN or proxy
  extension, a policy, the command line): a fixed server for the download's scheme (`http`,
  `https`, `socks4`, `socks5`), unless the address is on its bypass list, or its proxy auto-config
  script (by its address or inline). With no proxy, or the system's, nothing is sent and MDM goes
  by its own Options. A QUIC proxy can't be shared.

In both, the proxy's login is never read: a proxy that asks for the VPN extension's own login
can't be shared, and the download says so rather than going without it.

## Media on this page

Some pages play a video without a link to it, and some players hide it from the download button
too. The toolbar button lists what the page has loaded, so you can still save it:

- **Streams**: HLS (`.m3u8`) and DASH (`.mpd`), the way most sites play long videos. Each stream
  is listed once, however many qualities its player switched between, with its qualities (`1080p ·
  720p`) and its length when the extension can read them. A stream whose address doesn't end in
  `.m3u8` or `.mpd`, served as plain text, is found too: when you open the list (or press the
  download button over its player) and the player plays a stream, the extension reads the first
  few kilobytes of what that player fetched to recognise it. Nothing is read until you ask.
- **Video and sound files** the page played (MP4, WebM, MP3 and the like), with their size and,
  for a video, its picture size.

Start the video first: the list fills as the page loads things, and it's emptied when the tab goes
to another page. Each file is listed under the name it will be saved with. A file the site sends
through a program's address (`stream.php`, `remote_control.php`, `videoplayback`, a bare number)
has no name of its own: it's named after the page (`The page.mp4`; a title's site name at its end,
as in `The page | Example`, is left out), or after the `file=` name in
its address when there is one, with the type its server says it is (MP4, not PHP). With **Prefix
media file names with the page title** on, a file with a name of its own is named after the page,
then that name (`The page - clip.mp4`). Click **Download** beside one: a stream opens MDM's **Download video from a page**
window, which reads its qualities (yt-dlp does the work); a file goes to MDM's list like any other
download. Your cookies for its server, the page as the referrer and your browser's User-Agent go
with it, as they do for a link you click.

Pieces of a stream (its segments) are never listed. A copy-protected stream (DRM: Widevine,
PlayReady, FairPlay, as most paid streaming services use) is listed as **Protected**, with no
**Download** button: no tool may break that protection, and MDM doesn't try.

This needs nothing more from you: the extension already sees what the browser downloads in order
to hand downloads over. What it notes stays in the browser's memory and is forgotten when the
browser closes.

## Right-click: Download with Mister Download Manager

Right-click a link, a video, a sound or a picture and choose **Download with Mister Download Manager**. MDM downloads it whatever its type: you asked for it, so the file types list and the excluded sites don't apply here (they only decide which downloads MDM takes over by itself). A video that plays as a stream goes to the **Download video** window with the stream its player loaded (HLS or DASH), as the [button over videos](#the-download-button-over-videos) sends it, with your cookies for the stream's server and what the browser sent with the stream (its referrer and origin, else the page as the referrer).

Select part of a page and right-click it for **Download selected links with Mister Download Manager**, or right-click the page for **Download all links with Mister Download Manager**: the links open in **[Link grabber](link-grabber.md)**, sorted into packages, where you tick the ones you want. A page with tens of thousands of links works too: the extension sends a long list in parts, and Link grabber shows it once they are all in.

If MDM can't be reached when you choose one of these, the help page opens at the section that explains why.

## The download button over videos

![The Download video button over a YouTube video in Chrome](images/help/video-button.webp)
*The button over a video. Drag it where you like; it stays there.*

Point at a video on a web page and a small tab appears at its top corner: MDM's logo and a green **Download video** part, an arrow for its list and **✕** to hide it on this video. It follows Windows' light or dark mode. As it appears, MDM looks the video up (when MDM is running), and the button then names the best quality it found (**1080p**, **4K**). Click it and that quality downloads at once, with only its progress window; MDM's main window stays where it is (with **Do not start downloading, only add files to the queue** on in Options, it goes in the queue instead). Clicking while MDM is still looking works too: it takes the best quality as soon as it is known. A video that plays a plain file (not a stream) needs no look-up: the button shows its picture size and downloads the file itself. When nothing could be looked up (MDM wasn't running, yt-dlp isn't installed, the page is a playlist, or you send downloads to another MDM), a click opens the **Download video from a page** window, which says why and offers what to do.

It takes the video you clicked: its own file when it plays one, or the stream the player loaded (HLS or DASH), else the page. In a feed it takes the post the video sits in. When MDM finds nothing there (a feed that names no post, a post or a site yt-dlp can't read, such as Douyin's recommended videos), the button offers what the tab played for that video instead: the file or the stream whose length is the video's, never the next video the feed loaded meanwhile. When more than one could be it, it says **Can't tell which video this is** rather than guess; the toolbar button's [Media on this page](#media-on-this-page) lists what the page loaded. Embedded players work too, and your sign-in for the site, the page that embeds the player and your browser's User-Agent go with it, so videos for members, or that play only on the site that embeds them, can be downloaded. A stream goes with what the browser sent when the player asked for it, its referrer, its origin and its cookies (a player embedded from another site may have cookies the browser keeps for that page only), and MDM sends them again with each of its playlists and pieces: a site's video server that serves its own pages' players only lets MDM through as well. A video the site names (YouTube, Vimeo and the others yt-dlp reads) keeps its own title; a stream known by no title of its own (its address is only `playlist.m3u8`) is named after the page you're on, without the site's name at its end: on a page titled `A film | Example`, it's `A film.mp4`, even when the player is embedded from another site. Sound players get the button too.

After a click it says what happened, in its own colour:

- **Added to MDM** (green), with the quality: **Show** brings MDM's window up with that download selected.
- **Open the post to download it** (amber): the video sits in a feed and can't be downloaded from there. **Open post** opens the post in a new tab, when the button knows it; use the button there.
- **MDM didn't answer** (red): MDM isn't installed or didn't start. **Get MDM** opens its download page; **Retry** tries again.
- **Off on this site** (grey): after **Never on** the site, or when Options keep it off there. **Turn on** shows it on the site again.
- **Protected video · can't be saved** (grey, with a lock): a copy-protected video (DRM), which no tool may save.

It stays out of the way: it shows only while you move the pointer over a video (or the player has the keyboard's focus; **Tab** then moves to the button), fades 1.5 seconds after the pointer stops and at once when it leaves the video, and leaves out small videos and short muted loops (animated pictures, backgrounds). It doesn't cover the page's own buttons: when one lies under any part of its corner (a player's feedback or settings button, a thumbnail's watch-later button), it takes the next free corner and keeps it while the player shows and hides its buttons, so it stays put as you reach for it. Over a preview that plays while the pointer is on a thumbnail (YouTube's home page, a search, a channel), the page stops the preview as the pointer reaches the button; the button stays where it was while the pointer is on it, and a click downloads that preview's video. Playing or pausing the video doesn't bring it back. On a video under 360 pixels wide, or where its words wouldn't fit, it is only its logo, its arrow and **✕**; with **Button size** set to **Compact** in Options (below), it is that round button on every video. In fullscreen it follows the video when the player allows it, and entering full screen leaves it out of the way until you move the pointer again.

- **Its menu** (the arrow next to it, or **↓** when the button has the keyboard's focus): the video's title, length and site; **Download best** first, then one numbered line per quality under **VIDEO** and **AUDIO**, with its file type, codec, frame rate or bitrate and size (`1080p` `MP4 · AV1 · 60 fps`, as the Download video window says it), the best one marked, and **Most compatible (H.264 + AAC)** beside the resolution of the one that plays everywhere ([Choose the quality](video.md#choose-the-quality)); **All qualities and subtitles...**, which opens the **Download video from a page** window; then **Hide on this video** (**Esc**), **Never on** the site (MDM remembers the site), and **Button settings...**, which opens MDM's Options. The arrow keys move through it, **Enter** picks, **Esc** closes it.
- **Move it**: drag it by its dots along the video's edge; while you drag, the edge and the spots it snaps to (the corners and the middle) show. It stays there on that site.
- **Turn it off, make it smaller, or see the sites it stays off**: **Options → General → Customize MDM Download panels in browsers → Edit...**. **Button size** is **Full** as installed (its words where they fit) or **Compact** (only MDM's logo, the arrow and **✕**, on every video, as on a small one); after a click it still says what happened in words. Pages already open follow a change within a few seconds, without a reload.

![The Download panels settings: the button on or off, its size, and the sites it never shows on](images/help/download-panels.webp)
*Options → General → Edit...: turn the button off, choose its size, or list sites it never shows on.*

The button follows the rest of the integration: it doesn't show when **Use advanced browser integration** is off or the browser is unticked.

## Keys to force or prevent capture

Hold **Alt** (**Option** on a Mac) while clicking a link to let the browser download it instead of MDM. The extension follows the link as a plain click would, in every browser, and the browser keeps what comes of it: the link opens in the tab, so a site's bot check (Cloudflare's "Just a moment...") runs there first. Without the extension in the way, Chromium browsers and Safari would save the link without opening it, which saves the check's page instead of the file; Firefox would do nothing. When the link redirects, or the page it leads to starts the download by script ("your download starts in 5 seconds"), that download stays with the browser too when it starts within ten seconds; the link's own address, and a download its own page starts (a bot check's page once its check is passed), within a minute. **Alt** with **Ctrl**, **Shift** or **Cmd**, and the middle button, keep their meaning (a new tab or window). **Options → General → Keys...** says so, and holds **Also take files the browser would open**.

To keep a site's downloads in the browser for good, use the toolbar button's **Never take downloads from**, or add the site to **Don't start downloading automatically from the following sites**. To keep a file type in the browser, take it off the file types list.

## When MDM isn't running

You don't need to start MDM first. When the browser hands over a download and MDM isn't running, the helper starts it and passes the download on.

If MDM can't be reached at all (it was uninstalled, or the helper isn't registered), the download stays with the browser, so nothing is lost, and the toolbar button shows a red **!**. See [The browser and MDM aren't connected](browser-help.md), or run **Options → General → Test browser integration...**.

![Options, General with Test browser integration...](images/help/test-integration.webp)
*Test browser integration... says whether the browser reaches MDM.*

## Next steps

- [Tutorial: catch downloads from your browser →](guide-catch-browser-downloads.md)
- [Change which file types MDM takes →](options.md)
- [If the browser doesn't hand downloads over →](troubleshooting.md)
