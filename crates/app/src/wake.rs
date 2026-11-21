//! Scheduler → "Wake the computer to run this queue" (#142): the computer, asleep, is woken
//! shortly before a queue's start time, and kept awake while that queue runs.
//!
//! The times are worked out here, the same on every system; the waking itself is the system's:
//! - Windows: a waitable timer that resumes the computer (`win32::wake_at`), owned by MDM's
//!   process, as the Scheduler's starts are (MDM has to be running, in the tray if need be).
//!   Windows' power options must allow wake timers; when they don't, the Scheduler says so.
//! - Linux: the clock chip's wake alarm (`/sys/class/rtc/rtc0/wakealarm`), which only an
//!   administrator may set unless they let users set it (the guide shows how). A systemd user
//!   timer can't wake the system (`WakeSystem=` needs the system manager), nor can `rtcwake`
//!   or a timer on an alarm clock without root or CAP_WAKE_ALARM.
//! - macOS: `pmset schedule wake` and IOKit's scheduled power events need an administrator;
//!   MDM never asks for one by itself, so the option says why.
//!
//! While a queue with the option runs, and while any download runs (#939), the computer is kept
//! from sleeping by itself (Windows: a power request; Linux: logind's sleep inhibitor; macOS:
//! `caffeinate -i`), so a computer woken for a queue, or left downloading, doesn't fall asleep
//! before the downloads are done. The screen still turns off and locks as the system's settings
//! say. Afterwards the system's own sleep settings apply again, and the queue's "Turn off
//! computer when done" can put it to sleep at once.

use std::sync::mpsc::{Receiver, Sender};

use chrono::{DateTime, Datelike, Duration, Local, NaiveDateTime, TimeZone, Utc};

use crate::queue::{Queue, Schedule};

/// The computer is woken this long before the start, so the network is back by then.
pub const LEAD: Duration = Duration::seconds(60);

/// Whether this computer can be woken by MDM, and if not, why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Support {
    // macOS never can (see unix::wake_support).
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Ready,
    /// Windows' power options don't allow wake timers now (on battery: `battery`).
    TimersOff { battery: bool },
    /// Windows allows wake timers on mains power only, and this computer has a battery.
    NotOnBattery,
    /// Linux: the wake alarm is the administrator's (the guide shows how to allow it).
    NeedsAdmin,
    /// Linux: no clock chip with a wake alarm.
    NoAlarm,
    /// macOS: only an administrator can schedule a wake.
    MacAdmin,
}

impl Support {
    /// The checkbox can be ticked: waking is possible here, or becomes so in the system's own
    /// settings without an administrator (Windows' wake timers are a power option).
    pub fn can_tick(&self) -> bool {
        matches!(
            self,
            Support::Ready | Support::TimersOff { .. } | Support::NotOnBattery
        )
    }
}

/// When `s` next starts after `now` (local time): its "Once at" date at the start time, or
/// the next ticked weekday's. None without a start time, or when it can never come.
pub fn next_start(s: &Schedule, now: NaiveDateTime) -> Option<NaiveDateTime> {
    let (h, m, sec) = s.start_at?;
    let time = chrono::NaiveTime::from_hms_opt(h.into(), m.into(), sec.into())?;
    if let Some((y, mo, d)) = s.once {
        let at = chrono::NaiveDate::from_ymd_opt(y, mo, d)?.and_time(time);
        return (at > now).then_some(at);
    }
    (0..=7)
        .map(|n| (now.date() + Duration::days(n)).and_time(time))
        .find(|at| *at > now && s.days[at.weekday().num_days_from_sunday() as usize])
}

/// When to wake for a start at `start`: [`LEAD`] before it, or at the start itself when that
/// is already past.
pub fn wake_for(start: NaiveDateTime, now: NaiveDateTime) -> NaiveDateTime {
    let early = start - LEAD;
    if early > now { early } else { start }
}

/// A wake the queues want: when (local time), for which start, of which queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wake {
    pub at: NaiveDateTime,
    pub start: NaiveDateTime,
    pub queue: String,
}

/// The earliest wake the queues want after `now`.
pub fn next_wake(queues: &[Queue], now: NaiveDateTime) -> Option<Wake> {
    queues
        .iter()
        .filter(|q| q.schedule.wake)
        .filter_map(|q| {
            let start = next_start(&q.schedule, now)?;
            Some(Wake {
                at: wake_for(start, now),
                start,
                queue: q.name.clone(),
            })
        })
        .min_by_key(|w| w.at)
}

