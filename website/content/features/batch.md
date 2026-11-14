---
name: Batch downloads
title: Batch download numbered files and lists of links
headline: A whole numbered series from one address.
line: One address with an asterisk fetches a numbered series; a text file of links adds them all.
lede: Files named photo001.jpg to photo150.jpg don't need 150 clicks. Write the address once with an asterisk where the number changes, and MDM queues every file. A list of links in a text file goes in just as easily.
description: Batch downloads in Mister Download Manager: a numbered series of files from one address with an asterisk, or every link in a text file at once.
group: downloading
order: 80
since: 0.1.0
shot: images/help/batch.webp
shot_alt: Batch download: an address with an asterisk, what replaces it, and the first, second and last file
where: Tasks → Add batch download
help: adding-downloads.md#batch-download-many-numbered-files
---

## One asterisk, a whole series

Choose **Tasks → Add batch download** and write the address with one asterisk where the files
differ: `https://example.com/photo*.jpg`. Under **Replace asterisk to**, choose **Numbers** or
**Letters**, then **From** and **To**. For numbers, **Wildcard size** sets the number of digits:
with 3, the files are `photo001.jpg`, `photo002.jpg` and so on.

Before you click **OK**, the window shows the **First file**, **Second file** and **Last file**,
so a typo shows up before anything downloads. If the server needs a login, tick **Use
authorization**. Copied one address with a number in it, such as `…/ep01.mp4`? **Tasks → Add
batch download from clipboard** (Ctrl+B) puts the asterisk where the number was and waits for the
last one in **To**. Copied a whole list of links, or a forum post with links in it? The same
command lists every one of them in Link grabber, ticked and ready to start.

## Straight into a queue

The files go to a queue, not all at once. MDM asks which one, and that question can be turned
off in **Options → Downloads**. From there the queue runs as many files at a time as you set,
and the Scheduler can start it at night. See [Queues and the Scheduler](queues-and-scheduler.md).

## A list of links from a file

Have the links already, in a text file? **Tasks → Import → From text file** reads every web
address in it, and you untick the ones you don't want. **From IDM export file** reads an `.ef2`
list, with each download's referrer and cookies, so a list exported from IDM comes over with
what its downloads need. You can also drop a `.txt` or `.ef2` list on the download list.

The other way works too: **Tasks → Export → To text file** writes one address per line, and
**To IDM export file** writes an `.ef2` list.

When an address is already in the list, MDM asks what to do, or follows the answer you chose in
**Options → Downloads → If a duplicate download link is added**.

## Imports with more to see

<!--@coming 0.5-->

Imported lists open in [Link grabber](/features/link-grabber.html): the links sorted into
packages, each one checked online with its real name and size, and offline links unticked. A
batch download of links from a file host goes through your
[debrid account](/features/debrid.html) when its service supports the host.
