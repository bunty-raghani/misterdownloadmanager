//! What a finished download shows (#766, #741): downloads added together (a playlist, a batch,
//! Link grabber's links) share a group and show one summary once the whole group is done, not a
//! Download complete window each; and Download complete can be snoozed for a while without
//! touching its setting.

use std::collections::BTreeSet;

/// How long Download complete stays away (its window's Snooze menu).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnoozeFor {
    HalfHour,
    Hour,
    EightHours,
    /// Until MDM starts again: never saved.
    Restart,
}

impl SnoozeFor {
    pub const ALL: [SnoozeFor; 4] = [
        SnoozeFor::HalfHour,
        SnoozeFor::Hour,
        SnoozeFor::EightHours,
        SnoozeFor::Restart,
    ];
    /// The Snooze menu's entries, in `ALL`'s order.
    pub const LABELS: [&'static str; 4] = [
        // i18n: Snooze menu of the Download complete window: show no Download complete window for this long
        crate::i18n::mark("For 30 minutes"),
        // i18n: Snooze menu of the Download complete window: show no Download complete window for this long
        crate::i18n::mark("For 1 hour"),
        // i18n: Snooze menu of the Download complete window: show no Download complete window for this long
        crate::i18n::mark("For 8 hours"),
        // i18n: Snooze menu of the Download complete window: show no Download complete window until MDM is started again
        crate::i18n::mark("Until MDM restarts"),
    ];

    /// Its length in seconds; none for `Restart`.
    pub fn seconds(self) -> Option<i64> {
        match self {
            SnoozeFor::HalfHour => Some(30 * 60),
            SnoozeFor::Hour => Some(60 * 60),
            SnoozeFor::EightHours => Some(8 * 60 * 60),
            SnoozeFor::Restart => None,
        }
    }
}

/// Until when a snooze lasts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Until {
    /// Unix seconds (the wall clock, so "until 14:30" holds across the computer's sleep).
    At(i64),
    /// Until MDM restarts.
    Restart,
}

/// Download complete snoozed, or not. The "Show download complete dialog" setting stays as it is;
/// a snooze ends by itself. A timed one is saved (it holds across a restart until its time), one
/// "until MDM restarts" is not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Snooze(Option<Until>);

impl Snooze {
    /// As saved (`Settings::complete_snoozed_until`): over already, none.
    pub fn from_saved(saved: Option<i64>, now: i64) -> Self {
        Snooze(saved.filter(|&t| t > now).map(Until::At))
    }

    pub fn start(&mut self, choice: SnoozeFor, now: i64) {
        self.0 = Some(match choice.seconds() {
            Some(s) => Until::At(now + s),
            None => Until::Restart,
        });
    }

    /// "Resume now" (Options → Downloads).
    pub fn resume(&mut self) {
        self.0 = None;
    }

    pub fn active(&self, now: i64) -> bool {
        match self.0 {
            Some(Until::At(t)) => now < t,
            Some(Until::Restart) => true,
            None => false,
        }
    }

    /// The snooze as it runs now (none once over).
    pub fn until(&self, now: i64) -> Option<Until> {
        self.0.filter(|_| self.active(now))
    }

    /// What is saved: a timed snooze's end; nothing for one until MDM restarts.
    pub fn saved(&self) -> Option<i64> {
        match self.0 {
            Some(Until::At(t)) => Some(t),
            _ => None,
        }
    }

    /// A timed snooze whose time came ends here: true when it just did.
    pub fn expire(&mut self, now: i64) -> bool {
        if self.0.is_some() && !self.active(now) {
            self.0 = None;
            return true;
        }
        false
    }

    /// Seconds until a timed snooze ends (for the next frame to come then).
    pub fn left(&self, now: i64) -> Option<i64> {
        match self.0 {
            Some(Until::At(t)) if t > now => Some(t - now),
            _ => None,
        }
    }
}

/// Where a group's download stands, for its summary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    /// Running, or waiting in a running queue for its turn: the group isn't done.
    Pending,
    Finished,
    Failed,
    /// Stopped, or waiting in a queue that doesn't run: left out of the summary, and it keeps
    /// its group (it is summed up when it runs and its group is done again).
    Idle,
}

/// A group done: its downloads that finished and those that failed, in list order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settled {
    pub group: u64,
    pub finished: Vec<u64>,
    pub failed: Vec<u64>,
}

/// The groups in `open` (a download of theirs finished or failed since their last summary) that
/// are done now: none of their downloads (`members`: id, group, standing) still pending. Those
/// leave `open`; a group whose downloads all went from the list leaves it with nothing to say.
pub fn settle(
    open: &mut BTreeSet<u64>,
    members: impl IntoIterator<Item = (u64, u64, Standing)>,
) -> Vec<Settled> {
    if open.is_empty() {
        return Vec::new();
    }
    let mut busy = BTreeSet::new();
    let mut done: std::collections::BTreeMap<u64, Settled> = open
        .iter()
        .map(|&group| {
            (
                group,
                Settled {
                    group,
                    finished: Vec::new(),
                    failed: Vec::new(),
                },
            )
        })
        .collect();
    for (id, group, standing) in members {
        let Some(s) = done.get_mut(&group) else {
            continue;
        };
        match standing {
            Standing::Pending => {
                busy.insert(group);
            }
            Standing::Finished => s.finished.push(id),
            Standing::Failed => s.failed.push(id),
            Standing::Idle => {}
        }
    }
    let mut out = Vec::new();
    for (group, s) in done {
        if busy.contains(&group) {
            continue;
        }
        open.remove(&group);
        if !s.finished.is_empty() || !s.failed.is_empty() {
            out.push(s);
        }
    }
    out
}

