//! Link grabber (#37): the links of a page (the browser's "Download all links"), of a list file
//! (Tasks → Import), copied (Tasks → Add batch download from clipboard, #709), dropped or
//! pasted, gathered into packages to tick before they are added. An address that came on its own
//! and turns out to be a web page is read for its links, which join as that page's (#705).
//! It grew out of the Import window and keeps its ways (IDM's "Download all links": every link
//! looked up for its name and size, Check all / Uncheck all, "Hide web pages", OK adds the
//! ticked ones to the queue); the packages come from JDownloader's link grabber: a split
//! archive's volumes are one package, the other links of one page another, the rest "Various
//! files". A package has its folder, an archive password and "Extract when done".

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use egui::{Pos2, Rect, Ui, pos2, vec2};
use url::Url;

use crate::dialogs::{Outcome, window};
use crate::model::{Category, category_folder};
use crate::transfer::Imported;
use crate::winui::{self, ButtonKind, LIGHT_BG, LIGHT_TEXT, fill_l as fill, r, text};

const ROW: f32 = 18.0;
/// The top of the window's last row of buttons (Add to queue, Start, Cancel).
const BUTTONS_Y: f32 = 446.0;
/// Links looked up at once, and at once from one host.
const CHECKS: usize = 8;
const CHECKS_PER_HOST: usize = 4;
/// Web pages read for their links at once.
const READS: usize = 2;

crate::theme::themed! {
    // The lists' header colour (winui::LIST_HEADER), as Add torrent's file list.
    LIST_HEAD => header;
    LIST_LINE => table_line;
}

/// What looking a link up found.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkState {
    Unchecked,
    Checking,
    Online,
    /// The server said it isn't there (404, 410).
    Offline(u16),
    /// 401 or 403: a login or the page's cookies are needed.
    NeedsLogin,
    /// The server asked to wait (429, 503).
    Busy,
    /// Anything else: what the row says, and the whole sentence, which the cell shows on hover.
    Failed {
        short: String,
        full: String,
    },
}

/// Which package a link goes in: a split archive's volumes together, else the links of one page,
/// else "Various files".
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Origin {
    /// Volumes of one archive on one host (`movie.part1.rar`, `movie.part2.rar`).
    Volumes {
        host: String,
        base: String,
    },
    /// The page the links were on (its host and path).
    Page(String),
    Various,
    /// In the download list already.
    Known,
    /// Packed by its site (Click'n'Load): the site's package name.
    Site(String),
}

impl Origin {
    /// Where the link `url` named `name` (found on page `referer`) belongs.
    pub fn of(url: &Url, name: &str, referer: Option<&str>) -> Origin {
        let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
        if let Some(v) = mdm_extract::volume_key(name) {
            return Origin::Volumes { host, base: v.base };
        }
        if let Some(page) = referer.and_then(|r| Url::parse(r).ok())
            && let Some(h) = page.host_str()
        {
            let path = page.path().trim_end_matches('/');
            return Origin::Page(format!("{}{path}", h.to_ascii_lowercase()));
        }
        Origin::Various
    }

    /// What tells packages apart: volumes of one archive are one set whatever case their names
    /// are in.
    fn key(&self) -> Origin {
        match self {
            Origin::Volumes { host, base } => Origin::Volumes {
                host: host.clone(),
                base: base.to_lowercase(),
            },
            o => o.clone(),
        }
    }

    fn name(&self) -> String {
        match self {
            Origin::Volumes { base, .. } => base.clone(),
            Origin::Page(p) => p.clone(),
            // i18n: Link grabber: the package of links that belong to no other
            Origin::Various => crate::i18n::t("Various files").to_owned(),
            // i18n: Link grabber: the package of links the download list has already
            Origin::Known => crate::i18n::t("Already in the list").to_owned(),
            Origin::Site(name) => name.clone(),
        }
    }
}

/// A web page's name rather than a file's (IDM's "Hide HTML files").
pub fn web_page(name: &str) -> bool {
    // Nothing copied: it runs for every link the list shows, at each frame.
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    [
        "htm", "html", "shtml", "xhtml", "php", "asp", "aspx", "jsp", "cgi",
    ]
    .iter()
    .any(|e| ext.eq_ignore_ascii_case(e))
}

pub struct Package {
    pub name: String,
    /// Where its files go (as typed).
    pub folder: String,
    /// Typed by the user: no longer follows the files' category.
    folder_set: bool,
    pub password: String,
    pub extract: bool,
    extract_set: bool,
    pub delete: bool,
    open: bool,
}

pub struct Link {
    pub found: Imported,
    url: Url,
    pkg: usize,
    pub name: String,
    pub size: Option<u64>,
    pub state: LinkState,
    pub on: bool,
    known: bool,
    /// A magnet link (#709): a torrent, added once its files are known; never looked up here.
    magnet: bool,
    check: Option<Check>,
    /// Its look-up found a web page (`ProbeInfo::web_page`).
    page: bool,
    /// Reading it for its links (#705): asked (`Some`), and its answer once it came.
    read: Option<Read>,
    /// Its files' category, worked out when its name comes or changes, and when the categories
    /// do (#1063: not for each link at each frame).
    category: Category,
}

impl Link {
    /// A web page, as its look-up found or its name says (IDM's "Hide HTML files").
    fn is_page(&self) -> bool {
        self.page || web_page(&self.name)
    }

    /// A web page given on its own (pasted, copied, typed, in a list), not found on a page: one
    /// to read for its links (#705). A page's own links aren't read in turn, and a link a web
    /// page sent (Click'n'Load) never is: reading it is a request the user didn't ask for.
    fn to_read(&self) -> bool {
        self.page
            && self.state == LinkState::Online
            && self.found.referer.is_none()
            && !self.found.from_page
            && self.read.is_none()
            && !self.known
    }

    /// Its files' category.
    fn category(&self) -> Category {
        self.category
    }
}

/// A link's category: a torrent's is Torrents, whatever its name says.
fn category_of(magnet: bool, name: &str) -> Category {
    if magnet {
        Category::Torrents
    } else {
        Category::of(name)
    }
}

/// A magnet link's name: the one it gives (`dn=`), else its torrent's hash.
fn magnet_name(url: &Url) -> String {
    url.query_pairs()
        .find(|(k, _)| k == "dn")
        .map(|(_, v)| v.into_owned())
        .filter(|n| !n.trim().is_empty())
        .or_else(|| crate::transfer::magnet_hash(url.as_str()))
        .unwrap_or_else(|| url.to_string())
}

type Check = Arc<Mutex<Option<Result<mdm_engine::ProbeInfo, LinkState>>>>;
/// A page's links as they were read (`None` until then; empty when it had none or couldn't be
/// read).
type Read = Arc<Mutex<Option<Vec<Imported>>>>;

/// A package's ticked links, as the window hands them over.
pub struct Picked {
    pub name: String,
    pub folder: PathBuf,
    pub password: Option<String>,
    pub extract: bool,
    pub delete: bool,
    /// Each link, the name to save it under (looked up, else the address's), and its size.
    pub links: Vec<(Imported, String, Option<u64>)>,
}

/// What a row of the list is.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Row {
    Pkg(usize),
    Link(usize),
}

/// A package's links that show (not the hidden web pages): how many, how many ticked, and
/// their sizes added up (`None` while one is unknown).
#[derive(Clone, Copy)]
struct Shown {
    links: usize,
    on: usize,
    size: Option<u64>,
}

/// The list as a frame draws it: its rows, and each package's [`Shown`].
struct View {
    rows: Vec<Row>,
    packages: Vec<Shown>,
}

pub struct Grabber {
    pub packages: Vec<Package>,
    pub links: Vec<Link>,
    source: String,
    rows: winui::RowList,
    pub hide_pages: bool,
    /// The default folders (Options → Save to), as category_folder reads them.
    downloads: PathBuf,
    dirs: Vec<(String, String)>,
    /// Extract when done, and Delete archives after extracting, for a new package (Options).
    delete_default: bool,
    /// The package the box under the list edits.
    current: usize,
    /// One of that box's text boxes had the keys (last frame).
    editing: bool,
    /// Each package's place in `packages`, by its origin's key: a long list's links found theirs
    /// by going over every package (#1063).
    origins: std::collections::HashMap<Origin, usize>,
    /// `model::categories_changed` when the links' categories were worked out.
    categories: u64,
}

impl Grabber {
    pub fn new(
        downloads: PathBuf,
        dirs: Vec<(String, String)>,
        hide_pages: bool,
        delete_default: bool,
    ) -> Self {
        Grabber {
            packages: Vec::new(),
            links: Vec::new(),
            source: String::new(),
            rows: Default::default(),
            hide_pages,
            downloads,
            dirs,
            delete_default,
            current: 0,
            editing: false,
            origins: Default::default(),
            categories: crate::model::categories_changed(),
        }
    }

