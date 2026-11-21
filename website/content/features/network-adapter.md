---
name: Network adapter
title: Bind downloads to a VPN adapter, with a kill switch
headline: Through your VPN, or not at all.
line: Every connection MDM opens goes out through the adapter you pick, and stops when it disconnects.
lede: Pick a network adapter, usually your VPN's, and every connection MDM opens goes through it. If the VPN drops, nothing slips out through your normal connection: downloads stop, and start again when it's back.
description: Hold Mister Download Manager's downloads, torrents and videos to one network adapter, usually a VPN's, with a kill switch when it disconnects.
group: network
order: 210
since: 0.5
shot: images/help/options-proxy.webp
shot_alt: Options on its Proxy / Socks page, with Network adapter near the bottom set to Any (the system chooses)
where: Options → Proxy / Socks → Network adapter
help: options.md#proxy--socks
---

## A kill switch built in

**Any (the system chooses)** is the default. Choose an adapter instead, and MDM sends its
connections through that one only. When it disconnects, or the VPN is switched off, nothing goes
out another way. Running downloads stop with "… is not connected" and start again by themselves
when the adapter is back. The line under the list shows the adapter's address, or that it isn't
connected.

## Everything MDM opens

It covers downloads and FTP, torrents (peers, trackers, the DHT and the listening port), video
pages and image galleries, the DNS you chose, MDM's own downloads and its update check. The
command line's `get`, `video-get` and `gallery-get` take `--adapter`, with the adapter's name,
such as `Ethernet` or `wg0`, or one of its addresses.

yt-dlp, gallery-dl and ffmpeg can't be held to an adapter themselves. While one is chosen, MDM
gives them a proxy of its own on this computer, with a password only they get, and opens their
connections through the adapter as it opens its own. Video formats that can't go through a proxy
(RTMP, RTSP, MMS, FTP) are refused rather than sent around it.

## How it holds

On Windows, MDM binds the adapter's own address, and Windows then sends those packets only
through that adapter. Linux binds the device itself (Linux 5.7 or newer), and macOS binds the
interface. An address of a kind the adapter doesn't have, such as IPv6 on a VPN that has only
IPv4, is refused, not sent around it.

## Keep your lookups inside too

The system's DNS looks names up its own way, and Windows may ask over every adapter. To keep the
names inside the VPN as well, choose a DNS in **Options → Connection → DNS**: MDM then asks it
through the adapter. See [DNS and proxies](/features/dns-proxies.html).

## What it doesn't hold yet

`ftps://` addresses (implicit TLS) don't start while an adapter is chosen; `ftp://` with TLS is
fine. Connections to this computer itself aren't bound, so a proxy running on this computer
decides where your downloads go from there. On macOS, a VPN that reconnects may come back as a
new interface (`utun5` instead of `utun4`): pick it again.
