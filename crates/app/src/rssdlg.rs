//! The RSS feeds window (#39; beyond IDM): Tasks → RSS feeds… and the Add Torrent button's
//! arrow. Two tabs: **Feeds** (subscribe, each feed's articles, download one by hand) and
//! **Download rules** (what is added by itself as feeds bring new articles). It edits the app's
//! `rss::Store` in place and says what the app must do (`Act`).

use std::collections::HashSet;

use egui::{Pos2, Ui, pos2};

use crate::dialogs::{Outcome, window};
use crate::rss::{Article, Rule, Store};
use crate::winui::{self, ButtonKind, r, text};

/// What the window asks of the app.
#[derive(Debug, PartialEq, Eq)]
pub enum Act {
    /// The store changed: save it.
    Saved,
    /// Fetch these feeds now (empty: all).
    Refresh(Vec<u64>),
    /// Add torrent with this article's torrent (a magnet link or a .torrent's address).
    Download(String),
}

pub struct RssWindow {
    tab: usize,
    address: String,
    error: Option<String>,
    feeds: winui::RowList,
    articles: winui::RowList,
    rules: winui::RowList,
    rule_feeds: winui::RowList,
    preview: winui::RowList,
    minutes: String,
}

const ROW: f32 = 20.0;

impl RssWindow {
    /// `rules`: open on the Download rules tab.
    pub fn new(store: &Store, rules: bool) -> Self {
        let mut w = RssWindow {
            tab: usize::from(rules),
            address: String::new(),
            error: None,
            feeds: Default::default(),
            articles: Default::default(),
            rules: Default::default(),
            rule_feeds: Default::default(),
            preview: Default::default(),
            minutes: store.minutes.to_string(),
        };
        w.feeds.selected = (!store.feeds.is_empty()).then_some(0);
        w.rules.selected = (!store.rules.is_empty()).then_some(0);
        w
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        store: &mut Store,
        busy: &HashSet<u64>,
    ) -> Outcome<Act> {
        // What the app must do goes out after the window's pass, with the window still open: a
        // dialog's `Done` means it closed, and starts its next opening afresh, its text boxes
        // included (#353: each key typed saved the rule and took the box's focus away).
        let mut asked = None;
        let out = window(
            ctx,
            "rss",
            "RSS feeds",
            [760.0, 500.0],
            true,
            winui::BG(),
            |ui, o| {
                let p = ui.painter().clone();
                let tabs = [
                    // i18n: RSS feeds window: the tab of the feeds and their articles
                    crate::i18n::mark("Feeds"),
                    // i18n: RSS feeds window: the tab of the rules that add articles by themselves
                    crate::i18n::mark("Download rules"),
                ];
                let light: Vec<winui::LightTab> = winui::tab_row(&p, &tabs, 12.0, 748.0, 2.0, 8.0)
                    .into_iter()
                    .zip(tabs.iter())
                    .map(|((x0, x1, _), label)| winui::LightTab {
                        label,
                        sel: (x0 - 2.0, x1 + 2.0),
                        unsel: (x0, x1),
                    })
                    .collect();
                if let Some(i) =
                    winui::tabs_light(ui, o, r(o, 10.0, 8.0, 742.0, 446.0), &light, self.tab)
                {
                    self.tab = i;
                }
                asked = if self.tab == 0 {
                    self.feeds_page(ui, o, store, busy)
                } else {
                    self.rules_page(ui, o, store)
                };
                let close = winui::button(
                    ui,
                    r(o, 663.0, 466.0, 87.0, 21.0),
                    "Close",
                    ButtonKind::Normal,
                    false,
                    None,
                )
                .clicked();
                if close || winui::escape_pressed(ui) {
                    Outcome::<()>::Closed
                } else {
                    Outcome::Open
                }
            },
        );
        match out {
            Outcome::Closed => Outcome::Closed,
            _ => asked.map_or(Outcome::Open, Outcome::Done),
        }
    }

