//! FTP and FTPS: `ftp://`, `ftpes://` (explicit TLS: `AUTH TLS` on port 21) and `ftps://`
//! (implicit TLS, port 990), downloaded like HTTP (`download.rs`): the first connection asks
//! `SIZE` and `MDTM` and starts `RETR` at byte 0 while further connections fetch parts with
//! `REST` + `RETR`. Each connection is a control session with its data connection, passive by
//! default (`EPSV` when the server offers it, else `PASV`), active (`PORT`/`EPRT`) when
//! `Options::ftp_passive` is off.
//!
//! A part ends where the next one begins, usually before the end of the file: its data
//! connection is dropped and the session goes back to the download's pool, where the next
//! command first reads the server's answer to the dropped transfer. Reusing sessions saves a
//! connect, a TLS handshake and a login per part.
//!
//! The login comes from the address (`ftp://user:pass@host/…`), else from an
//! `Authorization: Basic` header (the app's "Use authorization" and Sites Logins), else
//! anonymous. The path is relative to the login folder (RFC 1738; `%2F` makes it absolute).
//! TLS uses the system's certificate store, like HTTPS.
//!
//! FTP doesn't go through an HTTP proxy. A SOCKS proxy (Options → Proxy / Socks, or a proxy
//! script's `SOCKS` answer, #1558) carries it: the control connection and, passive always (the
//! proxy can't take the server's connections back), each data connection, to the address the
//! control connection reached. With socks5h / socks4a the proxy looks the server's name up.
//! Implicit TLS (`ftps://`) can't go through one yet (suppaftp opens that connection itself).

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use base64::Engine;
use bytes::{Bytes, BytesMut};
use percent_encoding::percent_decode_str;
use reqwest::header::{AUTHORIZATION, HeaderMap};
use rustls::pki_types::CertificateDer;
use suppaftp::tokio::{
    AsyncRustlsConnector, AsyncRustlsFtpStream, AsyncRustlsStream, TransferStream,
};
use suppaftp::types::FileType;
use suppaftp::{FtpError, Mode, Status};
use tokio::io::AsyncReadExt;
use url::{Host, Url};

use crate::download::{Error, NetKind, Options};

type Session = AsyncRustlsFtpStream;

/// An idle pooled session older than this is closed rather than reused: servers drop idle
/// sessions (often after 60 s), and a dead one costs a failed try.
const IDLE: Duration = Duration::from_secs(30);
/// Bytes read from a data connection at a time.
const READ: usize = 64 * 1024;

/// Whether MDM downloads `url` over FTP.
pub fn is_ftp(url: &Url) -> bool {
    matches!(url.scheme(), "ftp" | "ftps" | "ftpes")
}

