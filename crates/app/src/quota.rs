//! Scheduler → Download limits: "Download no more than N MBytes every H hours". IDM "will
//! download the maximum it can under the quota, and then resume automatically at a later
//! time" [IDM-doc]. Bytes are counted from every download's progress over a sliding window.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// The sentence around the limit's two boxes, in Options → Connection and Scheduler →
/// Download limits (`winui::sentence`; the `\n` is where the second box's line starts).
pub const LIMITS_SENTENCE: &str =
    // i18n: {mb} and {hours} are input boxes, drawn where they stand in the translation; keep the one line break: the second line sits under the first box
    crate::i18n::mark("Download no more than {mb} MBytes\nevery {hours} hours");

#[derive(Default)]
pub struct Quota {
    /// Bytes received, per second, oldest first.
    samples: VecDeque<(Instant, u64)>,
    /// What each download had at the last record (to count only new bytes).
    last: HashMap<u64, u64>,
    /// The limit was reached: downloads are held until the window has room again.
    pub blocked: bool,
    /// Downloads stopped (or not started) because of the limit, to resume later.
    pub held: Vec<u64>,
}

impl Quota {
    /// Note how much each download (id, bytes so far) has now.
    pub fn record(&mut self, now: Instant, progress: impl Iterator<Item = (u64, u64)>) {
        let mut new = 0;
        for (id, bytes) in progress {
            let before = self.last.insert(id, bytes).unwrap_or(bytes);
            // A restart (fewer bytes than before) brings nothing new.
            new += bytes.saturating_sub(before);
        }
        if new == 0 {
            return;
        }
        match self.samples.back_mut() {
            Some((at, n)) if now.duration_since(*at) < Duration::from_secs(1) => *n += new,
            _ => self.samples.push_back((now, new)),
        }
    }

    /// Bytes received within `window` before `now` (older samples are dropped).
    pub fn used(&mut self, now: Instant, window: Duration) -> u64 {
        while self
            .samples
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) >= window)
        {
            self.samples.pop_front();
        }
        self.samples.iter().map(|(_, n)| n).sum()
    }

    /// `Some(true)` when the limit has just been reached, `Some(false)` when held downloads
    /// may go on again.
    pub fn check(&mut self, now: Instant, limit: u64, window: Duration) -> Option<bool> {
        let used = self.used(now, window);
        if !self.blocked && used >= limit {
            self.blocked = true;
            Some(true)
        } else if self.blocked && used < limit {
            self.blocked = false;
            Some(false)
        } else {
            None
        }
    }

    /// How long until enough old traffic leaves the window to go below `limit`.
    pub fn room_in(&mut self, now: Instant, limit: u64, window: Duration) -> Duration {
        let mut used = self.used(now, window);
        for (at, n) in &self.samples {
            if used < limit {
                break;
            }
            used -= n;
            if used < limit {
                return (*at + window).saturating_duration_since(now);
            }
        }
        Duration::ZERO
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: Duration = Duration::from_secs(3600);

    #[test]
    fn counts_new_bytes_across_downloads() {
        let mut q = Quota::default();
        let t = Instant::now();
        q.record(t, [(1, 100), (2, 50)].into_iter());
        assert_eq!(q.used(t, H), 0, "what was there before is not new");
        q.record(t + Duration::from_secs(1), [(1, 400), (2, 150)].into_iter());
        assert_eq!(q.used(t + Duration::from_secs(1), H), 400);
        // Download 2 restarted from zero: nothing new from it.
        q.record(t + Duration::from_secs(2), [(1, 400), (2, 10)].into_iter());
        assert_eq!(q.used(t + Duration::from_secs(2), H), 400);
    }

    #[test]
    fn blocks_at_the_limit_and_frees_when_old_traffic_leaves_the_window() {
        let mut q = Quota::default();
        let t = Instant::now();
        q.record(t, [(1, 0)].into_iter());
        q.record(t + Duration::from_secs(10), [(1, 1000)].into_iter());
        assert_eq!(q.check(t + Duration::from_secs(10), 1000, H), Some(true));
        assert_eq!(q.check(t + Duration::from_secs(20), 1000, H), None);
        let wait = q.room_in(t + Duration::from_secs(20), 1000, H);
        assert_eq!(wait, H - Duration::from_secs(10));
        assert_eq!(
            q.check(t + Duration::from_secs(10) + H, 1000, H),
            Some(false)
        );
    }
}