    /// More links, from `source` ("Links from the browser", a file's name): each in its
    /// package; one the download list has (`known`) comes unticked under "Already in the list";
    /// a link here already is not added again.
    pub fn add(&mut self, found: Vec<Imported>, source: String, known: impl Fn(&Url) -> bool) {
        // The addresses already here, looked up once: a list of 10,000 compared each link with
        // every other one, and its packages' defaults were worked out again for each (#805).
        let mut here: std::collections::HashSet<Url> =
            self.links.iter().map(|l| address_key(&l.url)).collect();
        let mut settle = std::collections::BTreeSet::new();
        for d in found {
            let Ok(url) = Url::parse(&d.url) else {
                continue;
            };
            if !here.insert(address_key(&url)) {
                continue;
            }
            let magnet = url.scheme() == "magnet";
            let name = mdm_engine::naming::sanitize(&if magnet {
                magnet_name(&url)
            } else {
                mdm_engine::naming::file_name(None, &url)
            });
            let known = known(&url);
            let origin = if known {
                Origin::Known
            } else if let Some(site) = d.package.as_deref().filter(|p| !p.trim().is_empty()) {
                Origin::Site(site.trim().to_owned())
            } else {
                Origin::of(&url, &name, d.referer.as_deref())
            };
            let pkg = self.package_for(origin);
            // The site's archive password, unless one is typed already.
            if let Some(pw) = d.password.as_deref().filter(|p| !p.is_empty())
                && self.packages[pkg].password.is_empty()
            {
                self.packages[pkg].password = pw.to_owned();
            }
            self.links.push(Link {
                found: d,
                url,
                pkg,
                category: category_of(magnet, &name),
                name,
                size: None,
                state: LinkState::Unchecked,
                on: !known,
                known,
                magnet,
                check: None,
                page: false,
                read: None,
            });
            settle.insert(pkg);
        }
        self.settle(&settle.into_iter().collect::<Vec<_>>());
        if !source.is_empty() && !self.source.split(", ").any(|s| s == source) {
            self.source = if self.source.is_empty() {
                source
            } else {
                format!("{}, {source}", self.source)
            };
        }
    }

    /// `/show grabber`: a page's links as looked up (the docs' screenshots), nothing asked of
    /// the network.
    pub fn sample(&mut self) {
        let page = Some("https://example.com/files/".to_owned());
        let links = [
            (
                "https://dl.example.com/holiday.part1.rar",
                Some(1_610_612_736),
            ),
            (
                "https://dl.example.com/holiday.part2.rar",
                Some(1_610_612_736),
            ),
            ("https://dl.example.com/holiday.part3.rar", None),
            ("https://example.com/files/manual.pdf", Some(2_516_582)),
            ("https://example.com/files/setup.exe", Some(48_234_496)),
            ("https://example.com/files/index.html", Some(18_432)),
        ];
        let found = links
            .iter()
            .map(|(u, _)| Imported {
                url: (*u).to_owned(),
                referer: page.clone(),
                ..Default::default()
            })
            .collect();
        // i18n: in Link grabber, where the addresses came from: a page in the browser
        let from = crate::i18n::t("Links from the browser").to_owned();
        self.add(found, from, |_| false);
        for (l, (_, size)) in self.links.iter_mut().zip(links) {
            l.size = size;
            l.state = if size.is_some() {
                LinkState::Online
            } else {
                l.on = false;
                LinkState::Offline(404)
            };
        }
    }

    fn package_for(&mut self, origin: Origin) -> usize {
        let key = origin.key();
        if let Some(&k) = self.origins.get(&key) {
            return k;
        }
        self.origins.insert(key, self.packages.len());
        self.packages.push(Package {
            name: origin.name(),
            folder: String::new(),
            folder_set: false,
            password: String::new(),
            extract: false,
            extract_set: false,
            delete: self.delete_default,
            open: true,
        });
        self.packages.len() - 1
    }

    /// Packages' defaults after their links changed: each one's folder is its files' category's
    /// when they have one; else none, each file going to its own category's folder (as Import always
    /// did); "Extract when done" when it holds an archive. One pass over the links for all of
    /// `ks` (#1063: a package at a time went over every link for each).
    fn settle(&mut self, ks: &[usize]) {
        /// What a package's links have: the first one's category, whether all share it, an
        /// archive.
        #[derive(Clone, Copy)]
        struct Seen {
            first: Option<Category>,
            one: bool,
            archives: bool,
        }
        let mut seen: Vec<Option<Seen>> = vec![None; self.packages.len()];
        for &k in ks {
            if let Some(s) = seen.get_mut(k) {
                *s = Some(Seen {
                    first: None,
                    one: true,
                    archives: false,
                });
            }
        }
        for l in &self.links {
            let Some(Some(s)) = seen.get_mut(l.pkg) else {
                continue;
            };
            match s.first {
                None => s.first = Some(l.category()),
                Some(c) => s.one &= c == l.category(),
            }
            s.archives = s.archives || (!l.magnet && crate::extractq::archive_name(&l.name));
        }
        for (k, s) in seen.into_iter().enumerate() {
            let Some(s) = s else { continue };
            let first = s.first.unwrap_or(Category::General);
            let folder = if s.one {
                category_folder(first, &self.downloads, &self.dirs)
                    .display()
                    .to_string()
            } else {
                String::new()
            };
            let p = &mut self.packages[k];
            if !p.folder_set {
                p.folder = folder;
            }
            if !p.extract_set {
                p.extract = s.archives;
            }
        }
    }

    /// The categories changed (Options, while the window is open): each link's again, and the
    /// packages' defaults that follow them.
    fn follow_categories(&mut self) {
        let now = crate::model::categories_changed();
        if now == self.categories {
            return;
        }
        self.categories = now;
        for l in &mut self.links {
            l.category = category_of(l.magnet, &l.name);
        }
        let all: Vec<usize> = (0..self.packages.len()).collect();
        self.settle(&all);
    }

    /// Shown (not a hidden web page)?
    fn shows(&self, l: &Link) -> bool {
        !(self.hide_pages && l.is_page())
    }

    /// Each package's links that show, counted in one pass over the links.
    fn shown(&self) -> Vec<Shown> {
        let mut out = vec![
            Shown {
                links: 0,
                on: 0,
                size: Some(0),
            };
            self.packages.len()
        ];
        for l in self.links.iter().filter(|l| self.shows(l)) {
            let s = &mut out[l.pkg];
            s.links += 1;
            s.on += usize::from(l.on);
            s.size = s.size.zip(l.size).map(|(a, b)| a.saturating_add(b));
        }
        out
    }

    /// The rows: each package with links that show, then (open) those links in their order.
    /// Linear in the links (#1063: each package went over every link, twice a frame): the
    /// links are placed by package from the counts, as a counting sort does.
    fn view(&self) -> View {
        let packages = self.shown();
        let mut start = Vec::with_capacity(packages.len());
        let mut total = 0;
        for s in &packages {
            start.push(total);
            total += s.links;
        }
        let mut grouped = vec![0; total];
        let mut next = start.clone();
        for (i, l) in self.links.iter().enumerate() {
            if self.shows(l) {
                grouped[next[l.pkg]] = i;
                next[l.pkg] += 1;
            }
        }
        let mut rows = Vec::with_capacity(packages.len() + total);
        for (k, (p, s)) in self.packages.iter().zip(&packages).enumerate() {
            if s.links == 0 {
                continue;
            }
            rows.push(Row::Pkg(k));
            if p.open {
                let links = &grouped[start[k]..start[k] + s.links];
                rows.extend(links.iter().map(|&i| Row::Link(i)));
            }
        }
        View { rows, packages }
    }

    #[cfg(test)]
    fn visible_rows(&self) -> Vec<Row> {
        self.view().rows
    }

    /// Links to look up now (at most `CHECKS` at once, `CHECKS_PER_HOST` from one host): the
    /// caller starts each ([`Grabber::checking`]) with the link's request.
    pub fn to_check(&self) -> Vec<(usize, Url, Imported)> {
        let busy: Vec<&str> = self
            .links
            .iter()
            .filter(|l| l.state == LinkState::Checking)
            .map(|l| l.url.host_str().unwrap_or_default())
            .collect();
        let mut room = CHECKS.saturating_sub(busy.len());
        let mut hosts = busy.clone();
        let mut out = Vec::new();
        for (i, l) in self.links.iter().enumerate() {
            if room == 0 {
                break;
            }
            let host = l.url.host_str().unwrap_or_default();
            // A link a web page sent (Click'n'Load) is never looked up: the page chose it, and a
            // look-up is a request the user didn't ask for.
            if l.state != LinkState::Unchecked
                || l.known
                || l.found.from_page
                || l.magnet
                || hosts.iter().filter(|h| **h == host).count() >= CHECKS_PER_HOST
            {
                continue;
            }
            hosts.push(host);
            room -= 1;
            out.push((i, l.url.clone(), l.found.clone()));
        }
        out
    }

