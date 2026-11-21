//! How a piece of work grows with its input, for tests: linear, or worse.
//!
//! A test that times its work against a fixed limit ("under 10 s") fails on a slow or busy
//! machine and passes a quadratic search on a fast one. This one times the work on one input of
//! `SCALE × n` items against the same work on `SCALE` inputs of `n` items, on the same machine,
//! a moment apart: as many items either way, so linear work costs about the same on both sides,
//! and quadratic work `SCALE` times more on the larger input, whatever the machine's speed.
//!
//! Both sides take about as long, so a busy machine (other programs, the other tests, a shared
//! runner) slows both alike. One long run against one short one did not: the long run was the
//! one a busy machine interrupted, and once looked like a square.

use std::time::{Duration, Instant};

/// The larger input is this many times the smaller one.
pub const SCALE: usize = 16;

/// The most the larger input may cost, in times the same items in smaller inputs cost. Linear
/// work costs about 1, `n log n` work about 1.4, quadratic work `SCALE` (16): 3 leaves room for
/// the caches a larger input misses and a busy machine, and none for a square.
pub const LIMIT: f64 = 3.0;

/// Each side runs at least this many times, interleaved, and its fastest run counts: a pause of
/// the machine only ever adds time to a run.
const MIN_ROUNDS: usize = 3;

/// Over the limit, each side runs again, up to this many times or for this long (but always
/// [`MIN_ROUNDS`] times): a busy machine
/// pauses some runs, a square slows all of them. Light work, which a pause slows most, gets the
/// most runs; work slow enough to fill the time alone (a square at the larger size) fails sooner.
const MAX_ROUNDS: usize = 20;
const RETRY_FOR: Duration = Duration::from_secs(20);

/// Asserts that `work` grows linearly with its input (see the module): `make(n)` builds an
/// input of size `n`, untimed; `work` is timed on it. `small` is the smaller size: large
/// enough that `work` takes a millisecond or so on it, so that a square would show.
///
/// # Panics
///
/// When the larger input costs `LIMIT` times the same items in smaller inputs or more, at its
/// fastest of `MAX_ROUNDS` runs or of those that fit in `RETRY_FOR`, with both times.
pub fn assert_linear<T, R>(
    small: usize,
    mut make: impl FnMut(usize) -> T,
    mut work: impl FnMut(T) -> R,
) {
    let large = small * SCALE;
    // `count` inputs of `n` items, built first, then worked on back to back, timed together.
    let mut time = |n: usize, count: usize| {
        let inputs: Vec<T> = (0..count).map(|_| make(n)).collect();
        let mut outs = Vec::with_capacity(count);
        let t = Instant::now();
        for input in inputs {
            outs.push(work(input));
        }
        let took = t.elapsed();
        drop(std::hint::black_box(outs));
        took
    };
    let start = Instant::now();
    // A warm-up: the first run pays for the allocator's first pages and lazy statics.
    time(small, SCALE);
    let (mut a, mut b) = (Duration::MAX, Duration::MAX);
    let mut ratio = f64::INFINITY;
    let mut rounds = 0;
    while rounds < MAX_ROUNDS {
        rounds += 1;
        a = a.min(time(small, SCALE));
        b = b.min(time(large, 1));
        ratio = b.as_secs_f64() / a.as_secs_f64().max(1e-6);
        if ratio < LIMIT {
            if rounds >= MIN_ROUNDS {
                return;
            }
        } else if rounds >= MIN_ROUNDS && start.elapsed() >= RETRY_FOR {
            // Never before MIN_ROUNDS: a single run that one long pause held past RETRY_FOR was
            // then the only run there was, and the pause was taken for a square.
            break;
        }
    }
    panic!(
        "{SCALE} inputs of {small} items took {a:?}, one of {large} took {b:?}, the fastest of \
         {rounds} runs: {ratio:.1} times as long (linear: about 1, a square: {SCALE}, the limit \
         {LIMIT})"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique(v: Vec<u64>) -> usize {
        let mut seen = Vec::new();
        for x in &v {
            if !seen.contains(x) {
                seen.push(*x);
            }
        }
        seen.len()
    }

    #[test]
    fn linear_work_passes() {
        assert_linear(
            50_000,
            |n| (0..n as u64).collect::<Vec<_>>(),
            |v| v.iter().map(|x| x.wrapping_mul(31)).sum::<u64>(),
        );
    }

    #[test]
    fn n_log_n_work_passes() {
        assert_linear(
            20_000,
            |n| {
                (0..n as u64)
                    .map(|x| x.wrapping_mul(0x9E37_79B9_7F4A_7C15))
                    .collect::<Vec<_>>()
            },
            |mut v| {
                v.sort_unstable();
                v
            },
        );
    }

    /// A pause of the machine on the larger input's first run: its fastest run counts.
    #[test]
    fn a_pause_on_one_run_passes() {
        let mut large_runs = 0;
        assert_linear(
            50_000,
            |n| (0..n as u64).collect::<Vec<_>>(),
            |v| {
                if v.len() > 50_000 {
                    large_runs += 1;
                    if large_runs == 1 {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                v.iter().map(|x| x.wrapping_mul(31)).sum::<u64>()
            },
        );
    }

    #[test]
    #[should_panic(expected = "times as long")]
    fn quadratic_work_fails() {
        assert_linear(300, |n| (0..n as u64).collect::<Vec<_>>(), unique);
    }
}
