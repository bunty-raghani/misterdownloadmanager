//! The log MDM keeps in release builds too: `<data>/logs/mdm.log`, rolled over at a size into
//! `mdm.1.log` … `mdm.3.log` (the crash monitor keeps `monitor.log` the same way). Each line is
//! handed to the log's own thread, which writes it at once; a report takes the last lines of
//! MDM's ([`flush`] first, so it has them all).
//!
//! The window's thread never writes the file itself (#513): while Windows held every write on
//! C: for a shadow copy, a line logged there (or one waiting behind another thread's held write)
//! kept the window from answering until the writes were let go.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// A log file grows to this, then rolls over.
pub const MAX_BYTES: u64 = 2 * 1024 * 1024;
/// Rolled-over files kept (`mdm.1.log` newest).
pub const KEEP: usize = 3;

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
    Line(Vec<u8>),
    /// Answered once everything handed over before it is written.
    Flush(mpsc::Sender<()>),
}

/// The writer behind the log: hands each line to the log's thread, which appends it to
/// `mdm.log`, rolling it over past [`MAX_BYTES`]. Writing never waits for the disk.
#[derive(Clone)]
pub struct Rolling {
    tx: mpsc::Sender<Msg>,
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
        Rolling::over(Inner {
            dir,
            name,
            file: Some(Box::new(file)),
            size,
        })
    }

    /// The log's thread, writing into `inner`.
    fn over(mut inner: Inner) -> io::Result<Rolling> {
        let name = inner.name;
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name(format!("{name} log"))
            .spawn(move || {
                for msg in rx {
                    match msg {
                        Msg::Line(line) => inner.append(&line),
                        Msg::Flush(done) => {
                            if let Some(f) = inner.file.as_mut() {
                                let _ = f.flush();
                            }
                            let _ = done.send(());
                        }
                    }
                }
            })?;
        Ok(Rolling { tx })
    }

    /// Wait, up to `within`, until every line handed over so far is written. False when it
    /// wasn't (the disk held, or the log's thread gone).
    pub fn flush_within(&self, within: Duration) -> bool {
        let (done, written) = mpsc::channel();
        self.tx.send(Msg::Flush(done)).is_ok() && written.recv_timeout(within).is_ok()
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
}

impl Write for Rolling {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.tx
            .send(Msg::Line(buf.to_vec()))
            .map_err(|_| io::Error::other("the log's thread is gone"))?;
        Ok(buf.len())
    }

    /// Waits for the log's thread to have written everything (tests, [`flush`]).
    fn flush(&mut self) -> io::Result<()> {
        let (done, written) = mpsc::channel();
        self.tx
            .send(Msg::Flush(done))
            .ok()
            .and_then(|()| written.recv().ok())
            .ok_or_else(|| io::Error::other("the log's thread is gone"))
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

#[cfg(test)]
mod tests {
    use super::*;

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

    /// #513: a line is logged at once while the disk holds the log's writes (Windows making a
    /// shadow copy held every write on C: for 8 s, and the window with them): only the log's
    /// own thread waits, and the lines are all written, in order, once the disk lets go.
    #[test]
    fn logging_never_waits_for_the_disk() {
        use std::sync::{Arc, Condvar, Mutex};
        use std::time::Instant;

        /// A file whose writes wait until it is let go, then land in `out`.
        struct Held {
            gate: Arc<(Mutex<bool>, Condvar)>,
            out: Arc<Mutex<Vec<u8>>>,
        }
        impl Write for Held {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                let (open, cv) = &*self.gate;
                let mut open = open.lock().unwrap();
                while !*open {
                    open = cv.wait(open).unwrap();
                }
                self.out.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let out = Arc::new(Mutex::new(Vec::new()));
        let tmp = tempfile::tempdir().unwrap();
        let log = Rolling::over(Inner {
            dir: tmp.path().to_owned(),
            name: APP,
            file: Some(Box::new(Held {
                gate: gate.clone(),
                out: out.clone(),
            })),
            size: 0,
        })
        .unwrap();

        // The window's thread logs while the disk holds the first line.
        let (said, done) = mpsc::channel();
        let mut w = log.clone();
        let window = std::thread::spawn(move || {
            let start = Instant::now();
            for n in 0..100 {
                writeln!(w, "line {n}").unwrap();
            }
            said.send(start.elapsed()).unwrap();
        });
        let took = done
            .recv_timeout(Duration::from_secs(5))
            .expect("logging waited for the held disk");
        assert!(took < Duration::from_secs(1), "{took:?}");
        window.join().unwrap();
        assert!(!log.flush_within(Duration::from_millis(100)), "still held");

        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        assert!(log.flush_within(Duration::from_secs(5)));
        let text = String::from_utf8(out.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 100);
        assert_eq!((lines[0], lines[99]), ("line 0", "line 99"));
    }
}
