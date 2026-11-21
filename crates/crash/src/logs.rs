//! The log MDM keeps in release builds too: `<data>/logs/mdm.log`, rolled over at a size into
//! `mdm.1.log` … `mdm.3.log` (the crash monitor keeps `monitor.log` the same way). Each line is
//! handed to the log's own thread, which writes it at once. A report takes the last lines of
//! MDM's: [`flush`] first, so the file has them all, or, when the disk didn't let them through in
//! time, the last lines the app kept in memory ([`recent`], sent with a panic or a freeze).
//!
//! The thread that logs never writes the file itself, nor waits for it (#513, #1354): while
//! Windows held every write on C: for a shadow copy, or a disk didn't answer for seconds, a line
//! logged on the window's thread kept the window from answering until the write was let go. The
//! lines wait in a queue of [`QUEUE`] at most; a line logged while it is full (the disk stuck that
//! long) is dropped instead, counted, and the count is written in the file once the disk answers.

use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, TrySendError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// A log file grows to this, then rolls over.
pub const MAX_BYTES: u64 = 2 * 1024 * 1024;
/// Rolled-over files kept (`mdm.1.log` newest).
pub const KEEP: usize = 3;
/// Lines waiting for the log's thread at most; a line logged while this many wait is dropped.
pub const QUEUE: usize = 10_000;
/// The last lines kept in memory, for a report made while the disk holds the file's (#1354).
pub const RECENT: usize = 150;

/// The log of a data folder.
pub fn dir(data: &Path) -> PathBuf {
    data.join("logs")
}

/// The app's log; the monitor's is "monitor".
pub const APP: &str = "mdm";
pub const MONITOR: &str = "monitor";

fn current(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.log"))
}

fn rolled(dir: &Path, name: &str, n: usize) -> PathBuf {
    dir.join(format!("{name}.{n}.log"))
}

/// What the log's thread is handed.
enum Msg {
    /// A line, logged after `dropped` lines that found the queue full.
    Line { dropped: u64, line: Vec<u8> },
    /// Only a count of dropped lines (a flush with no line after them).
    Dropped(u64),
}

/// The writer behind the log: hands each line to the log's thread, which appends it to
/// `mdm.log`, rolling it over past [`MAX_BYTES`]. Writing never waits for the disk.
#[derive(Clone)]
pub struct Rolling {
    shared: Arc<Shared>,
}

struct Shared {
    /// Held by a thread that logs only while it hands its line over (never across the disk):
    /// the queue's order is the order the lines were logged in, and `queued` counts it exactly.
    hand: Mutex<Hand>,
    done: Arc<Done>,
}

struct Hand {
    tx: mpsc::SyncSender<Msg>,
    /// Messages handed over so far.
    queued: u64,
    /// Lines dropped since the last one handed over.
    dropped: u64,
    /// The last [`RECENT`] lines logged, written or not.
    recent: VecDeque<String>,
    /// The start of a line written in pieces.
    partial: String,
}

/// How many messages the log's thread is done with (written, or failed to be).
#[derive(Default)]
struct Done {
    count: Mutex<u64>,
    changed: Condvar,
}

impl Done {
    fn one_more(&self) {
        *self.count.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        self.changed.notify_all();
    }