/// `url` without its password, as it may be written to disk (the resume state): whoever
/// resumes gives the login again.
pub fn without_password(url: &Url) -> Url {
    let mut u = url.clone();
    let _ = u.set_password(None);
    u
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Security {
    Plain,
    /// `AUTH TLS` after connecting.
    Explicit,
    /// TLS from the first byte.
    Implicit,
}

/// What the server tells about the file before sending it.
#[derive(Clone, Debug)]
pub(crate) struct Facts {
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
    /// `FEAT` lists `REST STREAM`: parts can be fetched. Unlisted, the first part tells.
    pub rest: bool,
}

/// One server and file: how to reach and log in, and the sessions not in use.
pub(crate) struct Site {
    host: String,
    port: u16,
    /// Sent as it is to `SIZE`, `MDTM` and `RETR`.
    path: String,
    user: String,
    pass: String,
    security: Security,
    passive: bool,
    connect_timeout: Duration,
    read_timeout: Duration,
    /// `Options::hosts`: fixed addresses for names.
    hosts: Vec<(String, Vec<IpAddr>)>,
    /// `Options::dns` and the certificates it may trust besides the system's.
    dns: crate::dns::DnsChoice,
    extra_roots: Vec<Vec<u8>>,
    /// The network adapter its connections go through (#35).
    route: crate::net::Route,
    /// The SOCKS proxy its connections go through (#1558).
    socks: Option<Socks>,
    tls: Option<tokio_rustls::TlsConnector>,
    /// The server offers `EPSV` (learned from `FEAT`): no address in the answer to get wrong
    /// behind NAT.
    epsv: AtomicBool,
    pool: Mutex<Vec<(Session, Instant)>>,
    max_pool: usize,
}

impl Site {
    pub fn new(url: &Url, headers: &HeaderMap, opts: &Options) -> Result<Site, Error> {
        let security = match url.scheme() {
            "ftps" => Security::Implicit,
            "ftpes" => Security::Explicit,
            _ => Security::Plain,
        };
        let host = match url.host() {
            Some(Host::Domain(d)) => d.to_owned(),
            Some(Host::Ipv4(a)) => a.to_string(),
            Some(Host::Ipv6(a)) => a.to_string(),
            None => return Err(net(NetKind::Connect, "the address has no server")),
        };
        let decode = |s: &str| percent_decode_str(s).decode_utf8_lossy().into_owned();
        let (user, pass) = if url.password().is_some() || header_login(headers).is_none() {
            (decode(url.username()), decode(url.password().unwrap_or("")))
        } else {
            header_login(headers).unwrap_or_default()
        };
        let (user, pass) = match user.as_str() {
            "" | "anonymous" | "ftp" if pass.is_empty() => ("anonymous".into(), "mdm@".into()),
            _ => (user, pass),
        };
        // The leading slash only separates the path from the server; `;type=i` is a transfer
        // type (always binary here).
        let raw = url.path().strip_prefix('/').unwrap_or(url.path());
        let raw = raw.rsplit_once(";type=").map_or(raw, |(p, _)| p);
        let tls = match security {
            Security::Plain => None,
            _ => Some(connector(&opts.extra_roots)?),
        };
        Ok(Site {
            host,
            port: url.port().unwrap_or(if security == Security::Implicit {
                990
            } else {
                21
            }),
            path: decode(raw),
            user,
            pass,
            security,
            passive: opts.ftp_passive,
            connect_timeout: opts.connect_timeout,
            read_timeout: opts.read_timeout,
            hosts: opts.hosts.clone(),
            dns: opts.dns.clone(),
            extra_roots: opts.extra_roots.clone(),
            route: opts.route.clone(),
            socks: Socks::for_url(&opts.proxy, url),
            tls,
            epsv: AtomicBool::new(false),
            pool: Mutex::new(Vec::new()),
            max_pool: opts.connections() as usize,
        })
    }

    /// A new session: connected, secured, logged in, binary.
    async fn connect(&self) -> Result<Session, Error> {
        if let Some(socks) = &self.socks {
            return tokio::time::timeout(self.connect_timeout, self.open_through(socks))
                .await
                .map_err(|_| net(NetKind::Timeout, ""))?;
        }
        let mut last = net(NetKind::Connect, "the server's name has no address");
        let mut addrs = self.addresses().await?;
        // Only those the chosen adapter can reach (#35); else say why none.
        if let Some(first) = addrs.first() {
            let refused = self.route.check(first.ip()).err();
            addrs.retain(|a| self.route.check(a.ip()).is_ok());
            if let (Some(e), true) = (refused, addrs.is_empty()) {
                return Err(e.into());
            }
        }
        if let crate::net::Route::Down { label } = &self.route {
            return Err(crate::net::NetError::Down {
                label: label.clone(),
            }
            .into());
        }
        for addr in addrs {
            match tokio::time::timeout(self.connect_timeout, self.open(addr)).await {
                Ok(Ok(s)) => return Ok(s),
                // Another address may answer; a refusal from the server itself won't change.
                Ok(Err(e @ Error::Net { .. })) => last = e,
                Ok(Err(e)) => return Err(e),
                Err(_) => last = net(NetKind::Timeout, ""),
            }
        }
        Err(last)
    }

    async fn addresses(&self) -> Result<Vec<SocketAddr>, Error> {
        if let Ok(ip) = self.host.parse::<IpAddr>() {
            return Ok(vec![SocketAddr::new(ip, self.port)]);
        }
        if let Some((_, ips)) = self
            .hosts
            .iter()
            .find(|(h, _)| h.eq_ignore_ascii_case(&self.host))
        {
            return Ok(ips
                .iter()
                .map(|ip| SocketAddr::new(*ip, self.port))
                .collect());
        }
        let found = tokio::time::timeout(
            self.connect_timeout,
            crate::dns::lookup(
                &self.dns,
                &self.host,
                self.port,
                &self.extra_roots,
                &self.route,
            ),
        )
        .await
        .map_err(|_| net(NetKind::Timeout, ""))?
        .map_err(|e| net(NetKind::Connect, &e.to_string()))?;
        Ok(found
            .into_iter()
            .map(|ip| SocketAddr::new(ip, self.port))
            .collect())
    }

    /// A new session through the SOCKS proxy (#1558): the server's name for the proxy to look
    /// up (socks5h, socks4a), else its address looked up here, as without a proxy.
    async fn open_through(&self, socks: &Socks) -> Result<Session, Error> {
        if self.security == Security::Implicit {
            return Err(net(
                NetKind::Connect,
                "ftps:// (TLS from the first byte) can't go through a SOCKS proxy yet: use ftpes:// (TLS after connecting) if the server offers it",
            ));
        }
        let target = if socks.remote_dns {
            Target::Name(self.host.clone(), self.port)
        } else {
            let addr = *self
                .addresses()
                .await?
                .first()
                .ok_or_else(|| net(NetKind::Connect, "the server's name has no address"))?;
            Target::Addr(addr)
        };
        let stream = socks
            .connect(&self.route, &self.dns, &self.extra_roots, &target)
            .await
            .map_err(|e| net(NetKind::Connect, &e.to_string()))?;
        let s = Session::connect_with_stream(stream)
            .await
            .map_err(|e| failure(e, NetKind::Connect))?;
        let s = self.secure_and_login(s).await?;
        // Passive always: the data connections go through the proxy too, to the address the
        // control connection reached (the port the server answered).
        let mut s = s;
        if self.epsv.load(Ordering::Relaxed) {
            s.set_mode(Mode::ExtendedPassive);
        } else {
            s.set_mode(Mode::Passive);
        }
        let (socks, route, dns, roots) = (
            Arc::new(socks.clone()),
            Arc::new(self.route.clone()),
            Arc::new(self.dns.clone()),
            Arc::new(self.extra_roots.clone()),
        );
        Ok(s.passive_stream_builder(move |addr| {
            let (socks, route, dns, roots) =
                (socks.clone(), route.clone(), dns.clone(), roots.clone());
            let data = target.with_port(addr.port());
            // On a task of its own: suppaftp wants a future it can share between threads,
            // which the proxy's handshake isn't; its handle is.
            let connecting =
                tokio::spawn(async move { socks.connect(&route, &dns, &roots, &data).await });
            Box::pin(async move {
                connecting
                    .await
                    .map_err(|e| FtpError::ConnectionError(std::io::Error::other(e)))?
                    .map_err(FtpError::ConnectionError)
            })
        }))
    }

    async fn open(&self, addr: SocketAddr) -> Result<Session, Error> {
        let connect = |e| failure(e, NetKind::Connect);
        let tls =
            || AsyncRustlsConnector::from(self.tls.clone().expect("TLS site has a connector"));
        let s = match (&self.security, &self.route) {
            (Security::Implicit, crate::net::Route::Any) => {
                Session::connect_secure_implicit(addr, tls(), &self.host)
                    .await
                    .map_err(connect)?
            }
            // suppaftp opens implicit TLS's connection itself (no way to give it a bound one):
            // refused rather than sent around the adapter.
            (Security::Implicit, route) => {
                return Err(crate::net::NetError::NotSupported {
                    label: route.label().unwrap_or_default().to_owned(),
                    what: "ftps://".into(),
                }
                .into());
            }
            (_, crate::net::Route::Any) => Session::connect(addr).await.map_err(connect)?,
            (_, route) => {
                let stream = crate::net::tcp_connect(route, addr)
                    .await
                    .map_err(|e| connect(FtpError::ConnectionError(e)))?;
                Session::connect_with_stream(stream)
                    .await
                    .map_err(connect)?
            }
        };
        let s = self.secure_and_login(s).await?;
        Ok(self.data_mode(s))
    }

    /// TLS as the address asks, the login, binary transfers.
    async fn secure_and_login(&self, mut s: Session) -> Result<Session, Error> {
        let connect = |e| failure(e, NetKind::Connect);
        let tls =
            || AsyncRustlsConnector::from(self.tls.clone().expect("TLS site has a connector"));
        match self.security {
            Security::Explicit => s = s.into_secure(tls(), &self.host).await.map_err(connect)?,
            // Implicit TLS protects the data connections too; say so, as clients do (some
            // servers otherwise expect them in the clear).
            Security::Implicit => {
                s.custom_command("PBSZ 0", &[Status::CommandOk])
                    .await
                    .map_err(connect)?;
                s.custom_command("PROT P", &[Status::CommandOk])
                    .await
                    .map_err(connect)?;
            }
            Security::Plain => {}
        }
        s.login(&self.user, &self.pass).await.map_err(connect)?;
        s.transfer_type(FileType::Binary).await.map_err(connect)?;
        Ok(s)
    }

    fn data_mode(&self, mut s: Session) -> Session {
        if !self.passive {
            return s.active_mode(self.read_timeout);
        }
        if self.epsv.load(Ordering::Relaxed) {
            s.set_mode(Mode::ExtendedPassive);
        } else {
            s.set_mode(Mode::Passive);
            // A server behind NAT often answers PASV with its private address: connect to
            // the address the session already reached instead.
            s.set_passive_nat_workaround(true);
        }
        // Data connections through the chosen adapter too (#35). (Active mode listens on the
        // control connection's own address, which already is the adapter's.)
        if !self.route.is_any() {
            let route = Arc::new(self.route.clone());
            s = s.passive_stream_builder(move |addr| {
                let route = route.clone();
                Box::pin(async move {
                    crate::net::tcp_connect(&route, addr)
                        .await
                        .map_err(FtpError::ConnectionError)
                })
            });
        }
        s
    }

    /// What the server says about the file, on `s`.
    async fn facts(&self, s: &mut Session) -> Result<Facts, Error> {
        let lost = |e| failure(e, NetKind::Lost);
        // Not every server has FEAT; without it, nothing is assumed.
        let features = s.feat().await.unwrap_or_default();
        let has = |name: &str, value: Option<&str>| {
            features.iter().any(|(k, v)| {
                k.eq_ignore_ascii_case(name)
                    && value.is_none_or(|want| {
                        v.as_deref().is_some_and(|v| {
                            v.split_whitespace().any(|w| w.eq_ignore_ascii_case(want))
                        })
                    })
            })
        };
        if has("EPSV", None) && (self.passive || self.socks.is_some()) {
            self.epsv.store(true, Ordering::Relaxed);
            s.set_mode(Mode::ExtendedPassive);
        }
        let rest = has("REST", Some("STREAM"));
        let size = match s.size(&self.path).await {
            Ok(n) => Some(n as u64),
            // A missing file is said now, not after a RETR.
            Err(FtpError::UnexpectedResponse(r)) if r.status.code() == 550 => {
                return Err(reply(&r));
            }
            Err(FtpError::UnexpectedResponse(_)) => None,
            Err(e) => return Err(lost(e)),
        };
        let modified = match s.mdtm(&self.path).await {
            Ok(t) => u64::try_from(t.and_utc().timestamp())
                .ok()
                .map(|secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs)),
            Err(FtpError::UnexpectedResponse(_)) => None,
            Err(e) => return Err(lost(e)),
        };
        Ok(Facts {
            size,
            modified,
            rest,
        })
    }

    /// The first connection: the facts, and the file from byte 0 on the same session.
    pub async fn start(self: &Arc<Self>) -> Result<(Facts, Transfer), Error> {
        let mut s = self.connect().await?;
        let facts = within(self.read_timeout, self.facts(&mut s)).await?;
        let stream = within(self.read_timeout, retr(&mut s, &self.path, 0)).await?;
        Ok((
            facts.clone(),
            Transfer {
                site: self.clone(),
                session: Some(s),
                stream: Some(stream),
                expected: facts.size,
            },
        ))
    }

    /// The facts on a session of their own (kept for the transfers).
    pub async fn check(&self) -> Result<Facts, Error> {
        let mut s = self.connect().await?;
        let facts = within(self.read_timeout, self.facts(&mut s)).await?;
        self.put_back(s);
        Ok(facts)
    }

    /// The file from byte `from` (of `size`, if known), on a pooled session if one is idle.
    pub async fn open_at(
        self: &Arc<Self>,
        from: u64,
        size: Option<u64>,
    ) -> Result<Transfer, Error> {
        let transfer = |s, stream| Transfer {
            site: self.clone(),
            session: Some(s),
            stream: Some(stream),
            expected: size.map(|n| n.saturating_sub(from)),
        };
        if let Some(mut s) = self.take() {
            match within(self.read_timeout, retr(&mut s, &self.path, from)).await {
                Ok(stream) => return Ok(transfer(s, stream)),
                // The server's own refusal: a new session would hear the same.
                Err(e @ Error::FtpReply { code: 500.., .. }) => return Err(e),
                // A session the server closed meanwhile: a fresh one.
                Err(_) => {}
            }
        }
        let mut s = self.connect().await?;
        let stream = within(self.read_timeout, retr(&mut s, &self.path, from)).await?;
        Ok(transfer(s, stream))
    }

    fn take(&self) -> Option<Session> {
        let mut pool = self.pool.lock().unwrap();
        pool.retain(|(_, at)| at.elapsed() < IDLE);
        pool.pop().map(|(s, _)| s)
    }

    fn put_back(&self, s: Session) {
        let mut pool = self.pool.lock().unwrap();
        if pool.len() < self.max_pool {
            pool.push((s, Instant::now()));
        }
    }
}

