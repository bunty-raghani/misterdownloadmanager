---
name: Mister Download Manager website
description: A download is a delivery. The site is the file's waybill, stuck on the real app window.
colors:
  board: "#1f63d6"
  board-deep: "#1854bb"
  board-edge: "#3b78e0"
  on-board: "#ffffff"
  on-board-2: "#e6eefd"
  paper: "#fcfcfb"
  paper-2: "#f0f3f7"
  ink: "#111318"
  ink-2: "#434956"
  ink-3: "#5f6676"
  line: "#dde1e8"
  go: "#57cc5a"
  go-hover: "#47b94a"
  go-ink: "#08200b"
  link: "#0d58c9"
  stamp: "#1b7a26"
  board-dark: "#0c1a33"
  board-deep-dark: "#081226"
  board-edge-dark: "#1c2e52"
  on-board-dark: "#f2f5fb"
  on-board-2-dark: "#c3cde0"
  paper-dark: "#11151d"
  paper-2-dark: "#192030"
  ink-dark: "#eef1f6"
  ink-2-dark: "#c3cad6"
  ink-3-dark: "#98a1b1"
  line-dark: "#2a3242"
  link-dark: "#86b6ff"
  stamp-dark: "#6bdb6e"
typography:
  display:
    fontFamily: "Archivo, Segoe UI, system-ui, sans-serif"
    fontSize: "clamp(48px, 8vw, 116px)"
    fontWeight: 900
    lineHeight: 0.92
    letterSpacing: "-0.025em"
    fontVariation: "'wdth' 68"
  headline:
    fontFamily: "Archivo, Segoe UI, system-ui, sans-serif"
    fontSize: "clamp(36px, 4.8vw, 62px)"
    fontWeight: 800
    lineHeight: 1.05
    letterSpacing: "-0.01em"
    fontVariation: "'wdth' 78"
  page-title:
    fontFamily: "Archivo, Segoe UI, system-ui, sans-serif"
    fontSize: "clamp(40px, 5.6vw, 72px)"
    fontWeight: 850
    lineHeight: 1.05
    letterSpacing: "-0.015em"
    fontVariation: "'wdth' 72"
  label-headline:
    fontFamily: "Archivo, Segoe UI, system-ui, sans-serif"
    fontSize: "clamp(30px, 3vw, 40px)"
    fontWeight: 850
    lineHeight: 1
    letterSpacing: "-0.012em"
    fontVariation: "'wdth' 72"
  title:
    fontFamily: "Archivo, Segoe UI, system-ui, sans-serif"
    fontSize: "clamp(26px, 2.6vw, 32px)"
    fontWeight: 800
    lineHeight: 1.05
    fontVariation: "'wdth' 78"
  band:
    fontFamily: "Archivo, Segoe UI, system-ui, sans-serif"
    fontSize: "17px"
    fontWeight: 800
    lineHeight: 1.15
    fontVariation: "'wdth' 80"
  lede:
    fontFamily: "Archivo, Segoe UI, system-ui, sans-serif"
    fontSize: "19px"
    fontWeight: 400
    lineHeight: 1.5
  body:
    fontFamily: "Archivo, Segoe UI, system-ui, sans-serif"
    fontSize: "17px"
    fontWeight: 400
    lineHeight: 1.6
  field-value:
    fontFamily: "Archivo, Segoe UI, system-ui, sans-serif"
    fontSize: "16.5px"
    fontWeight: 650
    lineHeight: 1.35
    fontFeature: "'tnum' 1"
  fine:
    fontFamily: "Archivo, Segoe UI, system-ui, sans-serif"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: 1.55
  field-name:
    fontFamily: "Archivo, Segoe UI, system-ui, sans-serif"
    fontSize: "12px"
    fontWeight: 750
    letterSpacing: "0.045em"
    fontVariation: "'wdth' 74"
  mono:
    fontFamily: "Cascadia Mono, Consolas, SF Mono, ui-monospace, monospace"
    fontSize: "0.88em"
