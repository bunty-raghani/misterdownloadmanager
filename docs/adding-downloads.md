# Adding downloads

[← Back to docs index](README.md)

There are several ways to give MDM something to download. Most of them end in the same window, **Download File Info**, where you choose where the file goes.

## Add URL

1. Copy the file's address (in a browser: right-click the link → **Copy link**).
2. Click **Add URL** on the toolbar, choose **Tasks → Add new download**, or press **Ctrl+N**.
3. **Enter new address to download** opens. If the clipboard holds a web address, it is already filled in. The arrow at the end of **Address** lists the last 20 addresses you entered; pick one to use it again. Right-click the box for **Cut**, **Copy**, **Paste**, **Delete** and **Select all**.
4. If the server needs a user name and password, tick **Use authorization** and type them.
5. Click **OK**.

MDM takes `http://`, `https://`, `ftp://`, `ftps://`, `ftpes://` and `magnet:` addresses. Anything else gets **Invalid address**.

![Enter new address to download, with the Address box, Use authorization and OK](images/help/add-url.webp)
*The numbers are the steps above: 3 the address, 4 a login if the server needs one, 5 OK.*

If the address is a video's page (YouTube, Vimeo, TikTok and others), the video window opens instead. See [Video downloads](video.md). A magnet link opens **Add torrent**; see [Torrents](torrents.md).

## FTP and FTPS

FTP downloads work like web downloads: several connections at once, pause and resume, and the
same progress window.

- `ftp://` — plain FTP.
- `ftpes://` — FTP secured with TLS from the start of the session (explicit FTPS, `AUTH TLS`, usually port 21). Use it when the server says it supports TLS.
- `ftps://` — implicit FTPS, port 990 unless the address gives another.

