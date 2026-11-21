//! #39: the order a torrent's pieces arrive in, measured on swarms on this computer (loopback
//! only, nothing else is asked): in order (each file's first and last pieces first, or strictly
//! from the start) against rarest first. For each swarm and order: the time to finish, how much
//! of the file's start is there at each quarter of its pieces (what a player could already
//! play), and for the scarce swarm when the scarce pieces arrived and when another peer had them
//! from us (how soon they spread).
//!
//!   cargo run --release -p mdm-torrent --example order -- <out dir> [runs] [video.mp4]
//!
//! With a video (an MP4 whose index is at its end, as most are written), each in-order run also
//! copies the partial file at 30% and 60% and asks ffprobe and ffmpeg (on PATH) whether its
//! start plays.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mdm_torrent::{Config, Phase, PieceOrder, Torrents};

const PIECE: u32 = 64 << 10;

struct Seeder {
    /// Upload limit, KiB/s.
    rate: u64,
    /// The pieces it has (by index), or all.
    has: Option<std::ops::Range<usize>>,
    /// Pieces it lacks besides (the scarce ones).
    lacks: Option<std::ops::Range<usize>>,
    /// It leaves after this long.
    leaves: Option<Duration>,
}

struct Swarm {
    name: &'static str,
    seeders: Vec<Seeder>,
    /// The pieces only some seeders have (for the "scarce" measures).
    scarce: Option<std::ops::Range<usize>>,
}

#[derive(Default, Debug)]
struct Run {
    finished: Option<Duration>,
    /// Contiguous pieces from the start, at 25/50/75% of pieces done.
    prefix: [usize; 3],
    /// When the scarce pieces were all here, and when a seeder that lacked them had them.
    scarce_here: Option<Duration>,
    scarce_spread: Option<Duration>,
    /// The first 32 arrivals.
    head: Vec<usize>,
    /// When the file's first 10% was all there (a player could start).
    start10: Option<Duration>,
    /// The partial video: playable at 30% and 60% of pieces?
    plays: Vec<(u8, bool)>,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(args.first().map_or("target/order", String::as_str));
    let runs: usize = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(3);
    let video = args.get(2).map(PathBuf::from);
    let _ = std::fs::remove_dir_all(&out);
    let data = out.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let file = data.join("movie.mp4");
    match &video {
        Some(v) => {
            std::fs::copy(v, &file).expect("the video copied");
        }
        None => write_pseudo_random(&file, 16 << 20),
    }
    let torrent = make_torrent(&data).await;
    let total = std::fs::metadata(&file)
        .unwrap()
        .len()
        .div_ceil(PIECE as u64) as usize;
    println!(
        "{} pieces of {} KiB ({} MiB)",
        total,
        PIECE >> 10,
        (std::fs::metadata(&file).unwrap().len() >> 20)
    );

