//! Measure a download to a disk slower than the network (#782): per-second speed, stalls and
//! the connections shed as "much slower than the others".
//!
//! cargo run --release -p mdm-engine --example slow_disk -- <url> <folder> [disk rate] [cache]
//!
//! `url`: a big file on a fast server (the lab server: `cargo run -p mdm-labserver -- --faults
//! rate=15M`, then `http://127.0.0.1:8080/gen/6G/a.bin`). `disk rate` and `cache` (bytes, with
//! K, M or G) give the part file the slow disk of `mdm_engine::slowdisk`; without them it is
//! written to the real disk. The file is deleted at the end.

use std::time::{Duration, Instant};

use mdm_engine::download::State;
use mdm_engine::slowdisk::SlowDisk;
use mdm_engine::{Destination, Download, Options, Request};

fn size(s: &str) -> u64 {
    let (n, unit) = s.split_at(s.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(s.len()));
    let n: f64 = n.parse().expect("a size: 60M, 1G, ...");
    let k = match unit.to_ascii_uppercase().as_str() {
        "" => 1u64,
        "K" => 1 << 10,
        "M" => 1 << 20,
        "G" => 1 << 30,
        u => panic!("unknown unit {u}"),
    };
    (n * k as f64) as u64
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [url, dir, rest @ ..] = &args[..] else {
        eprintln!("usage: slow_disk <url> <folder> [disk rate] [cache]");
        std::process::exit(2);
    };
    let disk = match rest {
        [rate, cache, ..] => Some(SlowDisk::new(size(rate), size(cache), Duration::ZERO)),
        [rate] => Some(SlowDisk::new(size(rate), 1 << 30, Duration::ZERO)),
        [] => None,
    };
    let opts = Options {
        slow_disk: disk.clone(),
        ..Options::default()
    };
    let slow = mdm_engine::Error::SlowConnection.to_string();
    let d = Download::start(
        Request {
            url: url.parse().expect("a URL"),
            headers: Default::default(),
            dest: Destination::Dir(dir.into()),
        },
        opts,
    );
    let started = Instant::now();
    // Per second: bytes received (written or waiting in memory for the disk), connections shed.
    let mut secs: Vec<(u64, usize)> = Vec::new();
    let (mut last, mut last_change, mut seen_events) = (0u64, Instant::now(), 0u64);
    // Stretches of 1 s or more with no byte received: (when it began, how long).
    let mut stalls: Vec<(f64, f64)> = Vec::new();
    let mut size_total = None;
    println!("   s     MB/s   received  receiving  shed");
    loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let v = d.snapshot();
        size_total = size_total.or(v.size);
        let sec = started.elapsed().as_secs() as usize;
        while secs.len() <= sec {
            if let Some((bytes, shed)) = secs.last() {
                let receiving = v
                    .connections
                    .iter()
                    .filter(|c| matches!(c.status, mdm_engine::download::ConnStatus::Receiving))
                    .count();
                let n = secs.len() - 1;
                println!(
                    "{n:4}  {:7.1}  {:7.2} GB  {receiving:9}  {shed:4}",
                    *bytes as f64 / 1e6,
                    v.downloaded as f64 / 1e9,
                );
            }
            secs.push((0, 0));
        }
        if v.downloaded > last {
            secs[sec].0 += v.downloaded - last;
            let quiet = last_change.elapsed();
            if quiet >= Duration::from_secs(1) {
                stalls.push(((last_change - started).as_secs_f64(), quiet.as_secs_f64()));
            }
            last = v.downloaded;
            last_change = Instant::now();
        }
        let since = seen_events;
        for e in v.events.iter().filter(|e| e.seq > since) {
            if e.text.contains(&slow) {
                secs[sec].1 += 1;
            }
            seen_events = e.seq;
        }
        if matches!(v.state, State::Done | State::Failed | State::Cancelled) {
            break;
        }
    }
    let total = started.elapsed().as_secs_f64();
    let out = d.wait().await;
    let shed: usize = secs.iter().map(|s| s.1).sum();
    let body = &secs[..secs.len().saturating_sub(1)];
    let mut sorted: Vec<u64> = body.iter().map(|s| s.0).collect();
    sorted.sort_unstable();
    let low = |q: f64| {
        sorted
            .get((sorted.len() as f64 * q) as usize)
            .copied()
            .unwrap_or(0)
    };
    println!();
    println!(
        "{:.2} GB in {total:.1} s: {:.1} MB/s on average, slowest second {:.1} MB/s, 10th percentile {:.1} MB/s",
        size_total.unwrap_or(last) as f64 / 1e9,
        size_total.unwrap_or(last) as f64 / 1e6 / total,
        low(0.0) as f64 / 1e6,
        low(0.1) as f64 / 1e6,
    );
    println!(
        "stalls of 1 s or more (nothing received): {}, {:.1} s in all, longest {:.1} s {:?}",
        stalls.len(),
        stalls.iter().map(|s| s.1).sum::<f64>(),
        stalls.iter().map(|s| s.1).fold(0.0, f64::max),
        stalls
            .iter()
            .map(|(at, len)| format!("{len:.1} s at {at:.0} s"))
            .collect::<Vec<_>>()
    );
    println!("connections shed as much slower: {shed}");
    if let Some(disk) = &disk {
        let st = disk.stats();
        let mut flushes: Vec<String> = st
            .flushes
            .iter()
            .map(|f| format!("{:.2}", f.as_secs_f64()))
            .collect();
        if flushes.len() > 40 {
            flushes.drain(20..flushes.len() - 20);
        }
        println!(
            "flushes: {} ({} s each); writes that waited for the disk: {}, {:.1} s in all",
            st.flushes.len(),
            flushes.join(" "),
            st.writes_waited,
            st.write_wait.as_secs_f64()
        );
    }
    match out {
        Ok(o) => {
            println!("done: {}", o.path.display());
            let _ = std::fs::remove_file(&o.path);
        }
        Err(e) => println!("failed: {e}"),
    }
}
