//! A SOCKS proxy for the tests (#1558): SOCKS5 (with or without a user name and password),
//! SOCKS4 and SOCKS4a. It records where each connection asked to go, a name (socks5h, socks4a:
//! the proxy looks it up) or an address (socks5, socks4: the client looked it up), and relays it.
//! Names it is told of (`SocksConfig::names`) go to their address, others are looked up.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// One connection the proxy was asked for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SocksRequest {
    /// 5 or 4.
    pub version: u8,
    /// The name or the address asked for.
    pub target: String,
    pub port: u16,
    /// A name, for the proxy to look up (socks5h, socks4a).
    pub by_name: bool,
    /// The user name given (SOCKS5's login, SOCKS4's user id), if any.
    pub user: Option<String>,
    /// Let through: logged in (when a login is asked) and connected.
    pub ok: bool,
}

/// Who called with every request, as it is answered.
pub type OnRequest = Arc<dyn Fn(&SocksRequest) + Send + Sync>;

#[derive(Clone, Default)]
pub struct SocksConfig {
    /// The user name and password SOCKS5 clients must give (SOCKS4 is then refused); None: no
    /// login.
    pub login: Option<(String, String)>,
    /// Names the proxy knows the address of (`lab.test` → 127.0.0.1).
    pub names: Vec<(String, IpAddr)>,
    pub on_request: Option<OnRequest>,
}

