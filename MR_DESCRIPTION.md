<!--
Suggested MR title:
Telegram media downloads, macOS video improvements, and category path fixes
-->

## Summary

This MR adds the improvements built on this branch compared with the current `main` branch. The branch is synced with `main` at version 0.5.0 and keeps all current upstream changes.

### At a glance

| Area | What this MR adds |
| --- | --- |
| Telegram media | Pill and right-click downloads through MDM, protected private-channel media, concurrent downloads, correct size and completion, stable speed and time left |
| macOS video and browser | Reliable onedir yt-dlp, Node.js runtime, all video heights, separate Premium 1080p, persistent video window, correct page URLs, Downloads folder permission, stale partial cleanup |
| Category folders | Home-relative folders such as `Downloads/Video` resolve under the user's home folder instead of the process working directory |
| Build and packaging | `MDM_MACOS_ARCH` option for local single architecture macOS builds |

The sections below describe each area in more detail.

## Telegram media downloads

### Download from the Telegram viewer

- The normal Download video button, the pill over the video, appears over Telegram media and starts a real MDM download.
- Right-clicking the video or the empty space around it uses the same MDM download path.
- Every Mister context-menu entry on Telegram now downloads the media currently shown in the viewer.
- This also fixes the old page-menu behavior where empty-space right-click collected thumbnail and media links and created several unrelated downloads.
- The finished file is written into the normal Video category and appears in the download list, progress window, Finished list, and automation API.

### Private channels and groups

- The media fetch runs in the Telegram page's main world, so it uses Telegram's own session and service worker.
- This is required for protected media. A request from outside the page receives a redirect and cannot fetch the file.
- The page-only media URL is never handed to yt-dlp, which removes the old 302 failure.
- Telegram's native download and completed-file import remain available as a fallback.

### Real MDM download, not a browser download

- MDM starts a small local HTTP server bound to loopback only.
- The extension receives the server port and a random token from the native host through `media.json`.
- The page never receives the MDM token. The content script uploads the chunks and MDM validates each request.
- The upload path supports start, chunk, end, cancel, and failure messages.
- MDM writes to a temporary partial file, moves the completed file into the selected category folder, and removes partial data on cancel or when the item is removed with files.
- CORS and Private Network Access headers are included for Chromium pages.

### Multiple downloads at once

- The content script keeps a map of active Telegram requests instead of one global slot.
- Each request has its own request id, media source, chunk chain, and MDM upload id.
- A second video can be downloaded while the first one is still running.
- Both downloads update their own progress and finish independently.

### Correct size and completion

- The media size is read from the Telegram media object, not guessed from incomplete response headers.
- Chunk offsets are tracked correctly. The previous code read the buffer length after transferring it, so the offset stayed at zero and the same first chunk was sent repeatedly. That caused incorrect sizes and large amounts of duplicate data.
- Uploads stop at the real media size and finish as soon as all bytes are written, even if the final page message is lost.
- The progress window closes when the item finishes, and the normal Download complete dialog opens when that setting is enabled.
- Cancel cannot move an already completed Telegram item back to Stopped.

### Speed and time left

- Telegram sends media in bursts, and two downloads can take turns. A single sample made the speed flash to zero between chunks.
- The transfer rate is now a 6 second rolling average of the bytes actually received.
- Speed and time left stay stable while data is flowing.
- If no bytes arrive for 6 seconds, the transfer rate drops to zero. This is a real stall, not a hidden or faked value.
- The automation API and status strip also report the media item's real rate.
- The app frame is woken when an upload starts or a chunk arrives, so the row and progress window appear even when MDM is otherwise idle.

## macOS video and browser improvements

- yt-dlp is installed as a folder build on macOS and warmed after installation. This avoids unpacking the single-file Python runtime on every video page and avoids the related Python framework failures.
- Node.js is passed to yt-dlp as its JavaScript runtime.
- Video quality lists offer every available height, including separate Premium 1080p and normal 1080p choices.
- The open Download video window stays open and refreshes its quality list when another video is clicked in the browser.
- The current page URL is sent when YouTube changes videos without a full page load.
- The video window lookup is reused for the immediate download path without a second lookup.
- MDM asks for Downloads folder access at startup instead of during the first Start action.
- The progress window is raised when a download starts.
- Stale yt-dlp `.part` files are removed when an item leaves the list.

## Category folder fix

A folder saved as `Downloads/Video` was treated as relative to the process working directory. When MDM was launched from the desktop, that became `/Downloads/Video`, which is on the read-only system volume. Home-relative folders now resolve under the user's home folder. Absolute paths and Windows paths keep their existing behavior.

This removes the false "Create Folder" prompt and the read-only folder error when saving videos.

## Build and packaging

- The macOS build script accepts `MDM_MACOS_ARCH=arm64` or `MDM_MACOS_ARCH=x86_64` for a local single architecture build.
- The extension version follows the app version. This branch was built and packaged as 0.5.0.

## Where to review

- Telegram upload server and cleanup: `crates/app/src/media.rs`
- Telegram rows, progress, completion, and cancellation: `crates/app/src/app.rs`
- Telegram fetch and right-click routing: `extension/background.js`
- Telegram pill, chunk upload, and concurrent requests: `extension/video.js`
- Rate and time left: `crates/app/src/model.rs`, `crates/app/src/progress.rs`
- Native host media handshake: `crates/nmhost/src/main.rs`
- macOS video window and quality handling: `crates/app/src/videodlg.rs`, `crates/tools/src/*`
- Category folder resolution: `crates/app/src/model.rs`
- macOS build switch: `tools/installer/build-macos.sh`

## Compatibility

- Other sites keep the existing video pill and yt-dlp flow.
- Telegram's native download button still works, and its completed file can still be imported as a fallback.
- Existing downloads and settings keep working.
- No database or file format migration is required.

## Testing

- 270 app tests passed, with 1 ignored and 1 known flaky macOS host-copy test filtered out.
- 18 native host tests passed.
- 7 IPC schema tests passed.
- The app and extension were built and installed on macOS arm64.
- Manually tested with protected Telegram media, including two videos downloading at the same time.
- Verified correct byte count, live speed and time left, progress window closing, Finished list placement, and file output in the Video folder.

## AI assistance

AI assistance was used to implement the changes in this MR. The code was reviewed, built, and tested locally. Both the app and the extension were manually exercised before submitting the MR.
