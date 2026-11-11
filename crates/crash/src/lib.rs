//! MDM's crash and freeze reports, built the way Firefox, Chrome (Crashpad) and Sentry build
//! theirs: the app never writes its own dump. A second MDM process, the monitor, waits beside it;
//! when the app crashes (a native exception or signal), panics, or its window stops answering, it
//! asks, and the monitor writes a minidump of every thread from outside. The monitor then reads
//! the dump with the release's symbols into a report (`report.rs`) and offers to send it on
//! GitHub.
//!
//! - `client`: the app's side (start the monitor, messages, dump requests).
//! - `monitor`: the monitor's side (the dump server and its events).
//! - `analyze`: a dump made readable (stacks with function, file and line).
//! - `modules`: a Linux dump's module list mended first (a program split in two by a gap).
//! - `report`: the report's text and the problem's signature.
//! - `github`: the issue form and the search for one already filed.
//! - `symbols`: the release's symbols, fetched on demand.
//! - `logs`: the log MDM keeps in release builds too, and its last lines.

pub mod analyze;
pub mod client;
pub mod github;
pub mod logs;
mod modules;
pub mod monitor;
pub mod protocol;
pub mod report;
pub mod symbols;

pub use crash_handler;

/// The argument that starts MDM as the monitor: `--crash-monitor <socket> --data <dir> --parent
/// <pid>`.
pub const MONITOR_ARG: &str = "--crash-monitor";

/// The monitor's arguments, when this process was started as one.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorArgs {
    pub socket: std::path::PathBuf,
    pub data: std::path::PathBuf,
    pub parent: u32,
}

impl MonitorArgs {
    pub fn parse(args: &[String]) -> Option<MonitorArgs> {
        let at = args.iter().position(|a| a == MONITOR_ARG)?;
        let value = |name: &str| {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1))
                .cloned()
        };
        Some(MonitorArgs {
            socket: args.get(at + 1)?.into(),
            data: value("--data")?.into(),
            parent: value("--parent")?.parse().ok()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_arguments_round_trip() {
        let args: Vec<String> = [
            "--crash-monitor",
            "/tmp/s.sock",
            "--data",
            "/d",
            "--parent",
            "42",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            MonitorArgs::parse(&args),
            Some(MonitorArgs {
                socket: "/tmp/s.sock".into(),
                data: "/d".into(),
                parent: 42
            })
        );
        assert_eq!(MonitorArgs::parse(&["--data".into(), "x".into()]), None);
    }
}