/// A local time as UTC (asks the system: the clock's thread uses it, `crate::clock::exact_utc`
/// for the window). A time the clocks skip (summer time) is taken an hour later, as the
/// clock reads then; a time they pass twice, the first time.
pub fn to_utc(local: NaiveDateTime) -> Option<DateTime<Utc>> {
    Local
        .from_local_datetime(&local)
        .earliest()
        .or_else(|| {
            Local
                .from_local_datetime(&(local + Duration::hours(1)))
                .earliest()
        })
        .map(|t| t.with_timezone(&Utc))
}

/// Whether this computer can be woken now, asked of the system at most every 2 s.
pub fn support() -> Support {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static LAST: Mutex<Option<(Instant, Support)>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, s)) = &*last
        && at.elapsed() < Duration::from_secs(2)
    {
        return s.clone();
    }
    let s = test_support().unwrap_or_else(ask_support);
    *last = Some((Instant::now(), s.clone()));
    s
}

/// An off-screen test run (MDM_OFFSCREEN) may play another system's answer with MDM_TEST_WAKE
/// (`timers-off`, `battery-off`, `not-on-battery`, `needs-admin`, `no-alarm`, `mac-admin`), so
/// every state of the Scheduler's line can be shown and checked on one computer.
fn test_support() -> Option<Support> {
    if !crate::offscreen() {
        return None;
    }
    Some(match std::env::var("MDM_TEST_WAKE").ok()?.as_str() {
        "timers-off" => Support::TimersOff { battery: false },
        "battery-off" => Support::TimersOff { battery: true },
        "not-on-battery" => Support::NotOnBattery,
        "needs-admin" => Support::NeedsAdmin,
        "no-alarm" => Support::NoAlarm,
        "mac-admin" => Support::MacAdmin,
        _ => return None,
    })
}

#[cfg(windows)]
fn ask_support() -> Support {
    // 0 off, 1 on, 2 "important wake timers only" (Windows' own, not an app's).
    let w = crate::win32::wake_timers();
    let now = if w.on_battery { w.dc } else { w.ac };
    if now != 1 {
        Support::TimersOff {
            battery: w.on_battery,
        }
    } else if w.has_battery && w.dc != 1 {
        Support::NotOnBattery
    } else {
        Support::Ready
    }
}

#[cfg(unix)]
fn ask_support() -> Support {
    crate::unix::wake_support()
}

/// Why the computer is kept awake: the words the system lists it with (Windows' `powercfg
/// /requests`, `systemd-inhibit --list`, GNOME's "Some applications are busy").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Awake<'a> {
    /// A queue with "Wake the computer" is running (#142): its name.
    Queue(&'a str),
    /// Downloads are running (#939): a computer left downloading doesn't fall asleep before
    /// they are done, as a browser's downloads keep it awake.
    Downloads,
}

impl Awake<'_> {
    fn reason(self) -> String {
        match self {
            Awake::Queue(queue) => crate::i18n::fmt(
                // i18n: why the computer isn't going to sleep, in the system's list of programs that keep it awake; {queue} = the queue's name
                crate::i18n::t("Mister Download Manager is running the queue \"{queue}\""),
                &[("queue", crate::i18n::t(queue))],
            ),
            Awake::Downloads => {
                // i18n: why the computer isn't going to sleep, in the system's list of programs that keep it awake (Windows' power requests, Linux's inhibitors)
                crate::i18n::t("Mister Download Manager is downloading").to_owned()
            }
        }
    }
}

/// Why the computer is kept awake now: a running queue that woke it (its name), else running
/// downloads (`downloading`: any download in the list is), else nothing.
pub fn why_awake(queues: &[Queue], downloading: bool) -> Option<Awake<'_>> {
    match queues.iter().find(|q| q.running && q.schedule.wake) {
        Some(q) => Some(Awake::Queue(&q.name)),
        None => downloading.then_some(Awake::Downloads),
    }
}

