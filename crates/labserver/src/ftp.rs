//! A small FTP server for tests: the same deterministic files as HTTP's `/gen`, at
//! `/gen/{size}[/{name}]` (seed 0, or `/gen/{size}/s{seed}/{name}`), with `SIZE`, `MDTM`,
//! `REST`, `RETR`, passive (`PASV`, `EPSV`) and active (`PORT`, `EPRT`) data connections, an
//! optional login, and FTPS: explicit (`AUTH TLS`, then required before logging in) or implicit
//! (TLS from the first byte), with protected data connections, under a certificate from its own
//! test CA (`FtpServer::ca_der`). It counts how many files it sends at once (tests check a
//! download used several connections) and can misbehave on purpose (`FtpConfig`).

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls;

use crate::content;

/// FTPS, or not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FtpTls {
    #[default]
    None,
    /// `AUTH TLS` on the plain control connection (`ftpes://`); required before `USER`.
    Explicit,
    /// TLS from the first byte (`ftps://`).
    Implicit,
}

#[derive(Clone, Debug)]
pub struct FtpConfig {
    /// The user name and password sessions must give (else any).
    pub login: Option<(String, String)>,
    pub tls: FtpTls,
    /// Bytes per second per transfer (else as fast as the socket takes them).
    pub rate: Option<u64>,
    /// Every this many sessions, one sends at `slow_rate` instead (a slow path to the server).
    pub slow_every: Option<usize>,
    pub slow_rate: u64,
    /// Each transfer stops after this many bytes (the data connection closes, `426`).
    pub cut: Option<u64>,
    /// More sessions at once are turned away with `421`, as servers limit connections per user.
    pub max_sessions: Option<usize>,
    /// `REST` works (and `FEAT` says so).
    pub rest: bool,
}

impl Default for FtpConfig {
    fn default() -> Self {
        FtpConfig {
            login: None,
            tls: FtpTls::None,
            rate: None,
            slow_every: None,
            slow_rate: 256 << 10,
            cut: None,
            max_sessions: None,
            rest: true,
        }
    }
}

/// What `MDTM` answers until `FtpServer::set_modified` changes it.
pub const MODIFIED: &str = "20260921141320";

/// A running FTP server; stopped when dropped.
pub struct FtpServer {
    addr: SocketAddr,
    shared: Arc<Shared>,
    task: JoinHandle<()>,
}

struct Shared {
    config: FtpConfig,
    tls: Option<(TlsAcceptor, Vec<u8>)>,
    modified: Mutex<String>,
    sending: AtomicUsize,
    peak: AtomicUsize,
    sessions: AtomicUsize,
    transfers: AtomicUsize,
    open: AtomicUsize,
    refused: AtomicUsize,
}

impl FtpServer {
    pub async fn start(bind: SocketAddr, config: FtpConfig) -> io::Result<FtpServer> {
        let listener = TcpListener::bind(bind).await?;
        let addr = listener.local_addr()?;
        let tls = match config.tls {
            FtpTls::None => None,
            _ => {
                let (config, ca) = certificate().map_err(io::Error::other)?;
                Some((TlsAcceptor::from(Arc::new(config)), ca))
            }
        };
        let shared = Arc::new(Shared {
            config,
            tls,
            modified: Mutex::new(MODIFIED.to_owned()),
            sending: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            sessions: AtomicUsize::new(0),
            transfers: AtomicUsize::new(0),
            open: AtomicUsize::new(0),
            refused: AtomicUsize::new(0),
        });
        let sh = shared.clone();
        let task = tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                let n = sh.sessions.fetch_add(1, Ordering::SeqCst) + 1;
                let slow = sh
                    .config
                    .slow_every
                    .is_some_and(|k| k > 0 && n.is_multiple_of(k));
                tokio::spawn(serve(sock, sh.clone(), slow));
            }
        });
        Ok(FtpServer { addr, shared, task })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The address of `path` (starting with `/`): `ftp://127.0.0.1:port`, or for FTPS
    /// `ftpes://` / `ftps://` at `localhost` (the certificate's name).
    pub fn url(&self, path: &str) -> String {
        let port = self.addr.port();
        match self.shared.config.tls {
            FtpTls::None => format!("ftp://{}{path}", self.addr),
            FtpTls::Explicit => format!("ftpes://localhost:{port}{path}"),
            FtpTls::Implicit => format!("ftps://localhost:{port}{path}"),
        }
    }

    /// The test CA's certificate (DER) for an FTPS server: a client trusting it trusts this
    /// server as `localhost`.
    pub fn ca_der(&self) -> Option<Vec<u8>> {
        self.shared.tls.as_ref().map(|(_, ca)| ca.clone())
    }

    /// What `MDTM` answers from now on (`YYYYMMDDHHMMSS`): the file "changed".
    pub fn set_modified(&self, stamp: &str) {
        *self.shared.modified.lock().unwrap() = stamp.to_owned();
    }

    /// The most files sent at the same time so far.
    pub fn peak_transfers(&self) -> usize {
        self.shared.peak.load(Ordering::SeqCst)
    }

    /// Sessions (control connections) so far, including refused ones.
    pub fn sessions(&self) -> usize {
        self.shared.sessions.load(Ordering::SeqCst)
    }

    /// Files (or parts of them) sent or started so far.
    pub fn transfers(&self) -> usize {
        self.shared.transfers.load(Ordering::SeqCst)
    }

    /// Sessions turned away by `max_sessions`.
    pub fn refused(&self) -> usize {
        self.shared.refused.load(Ordering::SeqCst)
    }
}

