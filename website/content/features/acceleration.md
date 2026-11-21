---
name: Acceleration and resume
title: Download acceleration and crash-safe resume
headline: Every connection at once. Picked up where it stopped.
line: A file split over up to 32 connections, re-split until the last byte, and resumed after any crash.
lede: MDM splits a file into parts and downloads them side by side, then keeps every connection busy until the last byte. If the line drops or the computer restarts, the download carries on from where it was.
description: How Mister Download Manager speeds up downloads: up to 32 connections per file, dynamic re-splitting, per-server limits it learns, and crash-safe resume.
group: downloading
order: 10
since: 0.1.0
shot: /assets/screens/progress-light.webp
shot_alt: The progress window: one file over eight connections, each one's share of the file drawn as it fills
where: Options → Connection
help: connections-and-speed.md
---

## No connection sits idle

MDM opens 8 connections by default, and you can set anything from 1 to 32 in **Options →
Connection**. When a connection finishes its part, it takes over half of the largest part still
left, so the work is shared out again and again until the file is done. A connection that falls
far behind the others is dropped and opened again.

The progress window draws it as it happens: a bar of the whole file with each connection's
start and progress, and a table with what every connection is doing right now.

## Measured, not claimed

On the same PC, downloading from a test server on that PC, MDM fetched a 1 GiB file in 1.0 s
against IDM 6.42's 5.5 s, and a 256 KiB file in 0.03 s against 3.1 s: the medians of alternating
runs. It is a loopback test: it times the programs, not an internet connection. The
[benchmark page](/benchmark.html) has every run, the method and the raw data.

More connections help only where a server sends each one at a limited speed. Most servers allow
a few connections per address, and MDM notices when one says no: it stops asking that server
for more and lists what it learned under **Exceptions** in **Options → Connection**. You can set
a number for a server yourself there too.

## More than 32, if you want them

<!--@coming 0.5-->

**Allow more than 32 connections** in **Options → Connection** raises the limit to 128 per
download, for server exceptions too. The command line's `-c` takes 1 to 128.

## Resume that survives a crash

MDM saves how far each part has got as it goes. A download picks up where it stopped after you
pause it, after the connection drops, or after Windows or MDM itself crashes. A connection that
fails is retried on its own, and the table shows **Retrying** with the reason.

Before it continues, MDM checks that the file on the server is still the same one, so a changed
file never gets stitched onto an old half. When a server can't send a file in parts, the
progress window says **Resume capability: No** before you rely on it.

## Straight to disk

Each connection writes directly into the file's place on disk, in a part file next to the
final one. There is no temporary folder and no joining step at the end: when the last part
arrives, MDM checks the size and renames the file.

[Resume a broken download](guide-resume-broken-download.md) shows what to do when a link has
expired.
