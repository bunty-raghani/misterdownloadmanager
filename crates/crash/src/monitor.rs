//! The monitor: a second MDM process (`--crash-monitor`) that waits beside the app. When the app
//! crashes, panics or freezes, it asks; the monitor writes the minidump of every thread from
//! outside (a crashed process can't be trusted to do it), and hands the report window an event.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
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
        // A frozen app lives on (it may recover); a crashed one disconnects by itself.
        if let Ok(mut n) = self.notes.lock() {
            n.panic = None;
        }
        minidumper::LoopAction::Continue
    }

    fn on_message(&self, kind: u32, buffer: Vec<u8>) {
        let recovered = kind == crate::protocol::kind::RECOVERED;
        if let Ok(mut n) = self.notes.lock() {
            n.take(kind, &buffer);
        }
        if recovered {
            self.send(Event::Recovered);
        }
    }

    fn on_client_disconnected(&self, num_clients: usize) -> minidumper::LoopAction {
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
    };
    let shutdown = AtomicBool::new(false);
    let result = server
        .run(Box::new(handler), &shutdown, None)
        .map_err(|e| e.to_string());
    let _ = std::fs::remove_file(socket);
    result
}