/// What the app keeps: the wake asked of the system, and whether it keeps the computer awake.
#[derive(Default)]
pub struct Waker {
    /// The wake last asked for: (UTC, the queue, the start it is for).
    wanted: Option<(DateTime<Utc>, String, NaiveDateTime)>,
    /// Whether the system has a wake of ours now.
    armed: bool,
    /// Why the last attempt failed (shown in the Scheduler), if it did.
    pub error: Option<String>,
    /// The wake is asked for again at least every minute (the system may have dropped it: a
    /// Linux alarm used up by another wake, an alarm another program set has gone off).
    checked: Option<std::time::Instant>,
    awake: bool,
    /// The reason last asked with (held, on its way or refused).
    reason: Option<String>,
    /// When keeping the computer awake was last refused, and how often in a row (asked again
    /// 5 s later, after a minute of refusals once a minute).
    refused: Option<(std::time::Instant, u32)>,
    /// The system's keep-awake, asked on a thread of its own.
    holder: Holder,
}

impl Waker {
    /// Set, move or cancel the system's wake for `queues` (called with the Scheduler's clock;
    /// it asks the system only when the wake changes, and once a minute).
    pub fn sync(&mut self, queues: &[Queue]) {
        // The local time and the start's UTC time from the clock's thread: asking the system here
        // could hold the window (#536). Until they have come, the wake stays as it is.
        let Some(now) = crate::clock::now_known() else {
            return;
        };
        let mut want = match next_wake(queues, now.naive_local()) {
            None => None,
            Some(w) => match crate::clock::exact_utc(w.at) {
                None => return,
                Some(at) => at.map(|at| (at, w.queue, w.start)),
            },
        };
        // A start keeps the wake set for it: in the minute before the start, the wake worked out
        // now would be the start itself, moving a timer that is just going off.
        if let (Some(w), Some(set)) = (&mut want, &self.wanted)
            && (&w.1, w.2) == (&set.1, set.2)
        {
            *w = set.clone();
        }
        let stale = self
            .checked
            .is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(60));
        // Once its time has come, a wake isn't set again (a Linux alarm in the past is refused).
        let passed = want.as_ref().is_some_and(|w| w.0 <= Utc::now());
        if want == self.wanted && (!stale || passed) {
            return;
        }
        self.checked = Some(std::time::Instant::now());
        let changed = want != self.wanted;
        self.wanted = want.clone();
        // Nothing to set where waking isn't possible (Linux, macOS without the rights).
        if want.is_some() && !support().can_tick() {
            if std::mem::take(&mut self.armed) {
                let _ = arm(None);
            }
            self.error = None;
            return;
        }
        if want.is_none() && !self.armed {
            self.error = None;
            return;
        }
        match arm(want.as_ref().map(|(at, q, _)| (*at, q.as_str()))) {
            Ok(()) => {
                if changed {
                    match &want {
                        Some((at, q, _)) => tracing::info!(
                            "wake: the computer wakes at {} UTC for the queue {q:?}",
                            at.format("%Y-%m-%d %H:%M:%S")
                        ),
                        None => tracing::info!("wake: no wake set"),
                    }
                }
                self.armed = want.is_some();
                self.error = None;
            }
            Err(e) => {
                if self.error.as_deref() != Some(e.as_str()) {
                    tracing::warn!("wake: could not set the wake: {e}");
                }
                self.armed = false;
                self.error = Some(e);
            }
        }
    }

    /// Keep the computer awake for `why` (a download is running, or a queue that woke it), None
    /// lets it sleep again. Returns at once: the system is asked on the holder's thread, and its
    /// answer read on a later call. A new reason (the queue is done, its downloads go on) is
    /// asked again with its own words, for the system's list of what keeps it awake.
    pub fn keep_awake(&mut self, why: Option<Awake<'_>>) {
        let Some(why) = why else {
            self.refused = None;
            self.reason = None;
            if std::mem::take(&mut self.awake) {
                tracing::info!("wake: no longer keeping the computer awake");
            }
            // A hold still on its way is let go once it is taken (the thread asks in turn).
            if self.holder.may_hold() {
                self.holder.release();
            }
            return;
        };
        if let Some(held) = self.holder.answer() {
            if held {
                tracing::info!(
                    "wake: keeping the computer awake: {}",
                    self.reason.as_deref().unwrap_or_default()
                );
                self.awake = true;
                self.refused = None;
            } else {
                self.awake = false;
                self.refusal();
            }
        }
        let reason = why.reason();
        let same = self.reason.as_deref() == Some(reason.as_str());
        // A refusal is asked again a little later: logind refuses while the sleep the computer
        // just woke from is still ending, which is when a woken queue starts.
        let wait = |n: u32| std::time::Duration::from_secs(if n < 12 { 5 } else { 60 });
        if (self.awake && same)
            || self.holder.asking()
            || (same && self.refused.is_some_and(|(t, n)| t.elapsed() < wait(n)))
        {
            return;
        }
        if !same {
            self.refused = None;
        }
        self.reason = Some(reason.clone());
        if !self.holder.hold(reason) {
            self.refusal();
        }
    }

    fn refusal(&mut self) {
        let n = self.refused.map_or(0, |(_, n)| n);
        self.refused = Some((std::time::Instant::now(), n + 1));
    }

    /// Cancel the wake (MDM is quitting: Linux's alarm would outlive it). The computer may sleep
    /// again: waited for at most [`QUIT_WAIT`] (the system lets go of it when MDM ends anyway).
    pub fn clear(&mut self) {
        self.clear_within(QUIT_WAIT);
    }

    /// [`Waker::clear`], waiting at most `limit` for the system to let the computer sleep.
    fn clear_within(&mut self, limit: std::time::Duration) {
        self.wanted = None;
        if std::mem::take(&mut self.armed) {
            let _ = arm(None);
        }
        self.awake = false;
        self.reason = None;
        if self.holder.may_hold() {
            self.holder.release_within(limit);
        }
    }
}

