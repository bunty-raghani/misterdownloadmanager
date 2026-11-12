# Image galleries

MDM downloads the pictures of a gallery (a Wikimedia Commons category, a Flickr album, a DeviantArt gallery, an Imgur album and many more) with the help of [gallery-dl](https://codeberg.org/mikf/gallery-dl), a free program that reads several hundred sites. Like yt-dlp for videos, it's an optional helper: MDM installs it the first time you need it, or uses your own copy.

## Download a gallery

1. Click **Add URL** and paste the gallery's address, for example `https://commons.wikimedia.org/wiki/Category:Featured_pictures_of_birds`.
2. MDM opens **Download a gallery** instead of downloading the page as a file. The first time, click **Install gallery-dl**.
3. gallery-dl checks the address on your computer, without going online. When the line reads **gallery-dl can download this gallery's files**, you're ready.
4. Check **Save to** (the General category's folder by default). The pictures go into a folder of their own there, named after the gallery.
5. Optionally, type a number in **Files at most** to download only the first ones the site lists. On most sites those are the newest. Leave it empty for all of them.
6. Click **Download Now**, or **Later** to add it to the list without starting.

The gallery gets one row in the list. Its size grows as the pictures arrive, and its log says how many were saved. Each picture is marked as downloaded from the internet, as any download is.

## How the pictures come down

gallery-dl lists the gallery's pictures: where each one comes from and the name it gets in the folder. MDM's own downloader then brings them in, 4 at a time (fewer when the site answers that it's busy, as Wikimedia's does), under your speed limits (a change applies at once), and the progress window's connections table shows each one. gallery-dl then runs as before: it finds those pictures already there and fetches only what MDM couldn't have, such as a picture from a site that wants a sign-in or headers of its own. The log says how many each saved.

A gallery-dl older than 1.32 (a Linux distribution's, often) can't list the pictures this way: it downloads them all itself, one a second, as before.

## Get the new pictures later

Start a finished gallery again (**Resume**) to fetch only the pictures that aren't in its folder yet. The ones already there are skipped.

## Stop, delete

- **Stop** ends MDM's downloads of the pictures, and gallery-dl with everything it started. The pictures saved so far stay.
- **Delete** takes the gallery off the list. Its folder and pictures stay, even if you tick the box to delete files: delete the folder yourself if you want it gone.

## Which addresses open the gallery window

Add URL sends these to the gallery window:
- Wikimedia Commons categories;
- Flickr photographers and albums;
- DeviantArt and ArtStation galleries;
- Imgur albums;
- Pixiv artists and works;
- Pinterest boards and pins;
- Danbooru and Gelbooru searches;
- Tumblr blogs;
- Bluesky profiles.

A picture's own address (one ending in `.jpg` or `.png`) downloads as a file, as before.

## gallery-dl itself

- **Where it comes from:** MDM downloads gallery-dl from its releases on Codeberg. It accepts it only when the release's checksum list is signed by gallery-dl's author's key, pinned in MDM, and the program matches that list. It keeps the previous version and never goes back to an older one. On Windows and on Linux (x86-64) MDM installs it; on macOS, install it with Homebrew (`brew install gallery-dl`) or pip.
- **Your system's copy:** on Linux and macOS, MDM finds a gallery-dl on your `PATH` and in the usual package-manager folders, including Homebrew's, as it does yt-dlp. A program you chose yourself comes first, then MDM's own copy; the system's is used when neither is there, and your package manager keeps it up to date. If yours is too old for a site, **Use MDM's own copy** in **Helpers...** downloads MDM's (Windows and Linux x86-64), used from then on.
- **Your own copy:** **Helpers...** (in the gallery window or the video window) lists yt-dlp, ffmpeg and gallery-dl, and **Use another program...** takes your own copy of each instead of MDM's.
- **How it asks:** gallery-dl and MDM's downloader ask sites as MDM; MDM brings 4 pictures at a time, and gallery-dl, for what's left to it, one a second at most. Wikimedia's servers refuse tools that ask much faster or without a name. gallery-dl uses no configuration file of yours.
- **Network adapter:** while one is chosen (**Options → Proxy / Socks → [Network adapter](options.md#proxy--socks)**), galleries go through it too, as video pages do: MDM gives gallery-dl a proxy of its own on this computer, opens every connection through the adapter and looks the names up with the DNS you chose. When the adapter disconnects, the gallery stops with the other downloads; nothing goes out through your normal connection.
- **The gallery-dl of your Linux distribution:** the one your distribution ships works too, from 1.20 (Ubuntu 22.04) on. Found on its own, it gets only the options every one of those versions knows; chosen in **Helpers...**, the ones its version knows. One older than that (Debian 11's 1.17) is refused when you choose it, saying which version MDM needs; found on its own, its galleries fail, and **Use MDM's own copy** is the way out.

## On the command line

- `install-gallery-dl` installs or updates gallery-dl, verified in the same way.
- `gallery-get URL [-o folder] [--limit N]` downloads a gallery into a folder named after it. See [Command line and automation](command-line.md).