/// `REST from` (when not 0) and `RETR path`.
async fn retr(
    s: &mut Session,
    path: &str,
    from: u64,
) -> Result<TransferStream<AsyncRustlsStream>, Error> {
    let lost = |e| failure(e, NetKind::Lost);
    s.resume_transfer(from as usize).await.map_err(lost)?;
    s.retr_as_stream(path).await.map_err(lost)
}

/// A file (or the rest of it from an offset) coming in.
pub(crate) struct Transfer {
    site: Arc<Site>,
    session: Option<Session>,
    stream: Option<TransferStream<AsyncRustlsStream>>,
    expected: Option<u64>,
}

impl Transfer {
    /// The bytes still to come, if the size is known.
    pub fn expected(&self) -> Option<u64> {
        self.expected
    }

    /// The next bytes; None at the end of the file.
    pub async fn chunk(&mut self) -> Result<Option<Bytes>, Error> {
        let Some(stream) = self.stream.as_mut() else {
            return Ok(None);
        };
        let mut buf = BytesMut::with_capacity(READ);
        match tokio::time::timeout(self.site.read_timeout, stream.read_buf(&mut buf)).await {
            Err(_) => Err(net(NetKind::Timeout, "")),
            Ok(Err(e)) => Err(net(NetKind::Lost, &e.to_string())),
            Ok(Ok(0)) => Ok(None),
            Ok(Ok(_)) => Ok(Some(buf.freeze())),
        }
    }