rounded:
  hairline: "2px"
  tag: "4px"
  item: "6px"
  control: "8px"
  sheet: "10px"
  label: "12px"
spacing:
  gutter: "clamp(16px, 4vw, 40px)"
  wrap: "1240px"
  wrap-narrow: "880px"
  section: "clamp(56px, 7vw, 96px)"
  sheet-pad: "clamp(24px, 4vw, 56px)"
  row-pad: "clamp(18px, 3vw, 32px)"
  field: "10px 16px 11px"
components:
  button-go:
    backgroundColor: "{colors.go}"
    textColor: "{colors.go-ink}"
    rounded: "{rounded.control}"
    padding: "0 20px"
    height: "46px"
  button-go-hover:
    backgroundColor: "{colors.go-hover}"
    textColor: "{colors.go-ink}"
  button-line:
    backgroundColor: "transparent"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "0 20px"
    height: "46px"
  sheet:
    backgroundColor: "{colors.paper}"
    textColor: "{colors.ink}"
    rounded: "{rounded.sheet}"
  waybill:
    backgroundColor: "{colors.paper}"
    textColor: "{colors.ink}"
    rounded: "{rounded.label}"
  label-band:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.paper}"
    typography: "{typography.band}"
    padding: "12px 18px"
  field:
    textColor: "{colors.ink}"
    typography: "{typography.field-value}"
    padding: "{spacing.field}"
  tag:
    textColor: "{colors.ink}"
    rounded: "{rounded.tag}"
    padding: "3px 8px"
  tag-stable:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.paper}"
    rounded: "{rounded.tag}"
    padding: "3px 8px"
  stamp:
    backgroundColor: "{colors.paper}"
    textColor: "{colors.stamp}"
    rounded: "{rounded.item}"
    padding: "5px 12px"
  segmented-tab-selected:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.paper}"
    padding: "10px 16px"
  header:
    backgroundColor: "{colors.board}"
    textColor: "{colors.on-board}"
    height: "68px"
  nav-link:
    textColor: "{colors.on-board}"
    rounded: "{rounded.item}"
    padding: "8px 10px"
  footer:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.paper}"
---

# Design System: Mister Download Manager website

## Overview

**Creative North Star: "The Waybill on the Parcel"**

A download is a delivery. The page is the board the parcel sits on, in the logo's blue; the real app window is the parcel; what the visitor reads is printed on thermal-label sheets stuck to it, in black ink with hairline field rules, ranked like a courier label: who sends it, the headline, the fine print, the fields, a real barcode. The benchmark is tracking: two shipments scanned event by event to the measured times, each event showing its rate as well as its time.

The world is the owner's choice. The blue board (#1f63d6) replaced kraft brown after four rendered options; dark theme is a navy depot where the same labels print in reverse (near-black stock, cool white ink). Density is a label's, not a brochure's: few elements, each one a field with a name and a value, numbers in tabular figures. Every screenshot is the real, unretouched app, shown at full content width; the site never draws a fake window, a glowing mock-up or a row of icon cards. Reading pages (help, FAQ, news, legal) take the same world with restraint: the board above, one sheet to read.

The site ships in 21 languages, Arabic and Persian right to left. Every direction-bound value is a logical property; labels tilt the other way in RTL; only native language names are isolated with `bdi`.

**Key Characteristics:**
- Blue board, white label stock, black ink; green only for Download and delivered.
- One self-hosted face, Archivo, condensed by its width axis for headings and field names.
- Fields, not cards: name over value, ink rules between.
- Real artefacts only: the unretouched app, a scannable Code 128 barcode, the benchmark's own times.
- Motion is mechanical and rare: the label feeds out once, progress moves in real time, a stamp lands.

## Colors

Two materials and one law: the board in the logo's blue, label stock in white and ink, and the logo's green reserved for the action and for arrival.

### Primary
- **Logo Blue Board** (board): the page itself, the sticky header and every surface between sheets. Text on it is On-Board White or On-Board Haze only (both checked WCAG AA). Board Deep marks the closing section; Board Edge is the header's and closing's hairline.
- **Navy Depot** (board-dark family): the same roles in dark theme; a deep navy board, never black.

