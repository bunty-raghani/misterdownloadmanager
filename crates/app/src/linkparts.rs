//! A page's links that came from the browser in parts (`mdm_ipc::LinksPart`): the browser's
//! native host takes at most 1 MB a message (`mdm_ipc::MAX_MESSAGE`), so the extension sends a
//! longer list (a page of 20,000 links) in parts, each with the page's referrer and cookies. They
//! are kept here until the last one is in, then go to Link grabber as one list, in their order,
//! as a short list does.
//!
//! A list whose parts stop coming (the browser closed, a part refused: the extension said so)
//! is dropped after [`WAIT`], and nothing of it is shown: Link grabber gets a page's links whole
//! or not at all.

use std::time::{Duration, Instant};

use mdm_ipc::{Command, LinksPart};

/// How long a list waits for its next part. The extension sends them one after another, each
/// in a moment; the first may start MDM, which the host waits for before handing it over.
pub const WAIT: Duration = Duration::from_secs(60);

/// The most lists waiting at once (two pages' menus clicked together): a new one past it drops
/// the one that waited longest.
const LISTS: usize = 4;

/// The lists still missing parts.
#[derive(Default)]
pub struct LinkParts {
    lists: Vec<List>,
}

/// One list's parts so far.
struct List {
    batch: String,
    /// The first part's command, its links taken out: the page's referrer, cookies, User-Agent,
    /// private window (each part carries the same).
    context: Command,
    /// Each part's links, at its place; None while it hasn't come.
    parts: Vec<Option<Vec<String>>>,
    /// When its last part came.
    at: Instant,
}

/// What became of a part.
#[derive(Debug)]
pub enum Placed {
    /// The list is whole: its command, with every part's links in order.
    Whole(Box<Command>),
    /// Kept: `got` of the list's `count` parts are in.
    Waiting { got: usize, count: usize },
    /// Not a part the app takes ([`LinksPart::valid`]).
    Refused,
}

impl LinkParts {
    /// `cmd`, a part of a list (`links_part`), kept with the others of its list; the whole list
    /// once it is complete. Lists that waited past [`WAIT`] are dropped first (with a line in
    /// the log).
    pub fn add(&mut self, mut cmd: Command, now: Instant) -> Placed {
        self.expire(now);
        let Some(part) = cmd.links_part.take().filter(LinksPart::valid) else {
            return Placed::Refused;
        };
        let count = part.count as usize;
        let at = match self
            .lists
            .iter()
            .position(|l| l.batch == part.batch && l.parts.len() == count)
        {
            Some(at) => at,
            None => {
                if self.lists.len() >= LISTS {
                    let oldest = (0..self.lists.len())
                        .min_by_key(|&k| self.lists[k].at)
                        .unwrap_or(0);
                    let l = self.lists.remove(oldest);
                    tracing::warn!(
                        "browser integration: a page's links dropped, {} of {} parts in (more \
                         pages' links came meanwhile)",
                        l.got(),
                        l.parts.len()
                    );
                }
                self.lists.push(List {
                    batch: part.batch.clone(),
                    context: Command {
                        links: Vec::new(),
                        ..cmd.clone()
                    },
                    parts: vec![None; count],
                    at: now,
                });
                self.lists.len() - 1
            }
        };
        let list = &mut self.lists[at];
        list.parts[part.index as usize] = Some(std::mem::take(&mut cmd.links));
        list.at = now;
        let got = list.got();
        if got < count {
            return Placed::Waiting { got, count };
        }
        let list = self.lists.remove(at);
        let mut whole = list.context;
        whole.links = list.parts.into_iter().flatten().flatten().collect();
        tracing::info!(
            "browser integration: a page's {} links came in {count} parts",
            whole.links.len()
        );
        Placed::Whole(Box::new(whole))
    }

    /// Drop the lists whose next part didn't come within [`WAIT`].
    fn expire(&mut self, now: Instant) {
        self.lists.retain(|l| {
            let waiting = now.saturating_duration_since(l.at) < WAIT;
            if !waiting {
                tracing::warn!(
                    "browser integration: a page's links dropped, {} of {} parts came",
                    l.got(),
                    l.parts.len()
                );
            }
            waiting
        });
    }
}

