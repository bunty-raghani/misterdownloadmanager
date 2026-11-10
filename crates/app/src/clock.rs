//! Local time for the window thread (#536).
//!
//! On Windows, chrono asks the system for the time zone's rules at every local time it works
//! out (`GetTimeZoneInformationForYear`, with no cache), and the system once took more than 8 s
//! to answer: a frozen window. Here the offset is asked on a thread of its own and kept: the
//! window reads it without ever waiting, and the thread asks again at each new minute (and when
//! a reader finds it a minute old, after the computer slept), so a change of summer time or of
//! the time zone shows within a minute.
//!
//! - [`now`]: the local time now, with the offset kept.
//! - [`local`]: a UTC time (a stored one, for display) with the offset of *now*. A time on the
//!   other side of a summer time change shows an hour off (a log line from before the change),
//!   as a clock read now would show it; nothing that is worked out depends on it.
//! - [`exact_utc`]: a local time (the Scheduler's start) as UTC by the zone's own rules for that
//!   date, worked out on the clock's thread and kept: `None` until it has been worked out.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::Duration;

use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone, Utc};

/// No answer yet (the minute of the last answer otherwise).
const NONE: i64 = i64::MIN;
/// The exact conversions kept at most (a few queues' next starts).
const KEPT: usize = 256;

type OffsetAt = Box<dyn Fn(DateTime<Utc>) -> FixedOffset + Send>;
type ToUtc = Box<dyn Fn(NaiveDateTime) -> Option<DateTime<Utc>> + Send>;

enum Ask {
    /// The offset is a minute old.
    Offset,
    /// A local time as UTC.
    Exact(NaiveDateTime),
}

#[derive(Default)]
struct Kept {
    /// Seconds east of UTC.
    offset: AtomicI32,
    /// The UTC minute the offset was asked at, `NONE` before the first answer.
    minute: AtomicI64,
    /// A new offset has been asked and not answered yet.
    asking: AtomicBool,
    /// Local times as UTC; `None` while being worked out.
    exact: Mutex<HashMap<NaiveDateTime, Option<Option<DateTime<Utc>>>>>,
}

pub struct Clock {
    kept: Arc<Kept>,
    ask: mpsc::Sender<Ask>,
}

impl Clock {
    /// A clock whose thread asks `offset_at` and `to_utc` (the system's, or a test's).
    fn spawn(offset_at: OffsetAt, to_utc: ToUtc) -> Clock {
        let kept = Arc::new(Kept {
            minute: AtomicI64::new(NONE),
            asking: AtomicBool::new(true),
            ..Kept::default()
        });
        let (ask, asked) = mpsc::channel();
        let k = kept.clone();
        let spawned = std::thread::Builder::new()
            .name("mdm-clock".into())
            .spawn(move || run(&k, &asked, &offset_at, &to_utc));
        if let Err(e) = spawned {
            tracing::warn!("clock: no thread for the local time ({e}); UTC shown");
        }
        Clock { kept, ask }
    }

    /// Whether the offset has been answered (UTC is shown until then).
    pub fn known(&self) -> bool {
        self.kept.minute.load(Ordering::Acquire) != NONE
    }

    /// The offset kept, asked again when it is a minute old. Never waits.
    pub fn offset(&self) -> FixedOffset {
        let minute = minute_of(Utc::now());
        if self.kept.minute.load(Ordering::Acquire) < minute
            && !self.kept.asking.swap(true, Ordering::AcqRel)
        {
            let _ = self.ask.send(Ask::Offset);
        }
        FixedOffset::east_opt(self.kept.offset.load(Ordering::Acquire))
            .unwrap_or_else(|| FixedOffset::east_opt(0).expect("UTC"))
    }

    pub fn now(&self) -> DateTime<FixedOffset> {
        self.local(Utc::now())
    }

    pub fn local(&self, utc: DateTime<Utc>) -> DateTime<FixedOffset> {
        utc.with_timezone(&self.offset())
    }

    /// `local` as UTC by the zone's rules for its date; `None` while the clock's thread works it
    /// out (asked here), `Some(None)` for a time that can't be had.
    pub fn exact_utc(&self, local: NaiveDateTime) -> Option<Option<DateTime<Utc>>> {
        let mut exact = self.kept.exact.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(known) = exact.get(&local) {
            return *known;
        }
        if exact.len() >= KEPT {
            exact.clear();
        }
        exact.insert(local, None);
        let _ = self.ask.send(Ask::Exact(local));
        None
    }
}

fn minute_of(t: DateTime<Utc>) -> i64 {
    t.timestamp().div_euclid(60)
}

/// How long the clock's thread waits before asking again: until just after the next minute.
fn wait_from(now: DateTime<Utc>) -> Duration {
    let into =
        now.timestamp().rem_euclid(60) as u64 * 1000 + u64::from(now.timestamp_subsec_millis());
    Duration::from_millis(60_000 - into.min(60_000) + 20)
}