/// What a group done shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    /// One download finished and none failed: the Download complete window, as a single
    /// download has.
    One(u64),
    /// The summary: how many finished, how many failed (those listed).
    Summary(Settled),
}

pub fn shown(s: Settled) -> Shown {
    match (&s.finished[..], &s.failed[..]) {
        ([one], []) => Shown::One(*one),
        _ => Shown::Summary(s),
    }
}

/// The group a set of downloads added together gets: its first download's id (ids are never
/// used again); none for a single download, which keeps its own window.
pub fn group_of(ids: &[u64]) -> Option<u64> {
    (ids.len() > 1).then(|| ids[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use Standing::*;

    fn open(groups: &[u64]) -> BTreeSet<u64> {
        groups.iter().copied().collect()
    }

    #[test]
    fn a_groups_summary_comes_once_after_its_last_download() {
        let mut o = open(&[10]);
        // Three of five finished, the others still running or waiting their turn: nothing yet.
        let mid = [
            (10, 10, Finished),
            (11, 10, Finished),
            (12, 10, Finished),
            (13, 10, Pending),
            (14, 10, Pending),
        ];
        assert!(settle(&mut o, mid).is_empty());
        assert!(o.contains(&10), "still open");
        // The last one in: one summary, of all five.
        let end = [
            (10, 10, Finished),
            (11, 10, Finished),
            (12, 10, Finished),
            (13, 10, Finished),
            (14, 10, Finished),
        ];
        let s = settle(&mut o, end);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].finished, vec![10, 11, 12, 13, 14]);
        assert!(s[0].failed.is_empty());
        // Once: the group left `open`, the next frames say nothing.
        assert!(o.is_empty());
        assert!(settle(&mut o, end).is_empty());
        assert!(matches!(shown(s[0].clone()), Shown::Summary(_)));
    }

    #[test]
    fn a_failure_is_counted() {
        let mut o = open(&[1]);
        let s = settle(&mut o, [(1, 1, Finished), (2, 1, Failed), (3, 1, Finished)]);
        assert_eq!(s[0].finished, vec![1, 3]);
        assert_eq!(s[0].failed, vec![2]);
        // One finished and one failed is still a summary (the failure is listed).
        let mut o = open(&[1]);
        let s = settle(&mut o, [(1, 1, Finished), (2, 1, Failed)]);
        assert!(matches!(shown(s[0].clone()), Shown::Summary(_)));
    }

    #[test]
    fn one_finished_alone_shows_its_own_window() {
        // A group of which only one ran (the others stopped): Download complete, as today.
        let mut o = open(&[5]);
        let s = settle(&mut o, [(5, 5, Finished), (6, 5, Idle), (7, 5, Idle)]);
        assert_eq!(shown(s[0].clone()), Shown::One(5));
        // A download without a group never gets one.
        assert_eq!(group_of(&[42]), None);
        assert_eq!(group_of(&[]), None);
        assert_eq!(group_of(&[7, 8, 9]), Some(7));
    }

    #[test]
    fn groups_are_summed_up_apart() {
        let mut o = open(&[1, 20]);
        let s = settle(
            &mut o,
            [
                (1, 1, Finished),
                (2, 1, Finished),
                (20, 20, Finished),
                (21, 20, Pending),
                // Another group's, not open: ignored.
                (30, 30, Pending),
            ],
        );
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].group, 1);
        assert_eq!(o, open(&[20]));
        // A group whose downloads went from the list: closed, nothing shown.
        let mut o = open(&[9]);
        assert!(settle(&mut o, []).is_empty());
        assert!(o.is_empty());
    }

    #[test]
    fn a_snooze_ends_by_itself() {
        let mut s = Snooze::default();
        assert!(!s.active(1000));
        s.start(SnoozeFor::HalfHour, 1000);
        assert!(s.active(1000));
        assert!(s.active(1000 + 30 * 60 - 1));
        assert_eq!(s.until(1000), Some(Until::At(1000 + 30 * 60)));
        assert_eq!(s.left(1000), Some(30 * 60));
        assert!(!s.expire(1000 + 30 * 60 - 1));
        assert!(!s.active(1000 + 30 * 60));
        assert_eq!(s.until(1000 + 30 * 60), None);
        assert!(s.expire(1000 + 30 * 60), "it ends at its time");
        assert!(!s.expire(1000 + 30 * 60), "once");
        assert_eq!(s, Snooze::default());
        s.start(SnoozeFor::EightHours, 0);
        assert!(s.active(8 * 3600 - 1) && !s.active(8 * 3600));
        s.start(SnoozeFor::Hour, 0);
        assert!(s.active(3599) && !s.active(3600));
        // Resume now.
        s.resume();
        assert!(!s.active(0));
    }

    #[test]
    fn a_restart_keeps_a_timed_snooze_and_ends_one_until_restart() {
        let mut s = Snooze::default();
        s.start(SnoozeFor::Hour, 100);
        // Saved, then read back by the next start before its time: still snoozed.
        let back = Snooze::from_saved(s.saved(), 200);
        assert!(back.active(200));
        assert_eq!(back.until(200), Some(Until::At(3700)));
        // Read back after its time: over.
        assert!(!Snooze::from_saved(s.saved(), 3700).active(3700));
        // Until MDM restarts: never saved, so the next start isn't snoozed.
        s.start(SnoozeFor::Restart, 100);
        assert!(s.active(i64::MAX));
        assert!(!s.expire(i64::MAX));
        assert_eq!(s.saved(), None);
        assert!(!Snooze::from_saved(s.saved(), 200).active(200));
    }
}
