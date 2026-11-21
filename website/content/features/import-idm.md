---
name: Moving from IDM
title: Import your download list from IDM
headline: Switching is easy. Bring your downloads with you.
line: Your IDM list comes over in one file, each download with its address, referrer and cookies.
lede: Moving from Internet Download Manager? Export your list there and import it in MDM. Every download comes over with its address, referrer and cookies, into a window laid out the way you know.
description: Move from Internet Download Manager to Mister Download Manager: import an IDM export file (.ef2) or a text list of links, with referrers and cookies.
group: organising
order: 160
since: 0.1.0
shot: images/help/link-grabber.webp
shot_alt: Link grabber listing imported links in packages, with their names, sizes and whether they are online
where: Tasks → Import → From IDM export file
help: adding-downloads.md#a-list-of-links
---

## Your list comes over in one file

In IDM, choose **Tasks → Export → To IDM export file**. In MDM, choose **Tasks → Import → From
IDM export file** and pick that `.ef2` file. Each download comes over with its address, the page
it came from (its referrer) and its cookies. The downloads join the **Main download queue**,
ready to start.

## A plain list of links works too

**Tasks → Import → From text file** reads a text file and picks out every web address in it.
You can also drop a `.txt` or `.ef2` list straight onto the download list. Untick what you don't
want, and add the rest to the queue.

Going the other way is just as easy: **Tasks → Export → To text file** saves one address per
line, and **To IDM export file** writes an `.ef2` file.

## Nothing new to learn

MDM is a separate program, written from scratch, but its windows are laid out the way IDM users
expect: the toolbar, the categories, the download list, the progress window with every
connection, queues and the Scheduler. It is free, with no trial and no sign-up.

IDM is the more mature product, refined over many years. MDM finished faster in [our
benchmark](/benchmark.html): a 1 GiB file in 1.0 s against IDM 6.42's 5.5 s, both on one PC,
downloading from a test server on that PC (a loopback test). The [FAQ](/faq/mdm-vs-idm.html) sums up the other differences.

## Choose before you add

<!--@coming 0.5-->

An import opens **Link grabber**, which sorts the links into packages: the volumes of one archive
together, the links of one page together, the rest in **Various files**. Each link is looked up
for its real name, its size and whether the server still has it, and the ones that are gone come
unticked. A link already in your list goes under **Already in the list**, unticked.

**Add to queue** adds the ticked links to the **Main download queue**, as the import always did,
and **Start** starts the queue as well. An `.ef2` file still brings each download's referrer and
cookies. See [Link grabber and archives](link-grabber.md).
