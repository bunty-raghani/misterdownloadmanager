//! A small torrent swarm on this computer, for trying the app's torrent downloads and taking
//! their pictures without the internet: a few seeders (each with its own upload limit) of a
//! generated multi-file torrent, and a tracker that hands them to whoever announces. Writes
//! `<out>/Open Movie Night.torrent`, then seeds until stopped.
//!
//!   cargo run --release -p mdm-torrent --example swarm -- target/swarm [size MB] [seeders] [port] [lsd|-] [piece KB]
//!
//! The app leaves out trackers on this computer (#40), so for the app add `lsd`: the seeders
//! then answer Local Service Discovery, and the app finds them that way (Torrent settings'
//! local peers on). Without it they talk to nothing but the tracker, and everything stays on
//! loopback (127.0.0.1). With `lsd` the seeders listen on the computer's network addresses, which
//! LSD names (so Windows Firewall asks about this program once). Pieces are 1 MiB unless
//! `piece KB` says otherwise (smaller ones make the list's progress, counted in whole pieces,
//! finer for measuring speeds). Two swarms of different sizes are two different torrents.

use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use mdm_torrent::{Config, Phase, Source, Torrents, librqbit};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const NAME: &str = "Open Movie Night";

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(args.first().map_or("target/swarm", String::as_str));
    let total: u64 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(600) << 20;
    let seeders: usize = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(4);
    let port: u16 = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(6969);
    let lsd = args.get(4).is_some_and(|a| a == "lsd");
    let piece: u32 = args
        .get(5)
        .and_then(|a| a.parse::<u32>().ok())
        .map_or(1 << 20, |kb| kb.max(16) << 10);

    let data = out.join("data").join(NAME);
    make_files(&data, total);
    let tracker = format!("http://127.0.0.1:{port}/announce");
    let spawner = librqbit::spawn_utils::BlockingSpawner::new(4);
    let made = librqbit::create_torrent(
        &data,
        librqbit::CreateTorrentOptions {
            name: Some(NAME),
            trackers: vec![tracker.clone()],
            piece_length: Some(piece),
        },
        &spawner,
    )
    .await
    .expect("torrent made");
    let torrent = made.as_bytes().expect("torrent written").to_vec();
    let file = out.join(format!("{NAME}.torrent"));
    std::fs::write(&file, &torrent).expect("torrent saved");

    // The seeders, slower and faster ones (KB/s), so their shares differ.
    let mut peers = Vec::new();
    let mut sessions = Vec::new();
    for n in 0..seeders {
        let t = Torrents::start(Config {
            dht: false,
            upnp: false,
            lsd,
            offline: !lsd,
            require_authorization: false,
            ..Config::new(out.join(format!("seeder{n}")))
        })
        .await
        .expect("seeder started");
        t.set_upload_limit(Some([700, 450, 300, 200][n % 4] << 10));
        let hash = t
            .add(&torrent, &data, None, Vec::new(), false)
            .await
            .expect("seeding");
        while t.stats(&hash).expect("listed").phase != Phase::Finished {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        peers.push(t.listen_addr().expect("listening"));
        sessions.push(t);
    }
    println!("{}", file.display());
    // 0 seeders: only the .torrent and its files (for the app to seed them itself).
    if let Some(first) = sessions.first() {
        let meta = first
            .metadata(
                &Source::File(torrent.clone()),
                Duration::from_secs(10),
                None,
            )
            .await
            .expect("readable");
        println!(
            "{} seeders of {} ({} files, {} MB), tracker {tracker}",
            peers.len(),
            meta.info_hash,
            meta.files.len(),
            meta.total() >> 20
        );
        // For an off-screen test copy of the app (MDM_LAB_PEERS, torrents.rs).
        let list: Vec<String> = peers.iter().map(|a| a.to_string()).collect();
        println!("peers {}", list.join(","));
    } else {
        let hash = librqbit::torrent_from_bytes(&torrent)
            .expect("readable")
            .info_hash
            .as_string();
        println!("0 seeders of {hash}, tracker {tracker}");
    }
    std::io::stdout().flush().ok();
    tracker_loop(port, peers).await;
}

/// The torrent's files: one large, a smaller one, extras and two small texts. Pseudo-random
/// bytes (xorshift), so no two pieces are alike. Kept when they are already there.
fn make_files(dir: &Path, total: u64) {
    let files = [
        ("open-movie-2160p.mkv", total * 70 / 100),
        ("open-movie-1080p.mkv", total * 24 / 100),
        ("extras/making-of.mp4", total * 6 / 100),
        ("subtitles/en.srt", 48 << 10),
        ("readme.txt", 2 << 10),
    ];
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    for (name, size) in files {
        let path = dir.join(name);
        if path.metadata().is_ok_and(|m| m.len() == size) {
            continue;
        }
        std::fs::create_dir_all(path.parent().expect("in a folder")).expect("folder");
        let mut f = std::io::BufWriter::new(std::fs::File::create(&path).expect("file"));
        let mut left = size;
        let mut buf = vec![0u8; 1 << 20];
        while left > 0 {
            for c in buf.as_chunks_mut::<8>().0 {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                c.copy_from_slice(&x.to_le_bytes());
            }
            let n = left.min(buf.len() as u64) as usize;
            f.write_all(&buf[..n]).expect("written");
            left -= n as u64;
        }
    }
}

/// An HTTP tracker that answers every announce with all the seeders (compact peers).
async fn tracker_loop(port: u16, peers: Vec<SocketAddr>) {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .await
        .expect("tracker port free");
    let mut compact = Vec::new();
    for p in &peers {
        if let IpAddr::V4(ip) = p.ip() {
            compact.extend_from_slice(&ip.octets());
            compact.extend_from_slice(&p.port().to_be_bytes());
        }
    }
    let mut body = format!(
        "d8:completei{}e10:incompletei0e8:intervali60e5:peers{}:",
        peers.len(),
        compact.len()
    )
    .into_bytes();
    body.extend_from_slice(&compact);
    body.push(b'e');
    loop {
        let Ok((mut s, _)) = listener.accept().await else {
            continue;
        };
        let body = body.clone();
        tokio::spawn(async move {
            let mut req = [0u8; 4096];
            let _ = s.read(&mut req).await;
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(head.as_bytes()).await;
            let _ = s.write_all(&body).await;
        });
    }
}
