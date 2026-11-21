---
title: Code signing policy
description: Mister Download Manager's code signing policy: free code signing by SignPath.io with a certificate by SignPath Foundation, which Windows files are signed, who may approve a signature, and what the app sends over the network.
lede: How Mister Download Manager's Windows programs are code-signed, who decides what gets signed, and what the app sends over the network.
priority: 0.3
---

<div class="summary-box">
<p translate="no" lang="en" dir="ltr">Free code signing provided by <a href="https://about.signpath.io">SignPath.io</a>, certificate by <a href="https://signpath.org">SignPath Foundation</a></p>
</div>

## What is signed

A code signature lets Windows show who published a program and check that it wasn't changed after it was built. When a release of Mister Download Manager ("MDM") is signed, its signature covers the Windows files that come from this project:

- the programs: `misterdownloadmanager.exe`, `misterdownloadmanager-nmhost.exe` (the browser helper), `misterdownloadmanager-cli.exe` and `misterdownloadmanager-mcp.exe`;
- the installers, `MisterDownloadManager-<version>-setup.exe` and its ARM64 and 32-bit versions, and the uninstaller they put on your computer;
- the same programs in the portable and plain zips.

Every signed file is built from the public source code at [github.com/anefzaoui/misterdownloadmanager]({{REPO}}) by GitHub Actions, on GitHub's servers, never on a personal computer. Each one says it is Mister Download Manager and which version it is.

Whatever the signature, you can still check every download against the release's signed checksums: see [Check your download](/download.html#check-your-download).

## Team roles

- **Committers and reviewers:** <a href="https://github.com/anefzaoui" translate="no">Ahmed Nefzaoui</a> (<span translate="no">@anefzaoui</span>). Contributions from other people are reviewed by Ahmed before they are merged.
- **Approvers:** <a href="https://github.com/anefzaoui" translate="no">Ahmed Nefzaoui</a> (<span translate="no">@anefzaoui</span>). Every signing request is approved by hand, release by release.

## Privacy

MDM has no telemetry, no analytics and no ads, and it sends nothing about you to us. It connects to other systems only for these:

- **What you ask for:** the servers you download from, torrent peers and trackers, and the services you turn on yourself, such as a proxy, a DNS or a debrid account.
- **Updates:** when it starts, and once a day while it stays open, MDM asks GitHub for its latest version. You can turn this off in **Help → Check for updates** (**Check for updates at every start**).
- **Helpers you install:** yt-dlp and ffmpeg (and gallery-dl) come from their official releases when you choose to install them, and yt-dlp is kept up to date when you download videos.
- **Crash reports:** after a crash, the report window may fetch that version's debugging symbols from MDM's GitHub release and ask GitHub whether the same problem was reported already. The report itself opens in your browser, on GitHub, only if you choose to send it.

Everything the app, its browser extension and this website do with your data is in the [privacy policy](/privacy.html).
