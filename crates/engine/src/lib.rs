//! Mister Download Manager download engine. See notes/ENGINE.md.

pub mod agent;
pub mod browserlike;
pub mod disk;
pub mod dns;
pub mod download;
pub mod fault;
pub mod ftp;
pub mod hosts;
// Public for the fuzz targets (fuzz/): `links` and `declared_videos` without a network.
pub mod html;
pub mod limiter;
pub mod naming;
pub mod net;
pub mod pac;
pub mod planner;
pub mod server;
pub mod sites;
#[doc(hidden)]
pub mod slowdisk;
mod sparse;
pub mod state;
pub mod tls;
pub mod unlock;
pub mod verify;

pub use dns::{Dns, DnsChoice, DnsError};
pub use download::{
    Destination, Download, Error, Event, FileHook, KeptAnswer, NetKind, Options, Outcome,
    ProbeInfo, Proxy, Request, Snapshot, fetch, hop_client, http_client, page_links, page_videos,
    probe, probe_kept, supported,
};
pub use html::{DeclaredVideo, DeclaredVideos};
pub use net::{NetError, Route};
pub use server::Server;
pub use sites::Refusal;
pub use tls::CertProblem;
pub use unlock::{ServiceFault, Unlocked, Unlocker};