/// How long quitting waits for the system to let the computer sleep again.
const QUIT_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

/// An order for the holder's thread, with its number.
enum Order {
    /// Keep the computer awake for this reason; answered with whether the system agreed.
    Hold(u64, String),
    /// Let it sleep again; answered once done.
    Release(u64),
}

/// The system's call: keep the computer awake for a reason, or let it sleep (None); false when
/// the system refused.
type Call = Box<dyn FnMut(Option<&str>) -> bool + Send>;

/// The holder's thread: the orders to it, and its answers (the order's number, the system's).
struct HolderThread {
    orders: Sender<Order>,
    answers: Receiver<(u64, bool)>,
}

/// The system's keep-awake (Windows' power request, logind's inhibitor, `caffeinate`), asked on a
/// thread of its own: logind can be slow to answer right after a resume, which is exactly when a
/// woken queue starts (and it is asked again every 5 s while it refuses), so the window never
/// waits for it. The thread takes the orders in turn: a release sent while a hold is still on
/// its way lets go of that hold.
struct Holder {
    /// Taken by the thread when it starts.
    call: Option<Call>,
    thread: Option<HolderThread>,
    /// The hold waiting for its answer.
    asked: Option<u64>,
    /// The system agreed to the last hold, and it wasn't released since.
    held: bool,
    next: u64,
}

impl Default for Holder {
    fn default() -> Self {
        Holder::new(Box::new(hold_awake))
    }
}

impl Holder {
    fn new(call: Call) -> Holder {
        Holder {
            call: Some(call),
            thread: None,
            asked: None,
            held: false,
            next: 0,
        }
    }

    /// Hand the order made with the next number to the thread, starting it the first time: that
    /// number, or None when there is no thread to take it.
    fn send(&mut self, order: impl FnOnce(u64) -> Order) -> Option<u64> {
        if self.thread.is_none() {
            let mut call = self.call.take()?;
            let (tx, orders) = std::sync::mpsc::channel::<Order>();
            let (answers, rx) = std::sync::mpsc::channel();
            let started = std::thread::Builder::new()
                .name("keep-awake".into())
                .spawn(move || {
                    for order in orders {
                        let answer = match order {
                            Order::Hold(n, reason) => (n, call(Some(&reason))),
                            Order::Release(n) => (n, call(None)),
                        };
                        if answers.send(answer).is_err() {
                            break;
                        }
                    }
                });
            if let Err(e) = started {
                tracing::warn!("wake: no thread to keep the computer awake on: {e}");
                return None;
            }
            self.thread = Some(HolderThread {
                orders: tx,
                answers: rx,
            });
        }
        self.next += 1;
        let n = self.next;
        let thread = self.thread.as_ref()?;
        thread.orders.send(order(n)).ok().map(|()| n)
    }