    /// Link `i` is being looked up; its answer comes in `slot`.
    pub fn checking(&mut self, i: usize, slot: Check) {
        if let Some(l) = self.links.get_mut(i) {
            l.state = LinkState::Checking;
            l.check = Some(slot);
        }
    }

    /// Web pages to read for their links now (#705), at most `READS` at once: the caller reads
    /// each ([`Grabber::reading`]) with the page's request.
    pub fn to_read(&self) -> Vec<(usize, Url, Imported)> {
        let busy = self
            .links
            .iter()
            .filter(|l| l.read.as_ref().is_some_and(|r| r.lock().unwrap().is_none()))
            .count();
        self.links
            .iter()
            .enumerate()
            .filter(|(_, l)| l.to_read())
            .take(READS.saturating_sub(busy))
            .map(|(i, l)| (i, l.url.clone(), l.found.clone()))
            .collect()
    }

    /// Page `i` is being read; its links come in `slot`.
    pub fn reading(&mut self, i: usize, slot: Read) {
        if let Some(l) = self.links.get_mut(i) {
            l.read = Some(slot);
        }
    }

    /// The links of the pages read since the last call, each page's as one list. A page that
    /// had links is unticked: its links were what was wanted, not the page.
    pub fn pages_read(&mut self) -> Vec<Vec<Imported>> {
        let mut out = Vec::new();
        for l in &mut self.links {
            let Some(found) = l.read.as_ref().and_then(|r| r.lock().unwrap().take()) else {
                continue;
            };
            if !found.is_empty() {
                l.on = false;
                out.push(found);
            }
        }
        out
    }

    /// Answers that came: the server's name and size, or why the link is no good.
    fn take_answers(&mut self) {
        let mut moved = Vec::new();
        for l in &mut self.links {
            let Some(slot) = &l.check else { continue };
            let Some(answer) = slot.lock().unwrap().take() else {
                continue;
            };
            l.check = None;
            match answer {
                Ok(info) => {
                    l.state = LinkState::Online;
                    l.page = info.web_page;
                    l.size = info.size;
                    let named = mdm_engine::naming::sanitize(&info.file_name);
                    if !named.is_empty() && named != l.name {
                        l.category = category_of(l.magnet, &named);
                        l.name = named;
                        moved.push(l.pkg);
                    }
                }
                Err(state) => {
                    // Not there: unticked, as IDM leaves a broken link out.
                    if matches!(state, LinkState::Offline(_)) {
                        l.on = false;
                    }
                    l.state = state;
                }
            }
        }
        moved.sort_unstable();
        moved.dedup();
        if !moved.is_empty() {
            self.settle(&moved);
        }
    }

    /// The package `k`'s links shown: all ticked, some, or none.
    fn ticks(&self, k: usize) -> (usize, usize) {
        let shown = self.links.iter().filter(|l| l.pkg == k && self.shows(l));
        shown.fold((0, 0), |(on, n), l| (on + l.on as usize, n + 1))
    }

    fn tick_package(&mut self, k: usize) {
        let (on, n) = self.ticks(k);
        let to = on < n;
        let hide = self.hide_pages;
        for l in self.links.iter_mut().filter(|l| l.pkg == k) {
            if !(hide && l.is_page()) {
                l.on = to;
            }
        }
    }

    fn picked(&self) -> Vec<Picked> {
        // Each package's ticked links, in one pass over the links.
        let mut by_package: Vec<Vec<(Imported, String, Option<u64>)>> =
            std::iter::repeat_with(Vec::new)
                .take(self.packages.len())
                .collect();
        for l in self.links.iter().filter(|l| l.on && self.shows(l)) {
            by_package[l.pkg].push((l.found.clone(), l.name.clone(), l.size));
        }
        self.packages
            .iter()
            .zip(by_package)
            .filter_map(|(p, links)| {
                (!links.is_empty()).then(|| Picked {
                    name: p.name.clone(),
                    folder: PathBuf::from(p.folder.trim()),
                    password: Some(p.password.clone()).filter(|s| !s.is_empty()),
                    extract: p.extract,
                    delete: p.extract && p.delete,
                    links,
                })
            })
            .collect()
    }

