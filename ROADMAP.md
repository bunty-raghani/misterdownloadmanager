# Roadmap

What's done for version 0.5.2, what's being built, and what's planned. Each item links to its
issue, where the details and the discussion are. Released changes are in
[CHANGELOG.md](CHANGELOG.md). Bug reports are welcome in
[Issues](https://github.com/anefzaoui/misterdownloadmanager/issues), ideas in
[Discussions](https://github.com/anefzaoui/misterdownloadmanager/discussions/categories/ideas).

✅ done · 🔄 being built · 📋 planned · 🔍 looking into it

## Version 0.5.0

Everything below ships together, in version 0.5.0.

### Done

- ✅ Menus on compositors that animate windows (Hyprland): no border and no slide-in, and going from
  title to title moves one window, so nothing fades; the reporter confirmed it
  ([#277](https://github.com/anefzaoui/misterdownloadmanager/issues/277))
- ✅ A very slow first window on Windows: MDM draws with Direct3D 12 by default and keeps it through
  a slow start ([#57](https://github.com/anefzaoui/misterdownloadmanager/issues/57))
- ✅ A crash on Linux as Download File Info opens
  ([#48](https://github.com/anefzaoui/misterdownloadmanager/issues/48))
- ✅ Linux on GNOME Wayland: right-click menus open where you click, and windows fit their buttons
  ([#29](https://github.com/anefzaoui/misterdownloadmanager/issues/29))
- ✅ Windows 11 Insider builds: no black band across the top of menus, the drop target, the
  progress window and the windows whose title bar MDM draws (the style themes); the reporter
  confirmed it ([#208](https://github.com/anefzaoui/misterdownloadmanager/issues/208))
- ✅ The progress window's Download status tab: as much room above its first row as below its
  last; where the server is says "this computer" or "the local network", each with its icon
- ✅ **View → Interface size**: every window larger, 100 % to 200 %, with Ctrl + / Ctrl − / Ctrl 0
  as in a browser; a window larger than the screen scrolls. Sharper text on Windows (part of
  [#262](https://github.com/anefzaoui/misterdownloadmanager/issues/262))
- ✅ Windows: **View → Text → Smooth** draws the text anti-aliased at its own size, not as
  thick, hard-edged whole-pixel letters that read badly on a 1440p screen; **Classic**, the
  default, keeps the hinted look MDM had. **Interface size** gains 110 %, 115 % and 120 % between 100 % and
  125 %
- ✅ Windows: sharp, evenly spaced text at every interface size: letters no longer run together
  or drift apart ([#323](https://github.com/anefzaoui/misterdownloadmanager/issues/323),
  [#334](https://github.com/anefzaoui/misterdownloadmanager/issues/334))
- ✅ Linux: Update now asks the package manager which package MDM is; a copy installed by
  another package manager (pacman) is left to the system's updates, with a plain message
- ✅ Linux: the .deb and .rpm install the browsers' host files for everyone and remove them on
  uninstall, so nothing is left in the home folders for Chrome, Chromium, Edge, Brave, Vivaldi and
  Firefox (the Snap too); the guide says what the AppImage and the tar.gz leave, and how to remove
  it ([#266](https://github.com/anefzaoui/misterdownloadmanager/issues/266))
- ✅ Linux: menus and drop-down lists open under what was clicked on tiling compositors (sway)
  too, through XWayland, as on GNOME and KDE
  ([#267](https://github.com/anefzaoui/misterdownloadmanager/issues/267))
- ✅ Linux: a library the window needs that the system lacks (libxkbcommon-x11 and the other X11
  ones, from the tar.gz and the AppImage) is named at start, on the terminal and in a message box,
  with the command that installs it, instead of a crash; in a Wayland session the window opens
  on Wayland instead ([#265](https://github.com/anefzaoui/misterdownloadmanager/issues/265))
- ✅ Linux: plain HTTP downloads work on a system without CA certificates, and HTTPS ones say to
  install them ([#264](https://github.com/anefzaoui/misterdownloadmanager/issues/264))
- ✅ Linux: torrents and magnet links work on a system without CA certificates too (peers, the
  DHT and `http://` trackers; `https://` trackers say why they can't), part of
  [#264](https://github.com/anefzaoui/misterdownloadmanager/issues/264)
- ✅ Update now finds the Linux and macOS release files whether or not their names end in
  "-experimental"
- ✅ The Linux release files are named without "-experimental" (`…-linux-x86_64.deb` and the
  others), in the packages' descriptions too; the macOS ones keep it
- ✅ Linux packages checked in clean Ubuntu 24.04, Debian 12 and Fedora 45 systems: install, the
  command line, the window, the desktop entry and icon, browser integration and uninstall; the
  .deb and .rpm now bring the keyboard library MDM's window needs under X11 and the CA
  certificates downloads need, and require the glibc their programs were built for (part of
  [#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164))
- ✅ Options → Proxy's Get System and the Scheduler's "when done" drop-down fit in the last
  languages that still cut them (Bulgarian, Armenian, Macedonian, Tamil; Greek, Malayalam, and
  six the translation review lengthened), and the Proxy
  page's title no longer runs over its icon or past the window's edge in Greek, Finnish, Catalan
  and a few more: a title too long for one line beside Get System takes two
  ([#192](https://github.com/anefzaoui/misterdownloadmanager/issues/192))
- ✅ Linux: galleries work with the gallery-dl the distribution ships (Ubuntu 24.04's 1.26.9
  failed at once): MDM passes only the options that release accepts, from 1.20 on, and an older
  one gives way to MDM's own copy, saying why; the lab's test streams are real ones, which ffmpeg
  6.1 and 8 download to the end
  ([#236](https://github.com/anefzaoui/misterdownloadmanager/issues/236))
- ✅ Run MDM on a server without a window (Docker, Unraid, any Linux machine), and send it
  downloads from the phone or the browser extension
  ([#157](https://github.com/anefzaoui/misterdownloadmanager/issues/157))
- ✅ Better translations in every language: each language has a style guide and the terms its
  reviewers settled, and every machine translation of the program and the website was reviewed
  again against them
- ✅ Linux: browser integration reaches Brave Origin (and the Beta, Dev and Nightly versions of
  Brave, Chrome and Edge, and Vivaldi's snapshot); on macOS Brave is set up where it reads it,
  Chrome's folder. From the AppImage, the helper program the browsers start is copied out
  reliably, even while a browser still runs the old one, and put back at each start if it went
  missing; Test browser integration says when it is missing, or why setting it up failed
  ([#253](https://github.com/anefzaoui/misterdownloadmanager/issues/253))
- ✅ The extension's popup shows **Open Mister Download Manager** whole: it was cut to "Open Mister
  Do…" on Linux and in longer languages; Options moves below it when both don't fit on one line
  (part of [#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164))
- ✅ The crash report window's status line (such as "The symbols couldn't be fetched: …") stays
  inside the window: cut with "…" at its margin and shown whole under the pointer; its title is
  in MDM's language, and its title bar in MDM's light or dark (part of
  [#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164))
- ✅ Control MDM from a phone or another computer on the local network: **Options → Remote**, off
  by default; a paired device adds links, magnets and torrents, sees the list and pauses, starts
  and removes downloads ([#141](https://github.com/anefzaoui/misterdownloadmanager/issues/141))
- ✅ Linux: moving the pointer from an open submenu back onto its menu, or to another item, closes
  only the submenu, not the whole menu
  ([#250](https://github.com/anefzaoui/misterdownloadmanager/issues/250))
- ✅ Download File Info's Preview, Options → Proxy's Get System and the Scheduler's "when done"
  drop-down fit their translated text (Russian and others), after #212
  ([#192](https://github.com/anefzaoui/misterdownloadmanager/issues/192))
- ✅ A menu of the menu bar shut with Escape opens again at the first click on its title (on
  Linux it took two; on Windows, Escape with the pointer on the title left the menu open)
- ✅ A YouTube look-up that fails with "Requested format is not available" (signed in, no
  JavaScript runtime for yt-dlp) tries once more without the cookies; the video window says when
  ffmpeg would bring the better qualities a page offers
- ✅ Signed-in videos from YouTube Music and YouTube's mobile site: the browser's cookies reach
  the YouTube host yt-dlp asks, each for the domain the browser keeps it for
  ([#229](https://github.com/anefzaoui/misterdownloadmanager/issues/229))
- ✅ Cookies for video sites: yt-dlp can read a browser's cookies on this computer (Firefox,
  Chrome, Edge, Brave, Opera, Vivaldi, Chromium, Whale; Safari on macOS), so a pasted YouTube
  link that asks to "confirm you're not a bot" works when you're signed in there; and the button
  over videos now passes the page's cookies to the download as well as to the look-up
  ([#229](https://github.com/anefzaoui/misterdownloadmanager/issues/229))
- ✅ When the browser keeps a download, MDM says why: each one is in MDM's log with the setting
  that decided, and the extension's button shows the last one. The extension from the Chrome Web
  Store needs this release (0.2.x serves only the copy in MDM's folder)
  ([#208](https://github.com/anefzaoui/misterdownloadmanager/issues/208))
- ✅ Waterfox, and Firefox set to ask what to do with each file: MDM takes the download before
  the browser's *What should … do with this file?* dialog, which no longer shows for the files
  MDM takes ([#208](https://github.com/anefzaoui/misterdownloadmanager/issues/208))
- ✅ More file types taken from the browser by default: DMG, PKG, DEB, RPM, APPIMAGE, MSIX, APPX,
  CAB, JAR, XZ, ZST, WEBM, FLAC, OPUS and others; a list never edited gains them
  ([#208](https://github.com/anefzaoui/misterdownloadmanager/issues/208))
- ✅ Windows: Download File Info and the progress window open in front of the browser, even when
  Windows doesn't let MDM take the foreground; the keyboard stays where it was until a click
  ([#208](https://github.com/anefzaoui/misterdownloadmanager/issues/208))
- ✅ The window no longer freezes on Linux when the system's sleep service (logind) or keyring
  (Secret Service) is slow or doesn't answer: keeping the computer awake for a woken queue is
  asked in the background, a keyring that doesn't answer at start is waited for once instead of
  once per saved password, and the tray check and Download File Info's Save As check no longer
  wait on every frame (follows [#222](https://github.com/anefzaoui/misterdownloadmanager/issues/222))
- ✅ Linux: Dark Mode support follows the desktop's dark or light theme on XFCE, MATE and others
  without the portal's setting, and on GNOME without the portal, live
  ([#156](https://github.com/anefzaoui/misterdownloadmanager/issues/156))
- ✅ Started with the computer, MDM waits in the notification area instead of opening its window
  in front, on Windows, Linux and macOS (minimized where no notification area shows its icon);
  startup entries written before are updated, and the installer writes the new one
  ([#226](https://github.com/anefzaoui/misterdownloadmanager/issues/226))
- ✅ Open folder in Download complete no longer freezes the window on Linux when the file
  manager is slow to answer or never does: MDM asks in the background and opens the folder after
  5 seconds without an answer; every other program a window starts (Set it up, the Trash, an
  update being put in place, pages and folders) no longer holds the window either
  ([#222](https://github.com/anefzaoui/misterdownloadmanager/issues/222))
- ✅ A download too big for its drive is refused on a very busy computer too: the free-space
  check's 5 seconds count from when it runs, not from when it was asked, and it asks only the
  folder's own drive (shared network folders too) instead of listing every drive; and the host
  connection limit test tells a reconnect from a connection too many ([#224](https://github.com/anefzaoui/misterdownloadmanager/issues/224))
- ✅ The crash report window in every language: the report keeps its lines (German and Arabic
  ran them together) and reads left to right in Arabic and Persian; **End and restart MDM** fits
  its text ([#204](https://github.com/anefzaoui/misterdownloadmanager/issues/204))
- ✅ Every window's buttons fit their text in every language: Download File Info (German
  **Später herunterladen** was cut), Download complete, Add torrent, Download a gallery, the
  progress window, the Scheduler, Options, the Help menu's windows, RSS feeds, the video helpers
  and the browser window; English is unchanged, right to left too
  ([#192](https://github.com/anefzaoui/misterdownloadmanager/issues/192))
- ✅ Linux builds and window shots in minutes: a WSL2 machine builds any branch in about 5
  minutes (under one after a change) and shoots every window in any language in about a second
  each, never showing on the screen; the Linux desktops still check what needs them (part of
  [#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164))
- ✅ The Scheduler wakes the computer from sleep to run a queue, and keeps it awake while the
  queue runs: on Windows when the power plan allows wake timers (the Scheduler says when it
  doesn't), on Linux once an administrator allows it, and on macOS it says why it can't
  ([#142](https://github.com/anefzaoui/misterdownloadmanager/issues/142))
- ✅ A finished download no longer fails with "part file still in use" on a busy computer, and
  the rest of the engine tests that failed now and then there (slow connections, a prompt stop,
  speed limits, links that expire, sparse files and a few more) wait for what they check, not for
  time; the slow-connection test now fails if the engine stops reconnecting slow connections,
  which it didn't before ([#201](https://github.com/anefzaoui/misterdownloadmanager/issues/201))
- ✅ A window closed with its title bar's × keeps its contents until it goes, instead of turning
  black for a moment (seconds on Linux) ([#207](https://github.com/anefzaoui/misterdownloadmanager/issues/207))
- ✅ A freeze while MDM handles a command, or minimized, opens the crash report window and writes
  its freeze report again ([#202](https://github.com/anefzaoui/misterdownloadmanager/issues/202))
- ✅ Where a download's server is: its country's flag and name and its round trip (near, medium or
  far) in Download File Info, the progress window and Properties, from a table that comes with MDM
  (no address is sent anywhere); a proxy or a CDN said as such
  ([#140](https://github.com/anefzaoui/misterdownloadmanager/issues/140))
- ✅ Torrents: Download in order, to watch a video while it downloads (each file's first and last
  pieces first, its own choice); other torrents now take the rarest pieces first
  ([#39](https://github.com/anefzaoui/misterdownloadmanager/issues/39))
- ✅ Every mark in the windows is drawn in every language (the browser window's ✔ Connected
  was an empty box, → on Linux), and the browser and video-helper windows put their logo at the
  start side in Arabic and Persian and fit a long German headline and subline
  ([#185](https://github.com/anefzaoui/misterdownloadmanager/issues/185),
  [#170](https://github.com/anefzaoui/misterdownloadmanager/issues/170))
- ✅ A Linux test machine for every change, scripted: an Ubuntu 24.04 desktop that builds each
  branch and drives MDM, Firefox and Chrome there (part of
  [#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164))
- ✅ The Linux test machine also has KDE Plasma, Cinnamon and XFCE copies, which test a build
  without making it; store packages and theme windows are checked there too (part of
  [#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164))
- ✅ Firefox: reading the browser's proxy (#42) is an opt-in, a switch in the extension's popup, so
  the extension's update asks for no new permission; a VPN add-on's proxy goes with the downloads
  taken before Firefox's file dialog too, and addresses on this computer skip the proxy, as in
  Firefox
- ✅ Chrome and the other Chromium browsers: downloads go through the browser's proxy or VPN
  extension too (#42), with the same switch, off as installed: a fixed server for each scheme and
  its bypass list, or an auto-config script
- ✅ Streams whose address doesn't end in .m3u8 or .mpd, served as plain text, are found too: by
  the button over the video, the right-click menu and the media list, read only when asked
  ([#171](https://github.com/anefzaoui/misterdownloadmanager/issues/171))
- ✅ The extension in the [Chrome Web Store](https://chromewebstore.google.com/detail/mister-download-manager/ejoleppdfgjdndddcjoehopknmlglojp): **Add to Chrome** beside **Add to Firefox**
  on the website, in the help, the FAQ and the README; MDM's **Add Mister Download Manager to
  your browser** window opens the store in Chrome, Edge, Brave, Opera and Vivaldi, with the
  manual install as the fallback
- ✅ Themes, the groundwork: every colour and size the controls are drawn with comes from one
  theme, and theme files can change them; the classic look is unchanged
  ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45))
- ✅ View → Theme: Classic (the default), Native (the system's accent colour and high contrast,
  followed as they change) and High contrast
  ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45))
- ✅ Themes: Metro, Windows 8's flat look (its controls, a drawn title bar on Windows, Fluent
  icons); themes choose each control's style and bring their own icons
  ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45))
- ✅ Themes: Adwaita, GNOME's look (its controls, a header bar on Windows, GNOME's Adwaita icons
  in colour and one colour)
  ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45))
- ✅ Themes: Breeze, KDE Plasma's look (its focus frame, check boxes, buttons and title bar on
  Windows, KDE's Breeze icons)
  ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45))
- ✅ Themes: Luna, Aero and Aqua, the looks of Windows XP, Windows 7 and Mac OS X (shaded and
  glossy buttons, their check boxes and progress bars, their title bars on Windows, icons drawn
  for MDM); Metro, Adwaita and Breeze get MDM's own icons where theirs had none
  ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45))
- ✅ Themes: Luna closer to Windows XP's own look: its title bar's shading and buttons, push
  buttons white over their top half, progress in XP's green blocks, scroll bars on pale blue
  buttons, column headers shaded along their bottom, Tahoma menus, and the progress window's
  connections in XP-era colours; theme files can take the headers, scroll bars and menu font
  too ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45))
- ✅ Themes: Base16 and pywal colour schemes imported as themes (View → Theme → Add a theme
  file…, or the themes folder); **pywal** follows `wal` as it writes new colours, text kept
  readable (4.5:1) ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45))
- ✅ Themes: Emerald (a white canvas, one near-black ink, emerald
  fills carrying near-black text, 6-point corners, Metro's one-colour icons); its theme file
  gives a few seeds per variant and derives every other colour, so a role added later arrives
  in this look rather than the classic one
  ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45))
- ✅ Dark mode: grey notes and error texts readable (4.5:1), Options' failed checks no longer dark
  red on the dark dialog ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45))
- ✅ A minimized MDM keeps working: queues, the Scheduler, download limits, the kill switch and
  saving the list no longer wait for the window to be restored
- ✅ Download video: every button fits its text in every language (German **ffmpeg installieren**
  was cut), right to left too ([#172](https://github.com/anefzaoui/misterdownloadmanager/issues/172))
- ✅ The engine tests that failed now and then on a busy computer (a refused connection coming
  back, the learned connection limit, a dead address) wait for what they check, not for time
  ([#136](https://github.com/anefzaoui/misterdownloadmanager/issues/136))
- ✅ HLS and DASH videos from the browser reach yt-dlp as their stream, by the button over the
  video, the right-click menu (it did nothing in Firefox) and the media list
  ([#155](https://github.com/anefzaoui/misterdownloadmanager/issues/155))
- ✅ A download or magnet link sent while the window is minimized opens its window at once
  ([#162](https://github.com/anefzaoui/misterdownloadmanager/issues/162))
- ✅ Linux: Ubuntu's Firefox (the Snap) reaches MDM: browser integration is set up for it, and the
  help says what the desktop asks the first time; Chromium as a Snap, which can't reach MDM, is no
  longer offered (part of [#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164))
- ✅ A smaller download: the installer 15 % and the zips 19 % smaller, nothing removed
  ([#158](https://github.com/anefzaoui/misterdownloadmanager/issues/158))
- ✅ Linux: MDM's first window opens seconds sooner; the system's fonts are looked up all at once
  while it starts, not one by one in the window (on a busy computer that froze it, part of
  [#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164))
- ✅ Firefox's add-on offered from [addons.mozilla.org](https://addons.mozilla.org/firefox/addon/mister-download-manager/)
  first: **Add to Firefox** on the website, in the help, the FAQ and the README, and in MDM's
  **Add Mister Download Manager to your browser** window (for Zen and LibreWolf too)
- ✅ Support MDM: a way to support its creator on [GitHub Sponsors](https://github.com/sponsors/anefzaoui),
  asked for only (**Help → Support MDM**, a line in About, the website, the README, the
  extension's popup and GitHub's Sponsor button); MDM stays free, nothing changes or unlocks
- ✅ A server that can't resume a download no longer gets the file downloaded twice on a busy
  computer (part of [#136](https://github.com/anefzaoui/misterdownloadmanager/issues/136))
- ✅ Translators: a reviewed translation is never replaced by machine translation, and CI checks it;
  Italian is the first catalog reviewed by a native speaker
  ([#150](https://github.com/anefzaoui/misterdownloadmanager/pull/150))
- ✅ Screen readers reach every row of a list, in view or not (300 around the view in long lists)
  ([#111](https://github.com/anefzaoui/misterdownloadmanager/issues/111))
- ✅ Less memory in the notification area after Minimize too, and again 2 minutes after hiding
  ([#72](https://github.com/anefzaoui/misterdownloadmanager/issues/72))
- ✅ Windows: MDM draws with Direct3D 12 by default; **View → Draw with** goes back to OpenGL, and
  either one falls back to the other when it can't start
  ([#89](https://github.com/anefzaoui/misterdownloadmanager/issues/89))
- ✅ A slow start no longer turns Direct3D off for good: its 10 seconds for the first frame count
  only Direct3D's own time, not MDM's loading (on one computer that sent MDM to OpenGL, where
  Intel's driver froze it); crash and freeze reports say near the top what drew, on which card
  and driver, and why
  ([#280](https://github.com/anefzaoui/misterdownloadmanager/issues/280))
- ✅ Linux and macOS: **View → Draw with → Vulkan** (Linux) and **Metal** (macOS), with the same
  fallback to OpenGL as Direct3D on Windows; OpenGL stays their default for now, and the menu
  says why when one can't start here
  ([#206](https://github.com/anefzaoui/misterdownloadmanager/issues/206))
- ✅ Screen readers see a window's contents the first time they read it, not only its title bar
  ([#131](https://github.com/anefzaoui/misterdownloadmanager/issues/131))
- ✅ Magnet links get their file list in seconds again: the DHT keeps going after a computer it
  asked is gone, and Add torrent shows the wait (peers found, from where, what may be in the
  way) ([#92](https://github.com/anefzaoui/misterdownloadmanager/issues/92))
- ✅ Two engine tests that failed now and then on busy machines are reliable
  ([#123](https://github.com/anefzaoui/misterdownloadmanager/issues/123))
- ✅ Numbers are grouped and pointed the interface language's way (German `10.200`, `1,50 MB`)
  ([#128](https://github.com/anefzaoui/misterdownloadmanager/issues/128))
- ✅ The extension's button lists the media the tab has loaded (HLS and DASH streams, video and
  sound files), to download a stream the page won't give up; copy-protected ones are shown as
  protected, never downloaded ([#38](https://github.com/anefzaoui/misterdownloadmanager/issues/38))
- ✅ Torrents: RSS feeds with download rules (words, episodes, each episode once, a folder per rule)
  ([#39](https://github.com/anefzaoui/misterdownloadmanager/issues/39))
- ✅ Torrents: Recheck, Reannounce, Add trackers and Copy magnet link; choose files while a
  torrent runs, or in Torrent → Files… for any torrent (stopped or complete too); a check of the
  files says "Checking 42%"; a finished torrent that isn't shared lets go of its files; a
  `.torrent` file's web address opens Add torrent ([#39](https://github.com/anefzaoui/misterdownloadmanager/issues/39))
- ✅ Link grabber: links in packages, looked up for name and size; archives (zip and tar
  built in; 7z and RAR need 7-Zip) extracted when they are downloaded, in place
  with HatchZip when asked ([#37](https://github.com/anefzaoui/misterdownloadmanager/issues/37));
  collecting copied links comes later
- ✅ Link grabber: Click'n'Load from link sites, in their packages with their archive passwords;
  off by default, asked once per site, the links not looked up before they are added
  ([#37](https://github.com/anefzaoui/misterdownloadmanager/issues/37))
- ✅ Archives: saved archive passwords, tried before the password is asked, the one that fits
  first ([#37](https://github.com/anefzaoui/misterdownloadmanager/issues/37))
- ✅ Image galleries with gallery-dl: Wikimedia Commons, Flickr, DeviantArt and many more,
  verified like yt-dlp, or your own copy ([#58](https://github.com/anefzaoui/misterdownloadmanager/issues/58))
- ✅ Torrents: seeding limits (a share ratio, or a time), off by default, and a finished torrent's
  tabs while it is shared ([#39](https://github.com/anefzaoui/misterdownloadmanager/issues/39))
- ✅ A paused or finished download no longer leaves its progress file to be written again a moment
  later ([#106](https://github.com/anefzaoui/misterdownloadmanager/issues/106)), and a download
  finishing during a save no longer fails ([#116](https://github.com/anefzaoui/misterdownloadmanager/issues/116))
- ✅ Tests and test tools stay on 127.0.0.1: no Windows Firewall questions while developing
- ✅ Debrid accounts (experimental): Real-Debrid, AllDebrid and Premiumize; file-host links downloaded through your own account (Options → Accounts)
  ([#44](https://github.com/anefzaoui/misterdownloadmanager/issues/44))
- ✅ Debrid accounts: a per-download "directly instead" choice, "through the service" in Download
  File Info, and the command line using the accounts
  ([#44](https://github.com/anefzaoui/misterdownloadmanager/issues/44))
- ✅ Debrid accounts: torrents and magnet links through the service, their files downloaded over
  HTTP, no peer connections ([#44](https://github.com/anefzaoui/misterdownloadmanager/issues/44))
- ✅ Debrid accounts (experimental): TorBox, for file-host links and torrents
  ([#391](https://github.com/anefzaoui/misterdownloadmanager/issues/391))
- ✅ Debrid accounts: Mega-Debrid, with its user name and password or an application's API key
  ([#44](https://github.com/anefzaoui/misterdownloadmanager/issues/44))
- ✅ Sign-ins in the system's keychain: Windows Credential Manager, the Secret Service keyring on
  Linux, the macOS keychain; for debrid accounts, Sites Logins and the proxy
  ([#44](https://github.com/anefzaoui/misterdownloadmanager/issues/44))
- ✅ Downloads through one network adapter (a VPN's), with a kill switch: nothing goes out
  another way when it disconnects ([#35](https://github.com/anefzaoui/misterdownloadmanager/issues/35))
- ✅ Torrents on Windows through the chosen network adapter, with the same kill switch
  ([#143](https://github.com/anefzaoui/misterdownloadmanager/issues/143))
- ✅ Video pages and image galleries through the chosen network adapter too, with the same kill
  switch ([#35](https://github.com/anefzaoui/misterdownloadmanager/issues/35))
- ✅ Downloads from a browser behind a VPN or proxy extension go through the same proxy
  ([#42](https://github.com/anefzaoui/misterdownloadmanager/issues/42))
- ✅ Hide to the notification area: a download's progress window (its own button, or Minimize)
  and the main window's Minimize, with the tray's "Downloads in progress"
  ([#63](https://github.com/anefzaoui/misterdownloadmanager/issues/63))
- ✅ Whole playlists and channels, one quality for all: also as the original sound, MP3, Opus or FLAC
  ([#69](https://github.com/anefzaoui/misterdownloadmanager/issues/69))
- ✅ Choose the DNS: Cloudflare, Google, Quad9, AdGuard, NextDNS or your own, over HTTPS, TLS or
  plain, with a Test button and no silent fallback
  ([#36](https://github.com/anefzaoui/misterdownloadmanager/issues/36))
- ✅ Less memory in the notification area: under 10 MB there, and the language list's fonts let
  go when it closes ([#72](https://github.com/anefzaoui/misterdownloadmanager/issues/72))
- ✅ Video downloads: subtitles, beside the video or inside it
  ([#68](https://github.com/anefzaoui/misterdownloadmanager/issues/68)); more audio choices:
  the original track, Opus and FLAC ([#70](https://github.com/anefzaoui/misterdownloadmanager/issues/70))
- ✅ Torrent security: stricter file-name checks, no silent overwrite, files marked as downloaded
  from the internet, network switches (incoming, UPnP, port, DHT, local peers), SOCKS5 proxy and
  IP blocklist ([#40](https://github.com/anefzaoui/misterdownloadmanager/issues/40))
- ✅ Torrents: a speed limit per torrent, remembered limits, and the global cap covering torrents
  too ([#39](https://github.com/anefzaoui/misterdownloadmanager/issues/39))
- ✅ Torrents: upload limits (all torrents, and each one), and a share ratio that survives pauses
  and restarts ([#39](https://github.com/anefzaoui/misterdownloadmanager/issues/39))
- ✅ Google Drive: public files download by their share links
  ([#34](https://github.com/anefzaoui/misterdownloadmanager/issues/34)); folders come later
- ✅ Video downloads: Stop ends yt-dlp and everything it started
  ([#61](https://github.com/anefzaoui/misterdownloadmanager/issues/61)); no error while yt-dlp
  goes on and finishes ([#79](https://github.com/anefzaoui/misterdownloadmanager/issues/79)); a
  quality the reachable YouTube servers don't serve falls back to the nearest one
  ([#94](https://github.com/anefzaoui/misterdownloadmanager/issues/94))
- ✅ A video's sound as MP3, at the quality you pick
  ([#32](https://github.com/anefzaoui/misterdownloadmanager/issues/32))
- ✅ More than 32 connections per download, if you want them
  ([#12](https://github.com/anefzaoui/misterdownloadmanager/issues/12))
- ✅ Your own yt-dlp and ffmpeg ([#51](https://github.com/anefzaoui/misterdownloadmanager/issues/51))
- ✅ View → Always dark ([#60](https://github.com/anefzaoui/misterdownloadmanager/issues/60))
- ✅ Zen, Helium and other Firefox- and Chromium-based browsers
  ([#41](https://github.com/anefzaoui/misterdownloadmanager/issues/41))
- ✅ Linux: menus open where clicked under Wayland
  ([#50](https://github.com/anefzaoui/misterdownloadmanager/issues/50)), and the window closes on
  desktops without a tray, such as MangoWC ([#59](https://github.com/anefzaoui/misterdownloadmanager/issues/59))
- ✅ Downloads that hand out one-time links no longer fail with 403
  ([#28](https://github.com/anefzaoui/misterdownloadmanager/issues/28))
- ✅ The extension's Download video menu keeps its highlight inside its edge
  ([#81](https://github.com/anefzaoui/misterdownloadmanager/issues/81))
- ✅ No crash when a window opens while MDM is minimized or in the notification area, such as a
  download or a magnet link from the browser
  ([#22](https://github.com/anefzaoui/misterdownloadmanager/issues/22),
  [#145](https://github.com/anefzaoui/misterdownloadmanager/issues/145))
- ✅ Long file names stay inside the progress window: the Save to path is shortened in the
  middle, in every language ([#62](https://github.com/anefzaoui/misterdownloadmanager/issues/62))
- ✅ Every button translated: Download complete's Open with... no longer stays in English
  ([#80](https://github.com/anefzaoui/misterdownloadmanager/issues/80))
- ✅ A smaller download ([#25](https://github.com/anefzaoui/misterdownloadmanager/issues/25)),
  AppImage updates with zsync ([#24](https://github.com/anefzaoui/misterdownloadmanager/issues/24)),
  the Bangladesh flag for Bangla ([#27](https://github.com/anefzaoui/misterdownloadmanager/issues/27))
- ✅ Download complete shows above other windows, Options from the extension comes to the front
  ([#77](https://github.com/anefzaoui/misterdownloadmanager/issues/77)), and dragging the file out
  of Download complete works in every language
  ([#78](https://github.com/anefzaoui/misterdownloadmanager/issues/78))
- ✅ The built-in categories can be renamed, given other file types and deleted, and restored
  ([#93](https://github.com/anefzaoui/misterdownloadmanager/issues/93))
- ✅ A file-type icon before every file name, in every window that names a file (the download
  list, Download File Info, Properties, Download complete, Add torrent, a torrent's files, the
  Scheduler, Batch download, Download video and its playlists), each the system's own icon for
  its type (Windows Explorer's, macOS Finder's, the Linux desktop's icon theme), sharp at every
  display scale
  ([#122](https://github.com/anefzaoui/misterdownloadmanager/issues/122)), and every
  list's rows named for screen readers ([#95](https://github.com/anefzaoui/misterdownloadmanager/issues/95)),
  inside their list, with their place in it ([#102](https://github.com/anefzaoui/misterdownloadmanager/issues/102));
  the Categories pane, the menu bar and every right-click menu too, with Shift+F10 and the Menu key
  ([#113](https://github.com/anefzaoui/misterdownloadmanager/issues/113))
- ✅ The README's, the docs' and the website's screenshots are taken on a stock Windows (GitHub's
  runner), so the file icons in them are Windows' own
- ✅ The help shows what it describes: pictures of each window, numbered where a step points at
  one control, step strips for the usual flows, in light and dark, made from one shot list
  ([#228](https://github.com/anefzaoui/misterdownloadmanager/issues/228))
- ✅ Website in European Portuguese, and an [intended use](https://misterdownloadmanager.com/intended-use.html) page
- ✅ Website: the [free IDM alternative](https://misterdownloadmanager.com/idm-alternative.html) page compares
  MDM with IDM and with nine other free download managers, each checked on its own site, with
  their logos, a feature-by-feature form and an MDM-vs-IDM label with the benchmark's times to scale
- ✅ Website: the header fits its row in every language at every width (it hides the Sponsor heart,
  then tightens, then turns into the menu, only as far as a language needs)
  ([#168](https://github.com/anefzaoui/misterdownloadmanager/issues/168)), and no page scrolls
  sideways on a phone, Arabic and Persian included: long paths, links and tables wrap or scroll
  inside the page ([#169](https://github.com/anefzaoui/misterdownloadmanager/issues/169))
- ✅ Help opens the website's help pages, in the interface language when the website has it
  ([#227](https://github.com/anefzaoui/misterdownloadmanager/issues/227))
- ✅ Website: a page for every feature, old and new, with its real window, where it is in the app
  and what it can't do yet; tiles of them all on the home page; and a
  [Themes](https://misterdownloadmanager.com/themes.html) page with every built-in theme, light and
  dark, and how to make, try and share your own
  ([#260](https://github.com/anefzaoui/misterdownloadmanager/issues/260))
- ✅ Thanks to the people who make MDM possible: everyone who sent a pull request, opened an issue
  or commented on GitHub, as round photos in the home page's first view and the README, on a
  [Contributors](https://misterdownloadmanager.com/contributors.html) page with what each one did,
  at the end of each release on the News page, and in each release's notes on GitHub
- ✅ Themes ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45)): today's look stays the default. Every colour, size and spacing now
  comes from the chosen theme (the classic look pixel for pixel the same); **View → Theme** with
  Classic, Native (follows the system's accent and contrast on Windows, Linux and macOS) and High
  contrast; style themes with their own control shapes, a drawn title bar on Windows and their own
  icons: Metro, Adwaita, Breeze, Luna, Aero and Aqua; and theme files anyone can make, try and
  share, in TOML or JSON, built on any theme (`inherits`), with their own icons; Base16 and
  pywal colour schemes read as themes ([the guide](docs/themes.md))
- ✅ Linux, no longer experimental: every change is checked on Ubuntu (GNOME) as well as Windows,
  and on KDE Plasma, Cinnamon and Xfce; Wayland sessions (through XWayland, menus under their
  titles on sway too); the .deb and .rpm installed in clean Ubuntu, Debian and Fedora systems, the
  AppImage and the tar.gz; the tray, Dark Mode support and Launch on startup on all four desktops;
  the Snap Firefox, Google Chrome and Brave reaching MDM
  ([#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164)), and its release files
  named without "-experimental"
- ✅ Every screenshot taken again on Windows and Linux (smaller files, WebP), and every page of the
  docs, the README and the website brought up to date
  ([#186](https://github.com/anefzaoui/misterdownloadmanager/issues/186))

## Version 0.5.2

Bug fixes from the first days of 0.5.0, a Safari extension, and more file types.

- ✅ A link from the browser no longer ends in 403 where a site refuses the browser's
  User-Agent: MDM asks once more as itself ([#372](https://github.com/anefzaoui/misterdownloadmanager/issues/372))
- ✅ The video button no longer covers a control of the page (a preview's own button): it takes
  a free corner, and dragging it keeps it where you put it
  ([#421](https://github.com/anefzaoui/misterdownloadmanager/issues/421))
- ✅ A background task stopping as MDM closes no longer shows a crash report
  ([#422](https://github.com/anefzaoui/misterdownloadmanager/issues/422))
- ✅ A freeze that was over before its dump was taken is reported as a freeze, not a crash in the
  watchdog (part of [#420](https://github.com/anefzaoui/misterdownloadmanager/issues/420))
- ✅ File names and titles in any script show their letters: the system font for a script
  outside the interface language loads the first time it is needed, instead of boxes
  ([#405](https://github.com/anefzaoui/misterdownloadmanager/issues/405))
- ✅ yt-dlp and gallery-dl use the proxy set in Options → Proxy / Socks
  ([#363](https://github.com/anefzaoui/misterdownloadmanager/issues/363))
- ✅ The window keeps answering while reading the clipboard or removing a cancelled download's
  files from a slow disk ([#418](https://github.com/anefzaoui/misterdownloadmanager/issues/418),
  [#423](https://github.com/anefzaoui/misterdownloadmanager/issues/423))
- ✅ Fast downloads keep going on a slow disk: flushing the resume state no longer holds up
  the connections, and a connection is never replaced for the disk's wait
  ([#361](https://github.com/anefzaoui/misterdownloadmanager/issues/361))
- ✅ Freeze reports taken after the window has already answered say so, instead of blaming a deadlock
  (part of [#355](https://github.com/anefzaoui/misterdownloadmanager/issues/355))
- ✅ **Move/Rename** works on several selected downloads at once: pick a folder and they all go there
  ([#333](https://github.com/anefzaoui/misterdownloadmanager/issues/333))
- ✅ Download video shows the size of a plain file (what the server says), not "size unknown"
  (part of [#320](https://github.com/anefzaoui/misterdownloadmanager/issues/320))
- ✅ Download video on pages whose player first loads a manifest's address answering something else
  (SVT Play): the real manifest goes to MDM, not that first answer
  ([#377](https://github.com/anefzaoui/misterdownloadmanager/issues/377))
- ✅ Windows: sharp, evenly spaced text at every interface size: letters no longer run together
  or drift apart ([#323](https://github.com/anefzaoui/misterdownloadmanager/issues/323),
  [#334](https://github.com/anefzaoui/misterdownloadmanager/issues/334))
- ✅ Far more file types taken by default: AI models and datasets, 3D and CAD, design and camera
  RAW, ebooks, Office, disk images and split archives; a list never edited gains them
  ([#382](https://github.com/anefzaoui/misterdownloadmanager/issues/382))
- ✅ A save folder relative to the home folder is found where it really is
- ✅ Browser link import: tighter handling of site data
- ✅ The extension asks for a newer MDM only when a whole release separates them, not over a
  bug-fix version
- ✅ Add URL: a YouTube playlist opens the video window; Dailymotion's embedded player is looked
  up as the video's own page ([#338](https://github.com/anefzaoui/misterdownloadmanager/issues/338),
  [#359](https://github.com/anefzaoui/misterdownloadmanager/issues/359))
- ✅ macOS: video look-ups work in a copy of MDM a browser downloaded, without the Terminal command:
  yt-dlp's folder build, unpacked once and checked against its signed list
  ([#330](https://github.com/anefzaoui/misterdownloadmanager/issues/330),
  [#339](https://github.com/anefzaoui/misterdownloadmanager/issues/339))
- ✅ In English, the list's dates follow the system's 12- or 24-hour clock
  ([#392](https://github.com/anefzaoui/misterdownloadmanager/issues/392))
- ✅ Help: what to do when an antivirus or VirusTotal flags MDM, and how to check a copy is the real
  one
- ✅ The Safari extension (macOS), part of the app: a click on a link to a listed file type, the
  right-click items, the button over videos, Media on this page and the toolbar button, from one
  source with the other browsers' extension. Safari gives extensions no way to see its downloads, so
  a download a page starts by itself stays with Safari
- ✅ A torrent's peers and share ratio as columns in the download list
  ([#357](https://github.com/anefzaoui/misterdownloadmanager/issues/357))
- ✅ The browser extension's media list can name a file after its page, as an option (off as
  installed)
- ✅ Malay: the menus, toolbar and list columns checked by a native speaker
- ✅ Closing the main window follows a setting (on by default, MDM keeps running), and with no tray
  icon the window goes to the taskbar instead of vanishing
  ([#457](https://github.com/anefzaoui/misterdownloadmanager/pull/457))
- ✅ Download File Info no longer stays open but invisible, with later browser downloads waiting
  behind it ([#440](https://github.com/anefzaoui/misterdownloadmanager/issues/440))
- ✅ Arabic and other non-Latin file names arrive readable from Firefox and browsers built on it
  ([#447](https://github.com/anefzaoui/misterdownloadmanager/issues/447))
- ✅ A file renamed to avoid a clash shows and opens under its new name
  ([#442](https://github.com/anefzaoui/misterdownloadmanager/issues/442))
- ✅ RSS feeds: the rule boxes keep the keyboard while you type
  ([#353](https://github.com/anefzaoui/misterdownloadmanager/issues/353))
- ✅ Video progress no longer reads 100 % while still downloading, and a video known only by its
  address is named after its page, not a long encoded string
  ([#344](https://github.com/anefzaoui/misterdownloadmanager/issues/344),
  [#345](https://github.com/anefzaoui/misterdownloadmanager/issues/345))
- ✅ A page that starts a download by script: MDM gets the file, not the page
  ([#374](https://github.com/anefzaoui/misterdownloadmanager/issues/374))
- ✅ "Hide tab" in a download's window takes the tab off its row; **Show hidden tabs** brings it back
  ([#326](https://github.com/anefzaoui/misterdownloadmanager/issues/326))
- ✅ A download stopped in a running queue stays stopped
  ([#341](https://github.com/anefzaoui/misterdownloadmanager/issues/341))
- ✅ No crash when removing the last feed in the RSS feeds window
  ([#321](https://github.com/anefzaoui/misterdownloadmanager/issues/321))
- ✅ **Open folder** works on a torrent still downloading
  ([#354](https://github.com/anefzaoui/misterdownloadmanager/issues/354))
- ✅ Alt and F10 open the menu bar while a control has the focus
  ([#328](https://github.com/anefzaoui/misterdownloadmanager/issues/328))
- ✅ Windows: no crash when a window opens while MDM falls back from Direct3D to OpenGL
  ([#337](https://github.com/anefzaoui/misterdownloadmanager/issues/337))
- ✅ Windows: no crash when a screen goes away while MDM runs
  ([#335](https://github.com/anefzaoui/misterdownloadmanager/issues/335))
- ✅ No duplicate prompt for a finished file that is gone
  ([#342](https://github.com/anefzaoui/misterdownloadmanager/issues/342))
- ✅ Crash reports: a monitor that cannot open its socket exits, and an unreachable one is cleaned up
  instead of staying beside MDM
  ([#458](https://github.com/anefzaoui/misterdownloadmanager/pull/458))

## Version 0.5.1

- ✅ The Download video button on YouTube: the video playing now instead of the one before or a
  whole list, Shorts included
  ([#311](https://github.com/anefzaoui/misterdownloadmanager/issues/311))
- ✅ The Download video button on YouTube in Chrome, Edge, Brave, Opera and Vivaldi

## Done since 0.5.2

These ship in the next version.

- ✅ YouTube's JavaScript runtime: MDM installs Node.js (its newest LTS, from nodejs.org, its
  checksum list checked against Node.js's releasers' signing keys) instead of Deno, for Windows
  (x64, ARM64, 32-bit), macOS and Linux (x64, ARM64), kept up to date with the other helpers; a
  runtime already on the computer (Deno, Node.js 22 or newer, QuickJS, Bun) comes first, and
  **Use another program...** in Helpers picks any of them; the first start offers Node.js only
  when none is found; the Deno an earlier version installed gives way to Node.js and is removed;
  the command line's `install-js-runtime` (`install-deno` still works)
  ([#1570](https://github.com/anefzaoui/misterdownloadmanager/issues/1570))
- ✅ HTTPS certificates are also checked against Mozilla's trusted roots (Firefox's) where the
  system's own don't complete the chain, so an older Windows that hasn't fetched Let's Encrypt's
  roots downloads from the sites Firefox opens; the system's own certificates stay trusted
  ([#1546](https://github.com/anefzaoui/misterdownloadmanager/issues/1546))
- ✅ macOS: file names in Sinhala show their letters on every Mac: its Sinhala font is used by
  name, as Khmer's and Lao's are, where the system's own choice could be a downloaded font MDM
  couldn't find ([#1592](https://github.com/anefzaoui/misterdownloadmanager/issues/1592))
- ✅ With the browser add-on sending downloads to another MDM, pointing at videos no longer adds
  the page to that MDM's list: only real downloads go there (a list of the message kinds that
  may, nothing else), and the video button asks this computer's MDM whether to show
- ✅ The Download video button on Bilibili stays on one corner, clear of the player's feedback
  button, and can be clicked: it looks for a page's controls under the whole of the spot it would
  take, not only its middle and corners ([#1582](https://github.com/anefzaoui/misterdownloadmanager/issues/1582))
- ✅ MDM keeps its own yt-dlp, gallery-dl, ffmpeg and Deno up to date by itself: a check a few
  seconds after it starts and then while it runs (yt-dlp and gallery-dl at most every 6 hours,
  Deno daily, ffmpeg weekly, remembered across starts), each new version checked as its first
  install was and switched to only once no download is using it, the previous one kept to go
  back to. A video or gallery that fails in a way that usually means its helper is out of date
  has MDM look for a newer one at once and try again with it. Helpers says how each stands, with
  **Check for updates**, **Update** and **Go back to** the previous version, and a switch to turn
  it off; the command line has `update-helpers`
  ([#1544](https://github.com/anefzaoui/misterdownloadmanager/issues/1544))
- ✅ Deleting a download keeps its file unless **Also delete the files from disk** is ticked in
  that very question: the box starts unticked every time (only **Shift+Delete** and **Remove
  with files** tick it, once), it says how many finished files go with it, a question turned
  off only takes downloads off the list, and deleting files always asks
  ([#1563](https://github.com/anefzaoui/misterdownloadmanager/issues/1563))
- ✅ The command line's downloads, look-ups, helper installs and torrents go through Options →
  Proxy / Socks (and its network adapter and DNS) as the app's do, with a `--proxy` switch for
  one run ([#1556](https://github.com/anefzaoui/misterdownloadmanager/issues/1556))
- ✅ SOCKS5 and SOCKS4 as a real choice in Options → Proxy / Socks: a **Type** (HTTP, HTTPS,
  SOCKS5, SOCKS4), site names looked up through the proxy (on by default), a SOCKS5 login, Get
  System reading Windows' SOCKS proxy, a **Test** button; every connection follows it: downloads
  and FTP, video pages and galleries, MDM's own downloads and the update check, a proxy script's
  SOCKS answers, and torrents when asked
  ([#1558](https://github.com/anefzaoui/misterdownloadmanager/issues/1558))
- ✅ 7z and RAR archives are extracted with the 7-Zip another archiver brings: PeaZip's and
  NanaZip's command line are found by themselves on Windows, and **Options → Archives → Use
  another program...** takes any other 7-Zip command line program (a portable copy), checked
  before it is used
  ([#1525](https://github.com/anefzaoui/misterdownloadmanager/issues/1525))
- ✅ The proxy reaches yt-dlp: Options' automatic configuration script is asked about the
  video's page for every yt-dlp run, "Use system settings" follows Windows' setup script (a
  proxy program's PAC mode), and a manual proxy chosen with no protocol ticked gets http and
  https ticked ([#1448](https://github.com/anefzaoui/misterdownloadmanager/issues/1448))
- ✅ Website: screenshots open over the page in [Vitrine](https://github.com/anefzaoui/vitrine), growing out of their
  place and back: arrows, keys and swipes through a group, zoom by click, double tap, pinch or
  wheel, Back and Escape to close, the focus and the page kept where they were, right to left,
  light and dark, reduced motion; the language picker opens and closes the same way
- ✅ A video download, look-up, playlist or gallery finishes when yt-dlp (or gallery-dl) does:
  a program it started that still held its output open kept the download running at its last
  progress ([#1521](https://github.com/anefzaoui/misterdownloadmanager/issues/1521))
- ✅ The browser add-on's popup lists MDM's newest downloads, live while it is open: the system's
  own icon for each file type, its progress, speed and time left, search and filters (running,
  done, failed), and on each one **Open**, **Show in folder**, **Pause** / **Resume**, **Copy
  file** (to paste the file itself into a folder, a chat or an upload box), **Share** (Windows'
  and macOS's share sheet), **Copy link**, **Download again**, **Remove from list** and **Delete
  file**; the toolbar button counts the running ones. The popup has tabs now: **Downloads**,
  **This page** and **Settings** ([#1496](https://github.com/anefzaoui/misterdownloadmanager/issues/1496))
- ✅ macOS crash and freeze reports: the release's symbols are found (by the program's name, or
  by its id when the report has no name for it) and carry unwinding tables and inlined calls, so
  no frame is skipped; a freeze says how much the window's thread ran and how many pages MDM read
  from disk meanwhile ([#1515](https://github.com/anefzaoui/misterdownloadmanager/issues/1515))
- ✅ Alt+click on a link behind a site's bot check (Cloudflare's "Just a moment...") opens it in
  the tab, so the browser passes the check and keeps the file: Chromium browsers and Safari saved
  the check's page instead ([#1494](https://github.com/anefzaoui/misterdownloadmanager/issues/1494))
- ✅ The browser-like connection's `sec-ch-ua` names the browser's own version when it is newer
  than the newest profile MDM has, its request's priority and Accept-Encoding are Chrome's, and
  the User-Agent goes once
  ([#1494](https://github.com/anefzaoui/misterdownloadmanager/issues/1494))
- ✅ Small windows (About, Tell a friend, Speed Limiter settings, Keys and the others) are exactly
  as large as their content at every display scale and interface size, with no scroll bars over
  their buttons ([#1495](https://github.com/anefzaoui/misterdownloadmanager/issues/1495))
- ✅ Chinese text from a font collection (macOS's Songti) is drawn with its regular face, not its
  first one, Songti SC Black ([#1483](https://github.com/anefzaoui/misterdownloadmanager/issues/1483))
- ✅ The browser extension remembers paired servers and switches between them and this computer
  with a click without re-pairing (thanks to [@kngkatak](https://github.com/kngkatak))
- ✅ Helpers downloads Deno, the JavaScript runtime YouTube now needs yt-dlp to have (without one a
  look-up is slow, lists few qualities or fails with "Requested format is not available"):
  Deno's official build, checked against its release's SHA-256, used by yt-dlp from then on; a
  Node.js or Deno already on the computer is found and said instead, and the Download video
  window offers **Install Deno** where YouTube lacked one
  ([#1448](https://github.com/anefzaoui/misterdownloadmanager/issues/1448),
  [#1470](https://github.com/anefzaoui/misterdownloadmanager/issues/1470))
- ✅ Video qualities say the file and the codec that downloads (`1080p MP4 · AV1 · 60 fps`): in
  the Download video window, the menu over a video and `video-info`; "MP4" alone read as H.264.
  A line whose very stream is gone takes the nearest of its codec first
  ([#1465](https://github.com/anefzaoui/misterdownloadmanager/issues/1465))
- ✅ A download the browser handed over is never called complete with an answer no larger than a
  web page where the browser's file was larger than any (a JSON or text error, an empty answer):
  it stops before anything is saved, says both sizes and suggests Alt+click; where the browser's
  own download ended is asked once first
  ([#1460](https://github.com/anefzaoui/misterdownloadmanager/issues/1460))
- ✅ A live stream that is recording is saved when it is stopped: the progress window says
  **Recording...** with **Downloaded** and **Transfer rate** following the recording's size, its
  pause button reads **Stop and save**, and Stop, Stop All, a queue's stop and the phone page's
  pause end the recording and make it an MP4 (streams copied, nothing re-encoded), complete as when
  a live stream ends by itself; the recording goes only once the MP4 is as long as it, else a
  `.ts` file is kept (beside the MP4 if there is one). Resuming records
  anew after setting the earlier recording aside as `<name> (recording 1).ts`, where it used to
  record over it; a new speed limit no longer restarts a recording. Closing MDM while it records
  and the command line still don't save it
  ([#1450](https://github.com/anefzaoui/misterdownloadmanager/pull/1450))
- ✅ A live stream being recorded says **Recording...** (and its connection Receiving) as soon as
  the recorder has opened its file, a few seconds in, instead of "Connecting..." with 0 KB until
  its first 256 KB block was written (some 15 s at 128 kbit/s, longer at lower bitrates); Stop and
  save before that block keeps nothing yet and leaves the download stopped to resume
  ([#1450](https://github.com/anefzaoui/misterdownloadmanager/pull/1450))
- ✅ Tests: the live recording checks that failed now and then (most runs on Linux) wait until
  enough is recorded for what they check, instead of saving at the first block: a recording that
  short is within the second an MP4 may lose and still count as whole, so whether the MP4 was
  judged short depended on when the save came
- ✅ Download video: a video's own file (Kwai's MP4) is offered and downloaded when its server
  doesn't say it is a video, instead of "No video this can download on that page"; the button's
  click downloads such a file at once
  ([#1458](https://github.com/anefzaoui/misterdownloadmanager/issues/1458))
- ✅ Your own yt-dlp config file: Options → Sites Logins → Use my yt-dlp config file (off by
  default) lets yt-dlp read it for the app's videos, most of MDM's own choices still winning; an
  external downloader it names doesn't stop the progress
- ✅ Videos in Google Drive shared folders: the Download video button reads the file's ID from
  the player's stream or preview dialog, and MDM downloads the file directly through its Google
  Drive segmented engine instead of failing on the folder address
  ([#758](https://github.com/anefzaoui/misterdownloadmanager/issues/758))
- ✅ Download File Info: a name typed in **Save as** that is a file already there asks first
  (the duplicate question: a numbered name, or overwrite it, replaced only once the download
  completes), instead of numbering it without a word
  ([#1447](https://github.com/anefzaoui/misterdownloadmanager/issues/1447))
- ✅ A Docker image of the headless server is published with each release, at
  `ghcr.io/anefzaoui/misterdownloadmanager` (x86-64 only for now, thanks to
  [@kngkatak](https://github.com/kngkatak))
- ✅ OpenGL: no crash when the graphics driver can't switch drawing between MDM's windows for a
  moment (an Intel HD Graphics 520); that window is drawn again just after, and one that can't be
  drawn for seconds starts MDM again, as a lost graphics card does
  ([#1448](https://github.com/anefzaoui/misterdownloadmanager/issues/1448))
- ✅ File names as browsers read them: a server's percent-encoded `filename="My%20Report.pdf"` is
  `My Report.pdf`, as in Chrome and Firefox (it stayed encoded), and RFC 2047's encoded words are
  read too; checked behind one and two redirects, relative ones included
  ([#1427](https://github.com/anefzaoui/misterdownloadmanager/issues/1427))
- ✅ Video qualities: **Most compatible (H.264 + AAC)**, the best H.264 picture with AAC sound in an
  MP4 that editors, older TVs and phones open as it is, in the Download video window, the menu over
  a video, a playlist's quality and `video-get -q compatible`; picked, never converted
  ([#1439](https://github.com/anefzaoui/misterdownloadmanager/issues/1439))
- ✅ SourceForge downloads from the browser: when a site's bot check refuses the browser's
  User-Agent and the browser-like connection too, MDM asks once more under its own name, with
  the same cookies and referrer, and remembers the site for the session, resumes included
  ([#1422](https://github.com/anefzaoui/misterdownloadmanager/issues/1422))
- ✅ Add batch download: a long address scrolls to its caret in right-to-left languages (every
  one-line box did not), and a Save to row puts the whole batch in one folder
  ([#1403](https://github.com/anefzaoui/misterdownloadmanager/issues/1403))
- ✅ Malay: 24 menu and column items reviewed by a native speaker (torrent columns, Linux display
  systems, text rendering and graphics notes) (thanks to [@9M2PJU](https://github.com/9M2PJU))
- ✅ Fuzzing: the parts that read what servers, pages and downloaded files send (HLS playlists,
  yt-dlp's answers, web pages, file names, .torrent files and magnet links, release signatures,
  Click'n'Load posts, archives) are tried with generated inputs on every pull request that
  changes them and every week (ClusterFuzzLite); robustness fixes in reading HLS and DASH
  streams' pieces, a video's sizes and a .torrent file's trackers came out of it, and Link grabber
  no longer lists an `ftps:` link that names no server

- ✅ Torrents connect over uTP (BitTorrent over UDP) beside TCP, on by default with a switch in
  Torrent settings: peers that take only uTP are reached, so torrents find as many peers as in
  other clients; off with a SOCKS5 proxy, held to a chosen network adapter
  ([#1384](https://github.com/anefzaoui/misterdownloadmanager/issues/1384))
- ✅ Torrents keep uTP when the port picked for them is taken for UDP by another program: the
  session listens on a port free for both TCP and UDP, where it went on with TCP alone and never
  reached the peers that take only uTP. Two tests that failed now and then on Windows runners
  because of it are steady, and a Rapidgator test whose link expired before a slow runner's first
  request times the link from that request
- ✅ A stream's own address — an HLS playlist (`.m3u8`) or DASH's (`.mpd`) — opens the video
  window like a page's: pasting a playlist downloads the video and merges it into one playable
  file, instead of saving the playlist itself
- ✅ The window never waits for the disk to log: lines go to the log's own thread through a
  queue that drops (and counts) what a stuck disk can't take, and a crash or freeze report
  carries the last lines from memory when the file doesn't have them yet
  ([#1354](https://github.com/anefzaoui/misterdownloadmanager/issues/1354))
- ✅ Check for updates and the yt-dlp and ffmpeg downloads work when GitHub's API refuses a shared
  internet address for its hourly limit: the release is read from github.com's own release pages
  instead, and checked as before (signed checksum lists, checksums)
  ([#1337](https://github.com/anefzaoui/misterdownloadmanager/issues/1337))
- ✅ Browser downloads open only the relevant dialog (Download File Info or Download video)
  without pulling up, unhiding, or focusing the main window, and starting MDM from the browser
  launches it in the notification area without marking an OS session login (thanks to
  [@9M2PJU](https://github.com/9M2PJU)) ([#1333](https://github.com/anefzaoui/misterdownloadmanager/pull/1333))
- ✅ Windows: dialogs open on the main window's monitor when the monitors have different display
  scales (a 125 % laptop and a 100 % monitor), where they opened out of sight; a window on no
  screen is brought onto one ([#1311](https://github.com/anefzaoui/misterdownloadmanager/issues/1311))
- ✅ Tests: more of the ones that failed now and then on a busy runner are steady. A port that
  stands for a server or a proxy that refuses is held for the whole test (one let go at once
  could be taken before the request came, and the answer found there read as "can't reach the
  server"); whether a program fell back is read once its run is over, not after a fixed wait; a
  call that must never wait is offered a long wait it would show taking, not a short one any
  busy machine takes; a linear-time check always runs its few rounds before it gives up; a DNS
  that never answers is given a timeout with room to be asked; a stream's time budget times
  its requests alone. Each was run in loops on a machine kept busy on purpose, on Windows and
  Linux
- ✅ AI assistants (MCP) can provide a referring page (referer) when adding downloads, preserving
  the Referer header for protected CDNs and expired link renewal (thanks to
  [@abdul-karim-mia](https://github.com/abdul-karim-mia))
- ✅ AI assistants (MCP) can provide a custom User-Agent header when adding downloads, enabling
  downloads from sites and CDNs that require specific browser signatures (thanks to
  [@abdul-karim-mia](https://github.com/abdul-karim-mia))
- ✅ AI assistants (MCP) can provide a Cookie header when adding downloads, enabling
  downloads that require session authentication (thanks to
  [@abdul-karim-mia](https://github.com/abdul-karim-mia))
- ✅ The automation API's add may carry the checksum the finished file must have (the command
  line's `--checksum` form), kept with the download for **Verify checksum** and the finish
  check; one the app can't read is refused (`bad-request`) instead of dropped
- ✅ The command line's add accepts `--referer` (or `--referrer`) to preserve the referring page
  address (Referer header) for protected CDNs and expired link renewal (thanks to
  [@abdul-karim-mia](https://github.com/abdul-karim-mia))
- ✅ The command line's add accepts `--user-agent` (or `--ua`) to send a custom User-Agent header
  with the download request (thanks to
  [@abdul-karim-mia](https://github.com/abdul-karim-mia))
- ✅ The command line's add accepts `--cookie` (or `--cookies`) to send a Cookie header with the
  download request; `-` reads it from standard input instead of the command line (thanks to
  [@abdul-karim-mia](https://github.com/abdul-karim-mia))
- ✅ The command line's get and resume accept `--cookie` (or `--cookies`), `--referer` (or
  `--referrer`), and `--user-agent` (or `--ua`) to send custom request headers with the download;
  `-` reads cookies from standard input instead of the command line, and resume accepts them
  afresh without saving headers to the state file (thanks to
  [@abdul-karim-mia](https://github.com/abdul-karim-mia))
- ✅ The command line's add accepts `--private` (or `-p`) to add downloads flagged as private, and
  the MCP server's add_download tool accepts `private: true` (thanks to
  [@abdul-karim-mia](https://github.com/abdul-karim-mia))
- ✅ The command line's resume accepts `--checksum`, `--dns`, and `--adapter`, matching get's
  options for verifying finished downloads, custom DNS resolvers, and network adapter routing (thanks to
  [@abdul-karim-mia](https://github.com/abdul-karim-mia))
- ✅ The tests run in English whatever Windows' display language is: a Chinese, German or
  Arabic Windows ran 27 of `mdm-app`'s tests red on code CI's English runner passed, as the
  interface language followed the system in tests too
- ✅ **Alt** and a button's underlined letter press the button (the **Scheduler**'s **Apply**,
  **Close** and the others pressed nothing), letters of other scripts too, and **Alt** in such a
  window underlines the letters in every look, as in the main window
- ✅ With **Start immediately** on, picking the folder the early download already saves to in
  **Save As** keeps its name: the download's own `.mdm.part` and `.mdm.json` no longer count as
  taken, so it isn't offered as `song (1).mp3` and doesn't start over
  ([#597](https://github.com/anefzaoui/misterdownloadmanager/issues/597))
- ✅ The crash crate's tests build on 32-bit Windows again: the alternate-stack test uses a
  top a 32-bit `usize` can hold, with that crash's own address kept in its comment, so CI's
  `windows-x86` job is green again
- ✅ **Options → General → Download panels → Edit... → Button size**: **Full**, as before, or
  **Compact**, the round Download video button (the logo, the arrow and ✕) over every video
  ([#1278](https://github.com/anefzaoui/misterdownloadmanager/issues/1278))
- ✅ The extension's checks also pass from a Windows checkout with Git for Windows' defaults
  (`core.autocrlf=true`): the tree keeps LF everywhere except batch files, and the tests read
  sources, JSON and the catalogs by lines, so the 103 checks pass with either line ending
- ✅ The Download video button over a YouTube preview: it stays as the pointer reaches it, and a
  click that opens the Download video window sends the preview's video, not the one watched last
  ([#1271](https://github.com/anefzaoui/misterdownloadmanager/issues/1271))
- ✅ Linux: a crash is reported on computers whose processor doesn't offer XSAVE (an older
  processor, a virtual machine), where the crash handler faulted a second time and no report came
  ([#1239](https://github.com/anefzaoui/misterdownloadmanager/issues/1239))
- ✅ A crawling connection at a download's end is replaced again after the disk set the pace, instead
  of holding a last piece for a minute; the shared-line slow-connection test no longer fails on a busy
  CI runner
- ✅ **View → Draw with**'s note on when a graphics system is tried again reads the time zone
  the way the rest of the window does, never waiting for Windows
- ✅ Every release file gets a signed build provenance record (`gh attestation verify`), and the
  repository an OpenSSF Scorecard (thanks to [@joyahmed](https://github.com/joyahmed))
- ✅ The Linux AppImage is packaged by the same appimagetool and runtime on every rebuild: each
  at a fixed release, checked by its sha256 (thanks to [@joyahmed](https://github.com/joyahmed))
- ✅ macOS: dialogs and download progress windows activate MDM and open in front of browsers and
  other applications (including when raised or opened while MDM is behind another program or hidden
  in the menu bar), without pulling up minimized or tray-hidden windows or stealing focus for
  Download complete notices
- ✅ Screen readers in the main window: Tab goes round the toolbar, the Categories tree and the
  download list, and the screen reader says and follows the download, category or toolbar button
  the keys are on; the toolbar is a toolbar of named buttons; the lists in MDM's windows say their
  selected row as the arrows move it
  ([#1190](https://github.com/anefzaoui/misterdownloadmanager/issues/1190))
- ✅ A crash monitor slower than half a second to start is still connected to from beside the
  window, instead of the session going without crash reports; the end-to-end tests no longer
  fail on a slow CI runner
- ✅ The crash reports' end-to-end tests wait for the crash monitor before crashing, so a slow
  CI runner no longer fails them
- ✅ The Linux lab scripts (tools/lab) keep the lab VM account's password in private files, never on
  a command line, on the host or in the VM
- ✅ Bangla: a plural is **-গুলো**, never "-গুলি" — "নামগুলো", "সাইটগুলো", "ফাইলগুলো" — and
  a tab or window title takes none (thanks to [@Seyamalam](https://github.com/Seyamalam))
- ✅ Bangla: a web page is a **পেজ**, never a পৃষ্ঠা (a book's page) — "এই পেজে", "পেজটি",
  "পেজের ঠিকানা" — and a checksum a **চেকসাম** (thanks to [@Seyamalam](https://github.com/Seyamalam))
- ✅ Bangla: the control verbs stay distinct — stop is **থামান** (its state "থেমে গেছে"), a pause
  "বিরতি দিন", a resume "পুনরায় শুরু করুন", remove "বাদ দিন" — and a queue is a **সারি**, never
  "কিউ" (thanks to [@Seyamalam](https://github.com/Seyamalam))
- ✅ Arabic: the browser is the **مستعرض**, never المتصفح — "ملحق المستعرض", "الربط المتقدم
  بالمستعرض" (thanks to [@Seyamalam](https://github.com/Seyamalam))
- ✅ Bangla: termbase alignment — export is **এক্সপোর্ট**, import is **ইম্পোর্ট**, help is
  **সহায়তা**, category is **শ্রেণি**, options is **বিকল্প** and redownload is **আবার ডাউনলোড**
  (thanks to [@abdul-karim-mia](https://github.com/abdul-karim-mia))
- ✅ Bangla: spelled the Bangla Academy way — a link is a **লিংক**, a category a **শ্রেণি**,
  "already" is **ইতোমধ্যে** and a plural takes **-গুলো**, as the style guide settles them
  (thanks to [@Seyamalam](https://github.com/Seyamalam))
- ✅ Bangla: a menu or button's shortcut is its letter in brackets — "ফাইল(&F)", "থামান(&T)" —
  not "&" on a Bangla letter no key can press (thanks to
  [@Seyamalam](https://github.com/Seyamalam))
- ✅ The browser extension's window and Test browser integration read the browsers' folders
  beside the window, so a slow folder can't freeze it; macOS: a browser's folder macOS keeps MDM
  out of is said, with the setting that allows it
  ([#1218](https://github.com/anefzaoui/misterdownloadmanager/issues/1218))
- ✅ Malay: a queue is a **baris gilir**, the clipboard a **papan klip** and a checksum a
  **jumlah semak**, as the reviewed termbase settles them — not "giliran", "papan keratan" or
  "jumlah hasil tambah" (thanks to [@Seyamalam](https://github.com/Seyamalam))
- ✅ Windows: finished downloads drag out of the list onto a folder or a program, one or several,
  moved or copied as from Explorer; a moved file is followed, as from Download complete
  ([#1210](https://github.com/anefzaoui/misterdownloadmanager/issues/1210))
- ✅ French: a torrent's **tracker** is a "tracker", as French download managers call it, not a
  "traqueur" (thanks to [@Seyamalam](https://github.com/Seyamalam))
- ✅ A way of drawing that failed once is tried again: after a graphics driver update, or a
  while that depends on why it failed (twice as long each time it fails again), and at the next
  start for a window slow just after signing in; View → Draw with says since when and until when
  ([#1205](https://github.com/anefzaoui/misterdownloadmanager/issues/1205))
- ✅ The guide's links to **Click'n'Load** and **Proxy / Socks** land on their sections (thanks to [@Seyamalam](https://github.com/Seyamalam))
- ✅ The termbases reach the newest machine rows too — Arabic's browser is the مستعرض and
  Bangla's details are বিস্তারিত (thanks to [@Seyamalam](https://github.com/Seyamalam))
- ✅ macOS: no more **The window stopped responding** report while a file dialog is open: the
  time its window thread spends in a system Open or Save dialog isn't counted
  ([#1192](https://github.com/anefzaoui/misterdownloadmanager/issues/1192))
- ✅ The speed limit shows where the speed does: the progress window's Transfer rate names the
  limit holding the download back (its own, the Speed Limiter's, or the one for all downloads),
  the list's Transfer rate says it in its tip, and the status bar and the tray's tip say while the
  Speed Limiter is on ([#1191](https://github.com/anefzaoui/misterdownloadmanager/issues/1191))
- ✅ The window opens at once when an unfinished download is on a network share that doesn't
  answer or a sleeping drive: downloads' progress, and a new video's name, are read beside it
  (found with [#1170](https://github.com/anefzaoui/misterdownloadmanager/issues/1170))
- ✅ macOS: MDM no longer closes when the displays change while it draws with OpenGL (a dock
  and the lid closed): it takes its OpenGL context back when the system left its thread without
  it, and a texture OpenGL still refuses is made on a later frame
  ([#1173](https://github.com/anefzaoui/misterdownloadmanager/issues/1173))
- ✅ **Offer to download links copied to the clipboard** no longer opens Download File Info again
  for a link still on the clipboard after a moment another program held it
  ([#1157](https://github.com/anefzaoui/misterdownloadmanager/issues/1157))
- ✅ No freeze when downloads are deleted with their files: the rows go at once and the files are
  deleted beside the window ([#1170](https://github.com/anefzaoui/misterdownloadmanager/issues/1170))
- ✅ No freeze when a torrent is deleted on a slow disk: its kept `.torrent` and renamed file's
  record go beside the window, in order with adding the same torrent again
- ✅ No freeze when a download starts over at its own place on a slow disk (Redownload, a
  duplicate overwritten, a restart under the same name): the old files go beside the window and
  the new start waits for them on its own; a finished duplicate's file is looked at beside it too
  (found with [#1170](https://github.com/anefzaoui/misterdownloadmanager/issues/1170))
- ✅ More disk work beside the window: a start under a name another download has, a playlist's
  names, batch, Link grabber, browser and paired-device adds' names, File Properties' new address,
  and a rename on completion's check
  (found with [#1170](https://github.com/anefzaoui/misterdownloadmanager/issues/1170))
- ✅ The window no longer waits for its crash monitor at start: the connection is made beside it,
  a monitor slow to listen is connected to late, with what was sent meanwhile told again, and one
  the system refuses for a while no longer ends the session's crash reports
  ([#476](https://github.com/anefzaoui/misterdownloadmanager/issues/476))
- ✅ **Stop All** stops the queues too: a running queue no longer starts its next download
  ([#1164](https://github.com/anefzaoui/misterdownloadmanager/issues/1164))
- ✅ macOS: closing the window leaves MDM running in the Dock, and a click on its icon there brings
  the window back, menu bar icon or not; Cmd+Q quits
- ✅ Windows: shutting down, restarting or signing out saves MDM as File → Exit does (its
  downloads stop where they are and resume there, no crash report at the next start), and an
  installer closing MDM through the Restart Manager closes it; File → Exit also stops the
  downloads where they are
- ✅ Linux: logging out or shutting down saves MDM as File → Exit does (its downloads stop where
  they are and resume there, no crash report at the next start), whether the system asks it to end
  or its display goes away first
- ✅ No more **The window stopped responding** report in the first minutes after MDM starts with
  the computer: as after a wake, a stall then is the computer's (logged, and reported only if it
  lasts past the first three minutes)
  (for [#1126](https://github.com/anefzaoui/misterdownloadmanager/issues/1126))
- ✅ No more **The window stopped responding** report while MDM waits in the notification area
  with no window on screen and its thread waits on the computer: logged, and reported only if it
  lasts past a minute, if MDM's thread is busy, or if MDM is asked for something meanwhile and
  doesn't answer (for [#1126](https://github.com/anefzaoui/misterdownloadmanager/issues/1126))
- ✅ Link grabber's **Status** column: a failure whose sentence ends in advice shows that advice
  in the column, with the whole sentence on hover, and the column is as wide as its longest answer
  ([#1110](https://github.com/anefzaoui/misterdownloadmanager/issues/1110))
- ✅ A malformed stream playlist no longer stops the video downloader: that download fails and
  says why ([#1141](https://github.com/anefzaoui/misterdownloadmanager/issues/1141))
- ✅ A queue (or a download) set to **Exit Mister Download Manager when done** quits MDM, as File →
  Exit does, with **Keep running when closing the window** on; it only hid MDM in the notification
  area
- ✅ Windows and macOS: names in any script a font on the computer has show their letters, whatever
  the interface language: the system's own font for the scripts MDM lists none for
  ([#1125](https://github.com/anefzaoui/misterdownloadmanager/issues/1125))
- ✅ Tests: the ones that failed now and then on a busy runner (keeping the computer awake, a
  program or a call to the system that hangs, a stream's time for both its requests, a helper's
  children ending with it, the clipboard used from many threads at once) check what they guard
  by the order things happen in, not by the runner's speed: a fake system that answers when the
  test says, a program that ends when the test lets it, a count of the work done instead of a
  fixed time
- ✅ The Download video button over a video in a feed, or on a site yt-dlp can't read (Douyin):
  when MDM finds nothing on the page, it offers what the tab played for that video, told apart
  from the feed's next one by its length, and says so when it can't tell
  ([#1092](https://github.com/anefzaoui/misterdownloadmanager/issues/1092))
- ✅ No freeze when a video or a gallery finishes on a slow or busy disk: its files are marked as
  downloaded from the internet by the download itself, before it's done, and a rename on
  completion happens beside the window (for [#1103](https://github.com/anefzaoui/misterdownloadmanager/issues/1103))
- ✅ A crash while checking for updates on some computers; a check that goes wrong says why
  ([#1123](https://github.com/anefzaoui/misterdownloadmanager/issues/1123))
- ✅ Torrents: trackers at addresses that can't be on the internet are skipped
- ✅ Declared videos: script text inside a script is not read as a declaration
- ✅ Reading a page's text stops at the right closing tag
- ✅ Torrents with a file and a folder of the same name are refused cleanly
- ✅ Archive volumes with unusual letters in their names no longer close MDM
- ✅ A damaged progress file no longer miscounts a download's size
- ✅ An RSS feed's failed first fetch leaves its backlog a backlog, and an empty search feed still
  gets its episode as news
- ✅ An excluded site is the same site written with the root's dot (`example.com.`), in the app
  and the extension
- ✅ Proxy auto-config follows the address a script was given, and an https address's path stays
  out of the script
- ✅ Tests: the programs tests write and then run (the extraction tests' stand-in for HatchZip,
  the Linux file manager's and the browser helper's) no longer fail now and then with "Text file
  busy" (a file written while another test started a program was still open there for writing
  when it ran); a program of their own writes them, and a test that gets another result says which
- ✅ A video with a long title (an X post's text) no longer fails at 0% with **No such file or
  directory** on Windows: its name is shortened so its download's files fit Windows' path limit,
  and a name or folder too long says so
  ([#1102](https://github.com/anefzaoui/misterdownloadmanager/issues/1102))
- ✅ A gallery of tens of thousands of files already in its folder no longer seems to hang: its
  files are listed in time linear in their number
  ([#1142](https://github.com/anefzaoui/misterdownloadmanager/issues/1142))
- ✅ The notification-area icon is sharp beside the others: it is the hand-made picture of the
  size the taskbar draws it at (16 px at 100 %, 24 at 150 %, 32 at 200 %), drawn again when the
  display's scale changes, instead of the 256 px one Windows shrank
  ([#1161](https://github.com/anefzaoui/misterdownloadmanager/issues/1161))
- ✅ Tests: the checks that a long list is read in linear time, which still failed now and then on
  a busy macOS runner, time one input 16 times as long against 16 short ones (as many items, as
  long a run), take each side's fastest of several runs, and fail at three times as long (a square
  costs 16 times): steady under load, and they catch more of the squares they guard against
- ✅ Tests: a MediaFire download resumed after its address expired is checked request by
  request (nothing it had on disk is asked again, but for the few bytes each part re-reads to
  check the file), not by a total of what the server sent, which grew on a busy runner when a
  part was split while its bytes were on the way
  ([#952](https://github.com/anefzaoui/misterdownloadmanager/issues/952))
- ✅ macOS: a browser hand-off while MDM is hidden from the menu bar opens its window
- ✅ Downloads for Windows on ARM, 32-bit Windows and Linux on ARM, beside the x64 ones: the
  installer, the portable zip and the plain zip; the AppImage, .deb, .rpm and .tar.gz. Each is
  built and tested on its own processor, Update now keeps each copy on its own, and the helpers
  come in each processor's build (no ffmpeg for 32-bit Windows: Helpers says so)
  ([#977](https://github.com/anefzaoui/misterdownloadmanager/discussions/977),
  [#1010](https://github.com/anefzaoui/misterdownloadmanager/discussions/1010))
- ✅ Torrents through a debrid service: a file already at the torrent's place counts as the
  torrent's only at its size; another one stays untouched and the torrent's file comes down
  beside it, numbered ([#1143](https://github.com/anefzaoui/misterdownloadmanager/issues/1143))
- ✅ The progress window's connections table shows connection numbers 10 and up instead of
  **...**: the number column fits its widest number
  ([#1101](https://github.com/anefzaoui/misterdownloadmanager/issues/1101))
- ✅ YouTube: a video the browser's button sends with the page's cookies, refused with 403 or the
  bot check as a signed-in viewer's, is looked up once more without them, as a private window
  gets it; private, members' and age-restricted videos keep them
  ([#686](https://github.com/anefzaoui/misterdownloadmanager/issues/686))
- ✅ RSS feeds: removing a feed turns off the download rules that watched only it, instead of
  widening them to every feed, and the rule says why
  ([#1144](https://github.com/anefzaoui/misterdownloadmanager/issues/1144))
- ✅ **Download File Info**: **...** beside **Save As** opens the system's Save dialog (the
  folder and the file name, the download's type listed), and the name typed there is the one
  saved ([#1088](https://github.com/anefzaoui/misterdownloadmanager/issues/1088))
- ✅ **Download File Info**: **Yes** to the Save dialog's "Replace?" replaces that file under its
  own name once the download is complete (it stays as it was until then); it was saved as
  `name (1)` ([#1088](https://github.com/anefzaoui/misterdownloadmanager/issues/1088))
- ✅ The browser's **Download all links** on a page with thousands of long links: the list goes to
  MDM in parts the browser lets through, and opens in Link grabber whole, as one list; past 1 MB of
  links nothing arrived
- ✅ Browser integration: a copy run from inside a zip (a temporary folder) no longer sets it up
  for itself and leaves the browsers looking for a copy that is gone; MDM takes back a
  registration that leads nowhere at each start, never one that works, and Test browser
  integration says which copy is gone
  ([#818](https://github.com/anefzaoui/misterdownloadmanager/issues/818))
- ✅ Text boxes keep their selection under a right-click, so the menu's Copy and Cut take it
  (Select all, then Copy, copied nothing)
  ([#1084](https://github.com/anefzaoui/misterdownloadmanager/issues/1084))
- ✅ A page that says which video it is about (its JSON-LD, `og:video` or `twitter:player`) gets
  that video from Add URL, Download video and the command line, not the first player written in
  its HTML (a hidden welcome video)
  ([#1076](https://github.com/anefzaoui/misterdownloadmanager/issues/1076))
- ✅ The browser extension's popup has **Hide the download button in full screen**, off as installed: entering full screen hides the **Download video** button at once, as the player's own controls, and the pointer's next move brings it back; on, a page in full screen shows no button at all, for watching without it in the way ([#670](https://github.com/anefzaoui/misterdownloadmanager/issues/670))
- ✅ Windows: MDM listed in Settings → Default apps for MAGNET and .torrent, and Torrent
  settings' switch says when another app chosen there still opens them, and opens that page
  ([#1091](https://github.com/anefzaoui/misterdownloadmanager/issues/1091))
- ✅ A .torrent downloaded in the browser stays with the browser (and your own torrent program)
  while Torrent settings' **Open magnet links and .torrent files with Mister Download Manager**
  is off ([#1058](https://github.com/anefzaoui/misterdownloadmanager/issues/1058))
- ✅ A video in a player embedded from another site (Vimeo's on a film site) is named after the
  page's title, without the site's name, instead of the player's own title
- ✅ No freeze of the window in Link grabber with thousands of links (a big page's links): each
  link's category is worked out once, and a frame goes over the links once, however many
  packages they make ([#1063](https://github.com/anefzaoui/misterdownloadmanager/issues/1063))
- ✅ **Refresh download address** opens the page the download was started from, not the site's
  home page ([#607](https://github.com/anefzaoui/misterdownloadmanager/issues/607))
- ✅ A video's look-up no longer waits 20 s on a server the network drops: 5 s, and once more
  with yt-dlp's own wait only when the look-up failed for a slow server
  ([#1053](https://github.com/anefzaoui/misterdownloadmanager/issues/1053))
- ✅ MediaFire: a download resumed after its address expired asks the file's MediaFire page for a
  fresh one and goes on; a MediaFire file page's link downloads the file
  ([#952](https://github.com/anefzaoui/misterdownloadmanager/issues/952))
- ✅ Chromium browsers: a video (or a PDF, an MP3) the browser would play, taken by MDM, keeps
  the name its server gives it instead of its address's (`main.mkv`)
  ([#948](https://github.com/anefzaoui/misterdownloadmanager/issues/948))
- ✅ A download refused for its site's security certificate says why (expired, not trusted, for
  another name) instead of a bare **Error** (follows [#762](https://github.com/anefzaoui/misterdownloadmanager/issues/762))
- ✅ **Options → Downloads → Customize "Download progress" dialog → Edit...** works: it can turn
  off the progress window that opens when a download starts; the button was greyed and did
  nothing ([#1041](https://github.com/anefzaoui/misterdownloadmanager/issues/1041))
- ✅ The browser popup's **Never take downloads from <site>** says it worked, with **Undo**
  ([#404](https://github.com/anefzaoui/misterdownloadmanager/issues/404))
- ✅ A download to a disk slower than the network no longer stands still for a second or more
  just after it starts (a second save of its progress came too soon after the first); a server
  whose name the DNS doesn't answer in time says it couldn't be found, not that it didn't answer.
  Two tests that failed now and then on macOS check both without the network or the runner's speed
  ([#782](https://github.com/anefzaoui/misterdownloadmanager/issues/782))
- ✅ Linux, Wayland: MDM chooses its windows' system the same way however it starts (its menu
  entry, the browser's helper, autostart): Wayland's own windows on a scaled screen where nothing
  tells X11 programs the scale (Hyprland, labwc, sway), XWayland elsewhere; **View → Window
  system** to choose ([#624](https://github.com/anefzaoui/misterdownloadmanager/issues/624))
- ✅ A PDF a page shows in a frame, an embed or an object goes to MDM like one opened from a link,
  in Chromium browsers and Firefox; a page's own PDF viewer keeps the file it reads
  (part of [#1087](https://github.com/anefzaoui/misterdownloadmanager/issues/1087))
- ✅ The sites in **Don't start downloading automatically from the following sites** take their
  subdomains and the files their pages link to on another server (a CDN), as the Download panels'
  list does
  ([#1049](https://github.com/anefzaoui/misterdownloadmanager/issues/1049))
- ✅ The menu bar with a screen reader: Alt opens it again, and the screen reader says each item
  as Alt, F10 or the arrows reach it; Shift+F10's menus too
  ([#328](https://github.com/anefzaoui/misterdownloadmanager/issues/328))
- ✅ Windows with Direct3D 12: the progress window no longer flashes a stretched picture when
  Show details or Hide details changes its size
  ([#471](https://github.com/anefzaoui/misterdownloadmanager/issues/471))
- ✅ Videos looked up about a second sooner on Windows (yt-dlp's folder build, which doesn't unpack
  itself at every start), and the video button's look-up starts once the pointer rests on it;
  in a private window only on the click (for
  [#1053](https://github.com/anefzaoui/misterdownloadmanager/issues/1053))
- ✅ Extracting archives: the size limit covers every entry, also the ones left out, and
  cancelling stops there too
  (part of [#37](https://github.com/anefzaoui/misterdownloadmanager/issues/37))
- ✅ No reconnecting in waves on a shared line: a connection is measured against its share of
  what the connections bring together, not the speed the first one had alone
  ([#896](https://github.com/anefzaoui/misterdownloadmanager/pull/896))
- ✅ Download panels (the video button): site exclusions support wildcards (`*`) like `*.youtube.*` or `youtube.*` ([#759](https://github.com/anefzaoui/misterdownloadmanager/issues/759))
- ✅ Firefox: one Download video button over a video, not two, after the extension is updated while
  it shows (seen on YouTube in full screen)
  ([#336](https://github.com/anefzaoui/misterdownloadmanager/issues/336))
- ✅ Alt+click (Option+click on a Mac) leaves a download to the browser in Firefox too, where it
  did nothing, and when the link redirects or a page's script starts the download
  ([#332](https://github.com/anefzaoui/misterdownloadmanager/issues/332))
- ✅ Left and Right in Download File Info's Save As box and Add URL's Address box move the caret,
  no longer the focus to the button beside the box
  ([#812](https://github.com/anefzaoui/misterdownloadmanager/pull/812))
- ✅ The progress window shows no scroll bars over content that fits at an interface size above
  100 % (125 %), where the window's client comes back a pixel short of its content
  ([#672](https://github.com/anefzaoui/misterdownloadmanager/issues/672))
- ✅ Less memory during fast downloads to a slower disk: what waits for the disk stays within its
  256 MB as allocated (a 4 GB download: 967 MB in 0.5.2, 523 MB now with the window open)
- ✅ Windows and macOS: **Options → General → Shortcut to show or hide window**: a global
  shortcut of two or three keys that shows or hides MDM's window from any program
  ([#558](https://github.com/anefzaoui/misterdownloadmanager/issues/558))
- ✅ Linux: no crash when fontconfig offers a font MDM can't draw with (an old bitmap font for
  Japanese or for the symbols): the best font that scales and MDM can read is used instead, and
  no font MDM can't read reaches its windows on any system
  ([#931](https://github.com/anefzaoui/misterdownloadmanager/issues/931),
  [#1412](https://github.com/anefzaoui/misterdownloadmanager/issues/1412))
- ✅ Bangla reviewed by a native speaker: the main window, the list, the progress window, the
  dialogs and the tray, with the menus' letters on English letters
  ([#927](https://github.com/anefzaoui/misterdownloadmanager/pull/927))
- ✅ Menus' keyboard letters: no two items of one menu share one. Arrange files' By Status has U, so A
  reaches By Date Added ([#926](https://github.com/anefzaoui/misterdownloadmanager/issues/926),
  [#928](https://github.com/anefzaoui/misterdownloadmanager/pull/928)); the list's blank-space menu
  adds a download with N, so A reaches Select all; View → Text has X. A test checks every menu
  MDM builds, in English and in the languages that keep its letters
- ✅ Windows 10: no FSI and PDI boxes around a file's name in the taskbar's preview and tip of a
  progress window, or in the notification-area icon's tip; right-to-left titles keep their order
  ([#919](https://github.com/anefzaoui/misterdownloadmanager/issues/919))
- ✅ Windows: Open starts the program Windows has for the file's type (7-Zip for a .zip when it
  opens them), as a double-click in Explorer does, instead of failing and showing the folder
  ([#811](https://github.com/anefzaoui/misterdownloadmanager/issues/811),
  [#854](https://github.com/anefzaoui/misterdownloadmanager/pull/854))
- ✅ The download list's column lines run straight from the header down: the rows' lines sat a
  pixel to the right of the header's dividers
  ([#924](https://github.com/anefzaoui/misterdownloadmanager/issues/924))
- ✅ With a style theme (Metro, Adwaita, Aero…), windows minimize, maximize and restore with
  Windows' animation, as with Classic, and fade out into the notification area; on Windows 10
  without its own title bar showing over MDM's for those frames
  ([#874](https://github.com/anefzaoui/misterdownloadmanager/issues/874))
- ✅ Windows 10 with a style theme (Metro, Adwaita, Aero…): no black band above the main window's
  title bar, and its menus and clicks land where they show; framed as on Windows 11, as the
  progress window already was ([#802](https://github.com/anefzaoui/misterdownloadmanager/issues/802))
- ✅ An idle MDM no longer redraws its window each second to watch the clipboard for copied
  links: the watch runs on its own thread and wakes the window only for a new text (with
  Windows' software renderer, idle MDM went from most of a processor core to a sixth of it)
  ([#847](https://github.com/anefzaoui/misterdownloadmanager/issues/847))
- ✅ Windows: a graphics card without Direct3D 12 (Intel's HD Graphics 3000 to 5500, older AMD
  Radeon cards) draws with OpenGL, then with Direct3D 12 on Windows' own software renderer (WARP)
  once OpenGL fails there: it can't start, is too old, shows no first window, or MDM freezes
  inside its driver (read from the freeze report); remembered, **View → Draw with** says why, and
  choosing **OpenGL** tries it again ([#847](https://github.com/anefzaoui/misterdownloadmanager/issues/847))
- ✅ Chinese, Japanese and Korean: interface text is at least 12 px (the theme's `font.cjk`) and
  is drawn by the same DirectWrite rasterizer as Latin, so it reads larger and stays sharp at
  110–120 % interface sizes instead of thinning out; English and the other languages keep their
  sizes ([#837](https://github.com/anefzaoui/misterdownloadmanager/issues/837))
- ✅ Help: videos that need you signed in, in short: the button over the video brings the page's
  own cookies, Options reads a browser profile's for pasted links and playlists (a separate
  profile with a spare account works well), and why some videos and networks need it (from
  [#721](https://github.com/anefzaoui/misterdownloadmanager/issues/721))
- ✅ A video whose picture and sound can't be joined says so in full and why, and that its downloaded
  parts stay in the folder for Resume; deleting it with its files removes them
- ✅ Tests: the merge's percentages, which a slow macOS runner once saw go back (the test's fake
  ffmpeg rewrote its file, read empty between two writes), never go back: the app holds them, and
  the test writes its file as ffmpeg does, empties it once on purpose and checks the end is reached
- ✅ Torrents: File Properties closes with OK, instead of asking for an http(s) or ftp(s)
  address; a torrent's magnet link is shown read only there
  ([#904](https://github.com/anefzaoui/misterdownloadmanager/issues/904))
- ✅ Tests: the test server's HLS videos carry a real picture (H.264), so its video and sound
  tracks merge into one file as a site's do; its video tracks held only sound, which made the
  merge fail ("Error opening output files: Invalid argument"). Real sites' videos merged already
- ✅ macOS: .torrent files open in MDM from Finder (double-click, Open With, the Dock's icon) and
  magnet links from the browser, and Torrent settings can make MDM their default app there
  ([#698](https://github.com/anefzaoui/misterdownloadmanager/issues/698))
- ✅ Windows: a menu closes at once when you click outside MDM, instead of 0.4 seconds later
  ([#859](https://github.com/anefzaoui/misterdownloadmanager/issues/859))
- ✅ Windows: programs MDM starts (an update's installer, a browser, the power options) start
  beside the window, which no longer waits while an antivirus checks them
  ([#846](https://github.com/anefzaoui/misterdownloadmanager/issues/846))
- ✅ No "The window stopped responding" report in the first minute after the computer wakes from
  sleep, while its disk, network and screen come back: such a stall is logged, and reported only
  if it lasts longer; every freeze report says what the window was doing when it stopped
  ([#574](https://github.com/anefzaoui/misterdownloadmanager/issues/574),
  [#613](https://github.com/anefzaoui/misterdownloadmanager/issues/613))
- ✅ A stream the browser played (an .m3u8) is looked up and downloaded with the Referer, Origin
  and cookies the browser sent with it, for its playlists and every piece too: a site's video
  server that serves its own pages' players only answered 403
  ([#900](https://github.com/anefzaoui/misterdownloadmanager/issues/900))
- ✅ A video of unknown size no longer reads 99.9 % from its first second while yt-dlp downloads it,
  and a video named after a page titled with its file's name gets its extension once (Dropbox's
  `Episode 31.mp4.mp4`) ([#622](https://github.com/anefzaoui/misterdownloadmanager/issues/622))
- ✅ Tests: the crash reports' end-to-end tests find their crashing example where Rust 1.100's
  Cargo builds the tests (in its build folder, no longer in deps/), as well as where earlier ones
  did ([#934](https://github.com/anefzaoui/misterdownloadmanager/issues/934))
- ✅ Where a download's server is: shown once the server answered, its address and round trip
  together, not a round trip alone (twice too long over https) while a slow server answers; it
  also made a test fail now and then on Linux
  ([#933](https://github.com/anefzaoui/misterdownloadmanager/issues/933))
- ✅ Tests: the slow disk's model is checked on its own clock, and the speed limiter's and a DNS
  check on Tokio's paused one, instead of the time a busy runner takes (one failed on macOS)
- ✅ Tests: the five that checked a long list is read in linear time against a fixed number of
  seconds (one failed on a slow macOS runner) now time the same work at two sizes, eight times
  apart, and fail only when the larger one costs over 24 times as much (a square costs 64 times),
  whatever the machine's speed
- ✅ Touch screens: a tap on a menu's item chooses it (the menu bar's, right-click menus,
  drop-downs), where only a mouse could
  ([#733](https://github.com/anefzaoui/misterdownloadmanager/issues/733))
- ✅ A video's Address is its file's (the stream MDM downloads) in the progress window, Download
  complete and File Properties, its page the web page it was obtained from; a download's page and
  Referer are kept after a restart
  ([#795](https://github.com/anefzaoui/misterdownloadmanager/issues/795))
- ✅ The progress window's connections table while a video downloads: MDM's own connections (as
  many as Options → Connection gives), and yt-dlp's one connection while it looks the page up or
  downloads a live stream itself, instead of an empty table
  ([#793](https://github.com/anefzaoui/misterdownloadmanager/issues/793))
- ✅ After the last byte, the progress window says **Verifying… (45%)** or **Merging… (45%)**
  and the list's Status column, wider by default, **Checking 45%** or **Merging 45%** (the
  whole words in its tip), the bar
  showing it in the accent colour, no transfer rate and no connection still receiving, and
  **Finishing…** rather than **Connecting…**; a finished download's map of parts has no gaps
  ([#736](https://github.com/anefzaoui/misterdownloadmanager/issues/736))
- ✅ Windows: the file dragged out of **Download complete** is moved or copied as Explorer does
  it (moved on the same drive or with Shift, copied to another drive or with Ctrl), no longer
  always copied, and its download follows a moved file to its new place; where MDM can't learn
  the new place, **Open** and **Open folder** say the file was moved elsewhere
  ([#869](https://github.com/anefzaoui/misterdownloadmanager/issues/869))
- ✅ Windows: a graphics card whose OpenGL is older than 3.0 and that has no Direct3D 12 (Intel's
  first HD Graphics) no longer draws MDM's windows as garbage: MDM notices that OpenGL, draws
  with Direct3D on Windows' own software renderer (WARP) instead, from then on, and **View → Draw
  with** says why; the reporter confirmed WARP draws it right
  ([#835](https://github.com/anefzaoui/misterdownloadmanager/issues/835))
- ✅ System helper detection on Windows and Unix: yt-dlp, ffmpeg, gallery-dl and JS runtimes
  (deno, node, quickjs, bun) are discovered across package managers (WinGet, Scoop, Chocolatey,
  Homebrew, Flatpak, Nix), pip environments and standard directories; a bgutil Proof-of-Origin
  Token (POT) provider the person already runs is detected (a server on 127.0.0.1:4416, a
  `bgutil-pot` binary, an installed yt-dlp plugin), never downloaded or started by MDM
- ✅ A helper (yt-dlp, ffmpeg, gallery-dl) that can't be downloaded says which server and why
  (not found, refused, no answer, a secure connection an antivirus or a proxy reads, GitHub's
  limit on requests, cut short) and what to do; its release list goes through the proxy set in
  Options, as its download did ([#816](https://github.com/anefzaoui/misterdownloadmanager/issues/816))
- ✅ Link grabber lists a web page's links when given the page's address (pasted, copied, in
  a list), as the browser's "Download all links" does, instead of only the page itself
  ([#705](https://github.com/anefzaoui/misterdownloadmanager/issues/705))
- ✅ Windows: **Open with...** shows Windows' "Select an app to open this file" again, for
  files whose name or folder has a space too
  ([#720](https://github.com/anefzaoui/misterdownloadmanager/issues/720))
- ✅ A file host's web page (a countdown, a captcha, DropMeFiles', dailyuploads') is never saved
  under the archive's name and called finished: the download stops, says the site sent its page
  and to finish in the browser, and the progress window's **Open in browser** opens the link
  there. The page is told by its first bytes too, when it comes as the file's own type or under
  its name, by the size the browser announced, and on resume, which keeps the data already
  downloaded ([#701](https://github.com/anefzaoui/misterdownloadmanager/issues/701),
  [#692](https://github.com/anefzaoui/misterdownloadmanager/issues/692))
- ✅ **Embed chapters**: a video's chapters go into the downloaded file (with ffmpeg), on by
  default, in the Download video window under Subtitles
  ([#827](https://github.com/anefzaoui/misterdownloadmanager/issues/827))
- ✅ A YouTube video that lists only the sound says why: yt-dlp has no JavaScript runtime. The
  video window, the menu over a video and the command line say to install Node.js or Deno, with
  a link to the help page's commands for each system
  ([#852](https://github.com/anefzaoui/misterdownloadmanager/issues/852))
- ✅ Video helpers: the JavaScript runtime yt-dlp is given is looked for more carefully
- ✅ Linux: no freeze of the window where the session bus doesn't answer (a session started
  without D-Bus, a bus that hangs): the tray icon is set up beside the window, which carries on
  without it until the bus answers, and gets it then, however late
  ([#828](https://github.com/anefzaoui/misterdownloadmanager/issues/828))
- ✅ Looking up a new video (put online in the last two days) no longer fails with "Permission
  denied" in MDM's install folder, or in `System32` when MDM started with Windows: the helpers
  run in your temporary folder, or MDM's own if that one can't be written; on Windows a file
  opened from MDM starts in its own folder
  ([#834](https://github.com/anefzaoui/misterdownloadmanager/issues/834),
  [#1216](https://github.com/anefzaoui/misterdownloadmanager/issues/1216))
- ✅ A video a site sends through a program's address (`stream.php?file=…`,
  `remote_control.php`) is named by what it is: the page's title (or the address's `file=` name)
  and its real type, `Lecture.mp4`, not `stream.php`; the browser's media list shows that name
  and type, and the Download video window saves it instead of failing on its `.php`
  ([#695](https://github.com/anefzaoui/misterdownloadmanager/issues/695),
  [#800](https://github.com/anefzaoui/misterdownloadmanager/issues/800))
- ✅ The issue tracker: an issue waiting for details from its reporter is labelled "needs info";
  with no answer 14 days after our question it closes with a note, and the reporter's reply
  opens it again
- ✅ Ideas move to GitHub Discussions (Ideas), where they can be talked over and voted on; the
  issue form for them points there
- ✅ **Add batch download from clipboard** reads every link copied (a list, a forum post): http,
  ftp and magnet links, each once, ticked in Link grabber, started or queued together with one
  summary at the end; a copied address with a number in it (`…/ep01.mp4`) opens Batch download
  with its pattern; also in the tray menu, and Ctrl+B. Copying several links no longer opens
  Download File Info with them run together
  ([#709](https://github.com/anefzaoui/misterdownloadmanager/issues/709),
  [#780](https://github.com/anefzaoui/misterdownloadmanager/issues/780))
- ✅ Windows: downloads follow the Group Policy "Do not preserve zone information in file
  attachments", as browsers' do; **Options → Save to → Mark downloaded files as coming from
  the internet** (on by default) turns the mark off for MDM alone, on every system
  ([#813](https://github.com/anefzaoui/misterdownloadmanager/issues/813))
- ✅ No freeze of the window on **Tasks → Import** of a large file, or of one that isn't a list
  (a video, an archive, another program's backup): it is read beside the window, the status strip
  says "Reading …", a file over 64 MB is refused with a message, and Link grabber takes 10,000
  links at once ([#805](https://github.com/anefzaoui/misterdownloadmanager/issues/805))
- ✅ A video quality's size is the size of what downloads: each line of the Download video window
  and of the browser's menu is the very video and sound it was sized from, and a size that is
  only an estimate shows as about ("~532 MB"); a portrait 4K video showed 3.79 GB and downloaded
  532 MB ([#801](https://github.com/anefzaoui/misterdownloadmanager/issues/801))
- ✅ Video qualities show their codec, frame rate and HDR (`2160p · VP9 · 60 fps · HDR`), and AV1,
  VP9 and H.264, HDR and SDR of one resolution are each their own choice; the best stays first
  ([#755](https://github.com/anefzaoui/misterdownloadmanager/issues/755))
- ✅ Fewer Download complete windows: downloads added together (a playlist, a batch, Link
  grabber's links) show one summary once all are done, with how many finished and which failed,
  Open folder and Show in list; a single download keeps its window
  ([#766](https://github.com/anefzaoui/misterdownloadmanager/issues/766)). **Snooze** in Download complete keeps it away for
  30 minutes, 1 hour, 8 hours or until MDM restarts, and Options → Downloads says until when, with
  Resume now ([#741](https://github.com/anefzaoui/misterdownloadmanager/issues/741))
- ✅ WinGet in the install steps: the README, the help and the website's download page give
  `winget install AhmedNefzaoui.MisterDownloadManager` and `winget upgrade`
  ([#798](https://github.com/anefzaoui/misterdownloadmanager/issues/798))
- ✅ The Download video button, redesigned ([#703](https://github.com/anefzaoui/misterdownloadmanager/issues/703)): MDM looks the video up as the
  button appears and the button names the best quality, which one click downloads; a plain file
  downloads as a file; its menu lists every quality; after a click it says what happened in its
  own colour, with Show, Open post, Get MDM and Retry, or Turn on
- ✅ The video button no longer comes back on play or pause, and fades sooner (part of
  [#741](https://github.com/anefzaoui/misterdownloadmanager/issues/741))
- ✅ No freeze of the window while it checks Windows' light or dark mode and the registry takes
  seconds to answer: the system's look and the theme files are read beside the window
  ([#719](https://github.com/anefzaoui/misterdownloadmanager/issues/719))
- ✅ Windows 10: no black band above the progress window's title bar, and clicks land on its
  buttons again ([#620](https://github.com/anefzaoui/misterdownloadmanager/issues/620),
  [#772](https://github.com/anefzaoui/misterdownloadmanager/issues/772)): the progress window and
  the style themes' dialogs are framed as on Windows 11, without a hidden title bar
- ✅ Windows: download progress on each download window's taskbar button, alongside the main
  window's existing combined progress
  ([#678](https://github.com/anefzaoui/misterdownloadmanager/issues/678))
- ✅ Fast downloads to a hard disk keep going past the first gigabytes: no more freezes of 7 to
  10 s once the system's cache is full, and no connection replaced as "much slower" for waiting on
  the disk; a disk slower than the network slows the download down evenly
  ([#782](https://github.com/anefzaoui/misterdownloadmanager/issues/782))
- ✅ Windows: MDM starts on a PC without Microsoft's Visual C++ runtime, or with the wrong copy of it
  (0xc000007b at setup's end): the runtime is inside MDM's programs
  ([#785](https://github.com/anefzaoui/misterdownloadmanager/issues/785))
- ✅ Videos in one file (YouTube's qualities and their sound, most sites' MP4s) come down through
  MDM's own downloader: yt-dlp finds them, then every connection Options → Connection gives, the
  speed limits (a change applies at once), pause and resume like a file, after a restart too,
  and a fresh address from yt-dlp when one expires (part of
  [#769](https://github.com/anefzaoui/misterdownloadmanager/issues/769))
- ✅ Videos sent in pieces (HLS, DASH) through MDM's own downloader too, several pieces at once
  (as many as a file's connections), AES-128 pieces decrypted, expired pieces' addresses asked of
  yt-dlp again ([#531](https://github.com/anefzaoui/misterdownloadmanager/issues/531), part of
  [#769](https://github.com/anefzaoui/misterdownloadmanager/issues/769))
- ✅ Image galleries through MDM's own downloader: gallery-dl lists the pictures, MDM brings
  them in 4 at a time under the speed limits, gallery-dl fetches only what is left
  ([#769](https://github.com/anefzaoui/misterdownloadmanager/issues/769))
- ✅ Sites behind Akamai's bot check (a 403 page for MDM, the file for a browser) download, through the
  browser-like connection ([#771](https://github.com/anefzaoui/misterdownloadmanager/issues/771))
- ✅ Crash and freeze reports no longer have their log's end filled with warnings about the
  file-type icons ([#767](https://github.com/anefzaoui/misterdownloadmanager/issues/767))
- ✅ Videos and image galleries keep to the speed limits (for one file, their own, and all
  downloads together), and start again with a new one when it changes; one started while the
  Scheduler's download limit is reached waits
  ([#708](https://github.com/anefzaoui/misterdownloadmanager/issues/708))
- ✅ The browser's Download video button in right-to-left languages: its corners and divider follow the text direction, so its outline stays whole
- ✅ Download video: a title with line breaks stays on one line, shortened with "..." and shown
  whole on hover, instead of drawing its extra rows over the list and past the window (part of
  [#350](https://github.com/anefzaoui/misterdownloadmanager/issues/350))
- ✅ A freeze of the download windows on a drive slow to answer (a sleeping disk, a USB drive, a
  network folder): Download File Info's numbered name, Show in folder, Download complete and
  creating a missing folder ask the disk beside the window
  ([#743](https://github.com/anefzaoui/misterdownloadmanager/issues/743),
  [#751](https://github.com/anefzaoui/misterdownloadmanager/issues/751))
- ✅ **Deferred Rename** on a download's **Download status** tab: save a new name while downloading to rename the finished file's stem (and video subtitles) or an exclusive finished folder on completion, leaving shared/save or pre-existing folders, split-archive volumes and single-file torrents under their original names ([#655](https://github.com/anefzaoui/misterdownloadmanager/pull/655))
- ✅ Links added by hand go with the User-Agent of your default browser, at the version you have
  (Chrome, Edge, Brave, Opera, Vivaldi, Firefox, Safari), instead of a years-old one; **Automatic**
  in Options, or one you type (part of
  [#668](https://github.com/anefzaoui/misterdownloadmanager/issues/668))
- ✅ Linux freeze and crash reports name MDM's functions again, and the right ones: the stack of
  a program whose code begins a page after its start (0.5.0's) was read at the wrong place, so
  reports showed only system libraries, or names from elsewhere in MDM
  ([#661](https://github.com/anefzaoui/misterdownloadmanager/issues/661))
- ✅ Windows: a freeze in the C runtime's memory routines, where the window hardly ran, is
  reported as a computer too busy or slow to load MDM from disk, not as a deadlock
  ([#663](https://github.com/anefzaoui/misterdownloadmanager/issues/663))
- ✅ macOS: helpers opened from Finder find Homebrew's Deno and ffmpeg, so YouTube formats
  load ([#659](https://github.com/anefzaoui/misterdownloadmanager/issues/659))
- ✅ The website is named as the official one in the README, the browser add-on and the
  site's data for search engines
- ✅ Left click on the tray icon toggles the main window (on Windows also with the icon under
  the taskbar's ^), and browser downloads open only their dialog when the main window is
  hidden or minimized ([#520](https://github.com/anefzaoui/misterdownloadmanager/pull/520))
- ✅ Debrid accounts: signing in by code opens only the service's own page
- ✅ A speed limit raised or turned off applies at once, to the connections already waiting
  under the old one too ([#621](https://github.com/anefzaoui/misterdownloadmanager/issues/621))
- ✅ The build and release workflows: each job gets only the permissions it needs, and every
  action and release tool is pinned to an exact version
- ✅ A freeze on a slow disk, the data folder looked up on every tick
  ([#565](https://github.com/anefzaoui/misterdownloadmanager/issues/565))
- ✅ A `funding.json` file, the open format funds use to find projects and what they need
- ✅ The tools that send the browser extension to the stores, at their current versions
- ✅ macOS: downloads to a network volume (SMB, AFP, NFS), refused for "0 bytes free"
  ([#623](https://github.com/anefzaoui/misterdownloadmanager/issues/623))
- ✅ Helium: MDM offers it the extension on Linux and macOS, through Helium's own services, the Linux
  packages' helper files cover it, and the docs say how to add the extension in Helium
  ([#685](https://github.com/anefzaoui/misterdownloadmanager/issues/685))
- ✅ Torrents: the window no longer freezes while torrents run on a busy or slow disk: their numbers
  are read beside the window instead of waiting on the torrent engine
  ([#693](https://github.com/anefzaoui/misterdownloadmanager/issues/693))
- ✅ Sites that let only browsers through (Cloudflare's bot check, FileHippo's 412): MDM tries once
  more connecting the way a browser does, with the cookies it has for that site (part of
  [#668](https://github.com/anefzaoui/misterdownloadmanager/issues/668))
- ✅ Downloads through the browser-like connection use all their connections, each asking for its
  part, and fall back to one when a site refuses parts there; a stream's pieces behind a bot check
  are asked that way too ([#668](https://github.com/anefzaoui/misterdownloadmanager/issues/668),
  [#1289](https://github.com/anefzaoui/misterdownloadmanager/issues/1289))
- ✅ Downloads from FileHippo no longer fail at once with 412 Precondition Failed: a link the site
  refuses goes on from where the browser's own download of it had reached, then through the
  browser-like connection above
  ([#630](https://github.com/anefzaoui/misterdownloadmanager/issues/630))
- ✅ Download video: a sound-only choice (MP3, a playlist as sound) saves in the Music folder and is
  listed under Music ([#611](https://github.com/anefzaoui/misterdownloadmanager/issues/611))
- ✅ Linux and macOS: the system's gallery-dl is used when MDM has none of its own, as yt-dlp is
  ([#571](https://github.com/anefzaoui/misterdownloadmanager/issues/571))
- ✅ Linux and macOS: the system's yt-dlp is used when MDM has none of its own, and Helpers can
  switch to MDM's copy ([#452](https://github.com/anefzaoui/misterdownloadmanager/issues/452))
- ✅ The Download video button lists the video's qualities, with their sizes, in a menu on the page,
  video and audio in their own tabs: the one picked downloads at once with only its progress window,
  MDM's main window staying where it is; "Open in Mister Download Manager…" keeps the Download video
  window. A stream the page plays (HLS) is listed at once, the same video asked again within 10
  minutes opens at once, and the pick starts without looking the video up a second time
  ([#449](https://github.com/anefzaoui/misterdownloadmanager/pull/449))
- ✅ Picking a video quality already in the list (the menu over a video, the Download video window)
  follows Options' duplicate setting instead of downloading it again
  ([#449](https://github.com/anefzaoui/misterdownloadmanager/pull/449))
- ✅ The progress window's bar glides instead of moving in steps
  ([#449](https://github.com/anefzaoui/misterdownloadmanager/pull/449))
- ✅ The download speed shown is steady: an average of the last few seconds whatever the frame
  rate, its figure (and the time left) changing twice a second, together in the list, the progress
  window, the status bar and the tray's tip
  ([#1176](https://github.com/anefzaoui/misterdownloadmanager/issues/1176))
- ✅ A stream's qualities (HLS) show a size in the Download video window, estimated from their
  bitrate and the video's length
  ([#449](https://github.com/anefzaoui/misterdownloadmanager/pull/449))
- ✅ Torrents: an **Upload speed** column in the download list, beside **Transfer rate** (part of
  [#357](https://github.com/anefzaoui/misterdownloadmanager/issues/357))
- ✅ Cancel in Download File Info asks whether the downloads waiting behind it are cancelled too (a
  browser selection's arrive one by one)
  ([#394](https://github.com/anefzaoui/misterdownloadmanager/issues/394))
- ✅ The browser's video button over a video in a feed sends the post the video sits in (LinkedIn,
  Facebook, X, Instagram, Reddit, TikTok, Vimeo, Dailymotion, and a post's own link on other sites),
  else the video's own file, else it says to open the post, instead of the feed's address where
  yt-dlp found nothing ([#689](https://github.com/anefzaoui/misterdownloadmanager/issues/689));
  checked on a signed-in LinkedIn feed and Facebook's video pages, Facebook's feed posts still to
  check ([#643](https://github.com/anefzaoui/misterdownloadmanager/issues/643))
- ✅ Torrents: a pause landing while a piece is being checked no longer leaves the torrent one piece
  short until MDM restarts: the piece is downloaded again
  ([#706](https://github.com/anefzaoui/misterdownloadmanager/issues/706))
- ✅ Windows: the tray icon's menu follows MDM's look, dark or light, on Windows 10 and 11
  ([#682](https://github.com/anefzaoui/misterdownloadmanager/issues/682))
- ✅ Torrents: magnet links without trackers find peers where the DHT's first nodes don't answer:
  four public routers asked together, the IPv6 nodes kept for the next start, and the port picked
  kept from one start to the next
  ([#650](https://github.com/anefzaoui/misterdownloadmanager/issues/650))
- ✅ Windows: the window no longer stops on a stalled system (a disk held for a restore point or a
  backup, for one) to check its title bars' colours: they are set once, when a window opens or the
  theme changes; MDM's log and the RSS feeds are written beside the window too
  ([#633](https://github.com/anefzaoui/misterdownloadmanager/issues/633),
  [#583](https://github.com/anefzaoui/misterdownloadmanager/issues/583),
  [#513](https://github.com/anefzaoui/misterdownloadmanager/issues/513))
- ✅ Windows: no crash when the clipboard is read in two places at once (the clipboard monitor and
  Add URL, or a paste in a text box): every read and write of it goes one at a time
  ([#629](https://github.com/anefzaoui/misterdownloadmanager/issues/629),
  [#1103](https://github.com/anefzaoui/misterdownloadmanager/issues/1103))
- ✅ The window no longer waits on the program that owns the clipboard when it doesn't answer (Linux
  especially): a text box's right-click menu, Add URL, Add batch download, Add torrent and Download
  video read the clipboard beside the window, and fill in a copied link when it comes (follows
  [#418](https://github.com/anefzaoui/misterdownloadmanager/issues/418))
- ✅ Categories without their own folder follow General's folder (`Video`, `Music`, `Torrents` inside
  it), and Add torrent starts with the Torrents category's folder instead of flipping from General
  when the metadata arrives ([#556](https://github.com/anefzaoui/misterdownloadmanager/issues/556),
  [#550](https://github.com/anefzaoui/misterdownloadmanager/issues/550))
- ✅ Windows: dragging a file out of Download complete no longer holds up the window for as long as
  the program under the pointer takes: the drag runs beside the window
  ([#561](https://github.com/anefzaoui/misterdownloadmanager/issues/561))
- ✅ The video button keeps one corner over YouTube's thumbnail previews, within the pointer's reach
  ([#560](https://github.com/anefzaoui/misterdownloadmanager/issues/560)), and over a player whose
  settings button comes and goes as the pointer moves, checked in a real browser
  ([#899](https://github.com/anefzaoui/misterdownloadmanager/issues/899))
- ✅ Menus: moving on from an item with a submenu closes the submenu at once, and only the item under
  the pointer is lit ([#579](https://github.com/anefzaoui/misterdownloadmanager/issues/579))
- ✅ Chromium browsers: a download MDM's extension leaves to the browser keeps the file name and
  folder another extension gives it (Save In's)
  ([#578](https://github.com/anefzaoui/misterdownloadmanager/issues/578))
- ✅ Without ffmpeg, YouTube's sound alone comes from a format sent as one file, and HLS pieces are
  saved as what they are, not as an `.mp4` no player opened
  ([#569](https://github.com/anefzaoui/misterdownloadmanager/issues/569))
- ✅ A progress window with a title of 256 characters or more gets MDM's own title bar, not the
  system's above it, so its buttons answer (part of
  [#540](https://github.com/anefzaoui/misterdownloadmanager/issues/540), with the Windows 10 fix
  above)
- ✅ macOS 15 and later: Chinese text finds PingFang again, which moved into the system's font assets
  ([#541](https://github.com/anefzaoui/misterdownloadmanager/issues/541))
- ✅ The video button goes when it is switched off, or its site listed, in Options, in pages already
  open ([#532](https://github.com/anefzaoui/misterdownloadmanager/issues/532)); and comes in a page
  opened while the browser couldn't reach MDM once it can, without a reload
  ([#474](https://github.com/anefzaoui/misterdownloadmanager/issues/474))
- ✅ macOS: Persian and other Arabic-script file names show their letters in the download list, not
  boxes ([#533](https://github.com/anefzaoui/misterdownloadmanager/issues/533))
- ✅ A server's refusal page is no longer saved under the document's name and shown as complete,
  where the browser had the file: a web page where a file was expected stops with an error (part of
  [#533](https://github.com/anefzaoui/misterdownloadmanager/issues/533))
- ✅ macOS: Download File Info opens in front of the browser that asked for it
  ([#484](https://github.com/anefzaoui/misterdownloadmanager/issues/484))
- ✅ macOS: MDM's icon shows in the menu bar, so closing the window keeps MDM running there instead
  of quitting ([#529](https://github.com/anefzaoui/misterdownloadmanager/issues/529))
- ✅ The video button fades after a click on the player, in fullscreen too, and stays while you use
  the keyboard; its browser test runs before the one that turns the button off
  ([#480](https://github.com/anefzaoui/misterdownloadmanager/issues/480))
- ✅ The video button stays over a hover-to-play preview (YouTube's thumbnails) long enough to be
  clicked ([#503](https://github.com/anefzaoui/misterdownloadmanager/issues/503))
- ✅ Add URL's **Use authorization** shows the login Options → Sites Logins has for the address, the
  one the download uses ([#511](https://github.com/anefzaoui/misterdownloadmanager/issues/511))
- ✅ Windows: closing the window hides MDM in the notification area again, its icon among the hidden
  ones behind ^ too; the window stays in the taskbar only when Windows says it has no icon of MDM's
  ([#750](https://github.com/anefzaoui/misterdownloadmanager/issues/750),
  [#718](https://github.com/anefzaoui/misterdownloadmanager/issues/718),
  [#754](https://github.com/anefzaoui/misterdownloadmanager/issues/754),
  [#545](https://github.com/anefzaoui/misterdownloadmanager/issues/545),
  [#534](https://github.com/anefzaoui/misterdownloadmanager/issues/534),
  [#527](https://github.com/anefzaoui/misterdownloadmanager/issues/527),
  [#590](https://github.com/anefzaoui/misterdownloadmanager/issues/590),
  [#779](https://github.com/anefzaoui/misterdownloadmanager/issues/779))
- ✅ The window no longer freezes while Explorer is slow to answer about MDM's notification-area
  icon: it is asked beside the window
  ([#747](https://github.com/anefzaoui/misterdownloadmanager/issues/747))
- ✅ Two tests that failed now and then on busy CI runners: the slow-flush one measures the transfer,
  not the runner's disk, and the crash-and-resume one found a real delay in saving a new download's
  progress, fixed too ([#518](https://github.com/anefzaoui/misterdownloadmanager/issues/518))
- ✅ The window no longer waits while Windows takes seconds to look up the time zone for the queues'
  schedules ([#536](https://github.com/anefzaoui/misterdownloadmanager/issues/536))
- ✅ Video look-ups no longer fail with "WinError 448 ... untrusted mount point" when a folder on
  PATH goes through a junction Windows won't let yt-dlp cross
  ([#508](https://github.com/anefzaoui/misterdownloadmanager/issues/508))
- ✅ Dailymotion's embedded player from the browser's button: looked up without the embedding site's
  Referer, which Dailymotion refused
  ([#359](https://github.com/anefzaoui/misterdownloadmanager/issues/359))
- ✅ A magnet link handed over while Add torrent is already open waits its turn instead of being
  dropped ([#507](https://github.com/anefzaoui/misterdownloadmanager/issues/507))
- ✅ No freeze when a video finishes: its file is marked as downloaded from the web beside the window
  ([#510](https://github.com/anefzaoui/misterdownloadmanager/issues/510))
- ✅ Linux: Dark Mode support on window managers such as Hyprland and sway, which often have no
  desktop portal to say dark or light: GNOME's setting is read
  ([#324](https://github.com/anefzaoui/misterdownloadmanager/issues/324))
- ✅ No bright white line under the menu bar in dark mode
  ([#379](https://github.com/anefzaoui/misterdownloadmanager/issues/379))
- ✅ The Download video window remembers the folder you saved into
  ([#475](https://github.com/anefzaoui/misterdownloadmanager/issues/475))
- ✅ Download video says in plain words when a site's bot check (Cloudflare) refuses yt-dlp (part of
  [#317](https://github.com/anefzaoui/misterdownloadmanager/issues/317))
- ✅ Google Colab: only its tokenized runtime-proxy transfers stay with the browser; ordinary
  notebook links can still be taken
  ([#603](https://github.com/anefzaoui/misterdownloadmanager/issues/603))
- ✅ A gofile share link says it is a web page to open in the browser, instead of saving the page as
  the file ([#346](https://github.com/anefzaoui/misterdownloadmanager/issues/346))
- ✅ Export and Import for the sites the download button stays off (part of
  [#424](https://github.com/anefzaoui/misterdownloadmanager/issues/424))
- ✅ Videos in the progress window: no percentage when the size isn't known, and the real file size
  at the end
- ✅ X (Twitter) videos from the media list are named after the tab instead of a random file name
  (part of [#493](https://github.com/anefzaoui/misterdownloadmanager/issues/493))
- ✅ Windows: **Open folder** uses the file manager set up to open folders, not always Explorer
  ([#446](https://github.com/anefzaoui/misterdownloadmanager/issues/446))
- ✅ Linux: downloads to an NTFS drive (`ntfs3`) start at once instead of filling the file with
  zeros first, which held the download at 0% and slowed the whole computer
- ✅ Linux on Wayland (XWayland) with NVIDIA's driver: typing and clicking no longer lag by 1 to 2
  seconds; MDM spaces its frames itself instead of waiting for the screen's refresh
- ✅ Linux on X11 and XWayland with Mesa's drivers (Intel, AMD, llvmpipe): one window at most waits
  for the screen's refresh, none under XWayland, as `vblank_mode=0` did for the lag when moving the
  main window and clicking in the download dialog (for
  [#347](https://github.com/anefzaoui/misterdownloadmanager/issues/347), waiting for the reporter)
  seconds; MDM spaces its frames itself instead of waiting for the screen's refresh (other drivers
  keep the refresh)
- ✅ The computer stays awake while anything downloads, not only during a queue that woke it, and
  sleeps as its settings say once the downloads are done
  ([#939](https://github.com/anefzaoui/misterdownloadmanager/issues/939))
- ✅ macOS: a menu whose item opens a file dialog (View → Theme → Add a theme file, Tasks → Open
  torrent file) closes first, instead of floating on top of every window until MDM quits
  ([#615](https://github.com/anefzaoui/misterdownloadmanager/issues/615))
- ✅ Download video works behind an antivirus or a proxy that reads HTTPS: yt-dlp is asked again
  with the system's certificates when it refuses a site's
  ([#955](https://github.com/anefzaoui/misterdownloadmanager/issues/955))
- ✅ Linux: MDM pins to the taskbar and the dock: its desktop entry names its windows' class, and
  every window of MDM carries it ([#940](https://github.com/anefzaoui/misterdownloadmanager/issues/940))
- ✅ A menu's underlined letter runs its item without typing that letter into the text box under
  the menu, the arrows move in the menu only, and Alt with a dialog's letter types nothing either;
  letters of other scripts (Arabic, Russian) choose their menu item
- ✅ The command line's `video-get` takes the best quality as the Download video window does (a
  Bilibili video, whose pictures and sounds come apart, failed), `--quality` takes a height or the
  nearest below it, and an error with no format to download lists the ones the page offers
  ([#1240](https://github.com/anefzaoui/misterdownloadmanager/issues/1240))
- ✅ Create new queue (the Categories tree's menu, Add to queue) shows its name box in front of the
  Scheduler, and proposes a name no queue has
  ([#1237](https://github.com/anefzaoui/misterdownloadmanager/issues/1237))
- ✅ Torrents: peers at addresses that can't be on the internet are left alone, whoever names
  them (a tracker, the DHT, another peer), as trackers there already were
- ✅ AI assistants (MCP) can stop the main download queue too (`stop_queue`), and the automation
  API's queue actions take the main download queue when no queue is named (thanks to
  [@abdul-karim-mia](https://github.com/abdul-karim-mia))
- ✅ The command line starts and stops a queue of the running app: `start-queue` and
  `stop-queue`, the main download queue unless one is named (thanks to
  [@abdul-karim-mia](https://github.com/abdul-karim-mia))
- ✅ The command line adds downloads to the running app: `add <url>`, with folder, file name, category, queue, checksum, referer and silent flags (thanks to
  [@abdul-karim-mia](https://github.com/abdul-karim-mia))
- ✅ AI assistants (MCP) can choose a download queue and a category when adding downloads, and start or stop a queue by name (thanks to
  [@abdul-karim-mia](https://github.com/abdul-karim-mia))
- ✅ AI assistants (MCP) can filter downloads by category or queue in `list_downloads`, and see the app's available queues and categories (thanks to
  [@abdul-karim-mia](https://github.com/abdul-karim-mia))
- ✅ AI assistants (MCP) can inspect a download by id and see free disk space in `list_downloads` (thanks to
  [@abdul-karim-mia](https://github.com/abdul-karim-mia))
- ✅ The command line lists and filters downloads: `status` (or `list`) takes `--id`, `--status`, `--category`, `--queue`, `--search` and `--limit` flags (thanks to
- ✅ The command line prints pairing details as JSON: `pair --json` outputs code, valid minutes and URLs for scripts (thanks to
  [@abdul-karim-mia](https://github.com/abdul-karim-mia))
- ✅ Linux (X11): Minimize to the notification area takes the window out of the taskbar's window
  list, as closing it does ([#1255](https://github.com/anefzaoui/misterdownloadmanager/issues/1255))
- ✅ A failed download says why in MDM's log too, with the site's name and no tokens
  ([#1262](https://github.com/anefzaoui/misterdownloadmanager/issues/1262))
- ✅ Download panels: the hint under the sites list has its own row instead of running under
  Export...
- ✅ Linux: pausing or cancelling a video (or a gallery) ends all of yt-dlp's processes, so it
  stops downloading; MDM's exit and the command line's Ctrl+C end them too
  ([#1301](https://github.com/anefzaoui/misterdownloadmanager/issues/1301); thanks to
  [@arifrhm](https://github.com/arifrhm), who found the same cause)
- ✅ Internet Archive: a download that needs your account keeps the browser's sign-in on its way to
  archive.org's storage servers (thanks to [@arifrhm](https://github.com/arifrhm))
  ([#1298](https://github.com/anefzaoui/misterdownloadmanager/issues/1298))
- ✅ Persian: a native speaker's review of the first 23 texts (thanks to
  [@RedemptionMHA](https://github.com/RedemptionMHA))
- ✅ Linux: the notification area's icon has its name (KDE showed none)
  ([#1305](https://github.com/anefzaoui/misterdownloadmanager/issues/1305))
- ✅ Pause and Resume answer at once on a slow disk: the download stops right away and MDM
  finishes writing in the background; a resume no longer waits for the disk before its first
  request ([#1328](https://github.com/anefzaoui/misterdownloadmanager/issues/1328))
- ✅ Windows: Open folder finds a file manager that names its own default verb for folders
  (One Commander) (thanks to [@haohaaaa](https://github.com/haohaaaa))
  ([#1335](https://github.com/anefzaoui/misterdownloadmanager/issues/1335))
- ✅ About's links include **Contributors**, which opens the website's page that thanks everyone who
  sent a pull request, opened an issue or joined a conversation (built from GitHub at every build,
  so it never goes stale): the smaller form of
  [#1253](https://github.com/anefzaoui/misterdownloadmanager/pull/1253), whose author
  [@9M2PJU](https://github.com/9M2PJU) handed it over
- ✅ Windows: a dialog that opens while you are in another program (the duplicate download
  question the browser asks, Download File Info) no longer brings the main window up behind it;
  it has its own taskbar button
- ✅ The same video picked from the quality menu in a second tab of its page is a duplicate, even
  when its stream's address is signed for each load (it was downloaded again without a word)
- ✅ **Refresh download address** opens the file page a file host's Download button was on (a
  premium Rapidgator file), not the site's home page, in Chromium browsers too: the page is
  noted as the download's load starts, before the browser closes the tab the button opened or
  the tab moves on; a site's address alone is the download's page only when nothing names a page
  on it ([#607](https://github.com/anefzaoui/misterdownloadmanager/issues/607))
- ✅ A download whose link expired goes on: Rapidgator's file page (the tab the download came
  from) is asked with the browser's sign-in for a fresh link, at most once per expiry and never
  again at once for a link it then refused; a Rapidgator download stopped with an expired or
  refused link, or any download after **Refresh download address**, takes the browser's own next
  hand-off of the same file (same site, same name, same size when known; never a private
  window's, nor an explicit, command-line or script add) as its new address and goes on, instead
  of a second row starting over; a fresh link on another of Rapidgator's own servers, whose copy
  has its own ETag and date, goes on there instead of failing at 100% with "the file changed on
  the server" (any other site's same-size copy still stops)
- ✅ Linux (XWayland on Hyprland): a click in MDM's window gives it the keyboard again where
  Hyprland had left it with another X11 program's window, or with none after the window came back
  from the notification area; a box in File Properties took the caret but no typing or Ctrl+V
- ✅ Linux: started without a Wayland display in a Wayland session (an XWayland-only start),
  MDM knows its windows are XWayland's; Download File Info and Download complete answered a click
  a second or two late while the main window was out of sight (on another workspace)
- ✅ Telegram Web's downloads stay with the browser, which makes them in the page
  ([#1345](https://github.com/anefzaoui/misterdownloadmanager/issues/1345))
- ✅ A video in another site's player embedded in a page is named by the page's title, not the
  player's own ("Watch file.mp4 - VOE")
  ([#1351](https://github.com/anefzaoui/misterdownloadmanager/issues/1351))
- ✅ Torrents start again after a crash or a power cut left their saved list empty: the list is
  kept aside and a new one begins, the download list's torrents are added again from MDM's own
  copies and find their pieces on disk; MDM's own rewrite of the list reaches the disk before it
  replaces the old one ([#1357](https://github.com/anefzaoui/misterdownloadmanager/issues/1357))
- ✅ **Options → General → Keep private-window downloads in the list** (off by default): a private
  window's downloads are saved like any other and survive MDM closing or restarting, still apart
  from normal windows' downloads (a saved private-window mark; their cookies never saved); a
  private window's video never falls back to Cookies for video sites, after a restart either
- ✅ CodeQL reads the Rust, the extension's JavaScript and the workflows on every pull request,
  every change to main and once a week; its results are in the Security tab
- ✅ CodeQL's first findings went through one by one; it leaves out the local test server and the
  test tooling, which never ship, and still reads everything that does
- ✅ The log's test of a flush writing what was dropped holds on a slow machine too (it failed now
  and then on macOS)
- ✅ Dependabot proposes updates to the workflows' actions, the Rust crates, the store tools and
  the website build's Python package, once a week, grouped into one pull request per kind
- ✅ The lab VM scripts clear the temporary files an interrupted run left behind
- ✅ The website's translation check takes a list of the app's names (themes) as it is, so those
  lines are no longer left in English
- ✅ The browser extension tells a stream's manifest by a simpler reading of its start
- ✅ The Docker image's base images, the website build's Python package and the lab VM's Rust
  installer are pinned to exact versions and checked by their hashes; the MCP server's library
  (rmcp) is updated to 3.9.0
- ✅ Dependency updates leave the font parser to move with the drawing library
- ✅ Website: Linux is called supported everywhere (no longer "experimental" or "to try"; macOS
  still is); the download page's **Get it safely** lists the official places to get MDM, how to
  check a download's checksum and signature, and why Windows or an antivirus may warn; the home
  page shows the browser stores' and GitHub's figures and what reviewers wrote, quoted exactly;
  a new [Press and mentions](https://misterdownloadmanager.com/press.html) page lists articles and
  videos by language, the fair criticism of the benchmark, and where MDM is available (official
  first, community packages labelled); the benchmark's figures say they come from a test server
  on the same PC
- ✅ CI's Linux libraries step retries a slow package mirror instead of timing out
- ✅ Website: a [code signing policy](https://misterdownloadmanager.com/code-signing-policy.html)
  page (which Windows files are signed, who reviews and approves, privacy), linked from every
  page's footer, the download page and the README; the privacy policy says what the crash
  report window asks GitHub
- ✅ The extension's translations match its catalogs again — the shipped Arabic, Bangla, Malay
  and Chinese texts take the termbase corrections (المستعرض, সব, বিস্তারিত, baris gilir) —
  and the catalogs carry the extension's **Forget** row again, so regenerating the locale
  files keeps the popup's forget-a-server button in every language (thanks to
  [@Seyamalam](https://github.com/Seyamalam))
- ✅ CI keeps the extension's locale files equal to the catalogs' output: a text added to
  English's messages.json without a translate.py run, or a catalog fix left unregenerated,
  fails the browser-extension job (thanks to [@Seyamalam](https://github.com/Seyamalam))
- ✅ How a change is verified, written down: a `verify` skill (`.claude/skills/verify`) for
  running a change for real before it's merged, and REVIEW.md, what to check by what a pull
  request touches; every pull request says what was verified and what wasn't
- ✅ Screen readers name the buttons a glyph or a shared word hid: "+" is "Add category",
  "..." is "Browse", Options' two "Edit..." buttons take their captions ("Customize MDM
  menu items in context menu of browsers", "Customize MDM Download panels in browsers"),
  and the remembered folder field is "Category folder", not the checkbox's caption
  ([#1407](https://github.com/anefzaoui/misterdownloadmanager/issues/1407); thanks to
  [@Seyamalam](https://github.com/Seyamalam))
- ✅ Screen readers name more of **Options → Connection**: the exceptions list is
  "Exceptions" instead of unnamed, and each of the two **Download limits** boxes says what
  it counts ("Download limit: MBytes", "Download limit: every how many hours") instead of
  only its number; the same two boxes on the **Scheduler**'s limits page are named too
  ([#1407](https://github.com/anefzaoui/misterdownloadmanager/issues/1407); thanks to
  [@Seyamalam](https://github.com/Seyamalam))
- ✅ Screen readers read the **Get ready for video downloads** window: its headline, each
  helper's name and description, the status line under the bar and the advice under it
  were painted text a reader never saw (the same banner wherever it heads a window); the
  window now opens with the focus on its default button, **Yes, download them
  (recommended)**, instead of on the frame, and **Close** takes it when Yes goes away
  ([#1407](https://github.com/anefzaoui/misterdownloadmanager/issues/1407); thanks to
  [@Seyamalam](https://github.com/Seyamalam))
- ✅ Tests: the video checks of an address that expires while downloading (a stream's, and DASH
  pieces') no longer fail now and then on a busy computer: the test server times such a link from
  its first request, not from the look-up that gave it, so a slow start can't kill it before the
  download has asked for it
- ✅ Linux and macOS: **Launch Mister Download Manager on startup** is ticked only when the startup
  entry is on and starts this copy; an entry naming a program that's gone (moved, or replaced by an
  upgrade while MDM ran) is written again as MDM starts
  ([#1484](https://github.com/anefzaoui/misterdownloadmanager/issues/1484))
- ✅ No more **The window stopped responding** report while MDM's window is minimized to the
  taskbar and its thread waits on the computer: as in the notification area, logged, and reported
  only if it lasts past a minute, if MDM's thread is busy, or if MDM is asked for something
  meanwhile and doesn't answer ([#1481](https://github.com/anefzaoui/misterdownloadmanager/issues/1481))
- ✅ LuluStream: a video's link (its watch page or player, on `lulustream.com`, `luluvdo.com` or
  `lplaygo.com`) downloads the video, in the best quality its download page lists; a download
  taken from the browser whose link expired asks the video's page for a fresh one

## Being worked on

These ship in a later version, when they're ready.

### Being built

- 🔄 **Add torrent**: choose whether a multi-file torrent gets an outer folder, with shared-destination ownership and deletion safeguards ([discussion #965](https://github.com/anefzaoui/misterdownloadmanager/discussions/965), [PR #1121](https://github.com/anefzaoui/misterdownloadmanager/pull/1121)): in review
- 🔄 **Add torrent**: a **Rename** row (main name and extension) to rename a single-file torrent or a multi-file torrent's folder before the download starts ([#517](https://github.com/anefzaoui/misterdownloadmanager/issues/517)): in review
- 🔄 Linux, what's left ([#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164)):
  screen readers (Orca) reading MDM's windows; browsers installed as a Flatpak (Firefox from
  Flathub may reach MDM through the desktop, as the Snap does: to check); native Wayland windows
  placing their menus, which needs popup windows the window library doesn't have yet, so
  XWayland stays the default unless X11's windows would be scaled up (#624); Vulkan as the
  default renderer (below)
- 🔄 The Safari extension (in 0.5.2): still to check in Safari itself, signed with a Developer ID

### Bugs being looked into

- 🔍 The system's automatic proxy settings beyond Windows' setup script: Windows'
  "Automatically detect settings" (WPAD), and the automatic configuration scripts of Linux
  desktops and macOS, aren't read yet
  ([#1448](https://github.com/anefzaoui/misterdownloadmanager/issues/1448))
- 🔍 Linux on native Wayland: while the main window is out of sight (on another workspace), the
  other windows (Download File Info, Download complete, the dialogs) don't open or don't answer
  until it is shown again, as the compositor doesn't let a window it isn't showing redraw and
  every dialog is drawn with the main window; the fix is in MDM's copy of the window library
  (eframe), to come with it
- 🔍 A look-up that waits 20 s on a video server the network doesn't answer
  ([#1053](https://github.com/anefzaoui/misterdownloadmanager/issues/1053))
- 🔍 The main window's own looks at the disk on a drive slow to answer: starting or resuming a
  download that was already under way, a rename or move when one finishes, and links added
  without Download File Info still wait for it (follows
  [#743](https://github.com/anefzaoui/misterdownloadmanager/issues/743))
- 🔍 A site's bot check refusing MDM's request (testfile.org, a Cloudflare challenge): the
  message now says so ([#376](https://github.com/anefzaoui/misterdownloadmanager/issues/376)), and
  Alt+click on the link leaves it to the browser, which passes the check
  ([#1494](https://github.com/anefzaoui/misterdownloadmanager/issues/1494)); a way to hand back a
  download that wasn't started from a link (a pasted address) is still to decide
- 🔍 The browser-like connection's handshake is Chrome 149's, the newest its library has
  released; Chrome 150 and later add signature algorithms and an extension to theirs, which come
  with the library's next release
  ([#1494](https://github.com/anefzaoui/misterdownloadmanager/issues/1494))
- 🔍 The window not answering for up to half a minute just after the computer wakes, while MDM
  waits in the notification area ([#574](https://github.com/anefzaoui/misterdownloadmanager/issues/574),
  [#613](https://github.com/anefzaoui/misterdownloadmanager/issues/613),
  [#304](https://github.com/anefzaoui/misterdownloadmanager/issues/304)): no longer reported as
  a freeze; what it waits on is to be seen in a test build's log

## Planned

### Look and performance

- 📋 Vulkan the default on Linux and Metal on macOS, once real graphics cards have tried them (AMD,
  Intel and NVIDIA, X11 and Wayland, older cards without Vulkan)
  ([#206](https://github.com/anefzaoui/misterdownloadmanager/issues/206))
- 📋 Benchmarks against other download managers
  ([#71](https://github.com/anefzaoui/misterdownloadmanager/issues/71))