    /// Ask for the computer to be kept awake; false when it can't be asked at all.
    fn hold(&mut self, reason: String) -> bool {
        self.asked = self.send(|n| Order::Hold(n, reason));
        self.asked.is_some()
    }

    /// A hold is on its way, not answered yet.
    fn asking(&self) -> bool {
        self.asked.is_some()
    }

    /// The computer may be kept awake now, or soon: a hold held, or still on its way.
    fn may_hold(&self) -> bool {
        self.held || self.asked.is_some()
    }

    /// The hold's answer, once it has come (true: held).
    fn answer(&mut self) -> Option<bool> {
        let asked = self.asked?;
        let thread = self.thread.as_ref()?;
        while let Ok((n, held)) = thread.answers.try_recv() {
            if n == asked {
                self.asked = None;
                self.held = held;
                return Some(held);
            }
        }
        None
    }

    /// Let the computer sleep again (after any hold still on its way): the order's number.
    fn release(&mut self) -> Option<u64> {
        self.asked = None;
        self.held = false;
        self.send(Order::Release)
    }

    /// [`Holder::release`], waiting at most `limit` for it to be done: whether it was.
    fn release_within(&mut self, limit: std::time::Duration) -> bool {
        let Some(n) = self.release() else {
            return false;
        };
        let Some(thread) = self.thread.as_ref() else {
            return false;
        };
        let start = std::time::Instant::now();
        loop {
            match thread
                .answers
                .recv_timeout(limit.saturating_sub(start.elapsed()))
            {
                Ok((m, _)) if m == n => return true,
                Ok(_) => {}
                Err(_) => {
                    tracing::warn!(
                        "wake: the system hasn't answered within {limit:?}; MDM's hold ends with it"
                    );
                    return false;
                }
            }
        }
    }
}

/// Set the system's wake for `at` (UTC), or cancel it (None).
fn arm(at: Option<(DateTime<Utc>, &str)>) -> Result<(), String> {
    #[cfg(windows)]
    {
        crate::win32::wake_at(at.map(|(t, q)| (t.timestamp(), q)))
    }
    #[cfg(unix)]
    {
        crate::unix::wake_at(at.map(|(t, _)| t.timestamp()))
    }
}