    /// Done with this transfer: at its end (`at_end`, the server's "complete" is read), or
    /// before (the data connection is dropped). The session goes back to the pool.
    pub async fn done(mut self, at_end: bool) {
        let Some(stream) = self.stream.take() else {
            return;
        };
        let ok = if at_end {
            matches!(
                tokio::time::timeout(self.site.read_timeout, stream.finish()).await,
                Ok(Ok(()))
            )
        } else {
            drop(stream);
            true
        };
        if ok && let Some(s) = self.session.take() {
            self.site.put_back(s);
        }
    }
}

/// The client configuration for FTPS: the system's verifier (as reqwest uses for HTTPS), with
/// `extra` certificates also trusted.
fn connector(extra: &[Vec<u8>]) -> Result<tokio_rustls::TlsConnector, Error> {
    let config = client_config(extra).map_err(|e| net(NetKind::Tls, &e.to_string()))?;
    Ok(tokio_rustls::TlsConnector::from(Arc::new(config)))
}

/// MDM's own TLS client configuration (FTPS, DNS over TLS and HTTPS): the system's verifier, as
/// reqwest uses for HTTPS, with Mozilla's roots besides (`tls::mozilla_roots`, #1546) and
/// `extra` certificates also trusted (the tests' own CA). On a system without certificates, one
/// that refuses every server, saying why (#264): the DNS setting that holds it may still use
/// plain DNS, and FTP stays plain FTP.
pub(crate) fn client_config(extra: &[Vec<u8>]) -> Result<rustls::ClientConfig, rustls::Error> {
    use rustls_platform_verifier::Verifier;
    if extra.is_empty() && !crate::tls::system_has_roots() {
        return Ok(crate::tls::no_roots_config());
    }
    let provider = crate::tls::provider();
    let extra = extra.iter().map(|c| CertificateDer::from(c.clone()));
    let verifier =
        Verifier::new_with_extra_roots(crate::tls::mozilla_roots().chain(extra), provider.clone())?;
    Ok(rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth())
}