impl Drop for FtpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A test CA and a `localhost` certificate it signed: the server's configuration, and the CA's
/// DER (FTPS here, HTTPS for `Config::tls`).
pub(crate) fn certificate() -> Result<(rustls::ServerConfig, Vec<u8>), String> {
    use rcgen::{
        BasicConstraints, CertificateParams, CertifiedIssuer, DnType, IsCa, KeyPair,
        KeyUsagePurpose,
    };
    let e = |e: rcgen::Error| e.to_string();
    let mut ca = CertificateParams::new(Vec::<String>::new()).map_err(e)?;
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.distinguished_name
        .push(DnType::CommonName, "MDM lab test CA");
    ca.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let ca = CertifiedIssuer::self_signed(ca, KeyPair::generate().map_err(e)?).map_err(e)?;
    let key = KeyPair::generate().map_err(e)?;
    let mut leaf = CertificateParams::new(vec!["localhost".to_owned()]).map_err(e)?;
    leaf.distinguished_name
        .push(DnType::CommonName, "localhost");
    let leaf = leaf.signed_by(&key, &ca).map_err(e)?;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(
            vec![leaf.der().clone(), ca.der().clone()],
            rustls::pki_types::PrivateKeyDer::Pkcs8(key.serialize_der().into()),
        )
        .map_err(|e| e.to_string())?;
    Ok((config, ca.der().to_vec()))
}

/// `/gen/{size}[/s{seed}][/{name}]` → (seed, size).
fn file_of(path: &str) -> Option<(u64, u64)> {
    let mut parts = path.trim_start_matches('/').split('/');
    if parts.next()? != "gen" {
        return None;
    }
    let size = content::parse_size(parts.next()?)?;
    let seed = parts
        .next()
        .and_then(|p| p.strip_prefix('s'))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Some((seed, size))
}

/// Where the next transfer's data connection comes from.
enum Data {
    None,
    Passive(TcpListener),
    Active(SocketAddr),
}

/// One session's state, kept across the switch to TLS.
struct Session {
    user: String,
    logged_in: bool,
    rest: u64,
    data: Data,
    /// The control connection is encrypted.
    secure: bool,
    /// `PROT P`: data connections are encrypted too.
    protect: bool,
    /// Sends at `slow_rate` (`slow_every`).
    slow: bool,
}

/// Counts an open session while it lives.
struct Open<'a>(&'a AtomicUsize);

impl Drop for Open<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn serve(mut sock: TcpStream, sh: Arc<Shared>, slow: bool) {
    let now = sh.open.fetch_add(1, Ordering::SeqCst) + 1;
    let _open = Open(&sh.open);
    if sh.config.max_sessions.is_some_and(|max| now > max) {
        sh.refused.fetch_add(1, Ordering::SeqCst);
        let _ = sock
            .write_all(b"421 Too many connections from your address\r\n")
            .await;
        return;
    }
    let mut s = Session {
        user: String::new(),
        logged_in: false,
        rest: 0,
        data: Data::None,
        secure: false,
        protect: false,
        slow,
    };
    let acceptor = sh.tls.as_ref().map(|(a, _)| a.clone());
    match (sh.config.tls, acceptor) {
        (FtpTls::Implicit, Some(acceptor)) => {
            let Ok(tls) = acceptor.accept(sock).await else {
                return;
            };
            (s.secure, s.protect) = (true, true);
            control(tls, &sh, &mut s, true).await;
        }
        (_, acceptor) => {
            // Returns the socket when the client asked for TLS.
            if let Some(plain) = control(sock, &sh, &mut s, true).await
                && let Some(acceptor) = acceptor
                && let Ok(tls) = acceptor.accept(plain).await
            {
                s.secure = true;
                control(tls, &sh, &mut s, false).await;
            }
        }
    }
}

