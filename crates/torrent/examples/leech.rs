//! A leecher on this computer, for measuring what the app sends (its upload limits, its share
//! ratio) without the internet: it downloads a .torrent from peers on the local network only
//! (Local Service Discovery; no DHT, no UPnP, no trackers), printing what arrived each second.
//!
//!   cargo run -p mdm-torrent --example leech -- <file.torrent> <out folder> [seconds] [peer]
//!
//! Stops when done or after `seconds` (default 60); its own state stays in `<out>/.leech`.
//! `peer` (e.g. 127.0.0.1:51413): dial that peer too. On loopback the firewall stays out of the
//! way (LSD names the computer's network address, which a firewall may filter).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use mdm_torrent::{Config, Phase, Torrents};

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(file), Some(out)) = (args.first(), args.get(1)) else {
        eprintln!("usage: leech <file.torrent> <out folder> [seconds]");
        std::process::exit(2);
    };
    let secs: u64 = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(60);
    let peers: Vec<std::net::SocketAddr> = args
        .get(3)
        .and_then(|a| a.parse().ok())
        .into_iter()
        .collect();
    let out = PathBuf::from(out);
    let torrent = std::fs::read(file).expect("the .torrent file");
    // It only dials out (to the peers LSD names), so it doesn't listen: no incoming
    // connections, and no firewall question about them. Its UDP tracker client stays on
    // loopback too (listen_ip), with nothing on every address.
    let t = Torrents::start(Config {
        listen: false,
        listen_ip: Some(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
        upnp: false,
        dht: false,
        lsd: true,
        require_authorization: false,
        ..Config::new(out.join(".leech"))
    })
    .await
    .expect("session started");
    let hash = t
        .add(&torrent, &out, None, peers, false)
        .await
        .expect("added");
    let start = Instant::now();
    let mut last = 0;
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let s = t.stats(&hash).expect("stats");
        println!(
            "{:5.1}s {:9} bytes {:7.1} KB/s peers {}",
            start.elapsed().as_secs_f64(),
            s.fetched,
            (s.fetched.saturating_sub(last)) as f64 / 1024.0,
            s.peers_live
        );
        last = s.fetched;
        if s.phase == Phase::Finished || start.elapsed() >= Duration::from_secs(secs) {
            break;
        }
    }
    t.stop().await;
}