    /// Wait, until `deadline`, for `target` messages done.
    fn wait_for(&self, target: u64, deadline: Instant) -> bool {
        let mut count = self.count.lock().unwrap_or_else(|e| e.into_inner());
        while *count < target {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            count = self
                .changed
                .wait_timeout(count, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        true
    }
}

struct Inner {
    dir: PathBuf,
    name: &'static str,
    /// The open log file (another writer in a test).
    file: Option<Box<dyn Write + Send>>,
    size: u64,
}

impl Rolling {
    pub fn new(dir: PathBuf, name: &'static str) -> io::Result<Rolling> {
        fs::create_dir_all(&dir)?;
        let path = current(&dir, name);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Rolling::over(
            Inner {
                dir,
                name,
                file: Some(Box::new(file)),
                size,
            },
            QUEUE,
        )
    }

    /// The log's thread, writing into `inner`, with up to `queue` lines waiting for it.
    fn over(mut inner: Inner, queue: usize) -> io::Result<Rolling> {
        let name = inner.name;
        let (tx, rx) = mpsc::sync_channel(queue);
        let done = Arc::new(Done::default());
        let thread_done = done.clone();
        std::thread::Builder::new()
            .name(format!("{name} log"))
            .spawn(move || {
                // Ends once every `Rolling` is gone.
                while let Ok(msg) = rx.recv() {
                    match msg {
                        Msg::Line { dropped, line } => {
                            inner.dropped(dropped);
                            inner.append(&line);
                        }
                        Msg::Dropped(n) => inner.dropped(n),
                    }
                    if let Some(f) = inner.file.as_mut() {
                        let _ = f.flush();
                    }
                    thread_done.one_more();
                }
            })?;
        Ok(Rolling {
            shared: Arc::new(Shared {
                hand: Mutex::new(Hand {
                    tx,
                    queued: 0,
                    dropped: 0,
                    recent: VecDeque::with_capacity(RECENT),
                    partial: String::new(),
                }),
                done,
            }),
        })
    }

    fn hand(&self) -> MutexGuard<'_, Hand> {
        self.shared.hand.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Wait, up to `within`, until every line handed over so far is written (and the count of
    /// those dropped). False when it wasn't (the disk held, or the log's thread gone).
    pub fn flush_within(&self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        loop {
            let (target, count_waits) = {
                let mut h = self.hand();
                if h.dropped > 0 && h.tx.try_send(Msg::Dropped(h.dropped)).is_ok() {
                    h.queued += 1;
                    h.dropped = 0;
                }
                (h.queued, h.dropped > 0)
            };
            if !self.shared.done.wait_for(target, deadline) {
                return false;
            }
            // The queue was still full for the count: there's room now.
            if !count_waits {
                return true;
            }
        }
    }

    /// The last [`RECENT`] lines logged, oldest first, whether the disk took them or not.
    pub fn recent(&self) -> Vec<String> {
        self.hand().recent.iter().cloned().collect()
    }
}

impl Hand {
    /// Keep the lines of `buf` among the recent ones (a line written in pieces once it ends).
    fn remember(&mut self, buf: &[u8]) {
        self.partial.push_str(&String::from_utf8_lossy(buf));
        while let Some(end) = self.partial.find('\n') {
            let line: String = self.partial.drain(..=end).collect();
            if self.recent.len() == RECENT {
                self.recent.pop_front();
            }
            self.recent
                .push_back(line.trim_end_matches(['\n', '\r']).to_owned());
        }
    }
}

impl Inner {
    fn roll(&mut self) {
        self.file = None;
        let (dir, name) = (&self.dir, self.name);
        for n in (1..KEEP).rev() {
            let _ = fs::rename(rolled(dir, name, n), rolled(dir, name, n + 1));
        }
        let _ = fs::rename(current(dir, name), rolled(dir, name, 1));
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(current(dir, name))
            .ok()
            .map(|f| Box::new(f) as Box<dyn Write + Send>);
        self.size = 0;
    }

    fn append(&mut self, line: &[u8]) {
        if self.size + line.len() as u64 > MAX_BYTES {
            self.roll();
        }
        if let Some(f) = self.file.as_mut()
            && f.write_all(line).is_ok()
        {
            self.size += line.len() as u64;
        }
    }

    /// Say in the file that `n` lines were dropped (the queue was full), as the log's own lines
    /// read (time, level, thread, target).
    fn dropped(&mut self, n: u64) {
        if n == 0 {
            return;
        }
        let lines = if n == 1 {
            "1 log line was dropped".to_owned()
        } else {
            format!("{n} log lines were dropped")
        };
        let line = format!(
            "{}  WARN {} log mdm_crash::logs: {lines} while the disk didn't answer\n",
            chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.6fZ"),
            self.name,
        );
        self.append(line.as_bytes());
    }
}

impl Write for Rolling {
    /// Hands `buf` over as it is: the log's lines come in one write each (tracing's formatter
    /// writes an event at once), so a line dropped is a whole one.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let line = buf.to_vec();
        let mut h = self.hand();
        h.remember(buf);
        let dropped = h.dropped;
        match h.tx.try_send(Msg::Line { dropped, line }) {
            Ok(()) => {
                h.queued += 1;
                h.dropped = 0;
            }
            // The disk hasn't taken the last QUEUE lines: this one waits for none of them.
            Err(TrySendError::Full(_)) => h.dropped += 1,
            Err(TrySendError::Disconnected(_)) => {
                return Err(io::Error::other("the log's thread is gone"));
            }
        }
        Ok(buf.len())
    }

    /// Waits, a while at most, for the log's thread to have written everything (tests).
    fn flush(&mut self) -> io::Result<()> {
        if self.flush_within(Duration::from_secs(5)) {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the log wasn't written in time",
            ))
        }
    }
}

