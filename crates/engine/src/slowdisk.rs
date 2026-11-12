//! A disk slower than the network, for tests and measurements (#361, #782): with
//! `Options::slow_disk`, the part file's writes and flushes go through it. It behaves like a
//! hard disk behind the system's write cache:
//! - a write lands in the cache, which the disk empties at `rate` bytes/s; once `cache` bytes
//!   wait there, a write waits for the disk (Windows and Linux hold writers to its pace);
//! - a flush waits until the disk has written everything waiting, plus `flush_takes`, and
//!   holds every write to the file meanwhile, as NTFS does (measured in #361).
//!
//! The real write or flush still runs, after the wait.

use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct SlowDisk {
    /// Bytes/s the disk writes. 0: as fast as the real disk (only `flush_takes` is slow).
    pub rate: u64,
    /// Bytes the system's cache takes in before writes wait for the disk.
    pub cache: u64,
    /// What every flush during the transfer takes on top of writing what waits in the cache.
    pub flush_takes: Duration,
    /// Held exclusively for each flush and shared for each write (tests watch it).
    pub gate: Arc<RwLock<()>>,
    backlog: Arc<Mutex<Backlog>>,
    stats: Arc<Mutex<DiskStats>>,
}

/// Bytes in the cache not yet on the disk, as of `at`.
#[derive(Debug)]
struct Backlog {
    bytes: f64,
    at: Instant,
}

/// What the disk did, for measurements.
#[derive(Clone, Debug, Default)]
pub struct DiskStats {
    /// How long each flush took, the wait for the disk included.
    pub flushes: Vec<Duration>,
    /// Writes that waited for the disk (its cache full, or a flush), and how long in all.
    pub writes_waited: u64,
    pub write_wait: Duration,
}

impl SlowDisk {
    pub fn new(rate: u64, cache: u64, flush_takes: Duration) -> Self {
        SlowDisk {
            rate,
            cache,
            flush_takes,
            gate: Default::default(),
            backlog: Arc::new(Mutex::new(Backlog {
                bytes: 0.0,
                at: Instant::now(),
            })),
            stats: Default::default(),
        }
    }

    /// A disk that is only slow to flush (`takes`), its writes as fast as the real disk's.
    pub fn slow_flush(takes: Duration) -> Self {
        Self::new(0, u64::MAX, takes)
    }

    pub fn stats(&self) -> DiskStats {
        self.stats.lock().unwrap().clone()
    }

    /// Run `write` (of `n` bytes) once the disk takes them in.
    pub fn write<T>(&self, n: u64, write: impl FnOnce() -> T) -> T {
        let began = Instant::now();
        let _flush_over = self.gate.read().unwrap();
        // A disk of `rate` 0 takes everything at once.
        while let Some(wait) = (self.rate > 0).then(|| self.room_in(n)).flatten() {
            std::thread::sleep(wait.min(Duration::from_millis(20)));
        }
        let waited = began.elapsed();
        if waited >= Duration::from_millis(1) {
            let mut s = self.stats.lock().unwrap();
            s.writes_waited += 1;
            s.write_wait += waited;
        }
        write()
    }

    /// Run `flush` once everything in the cache is on the disk; `extra`: plus `flush_takes`
    /// (the saves while the connections run, not the forced ones at the start and the end).
    pub fn flush<T>(&self, extra: bool, flush: impl FnOnce() -> T) -> T {
        let began = Instant::now();
        // A forced save of a disk only slow to flush is a plain one (the gate tells tests a slow
        // flush runs).
        let _writes_wait = (extra || self.rate > 0).then(|| self.gate.write().unwrap());
        let left = if self.rate > 0 {
            Duration::from_secs_f64(self.backlog().bytes / self.rate as f64)
        } else {
            Duration::ZERO
        };
        std::thread::sleep(
            left + if extra {
                self.flush_takes
            } else {
                Duration::ZERO
            },
        );
        self.backlog().bytes = 0.0;
        let done = flush();
        self.stats.lock().unwrap().flushes.push(began.elapsed());
        done
    }

    /// Take `n` bytes into the cache if they fit (or it is empty), else how long until they do.
    fn room_in(&self, n: u64) -> Option<Duration> {
        let mut b = self.backlog();
        let after = b.bytes + n as f64;
        if b.bytes == 0.0 || after <= self.cache as f64 {
            b.bytes = after;
            return None;
        }
        Some(Duration::from_secs_f64(
            (after - self.cache as f64) / self.rate as f64,
        ))
    }

    /// The cache's backlog now (the disk wrote `rate` bytes/s of it since last time).
    fn backlog(&self) -> MutexGuard<'_, Backlog> {
        let mut b = self.backlog.lock().unwrap();
        let now = Instant::now();
        let gone = (now - b.at).as_secs_f64() * self.rate as f64;
        b.bytes = (b.bytes - gone).max(0.0);
        b.at = now;
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The model itself: writes run at the cache's pace until it is full, then at the disk's;
    /// a flush waits for what is left.
    #[test]
    fn writes_wait_once_the_cache_is_full() {
        let d = SlowDisk::new(100 << 20, 10 << 20, Duration::ZERO);
        let t = Instant::now();
        for _ in 0..10 {
            d.write(1 << 20, || ());
        }
        assert!(t.elapsed() < Duration::from_millis(50), "{:?}", t.elapsed());
        // 10 MiB more at 100 MiB/s: about 0.1 s.
        let t = Instant::now();
        for _ in 0..10 {
            d.write(1 << 20, || ());
        }
        let took = t.elapsed();
        assert!(took >= Duration::from_millis(80), "{took:?}");
        // The cache is full (10 MiB): a flush writes it, about 0.1 s.
        let t = Instant::now();
        d.flush(true, || ());
        assert!(
            t.elapsed() >= Duration::from_millis(80),
            "{:?}",
            t.elapsed()
        );
        assert!(d.stats().writes_waited > 0);
    }
}