    let swarms = [
        Swarm {
            name: "even (4 full seeders, 700/450/300/200 KiB/s)",
            seeders: [700, 450, 300, 200]
                .map(|rate| Seeder {
                    rate,
                    has: None,
                    lacks: None,
                    leaves: None,
                })
                .into(),
            scarce: None,
        },
        Swarm {
            name: "uneven (a slow full seeder 200 KiB/s; first half and second half at 700 KiB/s)",
            seeders: vec![
                Seeder {
                    rate: 200,
                    has: None,
                    lacks: None,
                    leaves: None,
                },
                Seeder {
                    rate: 700,
                    has: Some(0..total / 2),
                    lacks: None,
                    leaves: None,
                },
                Seeder {
                    rate: 700,
                    has: Some(total / 2..total),
                    lacks: None,
                    leaves: None,
                },
            ],
            scarce: None,
        },
        Swarm {
            name: "scarce (2 seeders at 800 KiB/s lack 8 pieces at 80%, which a 150 KiB/s seeder alone has; it leaves after 4 s)",
            seeders: vec![
                Seeder {
                    rate: 800,
                    has: None,
                    lacks: Some(total * 8 / 10..total * 8 / 10 + 8),
                    leaves: None,
                },
                Seeder {
                    rate: 800,
                    has: None,
                    lacks: Some(total * 8 / 10..total * 8 / 10 + 8),
                    leaves: None,
                },
                Seeder {
                    rate: 150,
                    has: None,
                    lacks: None,
                    leaves: Some(Duration::from_secs(4)),
                },
            ],
            scarce: Some(total * 8 / 10..total * 8 / 10 + 8),
        },
    ];
    let orders = [
        (
            "in order, first and last first",
            PieceOrder::InOrder {
                first_and_last_first: true,
            },
        ),
        (
            "in order, strictly",
            PieceOrder::InOrder {
                first_and_last_first: false,
            },
        ),
        ("rarest first", PieceOrder::RarestFirst),
    ];
    // ORDER_SWARM=0 and ORDER_ONLY=0,2 (their indexes): only those, for more runs or tracing.
    let only = |var: &str, i: usize| {
        std::env::var(var)
            .ok()
            .is_none_or(|v| v.split(',').any(|x| x.trim().parse() == Ok(i)))
    };
    for (s, swarm) in swarms.iter().enumerate() {
        if !only("ORDER_SWARM", s) {
            continue;
        }
        println!("\n## {}", swarm.name);
        for (o, (name, order)) in orders.into_iter().enumerate() {
            if !only("ORDER_ONLY", o) {
                continue;
            }
            let mut all = Vec::new();
            for r in 0..runs {
                let dir = out.join(format!("s{s}-{r}-{}", name.replace([' ', ','], "")));
                let run =
                    measure(&dir, &file, &torrent, total, swarm, order, video.is_some()).await;
                all.push(run);
            }
            report(name, &all, total);
        }
    }
}