    fn feeds_page(
        &mut self,
        ui: &mut Ui,
        o: Pos2,
        store: &mut Store,
        busy: &HashSet<u64>,
    ) -> Option<Act> {
        let p = ui.painter().clone();
        let mut act = None;
        text(
            &p,
            o.x + 22.0,
            o.y + 57.0,
            // i18n: RSS feeds window: label before the box where a feed's web address is typed
            crate::i18n::t("Feed address:"),
            winui::TEXT(),
        );
        // Add feed as wide as its label in this language (#192); the box ends 10 px before it.
        let (add_x, add_w) =
            winui::buttons_to_right(&p, 740.0, 10.0, &[("Add feed", 120.0)], 200.0)[0];
        winui::edit(
            ui,
            r(o, 140.0, 42.0, add_x - 10.0 - 140.0, 22.0),
            &mut self.address,
            true,
            false,
        );
        let add = winui::button(
            ui,
            r(o, add_x, 42.0, add_w, 22.0),
            // i18n: RSS feeds window: button that subscribes to the feed whose address is typed
            "Add feed",
            ButtonKind::Normal,
            false,
            None,
        )
        .clicked();
        if add && !self.address.trim().is_empty() {
            match store.add_feed(&self.address) {
                Ok(id) => {
                    self.error = None;
                    self.address.clear();
                    self.feeds.selected = store.feeds.iter().position(|f| f.id == id);
                    act = Some(Act::Refresh(vec![id]));
                }
                Err(_) => {
                    self.error = Some(
                        // i18n: RSS feeds window: what was typed as a feed's address isn't an http(s) address
                        crate::i18n::t("A feed's address starts with http:// or https://").into(),
                    );
                }
            }
        }
        // The feeds.
        self.feeds.name = crate::i18n::t("Feeds");
        let names: Vec<String> = store
            .feeds
            .iter()
            .map(|f| {
                let name = f.name().to_owned();
                if busy.contains(&f.id) {
                    format!("{name} …")
                } else {
                    name
                }
            })
            .collect();
        let errors: Vec<bool> = store.feeds.iter().map(|f| f.error.is_some()).collect();
        let ev = winui::row_list(
            ui,
            "rss-feeds",
            r(o, 22.0, 76.0, 230.0, 330.0),
            names.len(),
            ROW,
            &mut self.feeds,
            |ui, clip, i, row, look| {
                look.name = names[i].clone();
                crate::fileicons::paint_at(
                    ui,
                    clip,
                    crate::fileicons::Kind::File("feed.rss"),
                    pos2(row.left() + 4.0, row.top() + 2.0),
                    16.0,
                );
                let ink = if errors[i] && !look.selected {
                    winui::ERROR()
                } else {
                    look.ink
                };
                winui::text_fit(
                    clip,
                    row.left() + 26.0,
                    row.top() + 14.0,
                    &names[i],
                    ink,
                    row.right() - 4.0,
                );
            },
        );
        if ev.picked.is_some() {
            self.articles = Default::default();
        }
        let sel = self.feeds.selected.filter(|i| *i < store.feeds.len());
        let refresh = winui::button(
            ui,
            r(o, 22.0, 414.0, 112.0, 21.0),
            // i18n: RSS feeds window: fetch the selected feed again now
            "Refresh",
            if sel.is_some() {
                ButtonKind::Normal
            } else {
                ButtonKind::Disabled
            },
            false,
            None,
        )
        .clicked();
        if refresh && let Some(i) = sel {
            act = Some(Act::Refresh(vec![store.feeds[i].id]));
        }
        let remove = winui::button(
            ui,
            r(o, 140.0, 414.0, 112.0, 21.0),
            // i18n: RSS feeds window: unsubscribe from the selected feed
            "Remove",
            if sel.is_some() {
                ButtonKind::Normal
            } else {
                ButtonKind::Disabled
            },
            false,
            None,
        )
        .clicked();
        if (remove || ev.delete.is_some())
            && let Some(i) = sel
        {
            let id = store.feeds[i].id;
            store.remove_feed(id);
            self.feeds.selected = (!store.feeds.is_empty()).then(|| i.min(store.feeds.len() - 1));
            self.articles = Default::default();
            act = Some(Act::Saved);
        }
        let sel = self.feeds.selected.filter(|i| *i < store.feeds.len());
        // The selected feed's articles.
        let articles: Vec<Article> = sel
            .map(|i| store.feeds[i].articles.clone())
            .unwrap_or_default();
        self.articles.name = crate::i18n::t("Articles");
        let ev = winui::row_list(
            ui,
            "rss-articles",
            r(o, 264.0, 76.0, 476.0, 330.0),
            articles.len(),
            ROW,
            &mut self.articles,
            |ui, clip, i, row, look| {
                let a = &articles[i];
                look.name = a.title.clone();
                // A torrent's icon when it has one, a page's when not.
                let kind = if a.torrent.is_empty() {
                    "page.html"
                } else {
                    "article.torrent"
                };
                crate::fileicons::paint_at(
                    ui,
                    clip,
                    crate::fileicons::Kind::File(kind),
                    pos2(row.left() + 4.0, row.top() + 2.0),
                    16.0,
                );
                // English coordinates on both sides (text_right's rect is the screen's, mirrored
                // in a right-to-left interface).
                let size = a.size.map(crate::model::size).unwrap_or_default();
                let w = winui::text_width(clip, &size);
                winui::text_right(clip, row.right() - 6.0, row.top() + 14.0, &size, look.ink);
                winui::text_fit(
                    clip,
                    row.left() + 26.0,
                    row.top() + 14.0,
                    &a.title,
                    look.ink,
                    row.right() - 16.0 - w,
                );
            },
        );
        let picked = self.articles.selected.and_then(|i| articles.get(i));
        let can = picked.is_some_and(|a| !a.torrent.is_empty());
        let download = winui::button(
            ui,
            r(o, 628.0, 414.0, 112.0, 21.0),
            // i18n: RSS feeds window: add the selected article's torrent (opens Add torrent)
            "Download...",
            if can {
                ButtonKind::Normal
            } else {
                ButtonKind::Disabled
            },
            false,
            None,
        )
        .clicked();
        let chosen = if download {
            self.articles.selected
        } else {
            ev.double_clicked.or(ev.entered)
        };
        if let Some(a) = chosen
            .and_then(|i| articles.get(i))
            .filter(|a| !a.torrent.is_empty())
        {
            act = Some(Act::Download(a.torrent.clone()));
        }
        // What is selected, or why the feed failed.
        let (line, color) = if let Some(e) = &self.error {
            (e.clone(), winui::ERROR())
        } else if let Some(a) = picked {
            (
                if a.date.is_empty() {
                    a.title.clone()
                } else {
                    format!("{}  ·  {}", a.date, a.title)
                },
                winui::MUTED(),
            )
        } else if let Some(f) = sel.map(|i| &store.feeds[i]) {
            match &f.error {
                Some(e) => (
                    crate::i18n::fmt(
                        // i18n: RSS feeds window: the selected feed's last fetch failed, with why
                        crate::i18n::t("Could not be read: {error}"),
                        &[("error", e)],
                    ),
                    winui::ERROR(),
                ),
                None if f.fetched.is_none() => (
                    // i18n: RSS feeds window: the selected feed hasn't been fetched yet
                    crate::i18n::t("Not fetched yet").into(),
                    winui::MUTED(),
                ),
                None => (
                    crate::i18n::tn(f.articles.len() as u64, "{n} article", "{n} articles"),
                    winui::MUTED(),
                ),
            }
        } else {
            (
                // i18n: RSS feeds window: shown when there are no feeds yet
                crate::i18n::t("Type a feed's address above and click Add feed.").into(),
                winui::MUTED(),
            )
        };
        winui::fit(
            ui,
            &p,
            o.x + 264.0,
            o.y + 429.0,
            &line,
            color,
            o.x + 620.0,
            ("rss", 1),
        );
        // How often feeds are fetched.
        // i18n: RSS feeds window, at the bottom: "Check feeds every [30] minutes"
        let every = crate::i18n::t("Check feeds every");
        text(&p, o.x + 22.0, o.y + 482.0, every, winui::TEXT());
        let x = 22.0 + winui::text_width(&p, every).ceil() + 6.0;
        winui::edit_right(ui, r(o, x, 467.0, 44.0, 22.0), &mut self.minutes, true);
        self.minutes.retain(|c| c.is_ascii_digit());
        self.minutes.truncate(4);
        text(
            &p,
            o.x + x + 50.0,
            o.y + 482.0,
            // i18n: RSS feeds window: the unit after the minutes box
            crate::i18n::t("minutes"),
            winui::TEXT(),
        );
        if let Ok(m) = self.minutes.parse::<u32>()
            && m >= 5
            && m != store.minutes
        {
            store.minutes = m;
            act = act.or(Some(Act::Saved));
        }
        act
    }