The server's certificate is checked against Windows' (or the system's) trusted certificates, as
for `https://`. A Linux system without them (a minimal install without the `ca-certificates`
package) still downloads over plain HTTP and FTP; each secure download then fails at once, saying
to install that package.

**Logging in.** Write the login in the address (`ftp://user:password@server/folder/file.iso`),
tick **Use authorization**, or save it in **Options → Sites Logins**. Without one, MDM logs in as
`anonymous`. The path is relative to the folder the login starts in; to start from the server's
root, write `%2F` after the server (`ftp://server/%2Fpub/file.iso`).

**Resuming.** MDM keeps the file's size and modification time. If either has changed when you
resume, it says the file changed on the server instead of mixing two versions
(**Options → Downloads → Ignore file modification time changes** skips the time check). The
password is not written to the resume file next to the download: MDM gives it again when it
resumes.

**Connections.** Passive mode is used by default. **Options → Proxy / Socks → Use FTP in PASV
mode** switches it off for servers that only take active connections. FTP connects to the
server directly, not through the HTTP proxy.

Browsers no longer download `ftp://` links themselves. Right-click one and choose **Download with
Mister Download Manager**, or copy it and use **Add URL**.

## Google Drive links

Paste a Google Drive link as it is, the one **Share → Copy link** gives:

- `https://drive.google.com/file/d/…/view?usp=sharing`, with or without `/view`;
- `https://drive.google.com/uc?id=…&export=download` and `https://drive.google.com/open?id=…`;
- `https://docs.google.com/document/d/…/edit` (and Sheets, Slides, Drawings).

MDM downloads the file itself, as it was uploaded (a video comes in its own format, not the
player's copy), with its real name and size, over several connections, and it can resume. For a
large file, Drive shows a page that says it **can't scan this file for viruses**; MDM goes past
it as **Download anyway** does.

**Google Docs, Sheets and Slides** come as Word (`.docx`), Excel (`.xlsx`) and PowerPoint
(`.pptx`) files, and Drawings as `.png`. These files are made when they are asked for, so their
size is unknown until they arrive and they can't be resumed: a cut-off download starts again.

**Files that aren't shared with everyone.** Right-click the link in the browser and choose
**Download with Mister Download Manager**, or use Drive's own **Download** button: the extension
passes your browser's Google sign-in with the download. MDM never asks for or keeps your
password. Without a sign-in, MDM says the file **needs you to sign in**.

**When Drive won't give a file.** Drive limits how often a file can be downloaded: after many
downloads in a day it answers "too many users have viewed or downloaded this file recently".
MDM shows Drive's own words and writes nothing. Try again later (it can take up to a day), or
sign in to Google in your browser and download from there, which often gets past the limit. MDM
doesn't try to get around it. A file whose owner turned off downloading, or that your account
can't open, gets **no access**.

**The video button over a Drive video** downloads the original file, not one of the player's
smaller copies.

**Folders** aren't downloaded yet: a folder's link gets **this is a Google Drive folder, not a
file**. Open the folder in the browser and download its files from there.

## Download File Info

MDM asks the server for the file's name and size, and shows them in **Download File Info**:

- **URL** — the address.
- **Category** — chosen from the file's type (Compressed, Documents, Music, Programs, Video or General). Each category has its own folder. **+** beside it makes a new category.
- **Save As** — the folder and file name. You can type another, pick one of the last 20 folders you saved to from the arrow at its end, or choose a folder with **...**.
- **Remember this path for "…" category** — tick it to make the folder you typed the category's folder from now on.
- **Description** — a note of your own. It shows in the list's **Description** column and Find can search it.
- **Server** — where the server is: its country's flag and name, and how long a round trip to it takes. See [Where the server is](connections-and-speed.md#where-the-server-is). It stays blank when the browser handed the download over with its name and size, since MDM didn't need to ask the server.

![Download File Info for a ZIP file: its address, the Compressed category, the folder it goes to, its size, and the Download Later, Start Download and Cancel buttons](images/help/download-file-info.webp)
*1 the category, 2 where the file goes, 3 start now, 4 or put it in a queue.*

Then choose:

- **Start Download** (or **Enter**) — download now. The progress window opens.
- **Download Later** — add it to a queue without starting. See below.
- **Cancel** (or **Esc**) — add nothing.

By default MDM starts downloading while this window is open, so no time is lost while you decide. **Cancel** drops what it fetched. You can turn this off in **Options → Downloads → Start downloading immediately while displaying "Download File Info" dialog**.

If the folder doesn't exist yet, MDM asks **Do you want to create it?** before it starts.

## Download Later

**Download Later** puts the download in a queue instead of starting it. By default MDM asks which one: pick the queue under **Add to queue**, and tick **Start the queue now** if you want it to run straight away. **Cancel** leaves the download in the list, in no queue.

To skip that question and always use the **Main download queue**, turn off **Options → Downloads → Show queue selection panel on pressing "Download Later" button**.

More about queues in [Queues and the Scheduler](queues-and-scheduler.md).

## From the clipboard

With **Options → General → Offer to download links copied to the clipboard** ticked (it is, unless you turned it off), when you copy a link to a file type in **Options → File types** (ZIP, EXE, MP4, PDF and many more), **Download File Info** opens with it filled in, ready for **Start Download**. Links to other pages are left alone. With **Options → Downloads → Show start download dialog** off, the download starts at once instead. Only links copied while MDM runs are taken, never one that was already there.

Copied several links at once, a list or a whole forum post? Choose **Tasks → Add batch download from clipboard** (**Ctrl+B**, also in the tray icon's menu). MDM reads the clipboard then, and only then, and finds every download link in the text: `http`, `https`, `ftp` and magnet links, one per line or among other words, in Markdown (`[name](link)`) or between `<` and `>`. Each one is listed once in **Link grabber**, ticked and sorted into packages: pick a folder for each package, then **Start** or **Add to queue**. Downloads added together end with one **Downloads complete** summary. Magnet links are added as torrents with all their files once their files are known. A copied address with a number in its name, such as `https://example.com/show/ep01.mp4`, opens **Batch download** instead (below). If the clipboard holds no text, or no link, MDM says so.

## Drag and drop

1. Choose **Tasks → Show drop target**. A small box with the MDM icon appears and stays on top of other windows.
2. Drag a link from your browser onto it. MDM adds it as if you had used **Add URL**.

You can also drop magnet links and `.torrent` files on it. Drag the box to move it, and double-click it to bring the main window back. Right-click it for **Show Mister Download Manager**, **Add new download** and **Hide drop target**.

## Batch download: many numbered files

For files whose names differ only by a number or a letter, such as `photo001.jpg` to `photo150.jpg`:

1. Choose **Tasks → Add batch download**. Copied one address with a number in its file name, such as `https://example.com/show/ep01.mp4`? **Tasks → Add batch download from clipboard** (**Ctrl+B**) puts the asterisk where the number was, sets **From** and **Wildcard size** from it (`ep*.mp4`, from 1, size 2) and waits in **To** for the last number.
2. In **Address**, write the address with one asterisk where the number or letter changes: `https://example.com/photo*.jpg`.
3. Under **Replace asterisk to**, choose **Numbers** or **Letters**, then **From** and **To**.
4. For numbers, set **Wildcard size** to the number of digits. With 3, the files are `photo001.jpg`, `photo002.jpg` and so on.
5. If the server needs a login, tick **Use authorization**.
6. Check **First file**, **Second file** and **Last file**, then click **OK**.

The downloads go to a queue. As with Download Later, MDM asks which queue unless you turn off **Options → Downloads → Show queue selection panel on closing batch download dialogs**.

![Batch download: an address with an asterisk, what replaces it, and the first, second and last file](images/help/batch.webp)
*One asterisk in the address, then numbers or letters from–to.*

## A list of links

To add every link in a text file:

1. Choose **Tasks → Import → From text file**.
2. Pick the file. **Link grabber** lists every web address MDM found in it, in packages. While MDM reads a long file, the strip under the list of downloads says **Reading** and the file's name; you can go on using MDM meanwhile. Import reads files of up to 64 MB, far more than any list of links holds: a larger file (a video, an archive, a backup picked by mistake) is refused with a message.
3. Untick the addresses you don't want (**Check all** and **Uncheck all** help), then click **Add to queue**.

![Link grabber listing the links of a page in packages, with their names, sizes and whether they are online](images/help/link-grabber.webp)
*Link grabber: untick what you don't want, then Add to queue.*

The downloads join the **Main download queue**, ready to start (**Start** starts it too). **Tasks → Import → From IDM export file** reads an `.ef2` file, with each download's referrer and cookies.

**Tasks → Link grabber...** opens the same window empty: paste text with links in it (**Ctrl+V**). See [Link grabber and archives](link-grabber.md) for the packages, the archive password and extracting archives when they are downloaded.

To save your own list, use **Tasks → Export → To text file** (one address per line) or **To IDM export file**.

## When the address is already in the list

By default MDM asks what to do when you add an address it already has. You can choose a fixed answer in **Options → Downloads → If a duplicate download link is added**:

- **Show a dialog and ask what to do.**
- **Add the duplicate with a numbered file name**
- **Add the duplicate and overwrite the existing file**
- **If existing file is complete, show download complete dialog; otherwise resume it.**

## Next steps

- [Send browser downloads to MDM →](browser-integration.md)
- [Watch a download in the progress window →](connections-and-speed.md)
- [Queue downloads for later →](queues-and-scheduler.md)