    /// `Done((start, packages))`: Start (`true`) or Add to queue (`false`) with the ticked
    /// links of each package.
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<(bool, Vec<Picked>)> {
        self.follow_categories();
        self.take_answers();
        if self.links.iter().any(|l| l.state == LinkState::Checking) {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        let out = window(
            ctx,
            "grabber",
            "Link grabber",
            [640.0, BUTTONS_Y + 30.0],
            true,
            LIGHT_BG(),
            |ui, o| self.body(ui, o),
        );
        match out {
            Outcome::Open => Outcome::Open,
            Outcome::Closed => Outcome::Closed,
            Outcome::Done(start) => Outcome::Done((start, self.picked())),
        }
    }

    fn body(&mut self, ui: &mut Ui, o: Pos2) -> Outcome<bool> {
        let p = ui.painter().clone();
        // Ctrl+V: the addresses in the pasted text, as Import finds them in a file.
        let pasted: Option<String> = ui.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Paste(t) => Some(t.clone()),
                _ => None,
            })
        });
        // Not while a box of the package has the keys: the text goes there.
        if let Some(t) = pasted.filter(|_| !self.editing) {
            let found = crate::transfer::import(&t);
            // i18n: Link grabber: where links came from: the clipboard (pasted with Ctrl+V)
            let from = crate::i18n::t("Pasted").to_owned();
            self.add(found, from, |_| false);
        }
        let line = crate::i18n::fmt(
            // i18n: Link grabber: {source} is where the links came from ("Links from the browser", a file's name, "Pasted")
            crate::i18n::t("Source: {source}"),
            &[("source", &self.source)],
        );
        winui::fit(
            ui,
            &p,
            o.x + 11.0,
            o.y + 20.0,
            &line,
            LIGHT_TEXT(),
            o.x + 629.0,
            "grabber-source",
        );
        self.list(
            ui,
            Rect::from_min_size(o + vec2(11.0, 30.0), vec2(618.0, 258.0)),
        );

        // Under the list: Hide web pages, the count, Check all / Uncheck all.
        let mut hide = self.hide_pages;
        // i18n: Link grabber check box (IDM's "Hide HTML files"): links to web pages are not shown
        winui::checkbox_light(ui, o + vec2(11.0, 299.0), &mut hide, "Hide web pages", true);
        self.hide_pages = hide;
        let (ticked, shown) = self
            .links
            .iter()
            .filter(|l| self.shows(l))
            .fold((0, 0), |(on, n), l| (on + usize::from(l.on), n + 1));
        let count = crate::i18n::fmt(
            crate::i18n::t("{n} of {total} selected"),
            &[
                ("n", &crate::i18n::integer(ticked as u64)),
                ("total", &crate::i18n::integer(shown as u64)),
            ],
        );
        // After the check box's caption, however long it is in this language.
        let after = 11.0 + 18.0 + winui::text_width(&p, crate::i18n::t("Hide web pages")) + 24.0;
        text(&p, o.x + after, o.y + 310.0, &count, LIGHT_TEXT());
        let picks = winui::buttons_light_right(
            ui,
            o,
            629.0,
            296.0,
            &[
                // i18n: Link grabber button (IDM's): tick every link
                (crate::i18n::mark("Check all"), ButtonKind::Normal),
                // i18n: Link grabber button (IDM's): untick every link
                (crate::i18n::mark("Uncheck all"), ButtonKind::Normal),
            ],
        );
        if picks[0].clicked() {
            let hide = self.hide_pages;
            for l in &mut self.links {
                l.on = !(hide && l.is_page());
            }
        }
        if picks[1].clicked() {
            self.links.iter_mut().for_each(|l| l.on = false);
        }

        self.package_box(ui, o);

        let any = ticked > 0;
        let kind = |default: bool| match (any, default) {
            (false, _) => ButtonKind::Disabled,
            (true, true) => ButtonKind::Default,
            (true, false) => ButtonKind::Normal,
        };
        let row = winui::buttons_light_right(
            ui,
            o,
            629.0,
            BUTTONS_Y,
            &[
                // i18n: Link grabber button: add the ticked links to the download queue without starting them (IDM's OK)
                (crate::i18n::mark("Add to queue"), kind(false)),
                // i18n: Link grabber button: download the ticked links now
                (crate::i18n::mark("Start"), kind(true)),
                (crate::i18n::mark("Cancel"), ButtonKind::Normal),
            ],
        );
        let (later, start, cancel) = (row[0].clicked(), row[1].clicked(), row[2].clicked());
        if cancel || winui::escape_pressed(ui) {
            Outcome::Closed
        } else if any && (start || winui::enter_pressed(ui)) {
            Outcome::Done(true)
        } else if any && later {
            Outcome::Done(false)
        } else {
            Outcome::Open
        }
    }

    /// The packages and their links, as a tree: a package's box ticks all its links (mixed
    /// when only some are), its arrow, a double-click, Right / Left open and close it; a link's
    /// name, size and what looking it up found.
    fn list(&mut self, ui: &mut Ui, list: Rect) {
        let p = ui.painter().clone();
        winui::solid(&p, winui::rect(list), winui::FIELD_EDGE());
        let inner = list.shrink(1.0);
        p.rect_filled(winui::rect(inner), 0.0, winui::FIELD());
        let head = Rect::from_min_size(inner.min, vec2(inner.width(), 20.0));
        p.rect_filled(winui::rect(head), 0.0, LIST_HEAD());
        fill(
            &p,
            head.left(),
            head.bottom() - 1.0,
            head.right() - 1.0,
            head.bottom() - 1.0,
            LIST_LINE(),
        );
        // The Status column is as wide as the longest remedy it can show; a failure's whole
        // sentence is the cell's hover tip, so it needn't fit. Never past the room the Name
        // column can spare: a row's name starts about 78 px in, and its right edge leaves 70 px
        // before the Size column.
        const NAME_LEDGE: f32 = 78.0;
        const NAME_FLOOR: f32 = 220.0;
        let widest = REMEDIES
            .iter()
            .map(|source| winui::text_width(&p, crate::i18n::t(source)))
            .fold(0.0_f32, f32::max);
        let room = head.width() - NAME_LEDGE - NAME_FLOOR - 70.0 - 14.0;
        let state_w = (widest + 10.0).clamp(118.0, 260.0).min(room.max(118.0));
        let (size_x, state_x) = (head.right() - state_w - 14.0, head.right() - state_w - 6.0);
        // i18n: column title: file name
        text(
            &p,
            head.left() + 25.0,
            head.top() + 14.0,
            crate::i18n::t("Name"),
            winui::TEXT(),
        );
        // i18n: column title: file size
        winui::text_right(
            &p,
            size_x,
            head.top() + 14.0,
            crate::i18n::t("Size"),
            winui::TEXT(),
        );
        // i18n: column title: Link grabber: what looking the link up found (Online, Offline…)
        text(
            &p,
            state_x,
            head.top() + 14.0,
            crate::i18n::t("Status"),
            winui::TEXT(),
        );
        let body = Rect::from_min_max(pos2(inner.left(), head.bottom()), inner.max);
        if self.links.is_empty() {
            // i18n: Link grabber, while it is empty
            let note = crate::i18n::t(
                "Paste links here (Ctrl+V), or send a page's links from the browser.",
            );
            let w = winui::text_width(&p, note);
            text(
                &p,
                body.center().x - w / 2.0,
                body.center().y + 4.0,
                note,
                winui::MUTED(),
            );
            return;
        }
        let view = self.view();
        let rows = &view.rows;
        // i18n: what a screen reader calls Link grabber's list
        self.rows.name = crate::i18n::t("Links");
        let mut toggle_link = None;
        let mut toggle_pkg = None;
        let mut flip = None;
        let this = &*self;
        let ev = {
            let (packages, links) = (&this.packages, &this.links);
            let mut st = this.rows;
            let ev = winui::row_list(
                ui,
                "grabber-list",
                winui::rect(body.expand(1.0)),
                rows.len(),
                ROW,
                &mut st,
                |ui, clip, k, row, look| {
                    let top = row.top();
                    match rows[k] {
                        Row::Pkg(n) => {
                            let pk = &packages[n];
                            let x = row.left() + 6.0;
                            let arrow = Rect::from_min_size(pos2(x, top), vec2(14.0, ROW));
                            if ui
                                .interact(
                                    winui::rect(arrow),
                                    egui::Id::unique(("grabber-arrow", n)),
                                    winui::pointer_only(),
                                )
                                .clicked()
                            {
                                flip = Some(n);
                            }
                            winui::tree_expander(clip, pos2(x + 4.0, top + 5.0), pk.open);
                            let shown = view.packages[n];
                            let (on_n, all) = (shown.on, shown.links);
                            let glyph = pos2(x + 16.0, top + 2.0);
                            let mut on = on_n == all;
                            // i18n: Link grabber: a package's row: {n} is its number of links
                            let files = crate::i18n::tn(all as u64, "{n} file", "{n} files");
                            look.name = format!("{}, {files}", pk.name);
                            if winui::row_check(
                                ui,
                                clip,
                                glyph,
                                &mut on,
                                &pk.name,
                                ("grabber-pkg", n),
                            ) {
                                toggle_pkg = Some(n);
                            }
                            if on_n > 0 && on_n < all {
                                winui::row_check_mixed(clip, glyph);
                            }
                            crate::fileicons::paint_at(
                                ui,
                                clip,
                                crate::fileicons::Kind::Folder,
                                pos2(glyph.x + 19.0, top + 1.0),
                                16.0,
                            );
                            let left = glyph.x + 19.0 + crate::fileicons::INDENT;
                            let name = crate::i18n::isolate(&pk.name);
                            // The name's English extent (fit's rect is on screen: mirrored in RTL).
                            let shown_w =
                                winui::text_width(clip, &name).min(state_x - 140.0 - left);
                            winui::fit(
                                ui,
                                clip,
                                left,
                                top + 13.0,
                                &name,
                                look.ink,
                                state_x - 140.0,
                                ("grabber-pkg-name", n),
                            );
                            let tint = if look.selected {
                                look.ink
                            } else {
                                winui::MUTED()
                            };
                            let what = match shown.size {
                                Some(b) => format!("({files}, {})", crate::dialogs::dialog_size(b)),
                                None => format!("({files})"),
                            };
                            winui::fit(
                                ui,
                                clip,
                                left + shown_w + 8.0,
                                top + 13.0,
                                &what,
                                tint,
                                row.right() - 8.0,
                                ("grabber-pkg-what", n),
                            );
                        }
                        Row::Link(i) => {
                            let l = &links[i];
                            let glyph = pos2(row.left() + 40.0, top + 2.0);
                            let mut on = l.on;
                            let size = l.size.map(crate::dialogs::dialog_size).unwrap_or_default();
                            let state = if l.found.from_page && l.state == LinkState::Unchecked {
                                // i18n: Link grabber, a link's status: a web page sent it (Click'n'Load), so it isn't looked up (name, size, online) before it is added
                                crate::i18n::t("Not looked up").to_owned()
                            } else if l.magnet && l.state == LinkState::Unchecked {
                                // i18n: Link grabber, a link's status: a magnet link (a torrent, its files looked up once it is added)
                                crate::i18n::t("Magnet link").to_owned()
                            } else {
                                state_text(&l.state)
                            };
                            let state_full = match &l.state {
                                LinkState::Failed { full, .. } => full.as_str(),
                                _ => state.as_str(),
                            };
                            look.name = [l.name.as_str(), &size, state_full]
                                .iter()
                                .filter(|s| !s.is_empty())
                                .copied()
                                .collect::<Vec<_>>()
                                .join(", ");
                            if winui::row_check(
                                ui,
                                clip,
                                glyph,
                                &mut on,
                                &l.name,
                                ("grabber-link", i),
                            ) {
                                toggle_link = Some(i);
                            }
                            crate::fileicons::paint_at(
                                ui,
                                clip,
                                crate::fileicons::Kind::File(&l.name),
                                pos2(glyph.x + 19.0, top + 1.0),
                                16.0,
                            );
                            let left = glyph.x + 19.0 + crate::fileicons::INDENT;
                            let name = crate::i18n::isolate(&l.name);
                            winui::fit(
                                ui,
                                clip,
                                left,
                                top + 13.0,
                                &name,
                                look.ink,
                                size_x - 70.0,
                                ("grabber-name", i),
                            );
                            winui::text_right(clip, size_x, top + 13.0, &size, look.ink);
                            let ink = if look.selected {
                                look.ink
                            } else {
                                match l.state {
                                    LinkState::Online => winui::SUCCESS(),
                                    LinkState::Offline(_) | LinkState::Failed { .. } => {
                                        winui::ERROR()
                                    }
                                    _ => winui::MUTED(),
                                }
                            };
                            winui::fit_whole(
                                ui,
                                clip,
                                state_x,
                                top + 13.0,
                                &state,
                                state_full,
                                ink,
                                row.right() - 6.0,
                                ("grabber-state", i),
                            );
                        }
                    }
                },
            );
            (ev, st)
        };
        let (ev, st) = ev;
        self.rows = st;
        if let Some(k) = ev.space.or(ev.double_clicked) {
            match rows[k] {
                Row::Pkg(n) if ev.double_clicked == Some(k) => flip = Some(n),
                Row::Pkg(n) => toggle_pkg = Some(n),
                Row::Link(i) => toggle_link = Some(i),
            }
        }
        if let Some(k) = self.rows.selected.filter(|k| *k < rows.len()) {
            self.current = match rows[k] {
                Row::Pkg(n) => n,
                Row::Link(i) => self.links[i].pkg,
            };
            if ev.focused
                && let Row::Pkg(n) = rows[k]
            {
                let open = self.packages[n].open;
                if (winui::take_key(ui, egui::Key::ArrowRight) && !open)
                    || (winui::take_key(ui, egui::Key::ArrowLeft) && open)
                {
                    flip = Some(n);
                }
            }
        }
        if ev.focused {
            // A row's name only when a letter was typed (not every row's at every frame).
            let (packages, links) = (&self.packages, &self.links);
            self.rows
                .type_to(ui, "grabber-list", rows.len(), |k| match rows[k] {
                    Row::Pkg(n) => packages[n].name.clone(),
                    Row::Link(i) => links[i].name.clone(),
                });
        }
        if let Some(n) = flip {
            self.packages[n].open = !self.packages[n].open;
        }
        if let Some(n) = toggle_pkg {
            self.tick_package(n);
        }
        if let Some(i) = toggle_link {
            self.links[i].on = !self.links[i].on;
        }
    }

    /// The selected package's name, folder, password and what happens to its archives.
    fn package_box(&mut self, ui: &mut Ui, o: Pos2) {
        let p = ui.painter().clone();
        // A package has a row when some of its links show.
        let shown = self.shown();
        let has = |k: usize| shown.get(k).is_some_and(|s| s.links > 0);
        if !has(self.current) {
            self.current = (0..self.packages.len()).find(|k| has(*k)).unwrap_or(0);
        }
        let enabled = has(self.current);
        // i18n: Link grabber: the box that edits the selected package
        let caption = crate::i18n::t("Package");
        let cw = winui::text_width(&p, caption);
        winui::group_box(
            &p,
            r(o, 11.0, 330.0, 618.0, 106.0),
            Some((o.x + 18.0, o.x + 22.0 + cw)),
        );
        text(&p, o.x + 20.0, o.y + 334.0, caption, LIGHT_TEXT());
        let mut blank = (String::new(), String::new(), String::new(), false, false);
        let pk = self.packages.get_mut(self.current).filter(|_| enabled);
        let (name, folder, password, extract, delete) = match pk {
            Some(pk) => (
                &mut pk.name,
                &mut pk.folder,
                &mut pk.password,
                &mut pk.extract,
                &mut pk.delete,
            ),
            None => (
                &mut blank.0,
                &mut blank.1,
                &mut blank.2,
                &mut blank.3,
                &mut blank.4,
            ),
        };
        let ink = if enabled {
            LIGHT_TEXT()
        } else {
            winui::LIGHT_DISABLED()
        };
        // Labels before their boxes: a column as wide as this language's longest (up to 160 px).
        // i18n: Link grabber: the package's name (a label before its box)
        let name_label = crate::i18n::t("Name:");
        // i18n: Link grabber: the folder the package's files go to (a label before its box)
        let folder_label = crate::i18n::t("Save to:");
        // i18n: Link grabber: the package's archives' password (a label before its box)
        let pw_label = crate::i18n::t("Password:");
        let wide = |s: &str| winui::text_width(&p, s);
        let bx = (21.0 + wide(name_label).max(wide(folder_label)) + 8.0).clamp(80.0, 160.0);
        let label = |ui: &Ui, x: f32, y: f32, s: &str, right: f32| {
            winui::fit(
                ui,
                &p,
                o.x + x,
                o.y + y,
                s,
                ink,
                o.x + right,
                ("grabber-label", y as i32, x as i32),
            );
        };
        // Row 1: the name, then the password.
        label(ui, 21.0, 360.0, name_label, bx - 4.0);
        let pw_x = 340.0;
        let pbx = (pw_x + wide(pw_label) + 8.0).min(470.0);
        let named = winui::edit_light(
            ui,
            r(o, bx, 345.0, pw_x - 12.0 - bx, 23.0),
            name,
            enabled,
            false,
        );
        label(ui, pw_x, 360.0, pw_label, pbx - 4.0);
        let pw = winui::password_light(ui, r(o, pbx, 345.0, 621.0 - pbx, 23.0), password, enabled);
        // i18n: what a screen reader calls Link grabber's password box
        let pw_name = crate::i18n::t("Archive password");
        ui.ctx().accesskit_node_builder(pw.id, |n| {
            n.set_label(pw_name);
            // UI Automation's id: tools tell the one password box apart by it (its name is
            // not read out to them).
            n.set_author_id("grabber-password");
        });
        // Row 2: the folder, and Browse.
        label(ui, 21.0, 391.0, folder_label, bx - 4.0);
        // i18n: verb: choose a folder
        let bw = winui::button_width(&p, "Browse", 73.0);
        let typed = winui::edit_light(
            ui,
            r(o, bx, 376.0, 621.0 - bw - 8.0 - bx, 23.0),
            folder,
            enabled,
            false,
        );
        if enabled && folder.is_empty() && !typed.has_focus() {
            // i18n: Link grabber, in the empty "Save to" box: each file goes to its category's folder (Options → Save to)
            let each = crate::i18n::t("Each file's category folder");
            winui::fit(
                ui,
                &p,
                o.x + bx + 5.0,
                o.y + 391.0,
                each,
                winui::MUTED(),
                o.x + 621.0 - bw - 12.0,
                "grabber-each",
            );
        }
        let browse = winui::button_light(
            ui,
            r(o, 621.0 - bw, 377.0, bw, 21.0),
            "Browse",
            if enabled {
                ButtonKind::Normal
            } else {
                ButtonKind::Disabled
            },
            false,
        )
        .clicked();
        if browse {
            let from = folder.clone();
            winui::start_pick(ui.ctx(), "grabber-folder", move || {
                rfd::FileDialog::new().set_directory(from).pick_folder()
            });
        }
        let mut chose = false;
        if let Some(dir) = winui::picked(ui.ctx(), "grabber-folder") {
            *folder = dir.display().to_string();
            chose = true;
        }
        self.editing = named.has_focus() || typed.has_focus() || pw.has_focus();
        // Row 3: what becomes of its archives.
        let was = *extract;
        // i18n: Link grabber check box: extract the package's archives once they are downloaded
        let extract_label = crate::i18n::mark("Extract when done");
        winui::checkbox_light(ui, o + vec2(21.0, 411.0), extract, extract_label, enabled);
        let extract_now = *extract;
        let next = 21.0 + 18.0 + wide(crate::i18n::t(extract_label)) + 24.0;
        // i18n: Link grabber check box: after extracting, the archives go to the Recycle Bin
        winui::checkbox_light(
            ui,
            o + vec2(next, 411.0),
            delete,
            "Delete archives after extracting",
            enabled && extract_now,
        );
        if enabled {
            let pk = &mut self.packages[self.current];
            pk.folder_set |= typed.changed() || chose;
            pk.extract_set |= was != extract_now;
        }
    }
}