fn report(name: &str, runs: &[Run], total: usize) {
    let mut times: Vec<f64> = runs
        .iter()
        .filter_map(|r| r.finished)
        .map(|d| d.as_secs_f64())
        .collect();
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let unfinished = runs.len() - times.len();
    let med = |v: &[f64]| {
        if v.is_empty() {
            f64::NAN
        } else {
            v[v.len() / 2]
        }
    };
    let pct = |p: usize| p * 100 / total.max(1);
    let prefix: Vec<String> = (0..3)
        .map(|q| {
            let mut v: Vec<usize> = runs.iter().map(|r| r.prefix[q]).collect();
            v.sort_unstable();
            format!("{}%", pct(v[v.len() / 2]))
        })
        .collect();
    let scarce = |f: fn(&Run) -> Option<Duration>| {
        let mut v: Vec<f64> = runs.iter().filter_map(f).map(|d| d.as_secs_f64()).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let missing = runs.len() - v.len();
        if v.is_empty() && missing == runs.len() {
            "never".to_owned()
        } else {
            format!(
                "{:.1} s{}",
                med(&v),
                if missing > 0 {
                    format!(" ({missing} never)")
                } else {
                    String::new()
                }
            )
        }
    };
    println!(
        "- {name}: finished in {:.1} s (median of {}; runs {:?}){}; the start there at 25/50/75% of pieces: {}{}{}",
        med(&times),
        times.len(),
        times
            .iter()
            .map(|t| (t * 10.0).round() / 10.0)
            .collect::<Vec<_>>(),
        if unfinished > 0 {
            format!(", {unfinished} unfinished")
        } else {
            String::new()
        },
        prefix.join(" / "),
        if runs
            .iter()
            .any(|r| r.scarce_here.is_some() || r.scarce_spread.is_some())
            || runs.iter().all(|r| r.finished.is_none())
        {
            format!(
                "; scarce pieces here {}, spread to another peer {}",
                scarce(|r| r.scarce_here),
                scarce(|r| r.scarce_spread)
            )
        } else {
            String::new()
        },
        if runs[0].plays.is_empty() {
            String::new()
        } else {
            format!(
                "; the partial video plays: {}",
                runs[0]
                    .plays
                    .iter()
                    .map(|(p, ok)| format!("{p}% {}", if *ok { "yes" } else { "no" }))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    );
    let mut s10: Vec<f64> = runs
        .iter()
        .filter_map(|r| r.start10)
        .map(|d| d.as_secs_f64())
        .collect();
    s10.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "  the first 10% there after {:.1} s (median; runs {:?})",
        med(&s10),
        s10.iter()
            .map(|t| (t * 10.0).round() / 10.0)
            .collect::<Vec<_>>()
    );
    println!("  first arrivals: {:?}", runs[0].head);
}

#[allow(clippy::too_many_arguments)]
async fn measure(
    dir: &Path,
    file: &Path,
    torrent: &[u8],
    total: usize,
    swarm: &Swarm,
    order: PieceOrder,
    video: bool,
) -> Run {
    let name = file.file_name().unwrap();
    // The seeders, each with its own copy: the pieces it lacks are zeroed, so its check finds
    // them missing.
    let mut seeders = Vec::new();
    let mut peers = Vec::new();
    for (n, s) in swarm.seeders.iter().enumerate() {
        let folder = dir.join(format!("seeder{n}"));
        std::fs::create_dir_all(&folder).unwrap();
        let copy = folder.join(name);
        std::fs::copy(file, &copy).unwrap();
        let held = |i: usize| {
            s.has.as_ref().is_none_or(|r| r.contains(&i))
                && !s.lacks.as_ref().is_some_and(|r| r.contains(&i))
        };
        zero_pieces(&copy, total, |i| !held(i));
        let t = Torrents::start(Config {
            dht: false,
            upnp: false,
            lsd: false,
            offline: true,
            require_authorization: false,
            ..Config::new(folder.join("state"))
        })
        .await
        .unwrap();
        t.set_upload_limit(Some(s.rate << 10));
        let hash = t
            .add(torrent, &folder, None, Vec::new(), false)
            .await
            .unwrap();
        // Checked: Finished (all), or Downloading (partial: it waits for peers).
        for _ in 0..600 {
            let st = t.stats(&hash).unwrap();
            if matches!(st.phase, Phase::Finished | Phase::Downloading) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        peers.push(t.listen_addr().unwrap());
        seeders.push((t, hash, s.leaves, s.lacks.is_some()));
    }
    let leecher = Torrents::start(Config {
        dht: false,
        upnp: false,
        lsd: false,
        offline: true,
        require_authorization: false,
        ..Config::new(dir.join("leecher-state"))
    })
    .await
    .unwrap();
    let got = dir.join("got");
    let hash = leecher.add(torrent, &got, None, peers, true).await.unwrap();
    leecher.set_order(&hash, order).unwrap();
    let start = Instant::now();
    leecher.resume(&hash).await.unwrap();

    let mut run = Run::default();
    let mut seen = vec![false; total];
    let mut done = 0usize;
    let mut quarter = 0;
    let mut plays_at = if video && matches!(order, PieceOrder::InOrder { .. }) {
        vec![30u8, 60]
    } else {
        Vec::new()
    };
    let deadline = start + Duration::from_secs(90);
    let trace = std::env::var_os("ORDER_TRACE").is_some();
    let only: Option<usize> = std::env::var("ORDER_SWARM")
        .ok()
        .and_then(|v| v.parse().ok());
    let _ = only;
    let addrs: Vec<String> = seeders
        .iter()
        .map(|(t, ..)| t.listen_addr().map(|a| a.to_string()).unwrap_or_default())
        .collect();
    let mut next_trace = Duration::ZERO;
    loop {
        let now = start.elapsed();
        if trace && now >= next_trace {
            next_trace += Duration::from_secs(1);
            let peers = leecher.peers(&hash);
            let per: Vec<String> = addrs
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let kib = peers
                        .iter()
                        .filter(|p| p.addr == *a)
                        .map(|p| p.fetched)
                        .sum::<u64>()
                        >> 10;
                    format!("s{i} {kib} KiB")
                })
                .collect();
            let head = seen.iter().take_while(|s| **s).count();
            eprintln!(
                "  t={:>4.1}s done {done}/{total} head {head} | {} | {} peers",
                now.as_secs_f64(),
                per.join(", "),
                peers.len()
            );
        }
        for (t, _, leaves, _) in &mut seeders {
            if leaves.is_some_and(|l| now >= l) {
                // It leaves the swarm: its session stops (its peers lose it).
                t.stop().await;
                *leaves = None;
            }
        }
        let have = leecher.pieces(&hash);
        for (i, h) in have.iter().enumerate() {
            if *h && !seen[i] {
                seen[i] = true;
                done += 1;
                if run.head.len() < 32 {
                    run.head.push(i);
                }
            }
        }
        if run.start10.is_none() && seen.iter().take_while(|s| **s).count() * 10 >= total {
            run.start10 = Some(now);
        }
        while quarter < 3 && done * 4 >= total * (quarter + 1) {
            run.prefix[quarter] = seen.iter().take_while(|s| **s).count();
            quarter += 1;
        }
        if let Some(r) = &swarm.scarce {
            if run.scarce_here.is_none() && r.clone().all(|i| seen[i]) {
                run.scarce_here = Some(now);
            }
            if run.scarce_spread.is_none() {
                for (t, h, _, lacked) in &seeders {
                    if *lacked
                        && t.pieces(h)
                            .get(r.clone())
                            .is_some_and(|p| p.iter().all(|x| *x))
                    {
                        run.scarce_spread = Some(now);
                    }
                }
            }
        }
        if let Some(p) = plays_at.first().copied()
            && done * 100 >= total * p as usize
        {
            plays_at.remove(0);
            run.plays.push((
                p,
                plays(&got.join(name), &dir.join(format!("partial-{p}.mp4"))),
            ));
        }
        if done == total {
            run.finished = Some(now);
            break;
        }
        if Instant::now() > deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    leecher.stop().await;
    for (t, ..) in seeders {
        t.stop().await;
    }
    run
}

/// Whether a copy of the partial file plays from its start: ffprobe reads its length, and
/// ffmpeg decodes its first 3 seconds without an error.
fn plays(partial: &Path, copy: &Path) -> bool {
    if std::fs::copy(partial, copy).is_err() {
        return false;
    }
    let probe = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
        ])
        .arg(copy)
        .output();
    let decode = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-xerror", "-i"])
        .arg(copy)
        .args(["-t", "3", "-f", "null", "-"])
        .output();
    matches!((probe, decode), (Ok(p), Ok(d)) if p.status.success() && !p.stdout.is_empty() && d.status.success() && d.stderr.is_empty())
}