pub struct SocksServer {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<SocksRequest>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for SocksServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl SocksServer {
    pub async fn start(bind: SocketAddr, config: SocksConfig) -> std::io::Result<SocksServer> {
        let listener = TcpListener::bind(bind).await?;
        let addr = listener.local_addr()?;
        let seen = Arc::new(Mutex::new(Vec::new()));
        let config = Arc::new(config);
        let task = {
            let seen = seen.clone();
            tokio::spawn(async move {
                loop {
                    // A connection the client dropped before it was taken (Windows says so as
                    // an error) mustn't end the proxy.
                    let conn = match listener.accept().await {
                        Ok((conn, _)) => conn,
                        Err(_) => {
                            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                            continue;
                        }
                    };
                    let (seen, config) = (seen.clone(), config.clone());
                    tokio::spawn(async move {
                        let _ = serve(conn, &config, &seen).await;
                    });
                }
            })
        };
        Ok(SocksServer { addr, seen, task })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Every request so far, in order.
    pub fn seen(&self) -> Vec<SocksRequest> {
        self.seen.lock().unwrap().clone()
    }
}

async fn serve(
    mut conn: TcpStream,
    config: &SocksConfig,
    seen: &Mutex<Vec<SocksRequest>>,
) -> std::io::Result<()> {
    let record = |r: SocksRequest| {
        if let Some(f) = &config.on_request {
            f(&r);
        }
        seen.lock().unwrap().push(r);
    };
    match conn.read_u8().await? {
        5 => {
            let n = conn.read_u8().await? as usize;
            let mut methods = vec![0; n];
            conn.read_exact(&mut methods).await?;
            let mut user = None;
            match &config.login {
                Some((want_user, want_pass)) => {
                    if !methods.contains(&2) {
                        conn.write_all(&[5, 0xFF]).await?;
                        return Ok(());
                    }
                    conn.write_all(&[5, 2]).await?;
                    let _ver = conn.read_u8().await?;
                    let given_user = read_short(&mut conn).await?;
                    let given_pass = read_short(&mut conn).await?;
                    let right = given_user == *want_user && given_pass == *want_pass;
                    conn.write_all(&[1, if right { 0 } else { 1 }]).await?;
                    if !right {
                        record(SocksRequest {
                            version: 5,
                            target: String::new(),
                            port: 0,
                            by_name: false,
                            user: Some(given_user),
                            ok: false,
                        });
                        return Ok(());
                    }
                    user = Some(given_user);
                }
                None => {
                    if !methods.contains(&0) {
                        conn.write_all(&[5, 0xFF]).await?;
                        return Ok(());
                    }
                    conn.write_all(&[5, 0]).await?;
                }
            }
            let mut head = [0; 4];
            conn.read_exact(&mut head).await?;
            let (target, by_name) = match head[3] {
                1 => {
                    let mut ip = [0; 4];
                    conn.read_exact(&mut ip).await?;
                    (Ipv4Addr::from(ip).to_string(), false)
                }
                4 => {
                    let mut ip = [0; 16];
                    conn.read_exact(&mut ip).await?;
                    (std::net::Ipv6Addr::from(ip).to_string(), false)
                }
                3 => (read_short(&mut conn).await?, true),
                _ => return Ok(()),
            };
            let port = conn.read_u16().await?;
            let upstream = if head[1] == 1 {
                connect(config, &target, port).await
            } else {
                None
            };
            record(SocksRequest {
                version: 5,
                target,
                port,
                by_name,
                user,
                ok: upstream.is_some(),
            });
            let Some(mut upstream) = upstream else {
                // 7: command not supported; 5: connection refused.
                let code = if head[1] == 1 { 5 } else { 7 };
                conn.write_all(&[5, code, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
                return Ok(());
            };
            conn.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            tokio::io::copy_bidirectional(&mut conn, &mut upstream).await?;
        }
        4 => {
            let command = conn.read_u8().await?;
            let port = conn.read_u16().await?;
            let mut ip = [0; 4];
            conn.read_exact(&mut ip).await?;
            let user = read_nul(&mut conn).await?;
            // 0.0.0.x (x not 0): SOCKS4a, the name follows.
            let (target, by_name) = if ip[..3] == [0, 0, 0] && ip[3] != 0 {
                (read_nul(&mut conn).await?, true)
            } else {
                (Ipv4Addr::from(ip).to_string(), false)
            };
            let upstream = if command == 1 && config.login.is_none() {
                connect(config, &target, port).await
            } else {
                None
            };
            record(SocksRequest {
                version: 4,
                target,
                port,
                by_name,
                user: (!user.is_empty()).then_some(user),
                ok: upstream.is_some(),
            });
            let Some(mut upstream) = upstream else {
                conn.write_all(&[0, 0x5B, 0, 0, 0, 0, 0, 0]).await?;
                return Ok(());
            };
            conn.write_all(&[0, 0x5A, 0, 0, 0, 0, 0, 0]).await?;
            tokio::io::copy_bidirectional(&mut conn, &mut upstream).await?;
        }
        _ => {}
    }
    Ok(())
}

/// The connection asked for, None when it can't be made.
async fn connect(config: &SocksConfig, target: &str, port: u16) -> Option<TcpStream> {
    let known = config
        .names
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(target))
        .map(|(_, ip)| *ip);
    let all: Vec<SocketAddr> = match known.or_else(|| target.parse().ok()) {
        Some(ip) => vec![SocketAddr::new(ip, port)],
        None => tokio::net::lookup_host((target, port))
            .await
            .ok()?
            .collect(),
    };
    // Each address in turn (an IPv6 one this computer can't reach comes first now and then).
    for at in all {
        let tried =
            tokio::time::timeout(std::time::Duration::from_secs(10), TcpStream::connect(at)).await;
        if let Ok(Ok(s)) = tried {
            return Some(s);
        }
    }
    None
}

/// A length-prefixed string (SOCKS5's names, user names and passwords).
async fn read_short(conn: &mut (impl AsyncRead + Unpin)) -> std::io::Result<String> {
    let n = conn.read_u8().await? as usize;
    let mut buf = vec![0; n];
    conn.read_exact(&mut buf).await?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// A NUL-terminated string (SOCKS4's user id and name), at most 255 bytes.
async fn read_nul(conn: &mut (impl AsyncRead + Unpin)) -> std::io::Result<String> {
    let mut buf = Vec::new();
    loop {
        match conn.read_u8().await? {
            0 => break,
            b if buf.len() < 255 => buf.push(b),
            _ => return Err(std::io::ErrorKind::InvalidData.into()),
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}