/// The login an `Authorization: Basic` header carries.
fn header_login(headers: &HeaderMap) -> Option<(String, String)> {
    let v = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let token = v
        .strip_prefix("Basic ")
        .or_else(|| v.strip_prefix("basic "))?;
    let text = base64::engine::general_purpose::STANDARD
        .decode(token.trim())
        .ok()?;
    let text = String::from_utf8(text).ok()?;
    let (user, pass) = text.split_once(':')?;
    Some((user.to_owned(), pass.to_owned()))
}

async fn within<T>(limit: Duration, f: impl Future<Output = Result<T, Error>>) -> Result<T, Error> {
    tokio::time::timeout(limit, f)
        .await
        .unwrap_or_else(|_| Err(net(NetKind::Timeout, "")))
}

/// Where a connection through the SOCKS proxy goes: the server's name (the proxy looks it up)
/// or its address.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Target {
    Name(String, u16),
    Addr(SocketAddr),
}

impl Target {
    /// The same server on another port (a data connection's).
    fn with_port(&self, port: u16) -> Target {
        match self {
            Target::Name(host, _) => Target::Name(host.clone(), port),
            Target::Addr(a) => Target::Addr(SocketAddr::new(a.ip(), port)),
        }
    }
}

/// A SOCKS proxy for FTP (#1558).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Socks {
    host: String,
    port: u16,
    /// 5, or 4.
    version: u8,
    /// socks5h / socks4a: the proxy looks the server's name up.
    remote_dns: bool,
    login: Option<(String, String)>,
}