### Secondary
- **Dispatch Green** (go): the Download button, the finished lane of the tracking race, and nothing else. Its text is Go Ink, a near-black green, never white; hover darkens to Go Hover.
- **Stamp Ink** (stamp): a darker delivered green for green text and rules printed on label stock (the rubber stamp, "Delivered"); lightens to a bright green on the navy depot.

### Tertiary
- **Label Link Blue** (link): links inside sheets (prose, FAQ answers) and the focus ring everywhere. On the board, links take the board's text colour and are underlined instead.

### Neutral
- **Thermal Paper** (paper): every sheet, label, menu and dialog. Paper Grey (paper-2) is the hover and selected row on a sheet, code and quote wells.
- **Label Ink** (ink): reading text on sheets, field rules, the label band, the footer and selected segmented tabs. Ink 2 is secondary text (sublines, answers); Ink 3 is field names, fine print and captions on sheets.
- **Hairline** (line): quiet dividers inside a sheet (table rows, list rows, the help nav's edge).

### Named Rules
**The Green By Law Rule.** Green appears only on the Download action and on delivered states (the done lane, the stamp). Never on a secondary button, a badge, a link or a decorative accent. One exception, the owner's choice: on the comparison page, MDM's line is live. Its foot carries a strip of eight connections that fill in green, each at its own pace with a head where the bytes arrive; then a Delivered stamp lands ("1 GiB in 1.01 s", the benchmark's median), and it loops every 6 s. Its ticks in the declaration are green. Reduced motion shows it delivered, at rest.

**The Two Text Systems Rule.** Text on the board is on-board or on-board-2; text on label stock is ink, ink-2 or ink-3. Never put ink on the board or board text on a sheet; every element printed on stock declares ink colour, whatever it inherits.

**The Colour Lives In Fields Rule.** Reading text stays ink. Colour belongs to the board, the action, the stamp and links, not to paragraphs or headings.

## Typography

**Display Font:** Archivo, variable width, condensed (with Segoe UI, system-ui)
**Body Font:** Archivo at normal width (with Segoe UI, system-ui; Tahoma and Noto Sans Arabic for Arabic and Persian)
**Label/Mono Font:** Cascadia Mono (with Consolas, SF Mono) for code and the barcode's human-readable line

**Character:** One self-hosted grotesk does everything, by its width axis: narrowed to about 68-80% and heavy for headlines, stamps and field names, normal width for reading. The condensed caps carry the label's voice; the plain body is the fine print you actually read.

### Hierarchy
- **Display** (900, clamp(48px, 8vw, 116px), 0.92, width 68%): one word or phrase per section at poster scale: the closing line, the race's ratio (up to 200px, width 70%), the 404 number.
- **Headline** (800, clamp(36px, 4.8vw, 62px), 1.05, width 78%): section headings on the board.
- **Page title** (850, clamp(40px, 5.6vw, 72px), width 72%): inner pages' h1 on the board, max 20ch.
- **Label headline** (850, clamp(30px, 3vw, 40px), 1.0, width 72%): the headline printed on a waybill.
- **Title** (800, clamp(26px, 2.6vw, 32px), width 78%): prose h2; h3 at 21px width 86%.
- **Band** (800, 17px, width 80%): the sender line in a label's ink band.
- **Lede** (400, 19px, 1.5): section and page introductions, max 62-64ch.
- **Body** (400, 17px, 1.6; 1.7 in prose): reading text, max 72ch; 16.5px on phones.
- **Field value** (650, 16.5px, 1.35, tabular figures): the value under a field name.
- **Fine** (400, 14px, 1.55): captions, sources, small print.
- **Field name** (750, 12px, 0.045em, uppercase, width 74%): the name over a field value, table column heads, the help nav's group names.

### Named Rules
**The Tabular Figures Rule.** Every number that can be compared (times, sizes, ratios, versions, dates) is set in tabular figures.

**The Condense, Don't Swap Rule.** Hierarchy comes from Archivo's width and weight axes, never from a second display face. Scripts Archivo lacks fall back to the system's face, without condensing, tracking or uppercase.

## Layout

A centred wrap of 1240px (880px for narrow reading) with a fluid gutter (clamp(16px, 4vw, 40px)); sections breathe at clamp(72px, 10vw, 128px), and consecutive sections drop their top padding. Section intros sit on the board, max 760px wide; their content sits on sheets below. Two-column sections split about 5:7 or 5:6 (text, then sheet).

The home's first viewport is the parcel: the real window at full content width, the waybill stuck across its lower end corner (its start edge on a column boundary so no clipped value peeks out), hanging a little past the edge. Under 980px everything becomes one column and the waybill moves below the window, overlapping its foot; the nav collapses to a menu under 1100px; under 720px field grids stack (the waybill keeps two columns) and segmented tabs stack vertically.

Help pages are one sheet with a sticky 250px nav column and the prose beside it; the nav folds into two, then one, column above the prose on narrow screens. Charts, screenshots and code stay left to right in RTL pages; everything else mirrors.

## Elevation & Depth

Two physical layers: the board, and paper laid on it. Sheets cast a short contact line plus a soft, board-tinted drop; labels that are stuck on (the waybill, menus, dialogs, screenshots) lift higher. Screenshots inside prose take a lighter contact shadow; nothing else has one. Rules and hairlines do the separating inside a sheet. In the navy depot the shadows turn to plain black.

### Shadow Vocabulary
- **Sheet** (`box-shadow: 0 1px 0 rgba(6, 22, 60, .14), 0 16px 32px -14px rgba(4, 18, 52, .5)`): every sheet lying on the board.
- **Lift** (`box-shadow: 0 2px 0 rgba(6, 22, 60, .16), 0 28px 48px -18px rgba(3, 14, 44, .6)`): the waybill, screenshots, menus, the language dialog, the language offer.
- **Sticker edge** (`box-shadow: inset 0 -3px 0 rgba(0, 0, 0, .18), 0 0 0 1.5px rgba(4, 32, 10, .55)`): the Download button only; a pressed-in lower edge and a dark rim, not a drop.

### Named Rules
**The Paper On Board Rule.** Depth means paper on the board. A surface either lies (Sheet) or is stuck on (Lift); there is no third height and no glow.

## Shapes

Gently rounded, like die-cut label stock: sheets 10px, stuck-on labels and dialogs 12px, controls and screenshots 8px, rows and stamps 6px, tags 4px, flags 2px. Rules carry the structure: 1.5px ink rules between fields and around segmented controls, 1px hairlines between quiet rows, a 2px dashed tear line above the barcode. The hero's label card is the one object with depth, and its depth is all in the shadow under it (a contact shadow and a long soft one, `--shadow-card`, neutral black in dark): it sits flat, turned 2deg, on the window's start corner (the owner's choice), in the same grid cell as the window so the section always clears it. The closing label sits turned 1.2deg; both mirror in RTL; stamps tilt 4-6deg. Everything else is square to the grid. Sections sit one rhythm apart (`--gap-section`, 56-96px): each carries the gap above it only, so gaps never stack, and the last block before the footer carries it below; an intro set beside its content has no bottom margin.

## Components

### Buttons
Tactile and few. One green sticker, everything else drawn in line.
- **Shape:** gently rounded (8px), 46px tall (40px in the header, 38px in the language offer).
- **Long labels:** a label wider than its space wraps to two lines inside the button's height; only the header's Download stays on one line.
- **Primary (Download):** Dispatch Green with Go Ink text, 700 weight at width 92%, 20px side padding, the sticker edge. A split variant keeps the system's download on the left and the other systems under an arrow that opens a sheet menu.
- **Hover / Focus:** hover darkens to Go Hover; press moves 1px down; focus is a 3px Label Link Blue ring offset 2px.
- **Line:** transparent with a 1.5px inset ring in the current text colour; hover tints 10% of that colour. Works on both board and sheet.
- **Icon button:** 40px square, transparent, 12% tint on hover.

### Chips (tags)
- **Style:** 4px corners, 11.5px uppercase caps at 0.06em, 1.5px ink ring on paper.
- **State:** Stable is filled ink with paper text; Recommended and Experimental are outlined.

### Cards / Containers (sheets)
- **Corner Style:** 10px; 12px for stuck-on labels.
- **Background:** Thermal Paper, ink text.
- **Shadow Strategy:** Sheet when lying, Lift when stuck on (see Elevation).
- **Border:** none outside; inside, 1.5px ink rules or 1px hairlines between rows.
- **Internal Padding:** clamp(24px, 4vw, 56px) for reading sheets; clamp(18px, 3vw, 32px) for rows.
- **Labelled sheet:** a sheet may open with an ink band: what it is on the start side (17px, 800, width 80%), its fine print on the end side.

### Inputs / Fields
The site has no form inputs. Its "fields" are the label's data grid: two columns, field name over value, a 1.5px ink rule above each and between the pair; one column on phones.

### Navigation
- **Header:** the board, 68px (60px on phones), a 1px Board Edge hairline under it, sticky. Links 15px/600 on-board, 6px rounded 12% tint on hover, the current page underlined 2px at 8px offset. The language button is outlined at 55% of the text colour; the header's green Download is always present.
- **Menu (under 1100px):** a Lift sheet with ink links.
- **Fit:** the header never runs past its row. Where a language's words don't fit, it gives up, in order and only as far as needed: the Sponsor heart, some spacing (16px gaps, 7px link padding), the links for the menu, then the language's name beside its flag.
- **Help nav:** field-name group labels, 14.5px links in ink-2, the current page filled ink with paper text.
- **Footer:** a full ink band with paper text, field-name column heads.

### The Waybill (signature)
The label stuck on the parcel: an ink band with the logo and sender line, the label headline, an ink-2 subline, the green split Download, the other systems in fine print, the fields (postage, delivery time, systems, version and size), and a real Code 128 barcode of the release under a dashed tear line with its text in spaced mono. On first load it feeds out once, top to bottom, like a thermal printer (1.1s, cubic-bezier(.16, 1, .3, 1)); at rest it never moves.

### The Tracking Race (signature)
Two lanes on one shared time axis, one continuous rule each: dashed while the app starts, solid ink while the file comes in (ink-2 for IDM), green once delivered, when a Delivered stamp thunks onto the finish. Events below list time and rate in tabular figures; the ratio grows at display scale in the foot. It plays in real time to the benchmark's measured medians, replays on demand, and shows the finish at once for reduced motion. Scenario tabs are an ink-ruled segmented control; selected is filled ink.

### Rubber Stamp
Stamp Ink caps at width 70%, 900 weight, a 2.5px rule plus a 1.5px outline offset 2px (inked twice round), on paper, set at a slant. Used only for delivered.

### Language Dialog
A Lift sheet, 12px corners, with an ink title band; a grid of flag, native name (isolated with `bdi`) and English name; the current language ringed in ink on Paper Grey. It slides up 14px over .28s on a navy backdrop.

### Coming Sticker (in transit)
What is in the source but not in a download yet: a paper sticker, ink text, a 1.5px **dashed** ink rule just inside its edge (in transit, like the race's dashed lane before the file moves), a clock icon, "Coming with version 0.5" and a fine line naming the current release. Three sizes from one builder (`build.py coming_banner`): on a page (Sheet shadow, on the board), on a section (Paper Grey, inside prose), on a tile (a 12px tag). Never green: nothing is delivered yet. One setting (`UPCOMING`) shows or removes them all.

### Contents (the packing list)
Every feature in one labelled sheet: an ink band ("Contents", the count in fine print), then one row per group, the group's field name in the start column and its features in a ruled grid (1px hairlines between cells, no card per item, no icons). Each cell is a link: the name at 17.5px/750 width 86%, its one line in ink-2, an arrow that moves 3px on hover, the coming tag when it applies. The home page and /features/ show the same sheet.

### Feature Parcel
A feature page's first view repeats the home's memorable moment at a smaller scale: the real window on the board (Lift, natural size, never upscaled) with a small waybill stuck across its lower end corner, tilted 1.6deg (mirrored in RTL), carrying the facts as fields (In the app, Since, Postage: Free, Full guide). Without a screenshot the label stands alone. Under 980px the label sits under the window's foot, overlapping it by 18px.

### Theme Tiles
Paper tiles two to a row, each its screenshot (light or dark: the switch above the grid chooses, the site's theme by default) over a 1.5px ink rule, then the name and its feel. Rows share their heights (subgrid). The last tile is empty: no paper, a 2px dashed outline on the board, a dashed plus, "Make your own theme". The three make-your-own steps are numbered at poster scale (900, width 68%), because their order is the instruction.

### The Crew (contributors)
The people who make MDM possible, from GitHub's public record (`tools/contributors.py`): round photos that link to each GitHub profile, with the login in an ink tag over the photo on hover and focus (kept inside the row by `main.js`). Real photos only, and no counts or claims beyond what GitHub shows.
- **In the hero (the owner's choice: a "liked by" line):** on the board under the window's caption, beside the label (after the label and the caption under 980px). The title "Made possible by these amazing humans" in board text, 800 weight at width 78%; then one row of 44px photos that overlap by 8px, each ringed 2.5px in on-board so it reads on the blue board and the navy depot; then "+N", an on-board disc in board-coloured figures, which opens `/contributors.html`. The row shows 7, 11 or 15 photos by width (`PILE_TIERS`). Hover lifts a photo 3px; reduced motion keeps it still.
- **On `/contributors.html`:** one labelled sheet per group (pull requests, issues, comments), its people in a ruled roster of 1px hairlines: a 48px photo ringed 1.5px in ink, the login at 750 weight, the counts in ink-2 with their numbers in tabular ink.
- **At the end of each release on the News page:** "Thanks to", then 40px photos ringed in ink, row under row, and a fine line naming the release before.

## Do's and Don'ts

### Do:
- **Do** set every page on the board (#1f63d6, navy #0c1a33 in dark) and everything read on paper sheets in ink.
- **Do** keep green for the Download action and delivered states only, with Go Ink text on it.
- **Do** use on-board / on-board-2 for text on the board and ink / ink-2 / ink-3 on sheets; check AA in both themes.
- **Do** present facts as fields: a 12px condensed uppercase name over a 650-weight tabular value, 1.5px ink rules between.
- **Do** show only the real, unretouched app in screenshots, in its day and night versions, left to right even on RTL pages.
- **Do** use logical properties for every direction, mirror tilts in RTL, and drop tracking, uppercase and condensing for Arabic, Persian, Indic, Thai and CJK scripts.
- **Do** keep the barcode a real, scannable Code 128 of what it says.
- **Do** show the other download managers on the comparison page with their own logos, 24px beside their names, unretouched, with the trademark notice and `website/assets/alts/SOURCES.txt`; never IDM's.
- **Do** honour reduced motion: the race shows its finish, the label does not feed, bars and grids sit at rest.

### Don't:
- **Don't** use green on secondary buttons, tags, links, icons or decoration.
- **Don't** put ink-coloured reading text directly on the board, or board text on a sheet.
- **Don't** add a second typeface for display; condense Archivo instead.
- **Don't** set a field name above a section headline as a kicker; field names name fields, table columns and nav groups.
- **Don't** add elevations beyond Sheet, Lift and the Download sticker edge (screenshots inside prose take a lighter contact shadow), and never a glow.
- **Don't** draw a mock window, retouch a screenshot or use IDM artwork; IDM appears only as a named, text-marked comparison.
- **Don't** animate anything on entry except the label's single feed and progress moving into place (the race, the speed-up bars, the connection grids).