impl List {
    fn got(&self) -> usize {
        self.parts.iter().filter(|p| p.is_some()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Part `index` of `count` of list `batch`: links `<batch>/<index>-<k>`, from page `page`.
    fn part(batch: &str, index: u32, count: u32, page: &str, n: usize) -> Command {
        Command {
            links: (0..n)
                .map(|k| format!("https://{page}/{batch}/{index}-{k}.zip"))
                .collect(),
            links_part: Some(LinksPart {
                batch: batch.into(),
                index,
                count,
            }),
            referer: Some(format!("https://{page}/")),
            cookies: Some(format!("page={page}")),
            user_agent: Some("UA".into()),
            ..Default::default()
        }
    }

    /// A list still waiting: (parts in, parts in all).
    fn waiting(placed: Placed) -> Option<(usize, usize)> {
        match placed {
            Placed::Waiting { got, count } => Some((got, count)),
            _ => None,
        }
    }

    fn whole(placed: Placed) -> Command {
        match placed {
            Placed::Whole(cmd) => *cmd,
            other => panic!("not whole: {other:?}"),
        }
    }

    #[test]
    fn parts_make_one_list_in_order() {
        let mut parts = LinkParts::default();
        let now = Instant::now();
        // Out of order: the list keeps the parts' order, not their arrival's.
        assert_eq!(
            waiting(parts.add(part("a", 2, 3, "x.test", 2), now)),
            Some((1, 3))
        );
        assert_eq!(
            waiting(parts.add(part("a", 0, 3, "x.test", 3), now)),
            Some((2, 3))
        );
        let cmd = whole(parts.add(part("a", 1, 3, "x.test", 1), now));
        let want: Vec<String> = ["0-0", "0-1", "0-2", "1-0", "2-0", "2-1"]
            .iter()
            .map(|k| format!("https://x.test/a/{k}.zip"))
            .collect();
        assert_eq!(cmd.links, want);
        assert_eq!(cmd.links_part, None, "the app sees one list");
        assert_eq!(cmd.referer.as_deref(), Some("https://x.test/"));
        assert_eq!(cmd.cookies.as_deref(), Some("page=x.test"));
        assert_eq!(cmd.user_agent.as_deref(), Some("UA"));
        assert!(parts.lists.is_empty(), "nothing kept after");
    }

    /// Two pages' menus clicked one after the other, their parts interleaved: two lists, each
    /// with its own page's links, referrer and cookies.
    #[test]
    fn two_pages_interleaved_stay_apart() {
        let mut parts = LinkParts::default();
        let now = Instant::now();
        assert!(matches!(
            parts.add(part("a", 0, 2, "one.test", 2), now),
            Placed::Waiting { .. }
        ));
        assert!(matches!(
            parts.add(part("b", 0, 2, "two.test", 1), now),
            Placed::Waiting { .. }
        ));
        let b = whole(parts.add(part("b", 1, 2, "two.test", 1), now));
        let a = whole(parts.add(part("a", 1, 2, "one.test", 2), now));
        assert_eq!(a.links.len(), 4);
        assert!(a.links.iter().all(|l| l.starts_with("https://one.test/a/")));
        assert_eq!(a.referer.as_deref(), Some("https://one.test/"));
        assert_eq!(b.links.len(), 2);
        assert!(b.links.iter().all(|l| l.starts_with("https://two.test/b/")));
        assert_eq!(b.cookies.as_deref(), Some("page=two.test"));
    }

    /// A list missing a part never shows: it is dropped once it waited too long, and a part
    /// coming after that starts a new list rather than completing the old one.
    #[test]
    fn a_missing_part_drops_the_list() {
        let mut parts = LinkParts::default();
        let start = Instant::now();
        parts.add(part("a", 0, 3, "x.test", 2), start);
        parts.add(part("a", 1, 3, "x.test", 2), start + WAIT / 2);
        // Part 2 never comes; another page's list later drops it.
        parts.add(part("b", 0, 2, "y.test", 1), start + WAIT / 2 + WAIT);
        assert_eq!(parts.lists.len(), 1);
        assert_eq!(parts.lists[0].batch, "b");
        assert_eq!(
            waiting(parts.add(part("a", 2, 3, "x.test", 2), start + WAIT / 2 + WAIT)),
            Some((1, 3)),
            "a late part doesn't complete the dropped list"
        );
        // While it waits less than WAIT between parts, a list may take as long as it needs.
        let mut parts = LinkParts::default();
        for k in 0..9 {
            let got = parts.add(part("c", k, 10, "x.test", 1), start + WAIT / 2 * k);
            assert!(matches!(got, Placed::Waiting { .. }), "{k}");
        }
        assert_eq!(
            whole(parts.add(part("c", 9, 10, "x.test", 1), start + WAIT / 2 * 9))
                .links
                .len(),
            10
        );
    }

    /// The same part twice (sent again) counts once; parts that don't add up are refused; a
    /// list of one part is whole at once; lists past `LISTS` drop the oldest.
    #[test]
    fn odd_parts() {
        let mut parts = LinkParts::default();
        let now = Instant::now();
        parts.add(part("a", 0, 2, "x.test", 2), now);
        assert_eq!(
            waiting(parts.add(part("a", 0, 2, "x.test", 2), now)),
            Some((1, 2))
        );
        assert_eq!(
            whole(parts.add(part("a", 1, 2, "x.test", 1), now))
                .links
                .len(),
            3
        );
        assert!(matches!(
            parts.add(part("a", 2, 2, "x.test", 1), now),
            Placed::Refused
        ));
        assert!(matches!(
            parts.add(part("a b", 0, 2, "x.test", 1), now),
            Placed::Refused
        ));
        let mut plain = part("a", 0, 1, "x.test", 1);
        plain.links_part = None;
        assert!(
            matches!(parts.add(plain, now), Placed::Refused),
            "not a part"
        );
        assert_eq!(
            whole(parts.add(part("one", 0, 1, "x.test", 5), now))
                .links
                .len(),
            5
        );
        // A part with nothing MDM downloads (the host still hands it over) completes its list.
        parts.add(part("e", 0, 2, "x.test", 0), now);
        assert_eq!(
            whole(parts.add(part("e", 1, 2, "x.test", 2), now))
                .links
                .len(),
            2
        );
        for (k, batch) in ["p", "q", "r", "s", "t"].iter().enumerate() {
            parts.add(
                part(batch, 0, 2, "x.test", 1),
                now + Duration::from_secs(k as u64),
            );
        }
        let kept: Vec<&str> = parts.lists.iter().map(|l| l.batch.as_str()).collect();
        assert_eq!(kept, ["q", "r", "s", "t"], "the oldest dropped");
    }
}