fn zero_pieces(path: &Path, total: usize, zero: impl Fn(usize) -> bool) {
    use std::io::{Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    let len = f.metadata().unwrap().len();
    for i in (0..total).filter(|i| zero(*i)) {
        let at = i as u64 * PIECE as u64;
        let n = (PIECE as u64).min(len - at) as usize;
        f.seek(SeekFrom::Start(at)).unwrap();
        f.write_all(&vec![0u8; n]).unwrap();
    }
}

fn write_pseudo_random(path: &Path, size: usize) {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut buf = vec![0u8; size];
    for c in buf.as_chunks_mut::<8>().0 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        c.copy_from_slice(&x.to_le_bytes());
    }
    std::fs::write(path, buf).unwrap();
}

async fn make_torrent(dir: &Path) -> Vec<u8> {
    let spawner = mdm_torrent::librqbit::spawn_utils::BlockingSpawner::new(2);
    let file = std::fs::read_dir(dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    mdm_torrent::librqbit::create_torrent(
        &file,
        mdm_torrent::librqbit::CreateTorrentOptions {
            name: None,
            trackers: Vec::new(),
            piece_length: Some(PIECE),
        },
        &spawner,
    )
    .await
    .unwrap()
    .as_bytes()
    .unwrap()
    .to_vec()
}
