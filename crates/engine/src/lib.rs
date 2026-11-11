//! Mister Download Manager download engine. See notes/ENGINE.md.

pub mod agent;
pub mod disk;
pub mod dns;
pub mod download;
pub mod ftp;
pub mod hosts;
mod html;
pub mod limiter;
pub mod naming;
pub mod net;
pub mod pac;
pub mod planner;
pub mod server;
pub mod sites;
mod sparse;
pub mod state;
pub mod tls;
pub mod unlock;
pub mod verify;

pub use dns::{Dns, DnsChoice, DnsError};
pub use download::{
    Destination, Download, Error, Event, FileHook, KeptAnswer, NetKind, Options, Outcome,
    ProbeInfo, Proxy, Request, Snapshot, fetch, http_client, probe, probe_kept, supported,
};
pub use net::{NetError, Route};
pub use server::Server;
pub use sites::Refusal;
pub use unlock::{ServiceFault, Unlocked, Unlocker};
