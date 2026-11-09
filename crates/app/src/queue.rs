//! Download queues (notes/ENGINE.md §5, IDM's scheduler help): a queue downloads N files at a
//! time in list order; a file that fails moves to the end and is retried 30 s later (or up to
//! a retry limit); finished files leave download queues; a queue with nothing left to start or
//! running stops.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// IDM: "If the file that IDM cannot download is left alone in the queue, IDM will retry …
/// with 30 seconds interval".
pub const RETRY_DELAY: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Schedule {
    pub start_on_startup: bool,
    /// Daily start time (h, m, s) and the weekdays it applies to (Sunday first).
    pub start_at: Option<(u8, u8, u8)>,
    pub days: [bool; 7],
    pub stop_at: Option<(u8, u8, u8)>,
    /// "Number of retries for each file if downloading failed" (None = retry forever).
    pub retries: Option<u32>,
    /// "Once at" this date (year, month, day) instead of "Daily".
    pub once: Option<(i32, u32, u32)>,
    /// When the queue is done: open this file, hang up, exit, turn the computer off.
    pub open_when_done: Option<String>,
    pub hang_up: bool,
    pub exit_when_done: bool,
    pub turn_off: Option<TurnOff>,
    /// With `turn_off`: don't let other programs delay it.
    pub force_close: bool,
    /// "Wake the computer to run this queue" (#142): woken shortly before `start_at`, kept
    /// awake while the queue runs (wake.rs).
    pub wake: bool,
}

/// What happens when a queue, or one download, is done (besides opening a file).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WhenDone {
    pub hang_up: bool,
    pub exit: bool,
    pub turn_off: Option<TurnOff>,
    /// With `turn_off`: don't let other programs delay it.
    pub force: bool,
}

/// "Turn off computer when done" modes, in IDM's combo order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnOff {
    ShutDown,
    Sleep,
    Hibernate,
}

impl Schedule {
    pub fn when_done(&self) -> WhenDone {
        WhenDone {
            hang_up: self.hang_up,
            exit: self.exit_when_done,
            turn_off: self.turn_off,
            force: self.force_close,
        }
    }

    /// Whether the start time applies on this day: "Once at" its date, else "Daily" on the
    /// checked weekdays. `weekday` counts from Sunday = 0.
    pub fn starts_on(&self, date: (i32, u32, u32), weekday: usize) -> bool {
        match self.once {
            Some(d) => d == date,
            None => self.days[weekday % 7],
        }
    }
}

impl TurnOff {
    pub const ALL: [TurnOff; 3] = [TurnOff::ShutDown, TurnOff::Sleep, TurnOff::Hibernate];
    pub const LABELS: [&'static str; 3] = [
        // i18n: what the computer does when the queue is done (verb: shut down the computer)
        crate::i18n::mark("Shut down"),
        // i18n: what the computer does when the queue is done (verb: put it to sleep)
        crate::i18n::mark("Sleep"),
        // i18n: what the computer does when the queue is done (verb: hibernate it)
        crate::i18n::mark("Hibernate"),
    ];
}