impl Socks {
    /// The SOCKS proxy `proxy` gives the FTP address `url`: Manual's when it is a SOCKS one, a
    /// proxy script's first answer when it is `SOCKS…`. An HTTP proxy (and the system's
    /// settings, which name HTTP proxies) carries no FTP: None, straight to the server.
    pub(crate) fn for_url(proxy: &crate::download::Proxy, url: &Url) -> Option<Socks> {
        use crate::download::Proxy;
        match proxy {
            Proxy::Manual {
                url: address,
                login,
                ..
            } => Socks::parse(address, login.clone()),
            Proxy::Pac { script } => match crate::pac::routes(script, url).ok()?.first()? {
                crate::pac::Route::Proxy(p) => Socks::parse(p, None),
                crate::pac::Route::Direct => None,
            },
            Proxy::None | Proxy::System => None,
        }
    }

    /// `socks5h://user:pass@host:port` and the like; None for another scheme.
    pub(crate) fn parse(address: &str, login: Option<(String, String)>) -> Option<Socks> {
        let u = Url::parse(address.trim()).ok()?;
        let (version, remote_dns) = match u.scheme() {
            "socks5h" => (5, true),
            "socks5" => (5, false),
            "socks4a" => (4, true),
            "socks4" => (4, false),
            _ => return None,
        };
        let decode = |s: &str| percent_decode_str(s).decode_utf8_lossy().into_owned();
        let login = login.or_else(|| {
            (!u.username().is_empty())
                .then(|| (decode(u.username()), decode(u.password().unwrap_or(""))))
        });
        Some(Socks {
            host: u.host_str()?.trim_matches(['[', ']']).to_owned(),
            port: u.port().unwrap_or(1080),
            version,
            remote_dns,
            login: login.filter(|_| version == 5),
        })
    }

