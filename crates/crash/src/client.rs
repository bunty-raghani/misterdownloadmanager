//! The app's side: start the monitor, connect to it, tell it what it needs to know, ask it for
//! dumps. (Catching native crashes is the app's platform layer, win32.rs / unix.rs: it hands
//! [`Reporter::dump`] to crash-handler.)

use std::path::Path;
use std::process::{Child, Command};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::protocol::{AppState, Freeze, Panic, Session, kind};

/// The app's connection to its monitor.
pub struct Reporter {
    client: minidumper::Client,
    monitor: Mutex<Option<Child>>,
    /// The last state sent (a change is sent, not every frame).
    last_state: Mutex<Option<AppState>>,
}

impl Reporter {
    /// Start `exe --crash-monitor <socket> --data <dir>` and connect to it (up to 5 s for it to
    /// listen). None when it can't start: MDM runs on without crash reports.
    pub fn start(exe: &Path, data: &Path) -> Option<Reporter> {
        let socket = crate::monitor::socket_path(std::process::id());
        let mut cmd = Command::new(exe);
        cmd.arg(crate::MONITOR_ARG)
            .arg(&socket)
            .arg("--data")
            .arg(data)
            .arg("--parent")
            .arg(std::process::id().to_string());
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("crash monitor didn't start: {e}");
                return None;
            }
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        let client = loop {
            match minidumper::Client::with_name(minidumper::SocketName::Path(&socket)) {
                Ok(c) => break c,
                Err(_) if Instant::now() < deadline => {
                    // A monitor that couldn't open its socket has already said why in its own
                    // log and gone: don't make the app wait the whole five seconds for it.
                    if let Ok(Some(_)) = child.try_wait() {
                        tracing::warn!("crash monitor exited before it listened");
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(25))
                }
                Err(e) => {
                    tracing::warn!("crash monitor didn't answer: {e}");
                    // It can never answer now: don't leave a second MDM process behind.
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        };
        tracing::info!("crash monitor {} listening", child.id());
        Some(Reporter {
            client,
            monitor: Mutex::new(Some(child)),
            last_state: Mutex::new(None),
        })
    }

    /// The monitor's process id (Linux lets only it read this process's memory: ptrace).
    pub fn monitor_pid(&self) -> Option<u32> {
        self.monitor
            .lock()
            .ok()
            .and_then(|m| m.as_ref().map(Child::id))
    }

    fn send(&self, kind: u32, body: &impl serde::Serialize) {
        if let Ok(b) = serde_json::to_vec(body) {
            let _ = self.client.send_message(kind, b);
        }
    }

    pub fn session(&self, s: &Session) {
        self.send(kind::SESSION, s);
    }

    /// Send the state when it changed since the last time.
    pub fn state(&self, s: &AppState) {
        let Ok(mut last) = self.last_state.lock() else {
            return;
        };
        if last.as_ref() != Some(s) {
            self.send(kind::STATE, s);
            *last = Some(s.clone());
        }
    }

    pub fn panic(&self, p: &Panic) {
        self.send(kind::PANIC, p);
        // The messages are in before the dump request (on macOS they travel separately).
        let _ = self.client.ping();
    }

    pub fn freeze(&self, f: &Freeze) {
        self.send(kind::FREEZE, f);
        let _ = self.client.ping();
    }

    pub fn recovered(&self) {
        let _ = self.client.send_message(kind::RECOVERED, []);
    }

    /// The app is closing normally: no report.
    pub fn clean(&self) {
        let _ = self.client.send_message(kind::CLEAN, []);
        let _ = self.client.ping();
    }

    /// Ask the monitor for a dump of this process, `context` being the thread that asks (from
    /// crash-handler). True when it was written.
    pub fn dump(&self, context: &crash_handler::CrashContext) -> bool {
        self.client.request_dump(context).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A monitor that couldn't open its socket exits at once: the app doesn't wait its five
    /// seconds for a process that is already gone.
    #[test]
    fn a_monitor_that_exits_before_listening_is_not_waited_for() {
        let dir = tempfile::tempdir().unwrap();
        // A system program that starts and exits at once; the monitor's arguments mean nothing
        // to it.
        let exe = if cfg!(windows) {
            "whoami.exe"
        } else {
            // macOS has no /bin/true: found on the PATH.
            "true"
        };
        let started = Instant::now();
        assert!(Reporter::start(Path::new(exe), dir.path()).is_none());
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "waited {:?} for a monitor that had already gone",
            started.elapsed()
        );
    }
}
