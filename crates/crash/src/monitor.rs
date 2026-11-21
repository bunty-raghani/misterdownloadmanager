//! The monitor: a second MDM process (`--crash-monitor`) that waits beside the app. When the app
//! crashes, panics or freezes, it asks; the monitor writes the minidump of every thread from
//! outside (a crashed process can't be trusted to do it), and hands the report window an event.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use crate::protocol::{Kind, Notes};

/// What the monitor saw, for the report window.
#[derive(Debug)]
pub enum Event {
    /// A dump was written: a crash, a panic or a freeze (the notes say which).
    Dump {
        path: PathBuf,
        kind: Kind,
        notes: Notes,
    },
    /// A dump was asked for but couldn't be written.
    DumpFailed {
        kind: Kind,
        notes: Notes,
        error: String,
    },
    /// The window answers again after a freeze.
    Recovered,
    /// The socket couldn't be opened: the app can never connect, so stop waiting for it.
    ServeFailed(String),
    /// The app is gone: `clean` when it closed normally.
    Gone { clean: bool, notes: Notes },
}

/// The socket the app and its monitor meet on, unique per app process.
pub fn socket_path(app_pid: u32) -> PathBuf {
    std::env::temp_dir().join(format!("mdm-crash-{app_pid}.sock"))
}

/// The folder reports are kept in.
pub fn crashes_dir(data: &Path) -> PathBuf {
    data.join("crashes")
}

/// A new report's file stem: when, and what (`2026-11-01_04-12-09-freeze`).
pub fn stem(kind: Kind) -> String {
    format!(
        "{}-{}",
        chrono::Local::now().format("%Y-%m-%d_%H-%M-%S"),
        kind.word()
    )
}

struct Handler {
    dir: PathBuf,
    notes: Mutex<Notes>,
    events: Mutex<Sender<Event>>,
    /// The app has said something: a connection gone before that was no app leaving.
    heard: AtomicBool,
}

impl Handler {
    fn send(&self, e: Event) {
        if let Ok(tx) = self.events.lock() {
            let _ = tx.send(e);
        }
    }

    fn notes(&self) -> Notes {
        self.notes.lock().map(|n| n.clone()).unwrap_or_default()
    }
}

impl minidumper::ServerHandler for Handler {
    fn create_minidump_file(&self) -> Result<(std::fs::File, PathBuf), std::io::Error> {
        std::fs::create_dir_all(&self.dir)?;
        let kind = self.notes().dump_kind();
        let path = self.dir.join(format!("{}.dmp", stem(kind)));
        let file = std::fs::File::create(&path)?;
        Ok((file, path))
    }

    fn on_minidump_created(
        &self,
        result: Result<minidumper::MinidumpBinary, minidumper::Error>,
    ) -> minidumper::LoopAction {
        let notes = self.notes();
        let kind = notes.dump_kind();
        if let Ok(mut n) = self.notes.lock() {
            n.dump_done();
        }
        match result {
            Ok(mut bin) => {
                use std::io::Write;
                let _ = bin.file.flush();
                tracing::info!(
                    "crash monitor: {} dump at {}",
                    kind.word(),
                    bin.path.display()
                );
                self.send(Event::Dump {
                    path: bin.path,
                    kind,
                    notes,
                });
            }
            Err(e) => {
                tracing::error!("crash monitor: the dump failed: {e:#}");
                self.send(Event::DumpFailed {
                    kind,
                    notes,
                    error: format!("{e:#}"),
                });
            }
        }
        // A frozen app lives on (it may recover); a crashed one disconnects by itself. The lines
        // it sent were this dump's: a later report reads the file again.
        if let Ok(mut n) = self.notes.lock() {
            n.panic = None;
            n.log.clear();
        }
        minidumper::LoopAction::Continue
    }

    fn on_message(&self, kind: u32, buffer: Vec<u8>) {
        self.heard.store(true, Ordering::Release);
        let recovered = kind == crate::protocol::kind::RECOVERED;
        if let Ok(mut n) = self.notes.lock() {
            n.take(kind, &buffer);
            if kind == crate::protocol::kind::LOG {
                tracing::info!(
                    "crash monitor: the app sent its last {} log lines",
                    n.log.len()
                );
            }
        }
        if recovered {
            self.send(Event::Recovered);
        }
    }

    fn on_client_disconnected(&self, num_clients: usize) -> minidumper::LoopAction {
        // A connection that went before saying anything is a try that didn't finish, not the
        // app leaving: on macOS a connection is a socket, then a Mach port, and a try that came
        // between the two (the monitor still starting) leaves a socket that closes at once. The
        // app's next try connects; leaving here would leave it no one to connect to. (An app
        // that died before it said anything is seen gone by the monitor's watch on it.)
        if num_clients == 0 && !self.heard.load(Ordering::Acquire) {
            tracing::info!("crash monitor: a connection went before it said anything");
            return minidumper::LoopAction::Continue;
        }
        if num_clients == 0 {
            let notes = self.notes();
            self.send(Event::Gone {
                clean: notes.clean,
                notes,
            });
            minidumper::LoopAction::Exit
        } else {
            minidumper::LoopAction::Continue
        }
    }
}

/// Serve the app's socket until it disconnects, sending what happens to `events`. Blocks: run it
/// on a thread of its own.
pub fn serve(socket: &Path, data: &Path, events: Sender<Event>) -> Result<(), String> {
    // A socket file left by an earlier run of the same pid would refuse the new one.
    let _ = std::fs::remove_file(socket);
    let mut server = minidumper::Server::with_name(minidumper::SocketName::Path(socket))
        .map_err(|e| e.to_string())?;
    let handler = Handler {
        dir: crashes_dir(data),
        notes: Mutex::new(Notes::default()),
        events: Mutex::new(events),
        heard: AtomicBool::new(false),
    };
    let shutdown = AtomicBool::new(false);
    let result = server
        .run(Box::new(handler), &shutdown, None)
        .map_err(|e| e.to_string());
    let _ = std::fs::remove_file(socket);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// A connection that goes before it says anything (a try caught halfway, as on macOS while
    /// the monitor starts) doesn't end the monitor: the app's next connection is served, and its
    /// leaving is the one that ends it.
    #[test]
    fn a_connection_gone_before_it_said_anything_is_not_the_app_leaving() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("half.sock");
        let data = dir.path().to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        let served = {
            let socket = socket.clone();
            std::thread::spawn(move || serve(&socket, &data, tx))
        };
        let connect = || {
            let limit = Instant::now() + Duration::from_secs(10);
            loop {
                match minidumper::Client::with_name(minidumper::SocketName::Path(&socket)) {
                    Ok(c) => return c,
                    Err(e) if Instant::now() > limit => panic!("the monitor never listened: {e}"),
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        };
        // A connection that says nothing and goes; the monitor has seen it go once it answers
        // the next connection's ping.
        drop(connect());
        let app = connect();
        app.ping().expect("the monitor still answers");
        assert!(
            rx.try_recv().is_err(),
            "the monitor took the silent connection for the app"
        );
        app.send_message(crate::protocol::kind::CLEAN, b"null")
            .unwrap();
        app.ping().unwrap();
        drop(app);
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Event::Gone { clean, .. }) => assert!(clean, "the app's clean exit was lost"),
            other => panic!("the app's leaving wasn't seen: {other:?}"),
        }
        served.join().unwrap().unwrap();
    }
}