/// A link's address as Link grabber tells links apart: without its `#fragment` (the same file);
/// a magnet link by its torrent's hash alone (its name and trackers aside).
fn address_key(u: &Url) -> Url {
    if let Some(hash) = crate::transfer::magnet_hash(u.as_str())
        && let Ok(key) = Url::parse(&format!("magnet:?xt=urn:btih:{hash}"))
    {
        return key;
    }
    let mut u = u.clone();
    u.set_fragment(None);
    u
}

/// What looking a link up found, as its row says it.
fn state_text(s: &LinkState) -> String {
    match s {
        LinkState::Unchecked => String::new(),
        LinkState::Checking => crate::i18n::t("Checking...").to_owned(),
        // i18n: Link grabber, a link's status: the server has the file
        LinkState::Online => crate::i18n::t("Online").to_owned(),
        LinkState::Offline(code) => crate::i18n::fmt(
            // i18n: Link grabber, a link's status: the server hasn't the file; {status} is the HTTP code (404)
            crate::i18n::t("Offline ({status})"),
            &[("status", &code.to_string())],
        ),
        // i18n: Link grabber, a link's status: the server wants a login (401, 403)
        LinkState::NeedsLogin => crate::i18n::t("Needs a login").to_owned(),
        // i18n: Link grabber, a link's status: the server asked to try later (429, 503)
        LinkState::Busy => crate::i18n::t("Busy, try later").to_owned(),
        LinkState::Failed { short, .. } => short.clone(),
    }
}

/// The words `state_of` puts in the Status column for a failure whose sentence ends in advice,
/// so the column can be as wide as the longest of them (a test keeps the two in step).
const REMEDIES: [&str; 5] = [
    "Alt+click the link",
    "Open in browser",
    "Refresh the address",
    "Download it again",
    "Can't resume",
];