/// Keep the computer awake (`reason`) or let it sleep (None); false when the system refused.
fn hold_awake(reason: Option<&str>) -> bool {
    #[cfg(windows)]
    {
        crate::win32::keep_awake(reason)
    }
    #[cfg(unix)]
    {
        crate::unix::keep_awake(reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, s)
            .unwrap()
    }

    fn daily(t: (u8, u8, u8)) -> Schedule {
        Schedule {
            start_at: Some(t),
            wake: true,
            ..Schedule::default()
        }
    }

    #[test]
    fn daily_starts_today_or_on_the_next_ticked_day() {
        // Friday 6 November 2026.
        let now = at(2026, 11, 6, 22, 0, 0);
        let s = daily((23, 0, 0));
        assert_eq!(next_start(&s, now), Some(at(2026, 11, 6, 23, 0, 0)));
        // Passed today: tomorrow.
        let s = daily((7, 30, 0));
        assert_eq!(next_start(&s, now), Some(at(2026, 11, 7, 7, 30, 0)));
        // Only on Mondays (1): the next Monday.
        let mut s = daily((23, 0, 0));
        s.days = [false, true, false, false, false, false, false];
        assert_eq!(next_start(&s, now), Some(at(2026, 11, 9, 23, 0, 0)));
        // Only on Fridays, already passed today: a week later.
        s.days = [false, false, false, false, false, true, false];
        assert_eq!(
            next_start(&s, at(2026, 11, 6, 23, 30, 0)),
            Some(at(2026, 11, 13, 23, 0, 0))
        );
        // No day ticked, or no start time: never.
        s.days = [false; 7];
        assert_eq!(next_start(&s, now), None);
        assert_eq!(next_start(&Schedule::default(), now), None);
    }

    #[test]
    fn once_at_a_date_until_it_has_passed() {
        let mut s = daily((6, 0, 0));
        s.once = Some((2026, 11, 8));
        assert_eq!(
            next_start(&s, at(2026, 11, 6, 22, 0, 0)),
            Some(at(2026, 11, 8, 6, 0, 0))
        );
        assert_eq!(next_start(&s, at(2026, 11, 8, 6, 0, 0)), None, "passed");
        assert_eq!(next_start(&s, at(2026, 12, 1, 0, 0, 0)), None);
    }

    #[test]
    fn the_wake_comes_a_minute_early_unless_that_has_passed() {
        let start = at(2026, 11, 6, 23, 0, 0);
        assert_eq!(
            wake_for(start, at(2026, 11, 6, 22, 0, 0)),
            at(2026, 11, 6, 22, 59, 0)
        );
        // 30 s before the start: at the start itself.
        assert_eq!(wake_for(start, at(2026, 11, 6, 22, 59, 30)), start);
    }

    #[test]
    fn the_earliest_queue_with_the_option_wins() {
        let now = at(2026, 11, 6, 12, 0, 0);
        let q = |name: &str, s: Schedule| Queue {
            schedule: s,
            ..Queue::new(name, false)
        };
        let mut off = daily((13, 0, 0));
        off.wake = false;
        let queues = vec![
            q("early but not waking", off),
            q("night", daily((23, 0, 0))),
            q("evening", daily((19, 0, 0))),
            q(
                "no start",
                Schedule {
                    wake: true,
                    ..Schedule::default()
                },
            ),
        ];
        assert_eq!(
            next_wake(&queues, now),
            Some(Wake {
                at: at(2026, 11, 6, 18, 59, 0),
                start: at(2026, 11, 6, 19, 0, 0),
                queue: "evening".to_owned()
            })
        );
        assert_eq!(next_wake(&queues[..1], now), None);
    }

    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::mpsc::{RecvTimeoutError, channel};
    use std::time::Instant;

    /// The queue's name (a constant: a literal in a call would be taken for a text to translate).
    const QUEUE: &str = "night";

    /// Long enough for anything a test waits for to come on the slowest runner, and the most a
    /// fake system waits for its answer: a deadlock breaker, never a time any check is about.
    const STUCK: std::time::Duration = std::time::Duration::from_secs(30);

    /// A fake system that answers each call when the test says so ([`System::answer`]): slow or
    /// hanging for exactly as long as the test wants, without a clock. Its calls are counted as
    /// they return, so a test sees whether keep_awake waited for an answer (it can't have had
    /// one before the test gave it) instead of timing it.
    struct System {
        /// Each call as it is made: a reason (hold) or None (let go).
        calls: std::sync::mpsc::Receiver<Option<String>>,
        answers: std::sync::mpsc::Sender<bool>,
        /// Calls that have returned with the test's answer.
        returned: Arc<AtomicU32>,
        /// A call waited [`STUCK`] for its answer: the test was held up, by the code it tests.
        stuck: Arc<AtomicBool>,
    }

    impl System {
        /// The next call the holder's thread makes: its reason, None to let go.
        #[track_caller]
        fn next(&self) -> Option<String> {
            self.calls.recv_timeout(STUCK).expect("the system is asked")
        }

        /// Answer the call under way, or the next one: whether the system agrees.
        fn answer(&self, agrees: bool) {
            self.answers.send(agrees).unwrap();
        }

        fn returned(&self) -> u32 {
            self.returned.load(Ordering::SeqCst)
        }

        fn stuck(&self) -> bool {
            self.stuck.load(Ordering::SeqCst)
        }
    }

    /// A Waker on a [`System`] of the test's.
    fn waker() -> (Waker, System) {
        let (calls, calls_rx) = channel();
        let (answers_tx, answers) = channel();
        let returned = Arc::new(AtomicU32::new(0));
        let stuck = Arc::new(AtomicBool::new(false));
        let (done, held_up) = (returned.clone(), stuck.clone());
        let call: Call = Box::new(move |reason| {
            let _ = calls.send(reason.map(str::to_owned));
            let agrees = match answers.recv_timeout(STUCK) {
                Ok(agrees) => agrees,
                Err(RecvTimeoutError::Timeout) => {
                    held_up.store(true, Ordering::SeqCst);
                    false
                }
                // The test is over.
                Err(RecvTimeoutError::Disconnected) => false,
            };
            done.fetch_add(1, Ordering::SeqCst);
            agrees
        });
        let w = Waker {
            holder: Holder::new(call),
            ..Waker::default()
        };
        // The translations are read once, on the first text (the app has read them long before).
        let _ = crate::i18n::t("Mister Download Manager is running the queue \"{queue}\"");
        let system = System {
            calls: calls_rx,
            answers: answers_tx,
            returned,
            stuck,
        };
        (w, system)
    }

    /// Call `keep_awake(why)` as the window does, tick after tick, until `done`: the holder's
    /// answer comes from its thread, read on a later tick.
    fn tick_until(w: &mut Waker, why: Option<Awake<'_>>, done: impl Fn(&Waker) -> bool) {
        let start = Instant::now();
        while !done(w) {
            assert!(start.elapsed() < STUCK, "never came");
            w.keep_awake(why);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// Orders sent to the holder's thread (holds and releases), whether or not it took them yet.
    fn orders(w: &Waker) -> u64 {
        w.holder.next
    }

    #[test]
    fn keeping_awake_never_waits_for_a_slow_system() {
        let (mut w, sys) = waker();
        w.keep_awake(Some(Awake::Queue(QUEUE)));
        // The system is asked, and answers only once the test says: keep_awake came back
        // without its answer (one that waited would have, the fake's own after STUCK).
        assert!(sys.next().unwrap().contains(QUEUE));
        assert_eq!(sys.returned(), 0, "keep_awake waited for the system");
        assert!(!w.awake, "not held before the system said so");
        // Every tick while the system takes its time comes back too, and asks nothing more.
        for _ in 0..50 {
            w.keep_awake(Some(Awake::Queue(QUEUE)));
        }
        assert!(!w.awake);
        assert_eq!((sys.returned(), orders(&w)), (0, 1), "asked once");
        sys.answer(true);
        tick_until(&mut w, Some(Awake::Queue(QUEUE)), |w| w.awake);
        assert_eq!(orders(&w), 1);
        // The queue stops: let go, without waiting for the system either.
        w.keep_awake(None);
        assert!(!w.awake);
        assert_eq!(sys.next(), None, "let go");
        assert_eq!(sys.returned(), 1, "keep_awake waited for the let go");
        sys.answer(true);
        assert!(!sys.stuck(), "keep_awake waited for the system");
    }

    #[test]
    fn a_refusal_is_asked_again_later_not_every_tick() {
        let secs = std::time::Duration::from_secs;
        let (mut w, sys) = waker();
        w.keep_awake(Some(Awake::Queue(QUEUE)));
        assert!(sys.next().is_some());
        sys.answer(false);
        tick_until(&mut w, Some(Awake::Queue(QUEUE)), |w| w.refused.is_some());
        assert_eq!(w.refused.map(|(_, n)| n), Some(1));
        // The next ticks within the 5 s don't ask again.
        for _ in 0..50 {
            w.keep_awake(Some(Awake::Queue(QUEUE)));
        }
        assert_eq!(orders(&w), 1);
        // 5 s later it is asked again, on the next tick.
        let earlier = Instant::now().checked_sub(secs(6)).unwrap();
        w.refused = Some((earlier, 1));
        w.keep_awake(Some(Awake::Queue(QUEUE)));
        assert_eq!(orders(&w), 2);
        assert!(sys.next().is_some());
        sys.answer(false);
        tick_until(&mut w, Some(Awake::Queue(QUEUE)), |w| {
            w.refused.is_some_and(|(_, n)| n == 2)
        });
        assert_eq!(orders(&w), 2);
        assert!(!w.awake);
        // Nothing held: nothing to let go.
        w.keep_awake(None);
        assert_eq!(w.refused, None);
        assert_eq!(orders(&w), 2);
        assert!(!sys.stuck());
    }

    #[test]
    fn stopping_while_the_hold_is_on_its_way_lets_it_go() {
        let (mut w, sys) = waker();
        w.keep_awake(Some(Awake::Queue(QUEUE)));
        w.keep_awake(None);
        // The hold, answered "yes" late, after the stop; then the let go.
        assert!(sys.next().is_some(), "the hold");
        sys.answer(true);
        assert_eq!(sys.next(), None, "let go");
        sys.answer(true);
        // The late "yes" is never taken for the stopped queue's: nothing to let go again.
        w.keep_awake(None);
        assert!(!w.awake && !w.holder.may_hold());
        assert_eq!(orders(&w), 2);
        // Nor for the next hold's: by the time the system is asked it, both answers are back.
        w.keep_awake(Some(Awake::Queue(QUEUE)));
        assert!(sys.next().is_some(), "the next hold");
        for _ in 0..50 {
            w.keep_awake(Some(Awake::Queue(QUEUE)));
        }
        assert!(!w.awake, "an old answer taken for the new hold's");
        assert!(!sys.stuck());
    }

    #[test]
    fn quitting_waits_for_a_hanging_system_at_most_a_second() {
        let (mut w, sys) = waker();
        w.keep_awake(Some(Awake::Queue(QUEUE)));
        // The system takes the hold and never answers while MDM quits.
        assert!(sys.next().is_some());
        let t = Instant::now();
        w.clear();
        let took = t.elapsed();
        // It waited for the let go as long as it may (the let go waits behind the hold), and
        // came back while the system still hangs.
        assert!(took >= QUIT_WAIT, "{took:?}");
        assert_eq!(sys.returned(), 0, "the system answered");
        assert!(!sys.stuck(), "clear waited for the system");
        assert!(!w.awake);
        assert_eq!(orders(&w), 2, "let go");
        // A system that answers: quitting lets go, and waits for it.
        let (mut w, sys) = waker();
        w.keep_awake(Some(Awake::Queue(QUEUE)));
        assert!(sys.next().is_some());
        sys.answer(true);
        tick_until(&mut w, Some(Awake::Queue(QUEUE)), |w| w.awake);
        let returned = sys.returned.clone();
        let system = std::thread::spawn(move || {
            assert_eq!(sys.next(), None, "let go");
            sys.answer(true);
            sys
        });
        w.clear_within(STUCK);
        // Back once the system let go, not before.
        assert_eq!(returned.load(Ordering::SeqCst), 2);
        let sys = system.join().unwrap();
        assert!(!w.awake && !sys.stuck());
    }

    /// Downloads keep the computer awake, a running queue that woke it with its own name
    /// (#939). A queue without the option, or nothing running, lets it sleep.
    #[test]
    fn downloads_keep_the_computer_awake() {
        let mut queues = vec![Queue::new(QUEUE, false), Queue::new(QUEUE, false)];
        assert_eq!(why_awake(&queues, false), None);
        assert_eq!(why_awake(&queues, true), Some(Awake::Downloads));
        queues[1].running = true;
        assert_eq!(why_awake(&queues, true), Some(Awake::Downloads));
        queues[1].schedule.wake = true;
        assert_eq!(why_awake(&queues, false), Some(Awake::Queue(QUEUE)));
        assert_eq!(why_awake(&queues, true), Some(Awake::Queue(QUEUE)));
        let _ = crate::i18n::t("Mister Download Manager is downloading");
        assert_eq!(
            Awake::Downloads.reason(),
            "Mister Download Manager is downloading"
        );
    }

    /// The queue that woke the computer is done and its downloads go on: the hold is asked again
    /// with the downloads' words, once; when they end, it is let go.
    #[test]
    fn a_new_reason_is_asked_again_once() {
        let (mut w, sys) = waker();
        w.keep_awake(Some(Awake::Queue(QUEUE)));
        assert!(sys.next().unwrap().contains(QUEUE));
        sys.answer(true);
        tick_until(&mut w, Some(Awake::Queue(QUEUE)), |w| w.awake);
        // The queue is done, its downloads go on: asked again on the next tick, in their words.
        w.keep_awake(Some(Awake::Downloads));
        assert_eq!(
            sys.next().as_deref(),
            Some("Mister Download Manager is downloading")
        );
        sys.answer(true);
        tick_until(&mut w, Some(Awake::Downloads), |w| !w.holder.asking());
        // More ticks with the same reason ask nothing more.
        for _ in 0..50 {
            w.keep_awake(Some(Awake::Downloads));
        }
        assert!(w.awake);
        assert_eq!(orders(&w), 2);
        // They end: let go.
        w.keep_awake(None);
        assert_eq!(sys.next(), None, "let go");
        sys.answer(true);
        assert_eq!(orders(&w), 3);
        assert!(!w.awake && !sys.stuck());
    }

    #[test]
    fn a_local_time_has_a_utc_time() {
        let local = at(2026, 11, 6, 23, 0, 0);
        let utc = to_utc(local).unwrap();
        assert_eq!(utc.with_timezone(&Local).naive_local(), local);
    }
}
