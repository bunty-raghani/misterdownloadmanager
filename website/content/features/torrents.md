---
name: Torrents
title: Download torrents and magnet links
headline: Torrents and magnet links, in the list you already use.
line: Magnet links and .torrent files beside your other downloads, queued and scheduled like any of them.
lede: MDM downloads torrents itself, with no second program to install. Pick the files you want, watch the peers and the pieces come in, and keep sharing if you like.
description: Mister Download Manager downloads torrents and magnet links: choose the files, see peers, trackers and pieces, and queue them with your other downloads.
group: downloading
order: 50
since: 0.1.0
shot: /assets/screens/torrent-light.webp
shot_alt: A torrent's progress window on its Pieces tab: a square for each piece, filled once it's here
where: Tasks → Add magnet link..., or the toolbar's Add Torrent
help: torrents.md
---

## One list for everything

Click **Add Torrent** on the toolbar to open a `.torrent` file, or use the arrow next to it for
**Add magnet link...**. A magnet link pasted into **Add URL** works too. Torrents land in the same
list as your other downloads, marked **Torrent** in the **Source** column, and go into queues and
the Scheduler like them.

The torrent engine is written in Rust, and every piece is checked against the torrent's
fingerprint, so a peer can't slip in altered data.

## Only the files you want

![Add torrent with a torrent's files as a folder tree, the folder they go to, and Download Now](images/help/torrent-add.webp)
*Untick what you don't want, then Download Now.*

**Add torrent** shows the torrent's files as a folder tree. Untick what you don't need;
**Selected** shows how many files and how much data are left. Torrents have their own category
and folder, `Downloads\Torrents` by default.

Double-click a torrent for its progress window. Its tabs show each file, the peers you get it
from, the trackers, a map of the pieces, and the share ratio.

## Sharing is your choice

A finished torrent stops, like any download. Turn on **Keep sharing torrents after they finish**
in **Tasks → Torrent settings...** to go on uploading to others.

Plainly: everyone sharing the same torrent sees your IP address. That is how BitTorrent works,
and no setting changes it. To hide it, use a VPN for the whole computer.
[Torrents and your security](torrent-security.md) says what others can see.

## More control in version 0.5.0

<!--@coming 0.5-->

- **Download in order**, to watch a video while it downloads, with each file's first and last
  pieces early. Other torrents fetch the rarest pieces first.
- **Torrent commands** in the right-click menu: **Recheck files**, **Reannounce**, **Add
  trackers...** and **Copy magnet link**. Files can be ticked and unticked while it runs.
- **Limits**: stop sharing at a share ratio or after some minutes, and cap what torrents send,
  all together or one by one.
- **The network is yours to choose**: incoming connections, UPnP, a fixed port, the DHT, local
  peers, a SOCKS5 proxy, an IP blocklist, and one network adapter (a VPN's) with a kill switch.
- **Safer files**: MDM asks before writing into files already in the folder, refuses names the
  system can't write safely, and marks the files as downloaded from the internet.
- [RSS feeds](/features/rss.html) that add new torrents by rule, and torrents sent to a
  [debrid account](/features/debrid.html) instead of to peers.