/// A look-up's failure as a link's status.
pub fn state_of(e: &mdm_engine::Error) -> LinkState {
    match e {
        mdm_engine::Error::Status(s) => match s.as_u16() {
            404 | 410 => LinkState::Offline(s.as_u16()),
            401 | 403 => LinkState::NeedsLogin,
            429 | 503 => LinkState::Busy,
            _ => failed(e, None),
        },
        mdm_engine::Error::Throttled(..) => LinkState::Busy,
        // i18n: Link grabber, a link's status: the remedy from the failure's sentence, for a site whose bot check lets only browsers through. The sentence, with the same advice, is the cell's hover tip
        mdm_engine::Error::BotCheck => failed(e, Some(crate::i18n::t("Alt+click the link"))),
        mdm_engine::Error::Refused(mdm_engine::Refusal::NeedsBrowser { .. }) => {
            // i18n: Link grabber, a link's status: the remedy from the failure's sentence, for a site link that is a web page, not the file (and a server that sent a web page instead of it). The sentence is the cell's hover tip
            failed(e, Some(crate::i18n::t("Open in browser")))
        }
        // i18n: Link grabber, a link's status: the remedy from the failure's sentence, for a server that sent a web page instead of the file. The sentence is the cell's hover tip
        mdm_engine::Error::GotWebPage => failed(e, Some(crate::i18n::t("Open in browser"))),
        // i18n: Link grabber, a link's status: the remedy from the failure's sentence, for a download link that expired. The sentence is the cell's hover tip
        mdm_engine::Error::LinkExpired => failed(e, Some(crate::i18n::t("Refresh the address"))),
        mdm_engine::Error::ChecksumMismatch(_) => {
            // i18n: Link grabber, a link's status: the remedy from the failure's sentence, for a file whose checksum didn't match. The sentence is the cell's hover tip
            failed(e, Some(crate::i18n::t("Download it again")))
        }
        // i18n: Link grabber, a link's status: a server that can't send parts of a file, so the download can't resume; there is nothing to do here. The sentence is the cell's hover tip
        mdm_engine::Error::NoRanges => failed(e, Some(crate::i18n::t("Can't resume"))),
        _ => failed(e, None),
    }
}