fn reply(code: u32, text: &str) -> String {
    format!("{code} {text}\r\n")
}

/// Answer commands on `stream` until the client leaves (None) or switches to TLS (the stream,
/// to wrap).
async fn control<S>(stream: S, sh: &Shared, s: &mut Session, greet: bool) -> Option<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (read, mut write) = tokio::io::split(stream);
    let mut lines = BufReader::new(read).lines();
    if greet
        && write
            .write_all(reply(220, "MDM lab FTP").as_bytes())
            .await
            .is_err()
    {
        return None;
    }
    let config = &sh.config;
    while let Ok(Some(line)) = lines.next_line().await {
        let (verb, arg) = match line.split_once(' ') {
            Some((v, a)) => (v.to_ascii_uppercase(), a.trim().to_owned()),
            None => (line.trim().to_ascii_uppercase(), String::new()),
        };
        let open = matches!(
            verb.as_str(),
            "USER" | "PASS" | "QUIT" | "FEAT" | "SYST" | "OPTS" | "AUTH" | "PBSZ" | "PROT"
        );
        let tls_first = config.tls == FtpTls::Explicit && !s.secure;
        let answer = if !s.logged_in && !open {
            reply(530, "Please log in")
        } else {
            match verb.as_str() {
                "AUTH" if sh.tls.is_some() && !s.secure && arg.eq_ignore_ascii_case("TLS") => {
                    if write
                        .write_all(reply(234, "AUTH TLS OK").as_bytes())
                        .await
                        .is_err()
                    {
                        return None;
                    }
                    // The client sends nothing more until the handshake.
                    return Some(lines.into_inner().into_inner().unsplit(write));
                }
                "AUTH" => reply(504, "Not available"),
                "PBSZ" if s.secure => reply(200, "PBSZ=0"),
                "PROT" if s.secure => {
                    s.protect = arg.eq_ignore_ascii_case("P");
                    reply(200, "OK")
                }
                "PBSZ" | "PROT" => reply(503, "AUTH first"),
                "USER" if tls_first => reply(530, "TLS required (AUTH TLS)"),
                "USER" => {
                    s.user = arg;
                    reply(331, "Password required")
                }
                "PASS" => match config.login.as_ref() {
                    Some((u, p)) if *u != s.user || *p != arg => reply(530, "Login incorrect"),
                    _ => {
                        s.logged_in = true;
                        reply(230, "Logged in")
                    }
                },
                "SYST" => reply(215, "UNIX Type: L8"),
                "FEAT" => format!(
                    "211-Features:\r\n SIZE\r\n MDTM\r\n{} EPSV\r\n{}211 End\r\n",
                    if config.rest { " REST STREAM\r\n" } else { "" },
                    if sh.tls.is_some() {
                        " AUTH TLS\r\n PBSZ\r\n PROT\r\n"
                    } else {
                        ""
                    },
                ),
                "TYPE" | "OPTS" | "NOOP" | "MODE" | "STRU" => reply(200, "OK"),
                "PWD" => reply(257, "\"/\""),
                "CWD" => reply(250, "OK"),
                "SIZE" => match file_of(&arg) {
                    Some((_, size)) => reply(213, &size.to_string()),
                    None => reply(550, "No such file"),
                },
                "MDTM" => match file_of(&arg) {
                    Some(_) => reply(213, &sh.modified.lock().unwrap().clone()),
                    None => reply(550, "No such file"),
                },
                "REST" if !config.rest => reply(502, "REST not implemented"),
                "REST" => match arg.parse() {
                    Ok(n) => {
                        s.rest = n;
                        reply(350, &format!("Restarting at {n}"))
                    }
                    Err(_) => reply(501, "Bad offset"),
                },
                "PASV" | "EPSV" => match TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await {
                    Ok(l) => {
                        let port = l.local_addr().map(|a| a.port()).unwrap_or(0);
                        s.data = Data::Passive(l);
                        if verb == "PASV" {
                            reply(
                                227,
                                &format!(
                                    "Entering Passive Mode (127,0,0,1,{},{})",
                                    port >> 8,
                                    port & 0xff
                                ),
                            )
                        } else {
                            reply(229, &format!("Entering Extended Passive Mode (|||{port}|)"))
                        }
                    }
                    Err(_) => reply(425, "Can't open data connection"),
                },
                "PORT" => {
                    let n: Vec<u16> = arg
                        .split(',')
                        .filter_map(|x| x.trim().parse().ok())
                        .collect();
                    if n.len() == 6 {
                        let ip = Ipv4Addr::new(n[0] as u8, n[1] as u8, n[2] as u8, n[3] as u8);
                        s.data = Data::Active(SocketAddr::new(IpAddr::V4(ip), n[4] << 8 | n[5]));
                        reply(200, "PORT OK")
                    } else {
                        reply(501, "Bad PORT")
                    }
                }
                "EPRT" => {
                    // |proto|address|port|
                    let f: Vec<&str> = arg.split('|').collect();
                    match (
                        f.get(2).and_then(|a| a.parse::<IpAddr>().ok()),
                        f.get(3).and_then(|p| p.parse().ok()),
                    ) {
                        (Some(ip), Some(port)) => {
                            s.data = Data::Active(SocketAddr::new(ip, port));
                            reply(200, "EPRT OK")
                        }
                        _ => reply(501, "Bad EPRT"),
                    }
                }
                "RETR" => {
                    let Some((seed, size)) = file_of(&arg) else {
                        let _ = write.write_all(reply(550, "No such file").as_bytes()).await;
                        continue;
                    };
                    let from = std::mem::take(&mut s.rest).min(size);
                    let conn = match std::mem::replace(&mut s.data, Data::None) {
                        Data::Passive(l) => l.accept().await.map(|(c, _)| c),
                        Data::Active(a) => TcpStream::connect(a).await,
                        Data::None => Err(io::Error::other("no data connection")),
                    };
                    let Ok(conn) = conn else {
                        let _ = write
                            .write_all(reply(425, "Can't open data connection").as_bytes())
                            .await;
                        continue;
                    };
                    if write
                        .write_all(reply(150, "Opening BINARY data connection").as_bytes())
                        .await
                        .is_err()
                    {
                        return None;
                    }
                    let now = sh.sending.fetch_add(1, Ordering::SeqCst) + 1;
                    sh.peak.fetch_max(now, Ordering::SeqCst);
                    sh.transfers.fetch_add(1, Ordering::SeqCst);
                    let end = config.cut.map_or(size, |cut| (from + cut).min(size));
                    let rate = if s.slow {
                        Some(config.slow_rate)
                    } else {
                        config.rate
                    };
                    let sent = match (&sh.tls, s.protect) {
                        (Some((acceptor, _)), true) => match acceptor.accept(conn).await {
                            Ok(mut tls) => send(&mut tls, seed, from, end, rate).await,
                            Err(e) => Err(e),
                        },
                        _ => {
                            let mut conn = conn;
                            send(&mut conn, seed, from, end, rate).await
                        }
                    };
                    sh.sending.fetch_sub(1, Ordering::SeqCst);
                    match sent {
                        Ok(()) if end == size => reply(226, "Transfer complete"),
                        _ => reply(426, "Connection closed; transfer aborted"),
                    }
                }
                "ABOR" => reply(226, "ABOR OK"),
                "QUIT" => {
                    let _ = write.write_all(reply(221, "Bye").as_bytes()).await;
                    return None;
                }
                _ => reply(502, "Not implemented"),
            }
        };
        if write.write_all(answer.as_bytes()).await.is_err() {
            return None;
        }
    }
    None
}

/// Bytes `[from, end)` of the file `seed`, at `rate` bytes/s if given; the data connection is
/// closed after.
async fn send<W>(conn: &mut W, seed: u64, from: u64, end: u64, rate: Option<u64>) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; if rate.is_some() { 16 << 10 } else { 64 << 10 }];
    let started = tokio::time::Instant::now();
    let mut at = from;
    while at < end {
        let n = ((end - at) as usize).min(buf.len());
        content::fill(seed, at, &mut buf[..n]);
        conn.write_all(&buf[..n]).await?;
        at += n as u64;
        if let Some(rate) = rate.filter(|r| *r > 0) {
            let due = Duration::from_secs_f64((at - from) as f64 / rate as f64);
            tokio::time::sleep_until(started + due).await;
        }
    }
    conn.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_name_files() {
        assert_eq!(file_of("/gen/64K/a.bin"), Some((0, 64 << 10)));
        assert_eq!(file_of("gen/2M/s7/b.iso"), Some((7, 2 << 20)));
        assert_eq!(file_of("/other/1K"), None);
    }

    #[test]
    fn makes_a_certificate() {
        assert!(certificate().is_ok());
    }
}
