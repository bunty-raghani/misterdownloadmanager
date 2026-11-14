# Torrents and your security

[← Back to docs index](README.md)

A torrent comes from other people's computers, not from one website. That changes what you can trust and what others can see. This page says plainly what MDM does and what it doesn't. The switches it mentions are in **Tasks → Torrent settings...**.

## What other people can see

- **Your IP address.** Everyone sharing the same torrent sees it, as do its trackers and the DHT (the network peers use to find each other). That's how BitTorrent works, and no setting in MDM changes it. To hide your address, use a VPN on your whole computer.
- **What you download, on your local network.** MDM tells other computers on your network which torrents it has, so they can share faster. On public Wi-Fi, turn off **Find peers on the local network**.
- **That you use MDM.** Peers see the name "MisterDownloadManager" and its major version, not the exact build.

MDM's torrents don't use **encryption** (sometimes called "protocol encryption"). That kind of encryption only hides the traffic from simple filters. It never hides who you are, so MDM doesn't claim to offer it. Some peers that require it won't connect.

## What reaches your computer

MDM starts its torrent engine the first time a torrent needs it. With **Stop the torrent engine when no torrent is running** on (the default), it stops again a minute after no torrent is running or sharing. While it runs, other peers can connect to MDM, and MDM asks your router to forward its port (UPnP). Windows may ask whether to allow MDM through the firewall: allow it on private networks only.

This is the only part of MDM that waits for connections from outside. Downloads, MDM's DNS choice (Options → Connection → DNS), its updates and its helpers only connect out, so they never need a firewall exception; the browser extension and the automation API reach MDM on this computer only (127.0.0.1).

In Torrent settings you can turn off **Accept incoming connections** and **Map the port on the router (UPnP)**, or the DHT. Downloads still work, sometimes more slowly. With the **Port** box empty, MDM picks a port the first time and keeps it (another one if a program has taken it since), so firewall and router rules keep working; a port typed there is always the one used. These changes apply the next time the engine starts, so when no torrent is running.

The torrent engine is written in Rust, which rules out many memory bugs, and MDM checks every piece against the torrent's fingerprint, so a peer can't slip in altered data. MDM keeps the engine up to date with the fixes its authors publish.

## Trackers

A torrent names the trackers MDM asks for peers. MDM doesn't contact a tracker on your own computer or your local network (`127.0.0.1`, `192.168.…`, `localhost` and the like), so a torrent can't make MDM send requests into the network it sits in. **Add trackers...** follows the same rule: it refuses such an address and tells you why, rather than keeping one that would never be asked.

A `.torrent` file fetched from a web address goes through the same checks as one opened from disk, and MDM reads at most 10 MB of it.

## Blocking addresses

**Block the addresses in this list** takes the address of a public IP blocklist, in the P2P (PeerGuardian) or eMule `.dat` format, plain or gzipped. MDM downloads it with its own downloader (your Proxy / Socks settings apply), checks it, and fetches it again every 7 days. Peers in it are refused. A list MDM can't read is left out, and the last good one stays. The engine reads the list when it starts.

## Files from a torrent

- MDM refuses a torrent with file names your system can't write safely, says which name, and writes nothing.
- MDM never writes into a file that's already there without asking.
- Files from torrents are marked as **downloaded from the internet**, like a browser's downloads. Windows SmartScreen checks a program before it first runs, and Office opens documents in Protected View. On a Mac, Gatekeeper checks apps. **Options → Save to → Mark downloaded files as coming from the internet** turns it off, and so does Windows' policy **Do not preserve zone information in file attachments**.
- A torrent can contain anything, and anyone can make one. Only open programs from people you trust, and scan them if in doubt.

## Proxies

MDM's Proxy / Socks settings are for web and FTP downloads. Torrents can use a SOCKS5 one too: set a `socks5://` proxy in **Options → Proxy / Socks** and turn on **Use the proxy** in Torrent settings. The proxy covers connections to peers and web trackers only, so with it MDM turns off the DHT, local peer discovery, incoming connections and UDP trackers; it opens no UDP socket toward the network at all. If the proxy is gone from Proxy / Socks while that switch is on, torrents don't start rather than go around it.

To route *everything* through another network, use a VPN for the whole computer.

## A network adapter

**Options → Proxy / Socks → Network adapter** holds MDM's connections to one adapter, usually a VPN's ([Options](options.md#proxy--socks)). The torrent engine binds every socket to it: peers (TCP, and through a SOCKS proxy), HTTP and UDP trackers, the DHT and the listening port. On Linux and macOS it binds the adapter itself. On Windows it binds the adapter's address (its IPv4 one when it has one), and Windows sends a packet only through the adapter that has its source address. UPnP and local peer discovery are turned off then, because they talk to the local network rather than through the adapter.

It works as a kill switch. When the adapter disconnects, running torrents stop with "… is not connected; it goes on when it is back", and they start again through the adapter when it is. Until MDM notices (within a second), the engine's sockets are still bound to an address that is gone, so nothing can leave another way. A torrent added while the adapter isn't connected doesn't start, and says why. Tracker names are still looked up by the system's DNS.

## Reporting a problem

Found a security issue? Please report it privately on GitHub (**Security → Report a vulnerability**, see [SECURITY.md](https://github.com/anefzaoui/misterdownloadmanager/blob/main/SECURITY.md)) rather than in a public issue.

## Next steps

- [Torrents →](torrents.md)
- [Back to the docs index →](README.md)
