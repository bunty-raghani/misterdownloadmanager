//! What stopped a request, in kinds a person can act on (#816): MDM's own requests (its helpers'
//! release lists and checksums, its updates) say which server they couldn't reach and why, not a
//! bare "network error".

use std::error::Error as StdError;
use std::io;

/// The kind of failure, from the error and its causes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// The server's name has no address: no connection, a DNS that can't be asked, or a network
    /// that blocks the name.
    NoSuchHost,
    /// The server, or something on the way (a firewall), refused the connection.
    Refused,
    /// The connection was cut while it opened (a firewall or a filter dropping it).
    Reset,
    /// No route to the server, or the network is down.
    Unreachable,
    /// The server didn't answer in time.
    TimedOut,
    /// The secure connection failed: a certificate this computer doesn't trust, usually an
    /// antivirus's web filter or a proxy that reads HTTPS.
    Secure,
    /// The server answered with this HTTP status.
    Status(u16),
    /// The connection closed before all of the answer came.
    CutShort,
    /// Something else.
    Other,
}

/// What kind of network failure the engine's `e` is; None for one that isn't about the network
/// (a disk's, a refusal the engine words itself).
pub fn of_engine(e: &crate::Error) -> Option<Fault> {
    use crate::{Error as E, NetKind};
    Some(match e {
        E::Http(h) => of_http(h),
        E::Status(s) | E::Throttled(s, _) => Fault::Status(s.as_u16()),
        E::ShortBody => Fault::CutShort,
        E::Net { kind, .. } => match kind {
            NetKind::Connect => Fault::Unreachable,
            NetKind::Timeout => Fault::TimedOut,
            NetKind::Lost => Fault::CutShort,
            NetKind::Tls => Fault::Secure,
        },
        _ => return None,
    })
}

/// What kind of failure the reqwest error `e` is.
pub fn of_http(e: &reqwest::Error) -> Fault {
    if let Some(s) = e.status() {
        return Fault::Status(s.as_u16());
    }
    let from_causes = of_causes(e);
    if from_causes != Fault::Other {
        return from_causes;
    }
    if e.is_timeout() {
        Fault::TimedOut
    } else if e.is_body() || e.is_decode() {
        Fault::CutShort
    } else if e.is_connect() {
        Fault::Unreachable
    } else {
        Fault::Other
    }
}

/// What the causes of `e` (itself included) say: a TLS failure, a name that wasn't found, the
/// system's word for a connection that failed. `Other` when none of them says.
pub fn of_causes(e: &(dyn StdError + 'static)) -> Fault {
    let mut found = Fault::Other;
    let mut next = Some(e);
    while let Some(e) = next {
        let here = one(e);
        // A secure connection's failure is the clearest word, wherever it is in the chain (its
        // handshake's I/O error comes with it).
        if here == Fault::Secure {
            return here;
        }
        if found == Fault::Other {
            found = here;
        }
        next = e.source();
    }
    found
}

/// What one error says by itself.
fn one(e: &(dyn StdError + 'static)) -> Fault {
    if e.downcast_ref::<rustls::Error>().is_some() || crate::tls::is_no_roots(e) {
        return Fault::Secure;
    }
    if let Some(d) = e.downcast_ref::<crate::DnsError>() {
        return match d {
            crate::DnsError::Adapter(_) => Fault::Unreachable,
            _ => Fault::NoSuchHost,
        };
    }
    if let Some(io) = e.downcast_ref::<io::Error>() {
        // An I/O error carrying another (tokio-rustls wraps the handshake's error this way):
        // its own `source` skips the one it carries.
        if let Some(inner) = io.get_ref() {
            let inner = one(inner);
            if inner != Fault::Other {
                return inner;
            }
        }
        return match io.kind() {
            io::ErrorKind::ConnectionRefused => Fault::Refused,
            io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted => Fault::Reset,
            io::ErrorKind::TimedOut => Fault::TimedOut,
            io::ErrorKind::HostUnreachable
            | io::ErrorKind::NetworkUnreachable
            | io::ErrorKind::NetworkDown
            | io::ErrorKind::AddrNotAvailable => Fault::Unreachable,
            io::ErrorKind::UnexpectedEof => Fault::CutShort,
            _ => Fault::Other,
        };
    }
    // hyper's connector names a failed name lookup "dns error" (not translated: its English,
    // parsed); the cause under it is the system's own words.
    if e.to_string().starts_with("dns error") {
        return Fault::NoSuchHost;
    }
    Fault::Other
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_kinds() {
        let refused = io::Error::from(io::ErrorKind::ConnectionRefused);
        assert_eq!(of_causes(&refused), Fault::Refused);
        let wrapped = io::Error::new(io::ErrorKind::InvalidData, rustls::Error::DecryptError);
        assert_eq!(of_causes(&wrapped), Fault::Secure);
        let no_name = crate::DnsError::NoSuchName {
            dns: "1.1.1.1".into(),
            host: "x.invalid".into(),
        };
        assert_eq!(of_causes(&no_name), Fault::NoSuchHost);
        assert_eq!(of_causes(&io::Error::other("what")), Fault::Other);
    }
}