/// The log [`init`] started in this process.
static LOG: std::sync::OnceLock<Rolling> = std::sync::OnceLock::new();

/// Wait, up to `within`, until every line logged so far is in the file: before a report reads
/// the log's end, and as MDM ends (lines still on their way would be lost with the process).
/// True when they are, or when there is no log.
pub fn flush(within: Duration) -> bool {
    LOG.get().is_none_or(|log| log.flush_within(within))
}

/// The last [`RECENT`] lines this process logged, from memory: a report's log while the disk
/// holds the file's (none before [`init`]).
pub fn recent() -> Vec<String> {
    LOG.get().map(Rolling::recent).unwrap_or_default()
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Rolling {
    type Writer = Rolling;
    fn make_writer(&'a self) -> Rolling {
        self.clone()
    }
}

/// Start logging into `<data>/logs`: MDM's own crates at `info`, everything else (egui, winit,
/// the network stack) at `warn`; `MDM_LOG` overrides it with an `EnvFilter` (e.g.
/// `MDM_LOG=debug`). Returns false when the folder can't be written (MDM runs on without a log).
pub fn init(data: &Path, name: &'static str) -> bool {
    use tracing_subscriber::EnvFilter;
    let Ok(writer) = Rolling::new(dir(data), name) else {
        return false;
    };
    let _ = LOG.set(writer.clone());
    let filter = std::env::var("MDM_LOG")
        .ok()
        .and_then(|f| EnvFilter::try_new(f).ok())
        .unwrap_or_else(|| EnvFilter::new("warn,minidump=error,minidump_processor=error,minidump_unwind=error,breakpad_symbols=error,mdm=info,mdm_app=info,misterdownloadmanager=info,mdm_engine=info,mdm_torrent=info,mdm_crash=info,mdm_tools=info,mdm_ipc=info"));
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_target(true)
        .with_thread_names(true)
        .with_env_filter(filter)
        .try_init()
        .is_ok()
}

/// The last `n` lines of the log (across the roll-over: the end of `mdm.1.log` when `mdm.log`
/// is short).
pub fn tail(data: &Path, n: usize) -> Vec<String> {
    let dir = dir(data);
    let mut lines: Vec<String> = Vec::new();
    for path in [current(&dir, APP), rolled(&dir, APP, 1)] {
        let Ok(text) = fs::read(&path) else { continue };
        let text = String::from_utf8_lossy(&text);
        let mut these: Vec<String> = text.lines().map(str::to_owned).collect();
        these.append(&mut lines);
        lines = these;
        if lines.len() >= n {
            break;
        }
    }
    let skip = lines.len().saturating_sub(n);
    lines.split_off(skip)
}

/// A report's log: the file's last `n` lines, or the last lines the app had in memory
/// (`recent`, sent with its panic or freeze) when the file doesn't have the newest of them yet
/// (the disk held them past the app's flush, #1354). The file's are taken when it has caught up:
/// it may hold lines logged since.
pub fn tail_or_recent(data: &Path, n: usize, recent: &[String]) -> Vec<String> {
    let file = tail(data, n);
    match recent.last() {
        Some(newest) if !file.contains(newest) => recent[recent.len().saturating_sub(n)..].to_vec(),
        _ => file,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn it_rolls_over_and_the_tail_spans_the_files() {
        let tmp = tempfile::tempdir().unwrap();
        let mut w = Rolling::new(dir(tmp.path()), APP).unwrap();
        let line = "x".repeat(1023) + "\n";
        // Past the size: the first lines move to mdm.1.log.
        for _ in 0..(MAX_BYTES / 1024 + 10) {
            w.write_all(line.as_bytes()).unwrap();
        }
        writeln!(w, "last").unwrap();
        w.flush().unwrap();
        assert!(rolled(&dir(tmp.path()), APP, 1).exists());
        let t = tail(tmp.path(), 20);
        assert_eq!(t.len(), 20);
        assert_eq!(t.last().unwrap(), "last");
    }

    /// A disk that holds every write until it is let go (Windows making a shadow copy, a disk
    /// that doesn't answer); `out` gets what it lets through.
    #[derive(Clone)]
    struct Held {
        open: Arc<(Mutex<bool>, Condvar)>,
        out: Arc<Mutex<Vec<u8>>>,
        /// A write is waiting for the disk now (the log's thread holds a line it took).
        waiting: Arc<AtomicBool>,
    }

    impl Held {
        fn new(open: bool) -> Held {
            Held {
                open: Arc::new((Mutex::new(open), Condvar::new())),
                out: Arc::new(Mutex::new(Vec::new())),
                waiting: Arc::new(AtomicBool::new(false)),
            }
        }

        /// Wait, up to 5 s, until the log's thread is held writing a line.
        fn until_held(&self) {
            let until = Instant::now() + Duration::from_secs(5);
            while !self.waiting.load(Ordering::SeqCst) {
                assert!(
                    Instant::now() < until,
                    "the log's thread never reached the disk"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
        }

        fn set(&self, open: bool) {
            *self.open.0.lock().unwrap() = open;
            self.open.1.notify_all();
        }

        fn text(&self) -> String {
            String::from_utf8(self.out.lock().unwrap().clone()).unwrap()
        }

        /// A log writing into this disk, with `queue` lines waiting at most.
        fn log(&self, queue: usize) -> Rolling {
            let tmp = std::env::temp_dir();
            Rolling::over(
                Inner {
                    dir: tmp,
                    name: APP,
                    file: Some(Box::new(self.clone())),
                    size: 0,
                },
                queue,
            )
            .unwrap()
        }
    }

    impl Write for Held {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let (open, cv) = &*self.open;
            let mut open = open.lock().unwrap();
            while !*open {
                self.waiting.store(true, Ordering::SeqCst);
                open = cv.wait(open).unwrap();
            }
            self.waiting.store(false, Ordering::SeqCst);
            self.out.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Log `n` lines ("line 0" …) on a thread of its own (the window's), and how long it took;
    /// fails when it waited for the disk.
    fn log_from_another_thread(log: &Rolling, n: usize) -> Duration {
        let (said, took) = mpsc::channel();
        let mut w = log.clone();
        let window = std::thread::spawn(move || {
            let start = Instant::now();
            for k in 0..n {
                w.write_all(format!("line {k}\n").as_bytes()).unwrap();
            }
            said.send(start.elapsed()).unwrap();
        });
        let took = took
            .recv_timeout(Duration::from_secs(5))
            .expect("logging waited for the held disk");
        window.join().unwrap();
        took
    }

    /// #513: a line is logged at once while the disk holds the log's writes (Windows making a
    /// shadow copy held every write on C: for 8 s, and the window with them): only the log's
    /// own thread waits, and the lines are all written, in order, once the disk lets go.
    #[test]
    fn logging_never_waits_for_the_disk() {
        let disk = Held::new(false);
        let log = disk.log(QUEUE);
        let took = log_from_another_thread(&log, 100);
        assert!(took < Duration::from_secs(1), "{took:?}");
        assert!(!log.flush_within(Duration::from_millis(100)), "still held");

        disk.set(true);
        assert!(log.flush_within(Duration::from_secs(5)));
        let text = disk.text();
        let lines: Vec<&str> = text.lines().collect();
        let want: Vec<String> = (0..100).map(|k| format!("line {k}")).collect();
        assert_eq!(lines, want, "every line, in order");
    }

    /// #1354: a file whose every write takes a while (a slow disk) slows only the log's thread.
    #[test]
    fn a_slow_disk_slows_only_the_logs_thread() {
        struct Slow(Arc<Mutex<Vec<u8>>>);
        impl Write for Slow {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                std::thread::sleep(Duration::from_millis(50));
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let out = Arc::new(Mutex::new(Vec::new()));
        let log = Rolling::over(
            Inner {
                dir: std::env::temp_dir(),
                name: APP,
                file: Some(Box::new(Slow(out.clone()))),
                size: 0,
            },
            QUEUE,
        )
        .unwrap();
        // 40 lines are 2 s of writing; logging them takes a fraction of one write.
        let took = log_from_another_thread(&log, 40);
        assert!(took < Duration::from_millis(500), "{took:?}");
        assert!(log.flush_within(Duration::from_secs(10)));
        let text = String::from_utf8(out.lock().unwrap().clone()).unwrap();
        assert_eq!(text.lines().count(), 40);
        assert_eq!(text.lines().last(), Some("line 39"));
    }

    /// #1354: the disk stuck past the queue: the lines that don't fit are dropped rather than
    /// waited for, and once the disk answers the file says how many, before the next line.
    #[test]
    fn a_full_queue_drops_lines_and_says_how_many() {
        let disk = Held::new(false);
        let log = disk.log(8);
        let took = log_from_another_thread(&log, 100);
        assert!(took < Duration::from_secs(1), "{took:?}");
        assert!(!log.flush_within(Duration::from_millis(50)), "still held");

        // The disk answers again and takes the lines that waited; the next line logged brings
        // the count with it.
        disk.set(true);
        let waited = log.hand().queued;
        let deadline = Instant::now() + Duration::from_secs(5);
        assert!(log.shared.done.wait_for(waited, deadline));
        writeln!(log.clone(), "after").unwrap();
        assert!(log.flush_within(Duration::from_secs(5)));
        let text = disk.text();
        let lines: Vec<&str> = text.lines().collect();
        // The lines that fit, in order, each count before the line that came after its gap, and
        // the line logged after: every line logged is either written or counted.
        assert_eq!(lines.last(), Some(&"after"), "{lines:?}");
        let (mut kept, mut dropped, mut next) = (0, 0, 0);
        for l in &lines[..lines.len() - 1] {
            if let Some(k) = l.strip_prefix("line ") {
                let k: usize = k.parse().unwrap();
                assert!(k >= next, "out of order: {lines:?}");
                (kept, next) = (kept + 1, k + 1);
            } else {
                dropped += dropped_in(l).unwrap_or_else(|| panic!("{l}"));
            }
        }
        assert!(
            kept <= 9,
            "the queue's 8 and the one being written: {lines:?}"
        );
        assert_eq!(kept + dropped, 100, "{lines:?}");
        assert!(
            dropped_in(lines[lines.len() - 2]).is_some(),
            "the count comes before the next line: {lines:?}"
        );
        // Memory kept them all.
        let recent = log.recent();
        assert_eq!(recent.len(), 101);
        assert_eq!(
            (recent[0].as_str(), recent[100].as_str()),
            ("line 0", "after")
        );
    }

    /// The count of lines in a "… dropped" line of the log's own.
    fn dropped_in(line: &str) -> Option<usize> {
        let (_, said) = line.split_once("  WARN mdm log mdm_crash::logs: ")?;
        if said == "1 log line was dropped while the disk didn't answer" {
            return Some(1);
        }
        said.strip_suffix(" log lines were dropped while the disk didn't answer")?
            .parse()
            .ok()
    }

    /// A line written in pieces is one line among the recent ones.
    #[test]
    fn a_line_written_in_pieces_is_remembered_whole() {
        let disk = Held::new(true);
        let mut log = disk.log(QUEUE);
        let (a, b) = ("in", "pieces");
        writeln!(log, "{a} {b}").unwrap();
        log.write_all(b"two\r\nlines\n").unwrap();
        assert_eq!(log.recent(), ["in pieces", "two", "lines"]);
        assert!(log.flush_within(Duration::from_secs(5)));
        assert_eq!(disk.text(), "in pieces\ntwo\r\nlines\n");
    }

    /// A count with no line after it is written by a flush (MDM ending, a report).
    #[test]
    fn a_flush_writes_the_count_too() {
        let disk = Held::new(false);
        let log = disk.log(2);
        // The log's thread first holds a line of its own at the disk, so no place in the queue
        // frees up meanwhile: two of the ten wait, the last eight are dropped, and no later line
        // carries their count (it did on a slow runner, where the thread took "line 0" while the
        // ten were being logged and "line 9" found a place).
        log.clone()
            .write_all(
                b"first
",
            )
            .unwrap();
        disk.until_held();
        log_from_another_thread(&log, 10);
        disk.set(true);
        assert!(log.flush_within(Duration::from_secs(5)));
        let text = disk.text();
        let (mut kept, mut dropped) = (0, 0);
        for l in text.lines() {
            match dropped_in(l) {
                Some(n) => dropped += n,
                None => kept += 1,
            }
        }
        assert!(dropped_in(text.lines().last().unwrap()).is_some(), "{text}");
        assert_eq!((kept, dropped), (3, 8), "{text}");
    }

    /// #1354: a report made while the disk holds the log's last lines still has them: the
    /// app's memory of them, sent with its freeze, stands in for the file's end.
    #[test]
    fn a_report_has_the_last_lines_while_the_disk_holds_them() {
        /// The real log file, behind a disk that can hold its writes.
        struct HeldFile(Held, fs::File);
        impl Write for HeldFile {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.0.write_all(buf)?;
                self.1.write(buf)
            }
            fn flush(&mut self) -> io::Result<()> {
                self.1.flush()
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path();
        fs::create_dir_all(dir(data)).unwrap();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(current(&dir(data), APP))
            .unwrap();
        let disk = Held::new(true);
        let log = Rolling::over(
            Inner {
                dir: dir(data),
                name: APP,
                file: Some(Box::new(HeldFile(disk.clone(), file))),
                size: 0,
            },
            QUEUE,
        )
        .unwrap();
        let mut w = log.clone();
        writeln!(w, "before the freeze").unwrap();
        assert!(log.flush_within(Duration::from_secs(5)));

        disk.set(false);
        writeln!(w, "a slow frame").unwrap();
        writeln!(w, "the window stopped responding").unwrap();
        assert!(!log.flush_within(Duration::from_millis(100)), "held");
        assert_eq!(tail(data, RECENT), ["before the freeze"], "not in the file");

        // As the monitor gets them: with the freeze, over the socket.
        let mut notes = crate::protocol::Notes::default();
        let body = serde_json::to_vec(&log.recent()).unwrap();
        notes.take(crate::protocol::kind::LOG, &body);
        let want = [
            "before the freeze",
            "a slow frame",
            "the window stopped responding",
        ];
        assert_eq!(tail_or_recent(data, RECENT, &notes.log), want);
        let made = crate::report::make(None, crate::protocol::Kind::Freeze, notes, data, &[]);
        let at = made
            .text
            .find("before the freeze")
            .expect("the log's start");
        let end = made
            .text
            .find("the window stopped responding")
            .expect("its end");
        assert!(at < made.text.find("a slow frame").unwrap() && at < end);

        // Once the disk caught up, the file is taken again (it may have lines logged since).
        disk.set(true);
        writeln!(w, "recovered").unwrap();
        assert!(log.flush_within(Duration::from_secs(5)));
        let file = tail_or_recent(data, RECENT, &want.map(String::from));
        assert_eq!(file.last().map(String::as_str), Some("recovered"));
    }
}
