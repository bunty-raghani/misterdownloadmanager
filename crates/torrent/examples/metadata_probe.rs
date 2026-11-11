//! How long a magnet takes to give its file list, phase by phase (#92): the session's start, the
//! DHT finding nodes, the first peer from the trackers and from the DHT, the first peer that
//! answers, and the file list. Fetches the metadata only, never a file's data. Each run starts a
//! fresh session in a temporary folder, as the app's first torrent does, with everything on
//! (listening on every address: Windows Firewall asks about this program once).
//!
//!   cargo run --release -p mdm-torrent --example metadata_probe -- [options] SOURCE...
//!
//! SOURCE: a magnet link, or a .torrent file (looked up as its magnet link, with its trackers).
//! Options: `--no-trackers` (the DHT alone), `--no-dht` (the trackers alone), `--runs N` (each
//! source N times), `--wait S` (give up after S seconds, 180 by default, as the app),
//! `--state DIR` (the session's folder, kept: a later run starts with the DHT nodes and the port
//! of the one before, as the app's next session does), `--bootstrap HOST:PORT,...` (the DHT's
//! first nodes instead of MDM's public routers, #650), `--listen-ip ADDR` (listen there only:
//! 127.0.0.1 or ::1 keeps Windows Firewall out of it; the DHT then stays on it too, and local
//! peer discovery is off).
//! Prints a Markdown table, a row a run; times in seconds from the session's start. RUST_LOG
//! (`librqbit_dht=debug`, ...) adds librqbit's log on stderr.

use std::time::{Duration, Instant};

use mdm_torrent::{Config, Lookup, LookupStatus, Source, Torrents};

#[derive(Default)]
struct Marks {
    dht8: Option<f64>,
    tracker: Option<f64>,
    dht: Option<f64>,
    connected: Option<f64>,
}

fn secs(v: Option<f64>) -> String {
    v.map_or("–".to_owned(), |s| format!("{s:.1}"))
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    // RUST_LOG=librqbit_dht=debug and the like: librqbit's own account, on stderr.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut flag = |name: &str| {
        let at = args.iter().position(|a| a == name);
        at.map(|i| args.remove(i)).is_some()
    };
    let (no_trackers, no_dht) = (flag("--no-trackers"), flag("--no-dht"));
    let mut value = |name: &str, default: u64| {
        let Some(i) = args.iter().position(|a| a == name) else {
            return default;
        };
        args.remove(i);
        args.remove(i).parse().expect("a number")
    };
    let runs = value("--runs", 1);
    let wait = Duration::from_secs(value("--wait", 180));
    let mut text = |name: &str| {
        let i = args.iter().position(|a| a == name)?;
        args.remove(i);
        Some(args.remove(i))
    };
    let state = text("--state").map(std::path::PathBuf::from);
    let listen_ip = text("--listen-ip").map(|a| a.parse().expect("an IP address"));
    let bootstrap = text("--bootstrap").map(|b| b.split(',').map(str::to_owned).collect());
    if args.is_empty() {
        eprintln!(
            "usage: metadata_probe [--no-trackers] [--no-dht] [--runs N] [--wait S] \
             [--state DIR] [--bootstrap HOST:PORT,...] [--listen-ip ADDR] SOURCE..."
        );
        std::process::exit(2);
    }
    let setup = Setup {
        dht: !no_dht,
        wait,
        state,
        bootstrap,
        listen_ip,
    };

    println!(
        "| source | trackers | DHT | session | DHT ≥ 8 nodes | 1st tracker peer | 1st DHT peer | 1st answer | file list | peers asked / answered |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for source in &args {
        let (name, magnet) = magnet_of(source, no_trackers);
        for _ in 0..runs {
            let row = run(&name, &magnet, &setup).await;
            println!("{row}");
        }
    }
}

/// The magnet link to look up, and a short name for the table.
fn magnet_of(source: &str, no_trackers: bool) -> (String, String) {
    let magnet = if source.starts_with("magnet:") {
        source.to_owned()
    } else {
        let bytes = mdm_torrent::read_file(std::path::Path::new(source)).expect(".torrent read");
        mdm_torrent::magnet_link(&bytes).expect("a .torrent")
    };
    let magnet = if no_trackers {
        magnet
            .split('&')
            .filter(|kv| !kv.to_ascii_lowercase().starts_with("tr="))
            .collect::<Vec<_>>()
            .join("&")
    } else {
        magnet
    };
    let name = url::Url::parse(&magnet)
        .ok()
        .and_then(|u| {
            u.query_pairs()
                .find(|(k, _)| k == "dn")
                .map(|(_, v)| v.into_owned())
        })
        .unwrap_or_else(|| magnet.chars().take(60).collect());
    (name, magnet)
}

struct Setup {
    dht: bool,
    wait: Duration,
    state: Option<std::path::PathBuf>,
    bootstrap: Option<Vec<String>>,
    listen_ip: Option<std::net::IpAddr>,
}

async fn run(name: &str, magnet: &str, setup: &Setup) -> String {
    let tmp = tempfile::tempdir().expect("a temporary folder");
    let dir = setup.state.clone().unwrap_or_else(|| tmp.path().to_owned());
    let t0 = Instant::now();
    let t = Torrents::start(Config {
        dht: setup.dht,
        dht_bootstrap: setup.bootstrap.clone(),
        listen_ip: setup.listen_ip,
        // Local peer discovery listens on every address (UDP 6771): not with one address.
        lsd: setup.listen_ip.is_none(),
        ..Config::new(dir)
    })
    .await
    .expect("session started");
    let wait = setup.wait;
    eprintln!("probe: listening on {:?}", t.listen_addr());
    let started = t0.elapsed().as_secs_f64();
    let lookup = std::sync::Arc::new(Lookup::default());
    let marks = std::sync::Arc::new(std::sync::Mutex::new(Marks::default()));
    let watch = {
        let (lookup, marks) = (lookup.clone(), marks.clone());
        tokio::spawn(async move {
            loop {
                {
                    let s: LookupStatus = lookup.status();
                    let now = Some(t0.elapsed().as_secs_f64());
                    let mut m = marks.lock().unwrap();
                    if s.dht_nodes >= 8 && m.dht8.is_none() {
                        m.dht8 = now;
                    }
                    if s.from_trackers > 0 && m.tracker.is_none() {
                        m.tracker = now;
                    }
                    if s.from_dht > 0 && m.dht.is_none() {
                        m.dht = now;
                    }
                    if s.connected > 0 && m.connected.is_none() {
                        m.connected = now;
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
    };
    let res = t
        .metadata(&Source::Magnet(magnet.to_owned()), wait, Some(&lookup))
        .await;
    let done = t0.elapsed().as_secs_f64();
    tokio::time::sleep(Duration::from_millis(30)).await;
    watch.abort();
    let s = lookup.status();
    let list = match &res {
        Ok(meta) => format!("**{done:.1}** ({} files)", meta.files.len()),
        Err(e) => format!("none at {done:.0}: {e}"),
    };
    let row = {
        let m = marks.lock().unwrap();
        format!(
            "| {name} | {} | {} | {started:.1} | {} | {} | {} | {} | {list} | {} / {} |",
            s.trackers,
            if s.dht { "on" } else { "off" },
            secs(m.dht8),
            secs(m.tracker),
            secs(m.dht),
            secs(m.connected),
            s.peers,
            s.connected,
        )
    };
    t.stop().await;
    row
}