/// A failure as a link's status: the words the row shows (`short` when the sentence's remedy
/// fits a narrow column, else the sentence), and the whole sentence for the cell's hover tip.
fn failed(e: &mdm_engine::Error, short: Option<&str>) -> LinkState {
    let full = crate::errors::describe(e);
    LinkState::Failed {
        short: short.map_or_else(|| full.clone(), str::to_owned),
        full,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(url: &str, referer: Option<&str>) -> Imported {
        Imported {
            url: url.into(),
            referer: referer.map(str::to_owned),
            ..Default::default()
        }
    }

    fn grabber() -> Grabber {
        Grabber::new(PathBuf::from("D:\\dl"), Vec::new(), true, false)
    }

    #[test]
    fn a_sites_packages_keep_its_names_and_password() {
        let site = |url: &str, pkg: &str, pw: Option<&str>| Imported {
            url: url.into(),
            package: Some(pkg.into()),
            password: pw.map(str::to_owned),
            from_page: true,
            ..Default::default()
        };
        let mut g = grabber();
        g.add(
            vec![
                // Volumes, but the site packed them: its package, not the volumes'.
                site("https://h.org/v.part1.rar", "Holiday", Some("secret")),
                site("https://h.org/v.part2.rar", "Holiday", Some("other")),
                site("https://h.org/notes.txt", "Notes", None),
            ],
            "Click'n'Load".into(),
            |_| false,
        );
        let got: Vec<(&str, &str, usize)> = g
            .packages
            .iter()
            .enumerate()
            .map(|(k, p)| {
                (
                    p.name.as_str(),
                    p.password.as_str(),
                    g.links.iter().filter(|l| l.pkg == k).count(),
                )
            })
            .collect();
        assert_eq!(got, vec![("Holiday", "secret", 2), ("Notes", "", 1)]);
        assert!(g.packages[0].extract);
        // Sent by a page: none is looked up; a link the user brought is.
        assert!(g.to_check().is_empty());
        g.add(
            vec![link("https://h.org/mine.zip", None)],
            String::new(),
            |_| false,
        );
        let check: Vec<String> = g.to_check().iter().map(|(_, u, _)| u.to_string()).collect();
        assert_eq!(check, vec!["https://h.org/mine.zip"]);
    }

    #[test]
    fn a_long_list_is_added_at_once_each_link_once() {
        // #805: 20,000 links, each compared with every other, held the window's thread.
        let lists = |n: usize| {
            let many: Vec<Imported> = (0..n)
                .map(|i| link(&format!("https://dl.example.com/f/{i}.zip"), None))
                .collect();
            // Added again (and with a #fragment): nothing more.
            let again: Vec<Imported> = many
                .iter()
                .map(|d| link(&format!("{}#again", d.url), None))
                .collect();
            (grabber(), many, again)
        };
        let add = |(mut g, many, again): (Grabber, Vec<Imported>, Vec<Imported>)| {
            g.add(many, "list.txt".into(), |_| false);
            g.add(again, "list.txt".into(), |_| false);
            g
        };
        // Each link compared once: a list 16 times as long costs about as much as 16 short
        // lists (a square: 16 times as much), on any machine.
        mdm_labserver::growth::assert_linear(1_000, &lists, &add);
        let g = add(lists(20_000));
        assert_eq!(g.links.len(), 20_000);
        // Settled as before: one package of archives, to extract.
        assert_eq!(g.packages.len(), 1);
        assert!(g.packages[0].extract);
    }

    #[test]
    fn packages_by_volumes_then_page_then_various() {
        let page = Some("https://site.org/files/?sort=1");
        let mut g = grabber();
        g.add(
            vec![
                link("https://cdn.site.org/Movie.2024.part1.rar", page),
                link("https://cdn.site.org/movie.2024.PART2.rar", page),
                link("https://cdn.site.org/movie.2024.part3.rar#x", page),
                link("https://site.org/notes.txt", page),
                link("https://site.org/setup.exe", page),
                link("https://other.org/x.7z.001", None),
                link("https://other.org/x.7z.002", None),
                link("https://other.org/alone.zip", None),
                link("https://site.org/a.zip", None),
                // Again: added once.
                link("https://cdn.site.org/Movie.2024.part1.rar#again", page),
                link("https://known.org/k.zip", page),
            ],
            "Links from the browser".into(),
            |u| u.host_str() == Some("known.org"),
        );
        let names: Vec<(&str, usize)> = g
            .packages
            .iter()
            .enumerate()
            .map(|(k, p)| {
                (
                    p.name.as_str(),
                    g.links.iter().filter(|l| l.pkg == k).count(),
                )
            })
            .collect();
        assert_eq!(
            names,
            [
                ("Movie.2024", 3),
                ("site.org/files", 2),
                ("x", 2),
                ("Various files", 2),
                ("Already in the list", 1),
            ]
        );
        assert!(g.packages[0].extract, "a package of archives is extracted");
        assert!(!g.packages[1].extract);
        assert!(g.packages[3].extract, "Various holds two zips");
        let known = g.links.iter().find(|l| l.known).unwrap();
        assert!(!known.on);
        // The ticked links, each package to its folder.
        let picked = g.picked();
        assert_eq!(picked.len(), 4);
        assert_eq!(picked[0].links.len(), 3);
    }

    /// IDM's export file through Tasks → Import: every download keeps its referrer, cookies
    /// and User-Agent, and a list of several kinds of files goes each to its category's folder
    /// (the package has no folder of its own), as Import always did.
    #[test]
    fn an_idm_export_file_keeps_its_request_context() {
        let ef2 = "<\r\nhttps://files.example.org/setup.exe\r\nreferer: https://example.org/get\r\n\
                   cookie: sid=42; theme=dark\r\nUser-Agent: Mozilla/5.0 Test\r\n>\r\n\
                   <\r\nhttps://files.example.org/manual.pdf\r\nreferer: https://example.org/get\r\n\
                   cookie: sid=42\r\n>\r\n<\r\nhttps://other.org/song.mp3\r\n>\r\n";
        let found = crate::transfer::import(ef2);
        assert_eq!(found.len(), 3);
        let mut g = grabber();
        g.add(found, "list.ef2".into(), |_| false);
        let picked = g.picked();
        let all: Vec<&(Imported, String, Option<u64>)> =
            picked.iter().flat_map(|p| &p.links).collect();
        assert_eq!(all.len(), 3);
        let setup = &all.iter().find(|l| l.1 == "setup.exe").unwrap().0;
        assert_eq!(setup.referer.as_deref(), Some("https://example.org/get"));
        assert_eq!(setup.cookies.as_deref(), Some("sid=42; theme=dark"));
        assert_eq!(setup.user_agent.as_deref(), Some("Mozilla/5.0 Test"));
        let manual = &all.iter().find(|l| l.1 == "manual.pdf").unwrap().0;
        assert_eq!(manual.cookies.as_deref(), Some("sid=42"));
        // The page's package (a program and a document): no folder, each to its category's.
        let page = picked.iter().find(|p| p.name == "example.org/get").unwrap();
        assert_eq!(page.folder, PathBuf::new());
        assert!(!page.extract);
        // One kind of file: its category's folder.
        let various = picked.iter().find(|p| p.links[0].1 == "song.mp3").unwrap();
        assert_ne!(various.folder, PathBuf::new());
    }

    #[test]
    fn same_name_on_two_hosts_is_two_packages() {
        let a = Origin::of(
            &"https://a.org/x.part1.rar".parse().unwrap(),
            "x.part1.rar",
            None,
        );
        let b = Origin::of(
            &"https://b.org/x.part2.rar".parse().unwrap(),
            "x.part2.rar",
            None,
        );
        assert_ne!(a.key(), b.key());
        let c = Origin::of(
            &"https://A.org/X.part2.rar".parse().unwrap(),
            "X.part2.rar",
            None,
        );
        assert_eq!(a.key(), c.key());
    }

    #[test]
    fn a_link_a_page_sent_is_never_read_for_its_links() {
        // A web page's address sent through Click'n'Load: not looked up, and not read either,
        // even if it ever came to be known as a web page (here: answered as one by hand).
        let mut g = grabber();
        g.add(
            vec![Imported {
                url: "https://example.com/water/epanet".into(),
                from_page: true,
                ..Default::default()
            }],
            "Click'n'Load".into(),
            |_| false,
        );
        assert!(g.to_check().is_empty());
        let slot: Check = Default::default();
        *slot.lock().unwrap() = Some(Ok(mdm_engine::ProbeInfo {
            final_url: g.links[0].url.clone(),
            size: None,
            file_name: "epanet.html".into(),
            server: None,
            web_page: true,
        }));
        g.checking(0, slot);
        g.take_answers();
        assert_eq!(g.links[0].state, LinkState::Online);
        assert!(g.to_read().is_empty());
    }

    #[test]
    fn a_page_given_on_its_own_is_read_for_its_links() {
        let mut g = grabber();
        let page = "https://example.com/water/epanet";
        let found_on = "https://example.com/files/setup.exe";
        g.add(
            vec![
                link(page, None),
                link(found_on, Some("https://example.com/other")),
            ],
            "Pasted".into(),
            |_| false,
        );
        assert!(
            g.to_read().is_empty(),
            "nothing is read before it is looked up"
        );
        // The look-ups: the address is a web page, and so is the link found on a page.
        for (i, url, _) in g.to_check() {
            let slot: Check = Default::default();
            *slot.lock().unwrap() = Some(Ok(mdm_engine::ProbeInfo {
                final_url: url.clone(),
                size: None,
                file_name: "epanet.html".into(),
                server: None,
                web_page: true,
            }));
            g.checking(i, slot);
        }
        g.take_answers();
        // Hidden as a web page, though its address doesn't end in one.
        assert!(g.links.iter().all(|l| !g.shows(l)));
        // Only the page given on its own is read, once.
        let reads = g.to_read();
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].1.as_str(), page);
        let slot: Read = Default::default();
        g.reading(reads[0].0, slot.clone());
        assert!(g.to_read().is_empty());
        assert!(g.pages_read().is_empty(), "nothing came yet");
        let on_page = |u: &str| Imported {
            url: u.into(),
            referer: Some(page.into()),
            ..Default::default()
        };
        *slot.lock().unwrap() = Some(vec![
            on_page("https://dl.example.org/epanet2.2_setup.exe"),
            on_page("https://dl.example.org/epanet2.2.zip"),
        ]);
        let read = g.pages_read();
        assert_eq!(read.len(), 1);
        g.add(read.into_iter().flatten().collect(), String::new(), |_| {
            false
        });
        assert!(g.pages_read().is_empty() && g.to_read().is_empty());
        // The page's links, as one package named after the page; the page itself unticked.
        let shown: Vec<&str> = g
            .links
            .iter()
            .filter(|l| g.shows(l))
            .map(|l| l.name.as_str())
            .collect();
        assert_eq!(shown, ["epanet2.2_setup.exe", "epanet2.2.zip"]);
        let page_link = g.links.iter().find(|l| l.url.as_str() == page).unwrap();
        assert!(!page_link.on);
        let pkg = g
            .links
            .iter()
            .find(|l| l.name == "epanet2.2.zip")
            .unwrap()
            .pkg;
        assert!(g.links.iter().filter(|l| l.pkg == pkg).count() == 2);
        assert!(g.source.contains("Pasted"));
    }

    #[test]
    fn web_pages_hide_and_stay_out() {
        let mut g = grabber();
        g.add(
            vec![
                link("https://s.org/index.html", Some("https://s.org/")),
                link("https://s.org/a.zip", Some("https://s.org/")),
            ],
            String::new(),
            |_| false,
        );
        assert!(web_page("index.HTML") && !web_page("a.zip") && web_page("view.php"));
        assert_eq!(g.picked()[0].links.len(), 1);
        g.hide_pages = false;
        assert_eq!(g.picked()[0].links.len(), 2);
        assert_eq!(g.packages[0].name, "s.org");
    }

    #[test]
    fn checks_are_limited() {
        let mut g = grabber();
        let many: Vec<Imported> = (0..10)
            .map(|k| link(&format!("https://one.org/f{k}.bin"), None))
            .chain((0..10).map(|k| link(&format!("https://h{k}.org/f.bin"), None)))
            .collect();
        g.add(many, String::new(), |_| false);
        let first = g.to_check();
        assert_eq!(first.len(), CHECKS);
        assert_eq!(
            first
                .iter()
                .filter(|(_, u, _)| u.host_str() == Some("one.org"))
                .count(),
            CHECKS_PER_HOST
        );
        for (i, _, _) in first {
            g.checking(i, Arc::default());
        }
        assert!(g.to_check().is_empty());
    }

    /// #709, Tasks → Add batch download from clipboard, as the window takes it: the clipboard
    /// read off the window's thread, its 15 links (written twice, among other text) and a
    /// magnet link become 16 ticked rows in two packages, the magnet one never looked up;
    /// Start hands them all over.
    #[test]
    fn copied_links_become_ticked_rows() {
        let mut post = String::from("Season 1, all episodes:\n");
        for k in 1..=15 {
            post.push_str(&format!(
                "- [Episode {k}](https://media.example.org/s1/ep{k:02}.mp4)\n"
            ));
        }
        post.push_str("Mirror of the first: https://media.example.org/s1/ep01.mp4.\n");
        post.push_str("Or the torrent: magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Season%201\n");
        let text = post.clone();
        let reader: &'static crate::clipboard::Reader =
            Box::leak(Box::new(crate::clipboard::Reader::new(
                move || crate::clipboard::Read::Text(text.clone()),
                || {},
            )));
        let mut asked = crate::clipboard::Prefill::ask_of(reader);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let got = loop {
            if let Some(got) = asked.answer() {
                break got;
            }
            assert!(
                std::time::Instant::now() < until,
                "the clipboard never answered"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        let crate::clipbatch::Clip::Links(found) = crate::clipbatch::read(got.as_deref()) else {
            panic!("links expected");
        };
        let mut g = grabber();
        g.add(found, "Clipboard".into(), |_| false);
        assert_eq!(g.links.len(), 16);
        assert!(g.links.iter().all(|l| l.on), "every row ticked");
        let rows = g.visible_rows();
        assert_eq!(rows.len(), 16 + 1, "one package: Various files");
        assert_eq!(g.packages[0].name, "Various files");
        let magnet = g.links.iter().find(|l| l.magnet).unwrap();
        assert_eq!(magnet.name, "Season 1");
        assert_eq!(magnet.category(), Category::Torrents);
        assert!(
            g.to_check().iter().all(|(_, u, _)| u.scheme() == "https"),
            "a magnet link is not looked up over HTTP"
        );
        let picked = g.picked();
        let all: Vec<&str> = picked
            .iter()
            .flat_map(|p| p.links.iter().map(|l| l.0.url.as_str()))
            .collect();
        assert_eq!(all.len(), 16);
        assert_eq!(all[0], "https://media.example.org/s1/ep01.mp4");
        assert!(all[15].starts_with("magnet:?"));
        // The same torrent copied again with its trackers: not a second row.
        g.add(
            vec![link(
                "magnet:?xt=urn:btih:0123456789ABCDEF0123456789ABCDEF01234567&tr=udp%3A%2F%2Ft.example%3A80",
                None,
            )],
            String::new(),
            |_| false,
        );
        assert_eq!(g.links.len(), 16);
    }

    /// A big page's "Download all links" (#1063): `n` links of one page, two in five a split
    /// archive's volumes (two per archive: `n / 5` packages), the rest zips, videos and
    /// documents, some names in capitals.
    fn big_page(n: usize) -> Vec<Imported> {
        let page = Some("https://example.com/big/");
        (0..n)
            .map(|i| match i % 5 {
                0 | 1 => link(
                    &format!("https://cdn.example.com/set{}.part{}.rar", i / 5, i % 5 + 1),
                    page,
                ),
                2 => link(&format!("https://example.com/f/{i}.ZIP"), page),
                3 => link(&format!("https://example.com/v/{i}.mp4"), page),
                _ => link(&format!("https://example.com/d/{i}.pdf"), page),
            })
            .collect()
    }

    /// #1063: a big page's links, a package for each archive's volumes, are added, settled, shown
    /// and handed over in time linear in the links: each package went over every link (when
    /// added, and twice at each frame for the list's rows), so 8 times the links took 64 times
    /// as long.
    #[test]
    fn a_big_pages_links_are_added_and_listed_in_linear_time() {
        mdm_labserver::growth::assert_linear(
            1_000,
            |n| (grabber(), big_page(n)),
            |(mut g, found)| {
                g.add(found, "bench".into(), |_| false);
                let rows = g.view().rows.len();
                let picked = g.picked().len();
                (g, rows, picked)
            },
        );
        let mut g = grabber();
        g.add(big_page(1_000), "bench".into(), |_| false);
        // 200 archives of two volumes each, and the page's other links.
        assert_eq!(g.packages.len(), 201);
        let view = g.view();
        assert_eq!(view.rows.len(), 201 + 1_000);
        assert_eq!(view.rows[0], Row::Pkg(0));
        assert_eq!(&view.rows[1..3], [Row::Link(0), Row::Link(1)]);
        let page = g.links[2].pkg;
        assert_eq!(view.packages[page].links, 600);
        // Each package's rows: its links in their order, as before.
        for (k, p) in g.packages.iter().enumerate() {
            assert!(!p.name.is_empty());
            let at = view.rows.iter().position(|r| *r == Row::Pkg(k)).unwrap();
            let mine: Vec<usize> = (0..g.links.len())
                .filter(|i| g.links[*i].pkg == k)
                .collect();
            let rows: Vec<Row> = mine.iter().map(|&i| Row::Link(i)).collect();
            assert_eq!(view.rows[at + 1..at + 1 + rows.len()], rows[..]);
        }
        assert!(g.packages[0].extract, "an archive's volumes are extracted");
    }

    /// The links' categories are worked out as they come and when their names change; a
    /// package's folder follows its files' category, and the categories' changes too.
    #[test]
    fn a_links_category_is_kept_and_follows_its_name() {
        let mut g = grabber();
        g.add(
            vec![
                link("https://s.org/a.mp4", Some("https://s.org/v")),
                link("https://s.org/b.mp4", Some("https://s.org/v")),
            ],
            String::new(),
            |_| false,
        );
        assert!(g.links.iter().all(|l| l.category() == Category::Video));
        let video = category_folder(Category::Video, &g.downloads, &g.dirs);
        assert_eq!(g.packages[0].folder, video.display().to_string());
        // A look-up names one a zip: the package has two kinds, so no folder of its own.
        let slot: Check = Default::default();
        *slot.lock().unwrap() = Some(Ok(mdm_engine::ProbeInfo {
            final_url: g.links[1].url.clone(),
            size: Some(5),
            file_name: "b.zip".into(),
            server: None,
            web_page: false,
        }));
        g.checking(1, slot);
        g.take_answers();
        assert_eq!(g.links[1].category(), Category::Compressed);
        assert_eq!(g.packages[0].folder, "");
        assert!(g.packages[0].extract);
        // As each name's category, from scratch.
        for l in &g.links {
            assert_eq!(l.category(), Category::of(&l.name));
        }
    }

    /// The list #1063's report pasted, in its shape: 17,154 zips of daily data on one host
    /// (`…/klines/<PAIR>/1m/<PAIR>-1m-<date>.zip`, 47 pairs), one address a line.
    fn pasted_list() -> String {
        let mut text = String::new();
        let mut n = 0;
        'days: for day in 0..400 {
            let (year, month, date) = (2019 + day / 336, day % 336 / 28 + 1, day % 28 + 1);
            for pair in 0..47 {
                if n == 17_154 {
                    break 'days;
                }
                let pair = format!("PAIR{pair}USDT");
                let file = format!("{pair}-1m-{year}-{month:02}-{date:02}.zip");
                text += &format!(
                    "https://data.example.com/data/futures/um/daily/klines/{pair}/1m/{file}\n"
                );
                n += 1;
            }
        }
        text
    }

    /// The timings in #1063's pull request (`cargo test -p mdm-app --release --bin
    /// misterdownloadmanager grabber::tests::timings_1063 -- --ignored --nocapture`): adding the
    /// links (a paste: its text read too), one frame's eight answers that renamed a link, and
    /// a frame of the window, for a big page's links and for the report's pasted list.
    #[test]
    #[ignore = "timings for the record, not a check"]
    fn timings_1063() {
        use std::time::Instant;
        let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
        type Links = Box<dyn Fn() -> Vec<Imported>>;
        let cases: Vec<(&str, Links)> = vec![
            ("page-5000", Box::new(|| big_page(5_000))),
            ("page-20000", Box::new(|| big_page(20_000))),
            (
                "pasted-17154",
                Box::new(|| crate::transfer::import(&pasted_list())),
            ),
            (
                "pasted-and-page",
                Box::new(|| {
                    let mut all = crate::transfer::import(&pasted_list());
                    all.extend(big_page(20_000));
                    all
                }),
            ),
        ];
        for (case, links) in cases {
            let mut g = grabber();
            let t = Instant::now();
            g.add(links(), "bench".into(), |_| false);
            let add = t.elapsed();
            // One frame's answers: eight look-ups that renamed their link.
            for i in (0..g.links.len()).step_by(997).take(CHECKS) {
                let slot: Check = Default::default();
                *slot.lock().unwrap() = Some(Ok(mdm_engine::ProbeInfo {
                    final_url: g.links[i].url.clone(),
                    size: Some(1),
                    file_name: format!("renamed-{i}.mkv"),
                    server: None,
                    web_page: false,
                }));
                g.checking(i, slot);
            }
            let t = Instant::now();
            g.take_answers();
            let answers = t.elapsed();
            // The window's frames, drawn on a context of their own (embedded, no real window).
            let ctx = egui::Context::default();
            let frame = |g: &mut Grabber| {
                let t = Instant::now();
                let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                    // What the app asks of Link grabber at each frame, then its window.
                    let _ = (g.pages_read(), g.to_read(), g.to_check());
                    let _ = g.show(ui.ctx());
                });
                out.textures_delta.clear();
                t.elapsed()
            };
            for _ in 0..3 {
                frame(&mut g);
            }
            let frames: Vec<f64> = (0..5).map(|_| ms(frame(&mut g))).collect();
            let mean = frames.iter().sum::<f64>() / frames.len() as f64;
            println!(
                "TIMING {case} links={} packages={} add={:.1} ms answers(8)={:.1} ms frame={mean:.1} ms",
                g.links.len(),
                g.packages.len(),
                ms(add),
                ms(answers),
            );
        }
    }

    /// #1110: the Status column names the remedy in words that fit, and keeps the whole
    /// sentence, remedy included, as the cell's hover tip.
    #[test]
    fn a_failure_says_its_remedy_and_keeps_its_sentence() {
        let cases = [
            (mdm_engine::Error::BotCheck, "Alt+click the link"),
            (
                mdm_engine::Error::Refused(mdm_engine::Refusal::NeedsBrowser {
                    site: "gofile".into(),
                }),
                "Open in browser",
            ),
            (mdm_engine::Error::GotWebPage, "Open in browser"),
            (mdm_engine::Error::LinkExpired, "Refresh the address"),
            (
                mdm_engine::Error::ChecksumMismatch("SHA-256"),
                "Download it again",
            ),
            (mdm_engine::Error::NoRanges, "Can't resume"),
        ];
        for (e, remedy) in cases {
            let LinkState::Failed { short, full } = state_of(&e) else {
                panic!("{e:?} is not a failure");
            };
            assert_eq!(short, remedy, "{e:?}");
            assert_eq!(full, crate::errors::describe(&e), "{e:?}");
            assert!(short.len() < full.len(), "{e:?}: {short} / {full}");
        }
        // A failure that offers no remedy keeps the sentence it had, cut as before.
        let LinkState::Failed { short, full } = state_of(&mdm_engine::Error::ShortBody) else {
            panic!("ShortBody is not a failure");
        };
        assert_eq!(short, full);
    }

    /// #1110: the column is sized from `REMEDIES`, so it must list exactly the words
    /// `state_of` shows, or a new remedy would be cut again.
    #[test]
    fn the_column_is_sized_for_every_remedy_it_shows() {
        let cases = [
            mdm_engine::Error::BotCheck,
            mdm_engine::Error::Refused(mdm_engine::Refusal::NeedsBrowser {
                site: "gofile".into(),
            }),
            mdm_engine::Error::GotWebPage,
            mdm_engine::Error::LinkExpired,
            mdm_engine::Error::ChecksumMismatch("SHA-256"),
            mdm_engine::Error::NoRanges,
        ];
        let mut shown: Vec<String> = cases
            .iter()
            .map(|e| match state_of(e) {
                LinkState::Failed { short, .. } => short,
                other => panic!("{e:?} is not a failure: {other:?}"),
            })
            .collect();
        let mut listed: Vec<String> = REMEDIES
            .iter()
            .map(|source| crate::i18n::t(source).to_owned())
            .collect();
        shown.sort();
        shown.dedup();
        listed.sort();
        assert_eq!(shown, listed);
    }
}