impl Default for Schedule {
    fn default() -> Self {
        // IDM's defaults: unchecked, 11:00:00 PM / 7:30:00 AM, all days, 10 retries when enabled.
        Schedule {
            start_on_startup: false,
            start_at: None,
            days: [true; 7],
            stop_at: None,
            retries: None,
            once: None,
            open_when_done: None,
            hang_up: false,
            exit_when_done: false,
            turn_off: None,
            force_close: false,
            wake: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queue {
    pub name: String,
    /// "Download N files at the same time".
    pub files_at_once: u32,
    /// Download ids in queue order.
    pub items: Vec<u64>,
    /// Synchronization queues keep finished files (they are re-checked, not removed).
    pub sync: bool,
    #[serde(default)]
    pub schedule: Schedule,
    #[serde(skip)]
    pub running: bool,
}

impl Queue {
    pub fn new(name: &str, sync: bool) -> Self {
        Queue {
            name: name.into(),
            // "Download 4 files at the same time", as the user's IDM shows (D-012).
            files_at_once: 4,
            items: Vec::new(),
            sync,
            schedule: Schedule::default(),
            running: false,
        }
    }
}

/// What a queue needs to know about one of its downloads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Member {
    /// Not started, or stopped: can start now.
    Waiting,
    /// Stopped by hand while the queue runs: left alone until started again (#341).
    Held,
    Running,
    Complete,
    /// Failed `tries` times, last at `at`.
    Failed {
        at: Instant,
        tries: u32,
    },
}

/// The ids to start now: in queue order, up to `files_at_once` running at once. Failed files
/// wait `RETRY_DELAY` and are skipped after `retries` attempts.
pub fn to_start(q: &Queue, member: impl Fn(u64) -> Member, now: Instant) -> Vec<u64> {
    let running = q
        .items
        .iter()
        .filter(|id| member(**id) == Member::Running)
        .count() as u32;
    let mut free = q.files_at_once.max(1).saturating_sub(running);
    let mut out = Vec::new();
    for id in &q.items {
        if free == 0 {
            break;
        }
        let ready = match member(*id) {
            Member::Waiting => true,
            Member::Failed { at, tries } => {
                q.schedule.retries.is_none_or(|max| tries < max)
                    && now.duration_since(at) >= RETRY_DELAY
            }
            Member::Running | Member::Complete | Member::Held => false,
        };
        if ready {
            out.push(*id);
            free -= 1;
        }
    }
    out
}

/// True when the queue has nothing running and nothing it may still start (now or after a
/// retry delay): processing is finished.
pub fn finished(q: &Queue, member: impl Fn(u64) -> Member) -> bool {
    q.items.iter().all(|id| match member(*id) {
        Member::Complete => true,
        Member::Failed { tries, .. } => q.schedule.retries.is_some_and(|max| tries >= max),
        Member::Waiting | Member::Held | Member::Running => false,
    })
}

/// A failed file goes to the end of the queue.
pub fn requeue_failed(q: &mut Queue, id: u64) {
    if let Some(i) = q.items.iter().position(|x| *x == id) {
        let v = q.items.remove(i);
        q.items.push(v);
    }
}

/// Did the clock pass `t` (h, m, s) between `prev` and `now` (seconds since midnight)?
pub fn crossed(t: (u8, u8, u8), prev: u32, now: u32) -> bool {
    let s = t.0 as u32 * 3600 + t.1 as u32 * 60 + t.2 as u32;
    if prev <= now {
        prev < s && s <= now
    } else {
        s > prev || s <= now
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(n: u32, items: &[u64]) -> Queue {
        Queue {
            files_at_once: n,
            items: items.to_vec(),
            ..Queue::new("Main download queue", false)
        }
    }

    #[test]
    fn starts_in_order_up_to_the_limit() {
        let now = Instant::now();
        let st = |id| {
            if id == 1 {
                Member::Running
            } else {
                Member::Waiting
            }
        };
        assert_eq!(to_start(&q(2, &[1, 2, 3]), st, now), vec![2]);
        assert_eq!(to_start(&q(3, &[1, 2, 3]), st, now), vec![2, 3]);
        assert!(to_start(&q(1, &[1, 2, 3]), st, now).is_empty());
    }

    /// #341: Stop in a running queue is left alone, and the queue isn't over because of it.
    #[test]
    fn a_download_stopped_by_hand_is_not_started_again() {
        let st = |id| {
            if id == 1 {
                Member::Held
            } else {
                Member::Waiting
            }
        };
        let now = Instant::now();
        assert_eq!(to_start(&q(2, &[1, 2]), st, now), vec![2]);
        assert!(to_start(&q(2, &[1]), st, now).is_empty());
        assert!(!finished(&q(1, &[1]), st));
    }

    #[test]
    fn failed_files_wait_and_respect_the_retry_limit() {
        let t0 = Instant::now();
        let failed = |tries| move |_| Member::Failed { at: t0, tries };
        assert!(
            to_start(&q(1, &[7]), failed(1), t0).is_empty(),
            "30 s delay"
        );
        assert_eq!(to_start(&q(1, &[7]), failed(1), t0 + RETRY_DELAY), vec![7]);
        let mut limited = q(1, &[7]);
        limited.schedule.retries = Some(2);
        assert!(to_start(&limited, failed(2), t0 + RETRY_DELAY).is_empty());
        assert!(finished(&limited, failed(2)));
        assert!(
            !finished(&q(1, &[7]), failed(9)),
            "no limit: keeps retrying"
        );
    }

    #[test]
    fn once_at_a_date_or_daily_on_checked_days() {
        let mut s = Schedule::default();
        s.days[0] = false; // not on Sundays
        assert!(!s.starts_on((2026, 11, 1), 0));
        assert!(s.starts_on((2026, 11, 2), 1));
        s.once = Some((2026, 11, 3));
        assert!(!s.starts_on((2026, 11, 2), 1), "only on its date");
        assert!(s.starts_on((2026, 11, 3), 2));
    }

    #[test]
    fn failed_file_moves_to_the_end() {
        let mut qq = q(1, &[1, 2, 3]);
        requeue_failed(&mut qq, 1);
        assert_eq!(qq.items, vec![2, 3, 1]);
    }

    #[test]
    fn schedule_time_crossing() {
        let eleven_pm = (23, 0, 0);
        assert!(crossed(eleven_pm, 82_799, 82_800));
        assert!(!crossed(eleven_pm, 82_800, 82_801));
        assert!(crossed((0, 0, 5), 86_398, 6), "across midnight");
    }
}
