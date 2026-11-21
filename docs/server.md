# Run MDM on a server

Run Mister Download Manager on a home server or a NAS (Docker, Unraid, any Linux machine) without
a window, and send it downloads from your phone or your browser: the server downloads them into
its own folders, such as a network share.

It is the same MDM as on a desktop: its list, categories, queues, Scheduler, speed limits and
torrents. You control it with the [remote control's page](remote-control.md) (from any phone or
computer on your network), the browser extension pointed at it, or its command line.

> **New:** the server mode is new in this version. Please
> [tell us how it goes](https://github.com/anefzaoui/misterdownloadmanager/issues).

## With Docker

From the release after 0.5.2, pull the published image from GitHub Container Registry (x86-64
only for now):

```sh
docker pull ghcr.io/anefzaoui/misterdownloadmanager:<version>
# or the latest release:
docker pull ghcr.io/anefzaoui/misterdownloadmanager:latest
```

Or build the image yourself from MDM's source:

```sh
git clone https://github.com/anefzaoui/misterdownloadmanager
cd misterdownloadmanager
docker build -t misterdownloadmanager .
```

The first build is long (up to an hour on a small server: the whole program is optimised as
one). The image is small, about 175 MB: Debian's slim base, MDM's two programs and the
certificates downloads are checked against. It needs no desktop libraries: MDM loads those only
to open a window, which a server never does.

Then run it, with a folder for its settings and one for the downloads. Make both folders first,
as the user MDM will run as: a folder Docker makes for you belongs to `root`, and MDM can't use it.

```sh
mkdir -p /srv/mdm-config /srv/downloads
docker run -d --name mdm --restart unless-stopped \
  -p 8463:8463 \
  -v /srv/mdm-config:/config \
  -v /srv/downloads:/downloads \
  --user "$(id -u):$(id -g)" \
  ghcr.io/anefzaoui/misterdownloadmanager:<version>
```

(Replace `ghcr.io/anefzaoui/misterdownloadmanager:<version>` with `misterdownloadmanager` if
you built the image yourself.)

- `/config` keeps the list, the settings, the certificate and the paired devices.
- `/downloads` is where files land: each category has its folder inside (`Music`, `Video`,
  `Torrents`…). Mount a network share there to download straight onto it.
- `--user`: the user that owns those folders (MDM runs as no particular user).

[docker/compose.yml](../docker/compose.yml) does the same with Docker Compose.

## Pair your phone

`docker logs mdm` shows the address to open and, while no device is paired, a pairing code with a
QR code:

```text
The remote control listens at: https://172.17.0.2:8463/
Its certificate's SHA-256 fingerprint: DE:26:7F:…
No device is paired yet. Open the address (or scan this) and enter the code 1037 1606 …
```

Inside a container, the address MDM finds is the container's own: give it the one you open, the
**server's** address with the port you published, as `-e MDM_REMOTE_URL=https://192.168.1.10:8463/`,
and the log and the QR code show that one. Then pair as
[Control MDM from your phone](remote-control.md#pair-your-phone) says. For another code, at any
time:

```sh
docker exec mdm misterdownloadmanager-cli pair
```

To open the page by the server's name (`https://nas.local:8463/`), give MDM that name:
`-e MDM_REMOTE_HOSTS=nas.local` (several: separated by commas). Other names are refused, so that
no web page can reach MDM through a name of its own.

## On Unraid

[docker/unraid.xml](../docker/unraid.xml) is a template: copy it to
`/boot/config/plugins/dockerMan/templates-user/` on the flash drive, then **Docker → Add
Container** and pick **misterdownloadmanager**. It maps `/config` to `appdata`, `/downloads` to a
share, the port 8463, and runs as `nobody:users` (99:100), as Unraid's shares expect. The image
must be built first (above), on the server or pushed to a registry you use. If the container's
log says a folder belongs to another user, give it to `nobody:users` once:
`chown -R 99:100 /mnt/user/appdata/misterdownloadmanager`.

## Without Docker

On any Linux machine with MDM installed (the `.deb`, `.rpm`, AppImage or `tar.gz`):

```sh
misterdownloadmanager --headless --data ~/mdm-server --downloads /srv/downloads
```

| Flag | Or the variable | What it sets |
|---|---|---|
| `--data <folder>` | `MDM_DATA_DIR` | the list, settings, certificate and paired devices |
| `--downloads <folder>` | `MDM_DOWNLOADS` | where downloads go (each category's folder inside) |
| `--port <number>` | `MDM_REMOTE_PORT` | the remote control's port (8463) |
| `--listen <address>` | `MDM_REMOTE_LISTEN` | where it listens: every local network address unless given |
| `--hosts <names>` | `MDM_REMOTE_HOSTS` | names besides its addresses the page may be opened by |
| | `MDM_REMOTE_URL` | the page's address as you open it, shown with the pairing code |

The remote control is on in this mode (`MDM_REMOTE=off` turns it off). It stops cleanly, saving
its list, when the system stops it (`docker stop`, systemd, Ctrl+C). It works on Windows and
macOS too, the same way.

The command line talks to it as to a desktop MDM, with the same data folder:
`MDM_DATA_DIR=~/mdm-server misterdownloadmanager-cli status` (once `api on`), `pair`, and adding
an address: `misterdownloadmanager /d https://example.com/file.zip`.

## Send downloads from your browser

In the browser extension's menu, under **Send downloads to**:

- Select **Another Mister Download Manager (a server)**: type the server's address,
  open it once in that browser to accept its certificate, then enter a pairing code (`pair`
  above).
- The extension remembers paired servers: switch between any paired server and **This computer**
  with a single click without having to re-pair. Switching back leaves the browser paired; click
  **×** next to a server to unpair and forget it on both sides.
- Downloads the extension catches, and **Download with Mister Download Manager** on a link,
  then go to the server instead of this computer. The browser's cookies are not sent to the server:
  a download that needs you to be signed in to a site stays with this computer.
- The **Download video** button over videos sends the server the video's page when you click it,
  and nothing before: whether it shows is up to this computer's MDM (its Options), and when
  there's no MDM on this computer, it shows on every site.

## What a server doesn't do

- Video pages (yt-dlp) and image galleries need their helpers, which the image doesn't include.
- Nothing opens a window: a download that would ask a question (a duplicate, files already
  there) takes the choice set in the settings, or isn't added.
- Don't publish the port on the internet; reach your home network through a VPN instead.