    /// A connection to `target` through the proxy: to the proxy through `route` (a proxy on this
    /// computer isn't bound), then the proxy's handshake.
    async fn connect(
        &self,
        route: &crate::net::Route,
        dns: &crate::dns::DnsChoice,
        roots: &[Vec<u8>],
        target: &Target,
    ) -> std::io::Result<tokio::net::TcpStream> {
        let other = |e: String| std::io::Error::other(e);
        let at = match self.host.parse::<IpAddr>() {
            Ok(ip) => SocketAddr::new(ip, self.port),
            Err(_) if crate::net::is_this_computer(&self.host) => {
                SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), self.port)
            }
            Err(_) => {
                let ips = crate::dns::lookup(dns, &self.host, self.port, roots, route)
                    .await
                    .map_err(|e| other(e.to_string()))?;
                SocketAddr::new(
                    *ips.first()
                        .ok_or_else(|| other(format!("the proxy {} has no address", self.host)))?,
                    self.port,
                )
            }
        };
        let stream = crate::net::tcp_connect(route, at).await.map_err(|e| {
            other(format!(
                "the SOCKS proxy {}:{} can't be reached: {e}",
                self.host, self.port
            ))
        })?;
        let said = |e: tokio_socks::Error| {
            other(format!(
                "the SOCKS proxy {}:{} refused: {e}",
                self.host, self.port
            ))
        };
        let target: tokio_socks::TargetAddr<'_> = match target {
            Target::Name(host, port) => {
                tokio_socks::TargetAddr::Domain(host.as_str().into(), *port)
            }
            Target::Addr(a) => tokio_socks::TargetAddr::Ip(*a),
        };
        if self.version == 4 {
            return Ok(
                tokio_socks::tcp::Socks4Stream::connect_with_socket(stream, target)
                    .await
                    .map_err(said)?
                    .into_inner(),
            );
        }
        Ok(match &self.login {
            Some((user, pass)) => tokio_socks::tcp::Socks5Stream::connect_with_password_and_socket(
                stream, target, user, pass,
            )
            .await
            .map_err(said)?
            .into_inner(),
            None => tokio_socks::tcp::Socks5Stream::connect_with_socket(stream, target)
                .await
                .map_err(said)?
                .into_inner(),
        })
    }
}

fn net(kind: NetKind, cause: &str) -> Error {
    if cause.contains(crate::tls::NO_ROOTS) {
        return Error::NoCertificates;
    }
    Error::Net {
        kind,
        cause: crate::download::strip_os_error_code(cause),
    }
}

/// The server's answer, without its code.
fn reply(r: &suppaftp::types::Response) -> Error {
    let text = String::from_utf8_lossy(&r.body);
    let text = text.trim();
    let code = r.status.code();
    let text = text
        .strip_prefix(&code.to_string())
        .map_or(text, |t| t.trim_start_matches(['-', ' ']));
    Error::FtpReply {
        code,
        text: text.lines().next().unwrap_or("").trim().to_owned(),
    }
}

/// An FTP failure as an engine error; `kind` says what was being done when the network failed.
fn failure(e: FtpError, kind: NetKind) -> Error {
    match e {
        FtpError::UnexpectedResponse(r) => reply(&r),
        FtpError::ConnectionError(io) if io.kind() == std::io::ErrorKind::TimedOut => {
            net(NetKind::Timeout, "")
        }
        FtpError::ConnectionError(io) => net(kind, &io.to_string()),
        FtpError::SecureError(s) => net(NetKind::Tls, &s),
        other => net(kind, &other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(url: &str, headers: &HeaderMap) -> Site {
        Site::new(&url.parse().unwrap(), headers, &Options::default()).unwrap()
    }

    #[test]
    fn addresses_name_the_server_file_and_login() {
        let none = HeaderMap::new();
        let s = site("ftp://ftp.example.com/pub/a%20b.iso;type=i", &none);
        assert_eq!((s.host.as_str(), s.port), ("ftp.example.com", 21));
        assert_eq!(s.path, "pub/a b.iso");
        assert_eq!((s.user.as_str(), s.pass.as_str()), ("anonymous", "mdm@"));
        let s = site("ftps://me:p%40ss@[::1]/%2Fabs/x", &none);
        assert_eq!(
            (s.host.as_str(), s.port, s.security),
            ("::1", 990, Security::Implicit)
        );
        assert_eq!(
            (s.user.as_str(), s.pass.as_str(), s.path.as_str()),
            ("me", "p@ss", "/abs/x")
        );
        let s = site("ftpes://host:2121/x", &none);
        assert_eq!((s.port, s.security), (2121, Security::Explicit));
    }

    #[test]
    fn a_basic_header_logs_in_when_the_address_has_no_password() {
        let mut h = HeaderMap::new();
        let token = base64::engine::general_purpose::STANDARD.encode("bob:secret");
        h.insert(AUTHORIZATION, format!("Basic {token}").parse().unwrap());
        let s = site("ftp://bob@host/x", &h);
        assert_eq!((s.user.as_str(), s.pass.as_str()), ("bob", "secret"));
        // The address's own password wins.
        let s = site("ftp://ann:pw@host/x", &h);
        assert_eq!((s.user.as_str(), s.pass.as_str()), ("ann", "pw"));
        let u: Url = "ftp://ann:pw@host/x".parse().unwrap();
        assert_eq!(without_password(&u).as_str(), "ftp://ann@host/x");
    }
}
