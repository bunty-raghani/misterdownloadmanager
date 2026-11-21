//! labserver: an HTTP Range server with a request log and fault injection.
//! Used in-process by Mister Download Manager's tests and standalone for the behaviour lab (notes/BEHAVIOR-LAB.md).

pub mod content;
pub mod debrid;
pub mod debrid_torrents;
pub mod drive;
pub mod faults;
mod feeds;
pub mod ftp;
mod github;
pub mod growth;
mod h264;
pub mod lulustream;
pub mod mediafire;
mod pages;
pub mod rapidgator;
mod server;
pub mod socks;
mod streams;

pub use faults::Faults;
pub use server::{Cert, Config, HELP, Held, LabServer, LogEntry, LogSink, SlotHold};
