//! What a download keeps in memory while the disk holds its writes is bounded by what is
//! allocated, not only by what the engine counts (`HOLD_MAX`, 256 MiB): every allocation of this
//! test's process is counted (its own binary, so no other test allocates meanwhile).

use mdm_engine::{Destination, Download, Options, Request};
use mdm_labserver::{Config, Faults, LabServer, content};

#[global_allocator]
static ALLOC: cap::Cap<std::alloc::System> = cap::Cap::new(std::alloc::System, usize::MAX);

/// The engine's `HOLD_MAX`.
const HOLD_MAX: usize = 256 << 20;

/// The disk holds every write (this test takes the slow disk's gate, as a flush on NTFS does)
/// while 8 connections receive as fast as the lab server sends (no rate: its chunks come in all
/// sizes, as from a fast real server), until their memory stops growing: full, they wait for
/// the disk. Before, the engine counted 256 MiB there and allocated more: a batch grew past its
/// 256 KiB when a chunk overran it (Vec doubling), and a write joined its batches into one
/// buffer, grown in steps up to 4 MiB. Measured here: 309 MiB allocated before on Windows (279
/// and 281 MiB on Linux), 266 to 269 MiB after (0.5.2, one buffer per connection grown by
/// doubling to its share of the cap, did worse).
#[tokio::test]
async fn memory_held_for_the_disk_stays_within_its_cap() {
    const SIZE: u64 = 512 << 20;
    let s = LabServer::start(Config {
        faults: Faults::parse("").unwrap(),
        ..Config::default()
    })
    .await
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    // A disk as fast as the real one, but for the gate.
    let disk = mdm_engine::slowdisk::SlowDisk::slow_flush(std::time::Duration::ZERO);
    let gate = disk.gate.clone();
    let before = ALLOC.allocated();
    let d = Download::start(
        Request {
            url: s.url("/gen/512M/a.bin?seed=946").parse().unwrap(),
            headers: Default::default(),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        Options {
            retry_delay: std::time::Duration::from_millis(50),
            slow_disk: Some(disk),
            ..Options::default()
        },
    );
    // The disk holds the writes until the memory is about full and stopped growing for a
    // second (each connection waiting for the disk), on a slow machine too (at most a minute).
    let (hold, release) = std::sync::mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        let _writes_wait = gate.write().unwrap();
        let _ = release.recv();
    });
    let held = {
        let began = std::time::Instant::now();
        let (mut most, mut grew_at) = (0, std::time::Instant::now());
        let full = |most: usize, grew_at: std::time::Instant| {
            most >= HOLD_MAX * 7 / 8 && grew_at.elapsed() >= std::time::Duration::from_secs(1)
        };
        while !full(most, grew_at) && began.elapsed() < std::time::Duration::from_secs(60) {
            let now = ALLOC.allocated().saturating_sub(before);
            if now > most + (1 << 20) {
                (most, grew_at) = (now, std::time::Instant::now());
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        most
    };
    drop(hold);
    holder.join().unwrap();
    let out = d.wait().await.expect("download succeeds");
    let peak = ALLOC.max_allocated().saturating_sub(before);
    eprintln!(
        "allocated beyond the start: {} MiB at most, {} MiB while the disk held the writes",
        peak >> 20,
        held >> 20
    );
    // The memory did fill up (else this proves nothing)...
    assert!(held >= HOLD_MAX * 3 / 4, "{} MiB held", held >> 20);
    // ...and no further than the cap, with room for the connections' own receive buffers, the
    // lab server's, and a chunk past the cap per connection.
    assert!(
        peak <= HOLD_MAX + (20 << 20),
        "{} MiB allocated at most, for {} MiB held",
        peak >> 20,
        HOLD_MAX >> 20
    );
    // The file is whole (read in pieces: this test counts memory).
    assert_eq!(out.size, SIZE);
    let mut f = std::fs::File::open(&out.path).unwrap();
    let (mut piece, mut want) = (vec![0; 4 << 20], vec![0; 4 << 20]);
    let mut at = 0;
    while at < SIZE {
        std::io::Read::read_exact(&mut f, &mut piece).unwrap();
        content::fill(946, at, &mut want);
        assert!(piece == want, "content mismatch at {at}");
        at += piece.len() as u64;
    }
}