/// The clock's thread: the offset at each new minute, and the conversions asked.
fn run(kept: &Kept, asked: &mpsc::Receiver<Ask>, offset_at: &OffsetAt, to_utc: &ToUtc) {
    let mut minute = NONE;
    loop {
        let now = Utc::now();
        if minute_of(now) != minute {
            let offset = offset_at(now).local_minus_utc();
            let before = kept.offset.swap(offset, Ordering::AcqRel);
            minute = minute_of(now);
            // Another offset: the zone's rules may be others too (the time zone changed).
            if before != offset || minute.rem_euclid(60) == 0 {
                kept.exact.lock().unwrap_or_else(|e| e.into_inner()).clear();
            }
            kept.minute.store(minute, Ordering::Release);
            kept.asking.store(false, Ordering::Release);
        }
        match asked.recv_timeout(wait_from(Utc::now())) {
            Ok(Ask::Exact(local)) => {
                let utc = to_utc(local);
                kept.exact
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(local, Some(utc));
            }
            Ok(Ask::Offset) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// The system's offset at `utc` (asks the system: the clock's thread only).
fn system_offset(utc: DateTime<Utc>) -> FixedOffset {
    chrono::Local.offset_from_utc_datetime(&utc.naive_utc())
}

fn clock() -> &'static Clock {
    static CLOCK: OnceLock<Clock> = OnceLock::new();
    CLOCK.get_or_init(|| Clock::spawn(Box::new(system_offset), Box::new(crate::wake::to_utc)))
}

/// Starts the clock's thread, so its first answer is there by the first frame.
pub fn start() {
    clock();
}

/// The local time now (UTC until the clock's first answer, a moment after the start).
pub fn now() -> DateTime<FixedOffset> {
    clock().now()
}

/// The local time now, once the clock has answered (what the Scheduler's starts go by).
pub fn now_known() -> Option<DateTime<FixedOffset>> {
    let c = clock();
    let now = c.now();
    c.known().then_some(now)
}

/// A UTC time with the offset of now (for display; see the module's notes).
pub fn local(utc: DateTime<Utc>) -> DateTime<FixedOffset> {
    clock().local(utc)
}

/// Seconds since 1970 as a local time with the offset of now (for display).
pub fn local_at(secs: i64) -> Option<DateTime<FixedOffset>> {
    DateTime::from_timestamp(secs, 0).map(local)
}

/// A local time as UTC by the zone's rules for its date (see [`Clock::exact_utc`]).
pub fn exact_utc(local: NaiveDateTime) -> Option<Option<DateTime<Utc>>> {
    clock().exact_utc(local)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn until<T>(limit: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
        let t = Instant::now();
        while t.elapsed() < limit {
            if let Some(v) = f() {
                return Some(v);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    #[test]
    fn a_held_lookup_never_holds_the_caller() {
        // The system holds the answer until the test lets it go.
        let hold = Arc::new(Mutex::new(()));
        let held = hold.lock().unwrap();
        let (h1, h2) = (hold.clone(), hold.clone());
        let two_hours = FixedOffset::east_opt(7200).unwrap();
        let c = Clock::spawn(
            Box::new(move |_| {
                let _wait = h1.lock().unwrap();
                two_hours
            }),
            Box::new(move |l| {
                let _wait = h2.lock().unwrap();
                Some(DateTime::from_naive_utc_and_offset(l, Utc))
            }),
        );
        let local = DateTime::from_timestamp(1_800_000_000, 0)
            .unwrap()
            .naive_utc();
        let t = Instant::now();
        for _ in 0..100 {
            let _ = c.now();
            assert_eq!(c.exact_utc(local), None);
        }
        let took = t.elapsed();
        assert!(took < Duration::from_millis(500), "{took:?}");
        assert!(!c.known());
        assert_eq!(c.now().offset().local_minus_utc(), 0);
        // Let go: the answers come.
        drop(held);
        assert!(until(Duration::from_secs(5), || c.known().then_some(())).is_some());
        assert_eq!(c.now().offset(), &two_hours);
        let utc = until(Duration::from_secs(5), || c.exact_utc(local));
        assert_eq!(
            utc,
            Some(Some(DateTime::from_naive_utc_and_offset(local, Utc)))
        );
    }

    #[test]
    fn its_time_is_chronos() {
        let c = Clock::spawn(Box::new(system_offset), Box::new(crate::wake::to_utc));
        assert!(until(Duration::from_secs(10), || c.known().then_some(())).is_some());
        let (ours, theirs) = (c.now(), chrono::Local::now());
        assert_eq!(ours.offset(), theirs.offset());
        let apart = (ours.naive_local() - theirs.naive_local())
            .num_milliseconds()
            .abs();
        assert!(apart < 2000, "{apart} ms apart");
        // Exact conversions, in winter and in summer (the rules for that date, not now's).
        let year = theirs.naive_local().date().format("%Y").to_string();
        let year: i32 = year.parse().unwrap();
        for month in [1, 7] {
            let local = chrono::NaiveDate::from_ymd_opt(year + 1, month, 15)
                .unwrap()
                .and_hms_opt(23, 0, 0)
                .unwrap();
            let utc = until(Duration::from_secs(10), || c.exact_utc(local));
            assert_eq!(utc, Some(crate::wake::to_utc(local)), "{local}");
        }
    }

    #[test]
    fn it_asks_again_at_the_next_minute() {
        // 1_800_000_000 is a whole minute (30_000_000 × 60).
        for (secs, millis, want) in [(0, 0, 60_020), (59, 990, 30), (30, 500, 29_520)] {
            let t = DateTime::from_timestamp(1_800_000_000 + secs, millis * 1_000_000).unwrap();
            assert_eq!(wait_from(t), Duration::from_millis(want), "{t}");
        }
    }
}