    fn rules_page(&mut self, ui: &mut Ui, o: Pos2, store: &mut Store) -> Option<Act> {
        let p = ui.painter().clone();
        let mut act = None;
        self.rules.name = crate::i18n::t("Download rules");
        let names: Vec<(String, bool)> = store
            .rules
            .iter()
            .map(|r| (r.name.clone(), r.enabled))
            .collect();
        let ev = winui::row_list(
            ui,
            "rss-rules",
            r(o, 22.0, 42.0, 200.0, 364.0),
            names.len(),
            ROW,
            &mut self.rules,
            |_ui, clip, i, row, look| {
                look.name = names[i].0.clone();
                let ink = if names[i].1 || look.selected {
                    look.ink
                } else {
                    winui::MUTED()
                };
                winui::text_fit(
                    clip,
                    row.left() + 6.0,
                    row.top() + 14.0,
                    &names[i].0,
                    ink,
                    row.right() - 4.0,
                );
            },
        );
        // As wide as their labels in this language (#192), under the list (it ends at 222) and
        // short of the rule's box beside it (from 240).
        let [new_at, delete_at] = winui::button_row_from_left(
            &p,
            &[("New rule", 22.0, 96.0), ("Delete", 126.0, 96.0)],
            8.0,
            232.0,
            160.0,
        )[..] else {
            unreachable!("two buttons")
        };
        let new = winui::button(
            ui,
            r(o, new_at.0, 414.0, new_at.1, 21.0),
            // i18n: RSS feeds window, Download rules: make a new rule
            "New rule",
            ButtonKind::Normal,
            false,
            None,
        )
        .clicked();
        if new {
            let id = store.new_id();
            // i18n: RSS feeds window: the name a new download rule starts with
            let name = crate::i18n::t("New rule");
            store.rules.push(Rule::new(id, name));
            self.rules.selected = Some(store.rules.len() - 1);
            act = Some(Act::Saved);
        }
        let sel = self.rules.selected.filter(|i| *i < store.rules.len());
        let delete = winui::button(
            ui,
            r(o, delete_at.0, 414.0, delete_at.1, 21.0),
            "Delete",
            if sel.is_some() {
                ButtonKind::Normal
            } else {
                ButtonKind::Disabled
            },
            false,
            None,
        )
        .clicked();
        if (delete || ev.delete.is_some())
            && let Some(i) = sel
        {
            store.rules.remove(i);
            self.rules.selected = (!store.rules.is_empty()).then(|| i.min(store.rules.len() - 1));
            return Some(Act::Saved);
        }
        let Some(i) = sel else {
            text(
                &p,
                o.x + 240.0,
                o.y + 58.0,
                // i18n: RSS feeds window, Download rules: shown when there are no rules yet
                crate::i18n::t(
                    "A rule adds the new articles whose titles it matches, as they come.",
                ),
                winui::MUTED(),
            );
            return act;
        };
        let before = store.rules[i].clone();
        let feeds: Vec<(u64, String)> = store
            .feeds
            .iter()
            .map(|f| (f.id, f.name().to_owned()))
            .collect();
        let rule = &mut store.rules[i];
        winui::checkbox(
            ui,
            pos2(o.x + 240.0, o.y + 46.0),
            &mut rule.enabled,
            // i18n: RSS feeds window, Download rules: the rule is on
            "Use this rule",
            true,
        );
        let label = |ui: &Ui, y: f32, s: &str| {
            winui::fit(
                ui,
                &p,
                o.x + 240.0,
                o.y + y,
                s,
                winui::TEXT(),
                o.x + 372.0,
                ("rss-label", y as i32),
            );
        };
        // i18n: RSS feeds window, Download rules: the rule's name
        label(ui, 88.0, crate::i18n::t("Name:"));
        winui::edit(
            ui,
            r(o, 380.0, 74.0, 360.0, 22.0),
            &mut rule.name,
            true,
            false,
        );
        // i18n: RSS feeds window, Download rules: words the article's title must contain
        label(ui, 116.0, crate::i18n::t("Must contain:"));
        winui::edit(
            ui,
            r(o, 380.0, 102.0, 360.0, 22.0),
            &mut rule.must,
            true,
            false,
        );
        // i18n: RSS feeds window, Download rules: words that make the rule skip an article
        label(ui, 144.0, crate::i18n::t("Must not contain:"));
        winui::edit(
            ui,
            r(o, 380.0, 130.0, 360.0, 22.0),
            &mut rule.must_not,
            true,
            false,
        );
        winui::checkbox(
            ui,
            pos2(o.x + 380.0, o.y + 158.0),
            &mut rule.regex,
            // i18n: RSS feeds window, Download rules: the two boxes above hold regular expressions
            "Regular expressions",
            true,
        );
        // i18n: RSS feeds window, Download rules: which episodes to take (season x episode)
        label(ui, 198.0, crate::i18n::t("Episodes:"));
        winui::edit(
            ui,
            r(o, 380.0, 184.0, 360.0, 22.0),
            &mut rule.episodes,
            true,
            false,
        );
        // The rule's own words, or why they can't be read.
        let problem = match rule.takes("") {
            Err(e) => Some(crate::i18n::fmt(
                // i18n: RSS feeds window, Download rules: a regular expression that can't be read, with why
                crate::i18n::t("Not a regular expression: {error}"),
                &[("error", &e)],
            )),
            Ok(_) if !crate::rss::episodes_ok(&rule.episodes) => Some(
                // i18n: RSS feeds window, Download rules: the episode filter can't be read
                crate::i18n::t("Episodes: like 1x2;1x5-8;2x1- (season x episode)").into(),
            ),
            Ok(_) => None,
        };
        let (hint, color) = match &problem {
            Some(e) => (e.clone(), winui::ERROR()),
            None => (
                // i18n: RSS feeds window, Download rules: under the boxes; | separates alternatives, * and ? are wildcards, 1x2 is season 1 episode 2
                crate::i18n::t("Words: all of them; | between alternatives; * and ?. Episodes: 1x2;1x5-8;2x1-. Each episode once.").into(),
                winui::MUTED(),
            ),
        };
        winui::paragraph(
            &p,
            o.x + 380.0,
            o.y + 222.0,
            &hint,
            color,
            o.x + 740.0,
            14.0,
        );
        // i18n: RSS feeds window, Download rules: the folder the rule's torrents go to
        label(ui, 262.0, crate::i18n::t("Save to:"));
        winui::edit(
            ui,
            r(o, 380.0, 248.0, 330.0, 22.0),
            &mut rule.folder,
            true,
            false,
        );
        if winui::button(
            ui,
            r(o, 716.0, 248.0, 24.0, 22.0),
            "...",
            ButtonKind::Normal,
            false,
            None,
        )
        .clicked()
        {
            let from = rule.folder.clone();
            winui::start_pick(ui.ctx(), "rss-folder", move || {
                rfd::FileDialog::new().set_directory(from).pick_folder()
            });
        }
        if let Some(d) = winui::picked(ui.ctx(), "rss-folder") {
            rule.folder = d.display().to_string();
        }
        if rule.folder.trim().is_empty() {
            winui::text(
                &p,
                o.x + 386.0,
                o.y + 263.0,
                // i18n: RSS feeds window, Download rules: shown in the empty Save to box
                crate::i18n::t("(the Torrents category's folder)"),
                winui::MUTED(),
            );
        }
        winui::checkbox(
            ui,
            pos2(o.x + 380.0, o.y + 278.0),
            &mut rule.later,
            // i18n: RSS feeds window, Download rules: add the torrents to the list without starting them
            "Add them without starting them",
            true,
        );
        // i18n: RSS feeds window, Download rules: the feeds the rule watches
        label(ui, 318.0, crate::i18n::t("Feeds:"));
        self.rule_feeds.name = crate::i18n::t("Feeds:");
        let ticked: Vec<bool> = feeds
            .iter()
            .map(|(id, _)| rule.feeds.contains(id))
            .collect();
        let mut flip = None;
        winui::row_list(
            ui,
            "rss-rule-feeds",
            r(o, 380.0, 304.0, 360.0, 38.0),
            feeds.len(),
            17.0,
            &mut self.rule_feeds,
            |ui, clip, k, row, look| {
                let mut on = ticked[k];
                look.name = feeds[k].1.clone();
                if winui::row_check(
                    ui,
                    clip,
                    pos2(row.left() + 5.0, row.top() + 2.0),
                    &mut on,
                    &feeds[k].1,
                    ("rss-feed", k),
                ) {
                    flip = Some(k);
                }
                winui::text_fit(
                    clip,
                    row.left() + 23.0,
                    row.top() + 12.0,
                    &feeds[k].1,
                    look.ink,
                    row.right() - 4.0,
                );
            },
        );
        if let Some(k) = flip {
            let id = feeds[k].0;
            if let Some(at) = rule.feeds.iter().position(|f| *f == id) {
                rule.feeds.remove(at);
            } else {
                rule.feeds.push(id);
            }
        }
        if rule.feeds.is_empty() {
            winui::text(
                &p,
                o.x + 380.0,
                o.y + 356.0,
                // i18n: RSS feeds window, Download rules: no feed ticked means the rule watches all of them
                crate::i18n::t("None ticked: every feed."),
                winui::MUTED(),
            );
        }
        let changed = *rule != before;
        // What it would take now, among the articles the feeds hold.
        let rule = rule.clone();
        let preview = if problem.is_none() {
            store.preview(&rule)
        } else {
            Vec::new()
        };
        let n = preview.len() as u64;
        // i18n: RSS feeds window, Download rules: how many articles the feeds hold now that the rule matches (it adds only new ones)
        let matches = crate::i18n::tn(
            n,
            "Matches {n} article now (it adds new ones only):",
            "Matches {n} articles now (it adds new ones only):",
        );
        winui::fit(
            ui,
            &p,
            o.x + 240.0,
            o.y + 376.0,
            &matches,
            winui::TEXT(),
            o.x + 740.0,
            ("rss", 2),
        );
        self.preview.name = crate::i18n::t("Articles");
        winui::row_list(
            ui,
            "rss-preview",
            r(o, 240.0, 382.0, 500.0, 53.0),
            preview.len(),
            17.0,
            &mut self.preview,
            |ui, clip, k, row, look| {
                let (feed, a) = &preview[k];
                look.name = a.title.clone();
                crate::fileicons::paint_at(
                    ui,
                    clip,
                    crate::fileicons::Kind::File("article.torrent"),
                    pos2(row.left() + 4.0, row.top()),
                    16.0,
                );
                let w = winui::text_width(clip, feed);
                winui::text_right(
                    clip,
                    row.right() - 6.0,
                    row.top() + 12.0,
                    feed,
                    winui::MUTED(),
                );
                winui::text_fit(
                    clip,
                    row.left() + 24.0,
                    row.top() + 12.0,
                    &a.title,
                    look.ink,
                    row.right() - 16.0 - w,
                );
            },
        );
        // What the rules did last, at the bottom.
        if let Some((t, what)) = store.log.last() {
            let at = crate::clock::local_at(*t)
                .map(|d| d.format("%H:%M").to_string())
                .unwrap_or_default();
            winui::fit(
                ui,
                &p,
                o.x + 22.0,
                o.y + 482.0,
                &format!("{}  {what}", crate::i18n::ltr(&at)),
                winui::MUTED(),
                o.x + 650.0,
                ("rss", 3),
            );
        }
        if changed { Some(Act::Saved) } else { act }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rss::Parsed;
    use egui::{Event, Modifiers, PointerButton, Rect, vec2};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A rule's text box, typed into as one sits at the keyboard: the window opened on Download
    /// rules, the box clicked once, then `typed` a character per frame.
    fn typed_into(y: f32, typed: &str) -> Rule {
        let mut store = Store::default();
        let id = store.new_id();
        store.rules.push(Rule::new(id, "Rule"));
        let mut dlg = RssWindow::new(&store, true);

        // The dialog's window is rendered as eframe renders it: its own pass, its own input.
        let events: Rc<RefCell<Vec<Event>>> = Rc::default();
        {
            let events = events.clone();
            egui::Context::set_immediate_viewport_renderer(move |ctx, viewport| {
                let input = egui::RawInput {
                    viewport_id: viewport.ids.this,
                    viewports: std::iter::once((viewport.ids.this, egui::ViewportInfo::default()))
                        .collect(),
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(760.0, 560.0))),
                    events: std::mem::take(&mut *events.borrow_mut()),
                    ..Default::default()
                };
                let mut ui_cb = viewport.viewport_ui_cb;
                let mut out = ctx.run_ui(input, |ui| ui_cb(ui));
                out.textures_delta.clear();
            });
        }
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        let mut pass = |store: &mut Store, send: Vec<Event>| {
            *events.borrow_mut() = send;
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let _ = dlg.show(ui.ctx(), store, &HashSet::new());
            });
            out.textures_delta.clear();
        };
        for _ in 0..3 {
            pass(&mut store, Vec::new());
        }
        let caption = if crate::dialogs::drawn_captions() {
            crate::winui::caption::CAPTION_H
        } else {
            0.0
        };
        let at = pos2(500.0, caption + y + 11.0);
        let button = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::default(),
        };
        pass(&mut store, vec![Event::PointerMoved(at)]);
        pass(&mut store, vec![button(true)]);
        pass(&mut store, vec![button(false)]);
        pass(&mut store, Vec::new());
        for c in typed.chars() {
            pass(&mut store, vec![Event::Text(c.to_string())]);
            pass(&mut store, Vec::new());
        }
        store.rules.remove(0)
    }

    /// #353: a key typed in a box of the rule editor saves the rule, and saving isn't closing:
    /// the box keeps the keyboard, so the next key lands in it too.
    #[test]
    fn typing_in_the_episodes_box_keeps_its_focus() {
        // A partly typed filter ("1" can be read, "1x" can't) as much as a whole one.
        assert_eq!(typed_into(184.0, "1x2-").episodes, "1x2-");
    }

    #[test]
    fn typing_in_the_words_boxes_keeps_their_focus() {
        assert_eq!(typed_into(102.0, "linux").must, "linux");
        assert_eq!(typed_into(130.0, "cam").must_not, "cam");
    }

    /// #321: removing a feed in the RSS feeds window (including the last one) updates the
    /// selection for the rest of the frame instead of indexing `store.feeds` with the old row.
    #[test]
    fn removing_a_feed_including_the_last_one_does_not_panic() {
        let mut store = Store::default();
        let first = store.add_feed("https://a.example/rss.xml").unwrap();
        let second = store.add_feed("https://b.example/rss.xml").unwrap();
        store.fetched(
            first,
            Parsed {
                title: "First".into(),
                articles: vec![Article {
                    id: "1".into(),
                    title: "One".into(),
                    torrent: "magnet:?xt=urn:btih:1".into(),
                    ..Default::default()
                }],
            },
            100,
        );
        store.fetched(
            second,
            Parsed {
                title: "Second".into(),
                articles: vec![Article {
                    id: "2".into(),
                    title: "Two".into(),
                    torrent: "magnet:?xt=urn:btih:2".into(),
                    ..Default::default()
                }],
            },
            100,
        );

        let ctx = egui::Context::default();
        let mut defs = egui::FontDefinitions::default();
        winui::install_fonts(&mut defs, None);
        ctx.set_fonts(defs);

        let mut win = RssWindow::new(&store, false);
        let busy = HashSet::new();
        let remove_at = pos2(196.0, 424.0);
        let click_remove = |win: &mut RssWindow, store: &mut Store| -> Option<Act> {
            let mut run = |events: Vec<egui::Event>, store: &mut Store| -> Option<Act> {
                let mut act = None;
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        Pos2::ZERO,
                        egui::vec2(760.0, 500.0),
                    )),
                    events,
                    ..Default::default()
                };
                let mut out = ctx.run_ui(input, |ui| {
                    act = win.feeds_page(ui, Pos2::ZERO, store, &busy);
                });
                out.textures_delta.clear();
                act
            };
            run(vec![egui::Event::PointerMoved(remove_at)], store);
            run(
                vec![egui::Event::PointerButton {
                    pos: remove_at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                }],
                store,
            );
            run(
                vec![egui::Event::PointerButton {
                    pos: remove_at,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                store,
            )
        };

        // Removing the first of two feeds selects the remaining one without indexing out of bounds.
        assert_eq!(click_remove(&mut win, &mut store), Some(Act::Saved));
        assert_eq!(store.feeds.len(), 1);
        assert_eq!(store.feeds[0].id, second);
        assert_eq!(win.feeds.selected, Some(0));

        // Removing the last (only) feed leaves no selection and does not panic (#321).
        assert_eq!(click_remove(&mut win, &mut store), Some(Act::Saved));
        assert!(store.feeds.is_empty());
        assert_eq!(win.feeds.selected, None);
    }
}
