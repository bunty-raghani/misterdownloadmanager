//! The connection a browser makes (#668), for the one retry a download gets when a site refused
//! MDM's own as a program's: its bot check (`Error::BotCheck`), or a 412 to a request without
//! preconditions (`download::refused_as_a_program`).
//!
//! Bot checks (Cloudflare's and others') don't only read the User-Agent: they recognise how the
//! connection is made, its TLS handshake, its HTTP/2 settings and the order of its headers.
//! MDM's own connection (reqwest on rustls) doesn't look like a browser's, so a site that lets
//! only browsers through refuses it even when the same file downloads in the browser. This
//! connection is made the way that browser family makes it (`wreq` on BoringSSL, with
//! `wreq-util`'s profiles: Chrome's for Chrome and the Chromium browsers, Edge's, Opera's,
//! Firefox's, Safari's), with the download's own User-Agent and headers (the browser's cookies
//! for that site, handed over by the extension, scoped to it as everywhere: `scoped_headers`).
//!
//! It solves nothing: no script runs, no challenge is answered. A site that wants its challenge
//! solved still refuses, and the download fails as before (the browser can take it).
//!
//! Everything else stays the engine's: the proxy (`Options::proxy`), the DNS and the network
//! adapter (`Options::dns`, `Options::route`: a connection that can't be held to the adapter
//! isn't made), speed limits, stop and resume (`download.rs` reads its answers like reqwest's).
//! It follows redirects itself, as `get` does, so credentials never leave the site.
//!
//! A download on it uses as many connections as any other (Options → Connection, the site's
//! exception), each asking for its part of the file (#668: one connection was slow where the
//! site takes many). Each request at a time has a connection of its own (`Lease`): over HTTP/2,
//! which these profiles speak as browsers do, the requests would otherwise share one. The first
//! part asked for tells whether the site takes ranges on it; one that refuses them (403, its bot
//! check, 412) or ignores them leaves the download on one connection, as before
//! (`download::request`).
//!
//! A stream's pieces (mdm-tools' `pieces`, #1289) use it through [`Connection`] once a piece was
//! refused as a program's ([`refuses_programs`]).
//!
//! Built with the `browser-connection` feature (on by default; BoringSSL needs CMake, a C
//! compiler, libclang and, on Windows, NASM to build). Without it, `AVAILABLE` is false and the
//! bot check's refusal stands.

#[cfg(feature = "browser-connection")]
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use reqwest::StatusCode;
use reqwest::header::{HeaderMap, HeaderValue};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::download::{Body, Error, Head, Options};

/// Whether this build has the browser-like connection.
pub const AVAILABLE: bool = cfg!(feature = "browser-connection");

/// Most HTTP redirects a request follows (as `download::MAX_REDIRECTS`, browsers' 20).
#[cfg_attr(not(feature = "browser-connection"), allow(dead_code))]
const MAX_REDIRECTS: usize = 20;

/// A browser-like client for one download (`Browser::new`).
pub(crate) struct Browser {
    #[cfg(feature = "browser-connection")]
    client: wreq::Client,
    /// The User-Agent its requests carry: the download's own.
    #[cfg_attr(not(feature = "browser-connection"), allow(dead_code))]
    agent: HeaderValue,
    #[cfg_attr(not(feature = "browser-connection"), allow(dead_code))]
    route: crate::net::Route,
    /// The first address is on this computer, which isn't bound to the adapter (#35): its
    /// redirects may only lead back here.
    #[cfg_attr(not(feature = "browser-connection"), allow(dead_code))]
    stay_here: bool,
    /// The connections whose answers are coming in, by number ([`Lease`]).
    #[cfg(feature = "browser-connection")]
    in_use: Arc<Mutex<Vec<bool>>>,
}

/// One connection of a [`Browser`], taken for one request and its answer: the lowest number no
/// answer coming in holds, given back when its answer is done with (dropped). Requests at the
/// same time get connections of their own (wreq's `Group`: its pool keeps each number's apart),
/// as MDM's own connection does (HTTP/1.1, `download::client`), where HTTP/2 would carry them
/// all on one; one request after another reuses the same, kept open.
#[cfg(feature = "browser-connection")]
struct Lease {
    in_use: Arc<Mutex<Vec<bool>>>,
    n: usize,
}

#[cfg(feature = "browser-connection")]
impl Lease {
    fn take(in_use: &Arc<Mutex<Vec<bool>>>) -> Lease {
        let mut taken = in_use.lock().unwrap_or_else(|e| e.into_inner());
        let n = match taken.iter().position(|t| !t) {
            Some(n) => n,
            None => {
                taken.push(false);
                taken.len() - 1
            }
        };
        taken[n] = true;
        Lease {
            in_use: in_use.clone(),
            n,
        }
    }
}

#[cfg(feature = "browser-connection")]
impl Drop for Lease {
    fn drop(&mut self) {
        let mut taken = self.in_use.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = taken.get_mut(self.n) {
            *t = false;
        }
    }
}

/// An answer's body coming in through the browser-like connection.
pub(crate) struct Stream {
    #[cfg(feature = "browser-connection")]
    inner: std::pin::Pin<Box<dyn futures::Stream<Item = wreq::Result<Bytes>> + Send>>,
    /// Its connection, held until the body is done with.
    #[cfg(feature = "browser-connection")]
    _lease: Lease,
    len: Option<u64>,
}

impl Stream {
    /// The next bytes; None at the end.
    pub(crate) async fn chunk(&mut self) -> Result<Option<Bytes>, Error> {
        #[cfg(feature = "browser-connection")]
        {
            use futures::StreamExt;
            match self.inner.next().await {
                None => Ok(None),
                Some(Ok(b)) => Ok(Some(b)),
                Some(Err(e)) => Err(net_error(&e)),
            }
        }
        #[cfg(not(feature = "browser-connection"))]
        Ok(None)
    }

    /// The bytes it will carry, if known.
    pub(crate) fn len(&self) -> Option<u64> {
        self.len
    }
}

impl Browser {
    /// A client for `url` with `opts`' User-Agent (or the one `headers` carry, which wins as it
    /// does on MDM's own requests), proxy, DNS and network adapter. Refused when this build has
    /// none, or when the adapter can't hold its connections (the kill switch, #35).
    pub(crate) fn new(opts: &Options, url: &Url, headers: &HeaderMap) -> Result<Browser, Error> {
        let agent = headers
            .get(reqwest::header::USER_AGENT)
            .cloned()
            .or_else(|| HeaderValue::from_str(&opts.user_agent).ok())
            .filter(|a| !a.is_empty())
            .or_else(|| HeaderValue::from_str(&crate::agent::automatic()).ok())
            .unwrap_or_else(|| HeaderValue::from_static(crate::agent::IE11_USER_AGENT));
        #[cfg(feature = "browser-connection")]
        {
            let (client, stay_here) = imp::client(opts, url, agent.to_str().unwrap_or(""))?;
            Ok(Browser {
                client,
                agent,
                route: opts.route.clone(),
                stay_here,
                in_use: Default::default(),
            })
        }
        #[cfg(not(feature = "browser-connection"))]
        {
            let _ = (url, agent);
            Err(Error::BotCheck)
        }
    }

    /// `GET url` with `headers` (a `Range` among them for a range: then without compression, as
    /// browsers ask for ranges), its HTTP redirects followed with the credentials scoped to each
    /// address and the cookies a redirect sets kept for its own site (as `get` does). With the
    /// answer, the headers as they go on at the address it ended at.
    pub(crate) async fn get(
        &self,
        url: Url,
        headers: HeaderMap,
        ranged: bool,
        cancel: &CancellationToken,
    ) -> Result<(Head, Body, HeaderMap), Error> {
        let (head, body, headers) = self.stream(url, headers, ranged, cancel).await?;
        Ok((head, Body::Browser(body), headers))
    }

    /// [`Browser::get`], its body as it comes.
    async fn stream(
        &self,
        url: Url,
        headers: HeaderMap,
        ranged: bool,
        cancel: &CancellationToken,
    ) -> Result<(Head, Stream, HeaderMap), Error> {
        #[cfg(feature = "browser-connection")]
        {
            imp::get(self, url, headers, ranged, cancel).await
        }
        #[cfg(not(feature = "browser-connection"))]
        {
            let _ = (url, headers, ranged, cancel);
            Err(Error::BotCheck)
        }
    }
}

/// A site's refusal of a program's connection (#668): its bot check (Cloudflare's challenge,
/// Akamai's edge page: `download::is_bot_check`), or 412 Precondition Failed to a request that
/// carries no precondition (FileHippo's, #630). The browser-like connection may get past it; a
/// plain 401, 403, 404 or 410 is the address's own refusal (an expired link), not this.
pub fn refuses_programs(status: StatusCode, headers: &HeaderMap) -> bool {
    crate::download::is_bot_check(status, headers) || status == StatusCode::PRECONDITION_FAILED
}

/// The browser-like connection for requests of MDM's other parts (a stream's pieces, #1289):
/// made for an address with the options and headers its requests go with, as a download's is
/// (the User-Agent of `headers`, else of the options; the proxy, the DNS and the adapter of the
/// options). Cloned, it is the same connection: requests at the same time each get a connection
/// of its own.
#[derive(Clone)]
pub struct Connection(std::sync::Arc<Browser>);

impl Connection {
    /// Refused when this build has none ([`AVAILABLE`]: `Error::BotCheck`), or when the network
    /// adapter can't hold its connections (#35).
    pub fn new(opts: &Options, url: &Url, headers: &HeaderMap) -> Result<Connection, Error> {
        Ok(Connection(std::sync::Arc::new(Browser::new(
            opts, url, headers,
        )?)))
    }

    /// `GET url` with `headers` (a `Range` among them asks for its bytes as they are), its
    /// redirects followed with the credentials kept on their site.
    pub async fn get(
        &self,
        url: Url,
        headers: HeaderMap,
        cancel: &CancellationToken,
    ) -> Result<Answer, Error> {
        let ranged = headers.contains_key(reqwest::header::RANGE);
        let (head, body, _) = self.0.stream(url, headers, ranged, cancel).await?;
        Ok(Answer {
            status: head.status,
            headers: head.headers,
            url: head.url,
            body,
        })
    }
}

/// An answer through a [`Connection`].
pub struct Answer {
    pub status: StatusCode,
    pub headers: HeaderMap,
    /// Where it came from, its redirects followed.
    pub url: Url,
    body: Stream,
}

impl Answer {
    /// The bytes its body will carry, if the server said.
    pub fn content_length(&self) -> Option<u64> {
        self.body.len()
    }

    /// Its body's next bytes; None at the end.
    pub async fn chunk(&mut self) -> Result<Option<Bytes>, Error> {
        self.body.chunk().await
    }
}

/// The first answer through the browser-like connection is the file (or its start): 200, or
/// 206; never the site's refusal, nor a web page where a file was expected (`expects`, as `get`
/// decides it).
pub(crate) fn check_first(head: &Head, expects: bool) -> Result<(), Error> {
    use crate::download::{is_bot_check, is_web_page, named, retry_after};
    match head.status {
        StatusCode::OK | StatusCode::PARTIAL_CONTENT => {}
        s if s == StatusCode::TOO_MANY_REQUESTS || s == StatusCode::SERVICE_UNAVAILABLE => {
            return Err(Error::Throttled(s, retry_after(&head.headers)));
        }
        s if is_bot_check(s, &head.headers) => return Err(Error::BotCheck),
        s => return Err(Error::Status(s)),
    }
    let page = is_web_page(&head.headers) && !named(&head.headers);
    if page && (expects || crate::sites::expects_file(&head.url)) {
        return Err(Error::GotWebPage);
    }
    Ok(())
}

/// A failure of the browser-like connection in the engine's words (`Error::Net`).
#[cfg(feature = "browser-connection")]
fn net_error(e: &wreq::Error) -> Error {
    use crate::download::NetKind;
    let kind = if e.is_tls() {
        NetKind::Tls
    } else if e.is_timeout() {
        NetKind::Timeout
    } else if e.is_body() || e.is_decode() {
        NetKind::Lost
    } else {
        NetKind::Connect
    };
    // The adapter's refusal (#35) as itself, from wherever it was raised.
    let mut source = std::error::Error::source(e);
    let mut cause = None;
    while let Some(s) = source {
        if let Some(n) = s.downcast_ref::<crate::net::NetError>() {
            return Error::Adapter(n.clone());
        }
        cause = Some(s.to_string());
        source = s.source();
    }
    let cause = cause
        .map(|c| crate::download::strip_os_error_code(&c))
        .filter(|c| !c.contains("://"))
        .unwrap_or_default();
    Error::Net { kind, cause }
}

#[cfg(feature = "browser-connection")]
mod imp {
    use std::sync::Arc;

    use reqwest::header::{self, HeaderMap, HeaderValue};
    use tokio_util::sync::CancellationToken;
    use url::Url;
    use wreq::IntoEmulation;
    use wreq_util::{Emulation, Platform, Profile};

    use super::{Browser, Lease, MAX_REDIRECTS, Stream, net_error};
    use crate::download::{
        Error, Head, Options, Proxy, add_cookies, cancellable, scoped_headers, set_cookies,
    };
    use crate::net::{self, NetError, Route};

    /// How long an idle connection is kept (as the download client's).
    const POOL_IDLE: std::time::Duration = std::time::Duration::from_secs(15);

    /// The client, and whether its connections stay on this computer (unbound).
    pub(super) fn client(
        opts: &Options,
        url: &Url,
        agent: &str,
    ) -> Result<(wreq::Client, bool), Error> {
        let route = &opts.route;
        net::check_url(route, url)?;
        let port = url.port_or_known_default().unwrap_or(443);
        let (proxy, through_local_proxy) = proxy_for(opts, url)?;
        let mut b = wreq::Client::builder()
            .emulation(emulation_for(agent))
            // Followed in `get`, where credentials are scoped as everywhere in MDM.
            .redirect(wreq::redirect::Policy::none())
            .connect_timeout(opts.connect_timeout)
            .read_timeout(opts.read_timeout)
            .pool_idle_timeout(POOL_IDLE)
            .pool_max_idle_per_host(1)
            .tls_cert_store(cert_store(&opts.extra_roots)?)
            .dns_resolver(Arc::new(Dns(crate::dns::Resolver::new(
                &opts.hosts,
                port,
                &opts.dns,
                &opts.extra_roots,
                route,
                Some(opts.connect_timeout),
            ))));
        b = match proxy {
            Some(p) => b.proxy(p),
            None => b.no_proxy(),
        };
        // As the download client (`download::client`): a connection to this computer isn't
        // bound, and its redirects may then only lead back here.
        let to_this_computer = through_local_proxy
            || (net::is_this_computer(url.host_str().unwrap_or_default())
                && !matches!(opts.proxy, Proxy::Manual { .. } | Proxy::Pac { .. }));
        let b = bind(b, route, to_this_computer)?;
        let client = b.build().map_err(|e| net_error(&e))?;
        Ok((
            client,
            to_this_computer && !through_local_proxy && !route.is_any(),
        ))
    }

    /// The certificates the connection trusts: Mozilla's, the system's (as MDM's own connection
    /// trusts them: an antivirus's or a company's HTTPS inspection puts its own there) and
    /// `extra` (the tests' own CA). The system's are read once.
    fn cert_store(extra: &[Vec<u8>]) -> Result<wreq::tls::trust::CertStore, Error> {
        static SYSTEM: std::sync::OnceLock<Vec<Vec<u8>>> = std::sync::OnceLock::new();
        let system = SYSTEM.get_or_init(|| {
            rustls_native_certs::load_native_certs()
                .certs
                .into_iter()
                .map(|c| c.to_vec())
                .collect()
        });
        wreq::tls::trust::CertStore::builder()
            .add_der_certs(
                webpki_root_certs::TLS_SERVER_ROOT_CERTS
                    .iter()
                    .map(|c| c.as_ref()),
            )
            .add_der_certs(system.iter())
            .add_der_certs(extra.iter())
            .build()
            .map_err(|e| net_error(&e))
    }

    /// `builder` held to `route`'s adapter as `net::bind_client` holds MDM's own clients: the
    /// address on Windows, the device on Linux, the interface on macOS. Elsewhere, refused.
    fn bind(
        builder: wreq::ClientBuilder,
        route: &Route,
        to_this_computer: bool,
    ) -> Result<wreq::ClientBuilder, NetError> {
        match route {
            Route::Any => Ok(builder),
            Route::Down { label } => Err(NetError::Down {
                label: label.clone(),
            }),
            Route::Bound(_) if to_this_computer => Ok(builder),
            Route::Bound(b) => {
                #[cfg(windows)]
                {
                    Ok(builder.local_address(b.windows_local()))
                }
                #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
                {
                    Ok(builder.interface(b.name.clone()))
                }
                // No way to hold a socket to the adapter here: no connection rather than one
                // around it.
                #[cfg(not(any(
                    windows,
                    target_os = "linux",
                    target_os = "android",
                    target_os = "macos"
                )))]
                {
                    let _ = builder;
                    Err(NetError::Down {
                        label: b.label.clone(),
                    })
                }
            }
        }
    }

    /// The proxy for `url` as `download::client` picks it (none, the system's, the manual one
    /// for its scheme, the script's first usable route), checked against the adapter the same
    /// way; and whether it is on this computer (its connections then aren't bound).
    fn proxy_for(opts: &Options, url: &Url) -> Result<(Option<wreq::Proxy>, bool), Error> {
        let (address, login) = match &opts.proxy {
            Proxy::None => (None, None),
            Proxy::System => (net::system_proxy(url), None),
            Proxy::Manual {
                url: proxy,
                login,
                http,
                https,
            } => {
                let covers = if url.scheme() == "https" {
                    *https
                } else {
                    *http
                };
                (covers.then(|| proxy.clone()), login.clone())
            }
            Proxy::Pac { script } => {
                let first = crate::pac::routes(script, url).ok().and_then(|routes| {
                    routes.into_iter().find_map(|r| match r {
                        crate::pac::Route::Direct => Some(None),
                        crate::pac::Route::Proxy(p) => {
                            wreq::Proxy::all(p.as_str()).ok().map(|_| Some(p))
                        }
                    })
                });
                (first.unwrap_or_else(|| net::system_proxy(url)), None)
            }
        };
        let Some(address) = address else {
            return Ok((None, false));
        };
        let host = Url::parse(&address)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_default();
        let local = net::is_this_computer(&host);
        if !local && let Ok(ip) = host.trim_matches(['[', ']']).parse() {
            opts.route.check(ip)?;
        }
        let bad = |_| Error::Net {
            kind: crate::download::NetKind::Connect,
            cause: format!("the proxy {host} can't be used"),
        };
        let mut proxy = wreq::Proxy::all(address.as_str()).map_err(bad)?;
        if let Some((user, pass)) = login {
            proxy = proxy.basic_auth(&user, &pass);
        }
        Ok((Some(proxy), local))
    }

    /// The profile of the browser `agent` names, at its version (the newest profile up to it;
    /// the oldest when it is older than all), on its system: Edge's, Opera's, Firefox's,
    /// Safari's, else Chrome's (Chrome, Brave, Vivaldi and the other Chromium browsers name
    /// themselves Chrome; so does nothing else). A Chrome or Edge newer than its newest profile
    /// names its own version in `sec-ch-ua` too (`client_hints`), as the User-Agent does
    /// (#1494): the profile's would name an older browser than the User-Agent.
    pub(super) fn emulation_for(agent: &str) -> wreq::Emulation {
        let (profile, platform) = choose(agent);
        let mut emulation = Emulation::builder()
            .profile(profile)
            .platform(platform)
            .build()
            .into_emulation();
        if let Some(hints) = client_hints(agent, profile)
            && let Ok(value) = HeaderValue::from_str(&hints)
        {
            // In the profile's place among its headers.
            emulation
                .headers
                .insert(header::HeaderName::from_static("sec-ch-ua"), value);
        }
        // Chrome and Edge ask for a page (and the file a link leads to) at the highest
        // priority, weight 256 in their HTTP/2 HEADERS frame, as their `priority: u=0, i` says;
        // these profiles say 220 (#1494, measured against Chrome and Edge 157).
        let name = format!("{profile:?}");
        if (name.starts_with("Chrome") || name.starts_with("Edge"))
            && let Some(h2) = emulation.http2_options.as_mut()
        {
            h2.headers_stream_dependency = Some(wreq::http2::StreamDependency::new(
                wreq::http2::StreamId::zero(),
                255,
                true,
            ));
        }
        emulation
    }

    /// The `sec-ch-ua` a Chrome or Edge newer than `profile` sends, from `agent`'s major
    /// version, in Chromium's own way: its brand, "Chromium" and a made-up brand, in an order
    /// and with a made-up brand's name and version that its version picks (Chrome 157:
    /// `"Not-A.Brand";v="99", "Google Chrome";v="157", "Chromium";v="157"`). None when
    /// `profile` is that version or newer (its own is right), or for another browser.
    pub(super) fn client_hints(agent: &str, profile: Profile) -> Option<String> {
        let major = |key: &str| -> Option<u32> {
            let at = agent.find(key)? + key.len();
            let digits: String = agent[at..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            digits.parse().ok()
        };
        if agent.contains("OPR/") || agent.contains("Firefox/") {
            return None;
        }
        let chromium = major("Chrome/")?;
        let (brand, version) = match major("Edg/") {
            Some(v) => ("Microsoft Edge", v),
            None => ("Google Chrome", chromium),
        };
        let name = format!("{profile:?}");
        let ours: u32 = name
            .strip_prefix("Chrome")
            .or_else(|| name.strip_prefix("Edge"))?
            .parse()
            .ok()?;
        if ours >= version {
            return None;
        }
        const CHARS: [&str; 11] = [" ", "(", ":", "-", ".", "/", ")", ";", "=", "?", "_"];
        const VERSIONS: [&str; 3] = ["8", "99", "24"];
        const ORDERS: [[usize; 3]; 6] = [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ];
        let seed = chromium as usize;
        let made_up = format!(
            "Not{}A{}Brand",
            CHARS[seed % CHARS.len()],
            CHARS[(seed + 1) % CHARS.len()]
        );
        let brands = [
            (made_up, VERSIONS[seed % VERSIONS.len()].to_owned()),
            ("Chromium".to_owned(), chromium.to_string()),
            (brand.to_owned(), version.to_string()),
        ];
        let mut placed: [&(String, String); 3] = [&brands[0]; 3];
        for (brand, at) in brands.iter().zip(ORDERS[seed % ORDERS.len()]) {
            placed[at] = brand;
        }
        Some(
            placed
                .iter()
                .map(|(b, v)| format!("\"{b}\";v=\"{v}\""))
                .collect::<Vec<_>>()
                .join(", "),
        )
    }

    /// The profile and system `emulation_for` takes for `agent`.
    pub(super) fn choose(agent: &str) -> (Profile, Platform) {
        // The dotted version after `key` ("Chrome/142.0.7444.59": [142, 0, 7444, 59]).
        let version_after = |key: &str| -> Option<Vec<u32>> {
            let at = agent.find(key)? + key.len();
            let dotted: String = agent[at..]
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            let parts: Vec<u32> = dotted.split('.').map_while(|d| d.parse().ok()).collect();
            (!parts.is_empty()).then_some(parts)
        };
        let (family, version) = if let Some(v) = version_after("Edg/") {
            ("Edge", Some(v))
        } else if let Some(v) = version_after("OPR/") {
            ("Opera", Some(v))
        } else if let Some(v) = version_after("Firefox/") {
            ("Firefox", Some(v))
        } else if let Some(v) = version_after("Chrome/") {
            ("Chrome", Some(v))
        } else if agent.contains("Safari/") {
            ("Safari", version_after("Version/"))
        } else {
            ("Chrome", None)
        };
        let platform = if agent.contains("Android") {
            Platform::Android
        } else if agent.contains("iPhone") || agent.contains("iPad") {
            Platform::IOS
        } else if agent.contains("Macintosh") || agent.contains("Mac OS X") {
            Platform::MacOS
        } else if agent.contains("Linux") || agent.contains("X11") || agent.contains("CrOS") {
            Platform::Linux
        } else {
            Platform::Windows
        };
        (profile(family, version), platform)
    }

    /// `family`'s profile closest to `version` from below (`emulation_for`).
    fn profile(family: &str, version: Option<Vec<u32>>) -> Profile {
        // Each profile's family and version, from its name ("Chrome142": [142], "Safari26_4":
        // [26, 4]): the plain desktop ones only (not FirefoxPrivate, SafariIos…).
        let mut ours: Vec<(Vec<u32>, Profile)> = Profile::VARIANTS
            .iter()
            .filter_map(|p| {
                let name = format!("{p:?}");
                let rest = name.strip_prefix(family)?;
                let parts: Option<Vec<u32>> = rest.split('_').map(|d| d.parse().ok()).collect();
                parts.filter(|v| !v.is_empty()).map(|v| (v, *p))
            })
            .collect();
        ours.sort_by(|a, b| a.0.cmp(&b.0));
        let pick = match version {
            Some(v) => ours
                .iter()
                .rev()
                .find(|(m, _)| *m <= v)
                .or(ours.first())
                .map(|(_, p)| *p),
            None => ours.last().map(|(_, p)| *p),
        };
        pick.unwrap_or_default()
    }

    /// MDM's resolver (`dns::Resolver`: the chosen DNS, only addresses the adapter reaches)
    /// for the browser-like client.
    struct Dns(crate::dns::Resolver);

    impl wreq::dns::Resolve for Dns {
        fn resolve(&self, name: wreq::dns::Name) -> wreq::dns::Resolving {
            match name.as_str().parse::<reqwest::dns::Name>() {
                Ok(n) => reqwest::dns::Resolve::resolve(&self.0, n),
                Err(e) => {
                    Box::pin(
                        async move { Err(Box::new(e) as Box<dyn std::error::Error + Send + Sync>) },
                    )
                }
            }
        }
    }

    pub(super) async fn get(
        b: &Browser,
        url: Url,
        headers: HeaderMap,
        range: bool,
        cancel: &CancellationToken,
    ) -> Result<(Head, Stream, HeaderMap), Error> {
        let (mut url, mut headers) = (url, headers);
        // A connection of its own while its answer comes in, its redirects asked on it too.
        let lease = Lease::take(&b.in_use);
        let group = wreq::Group::new(lease.n as u64);
        for _ in 0..=MAX_REDIRECTS {
            net::check_url(&b.route, &url)?;
            if b.stay_here && !net::is_this_computer(url.host_str().unwrap_or_default()) {
                return Err(NetError::Unbound {
                    label: b.route.label().unwrap_or_default().to_owned(),
                }
                .into());
            }
            // The download's User-Agent in the headers' place, never beside one they carry:
            // wreq's `header` adds a second (#1494).
            let mut sent = headers.clone();
            sent.insert(header::USER_AGENT, b.agent.clone());
            if range {
                // As browsers ask for a part of a file: its bytes as they are.
                sent.insert(
                    header::ACCEPT_ENCODING,
                    HeaderValue::from_static("identity"),
                );
            }
            let req = b
                .client
                .get(url.as_str())
                .group(group.clone())
                .headers(sent);
            let resp = cancellable(cancel, async {
                req.send().await.map_err(|e| net_error(&e))
            })
            .await?;
            if resp.status().is_redirection()
                && let Some(next) = resp
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|l| l.to_str().ok())
                    .and_then(|l| url.join(l).ok())
                    .filter(|n| matches!(n.scheme(), "http" | "https"))
            {
                let cookies = set_cookies(resp.headers());
                headers = scoped_headers(&headers, &url, &next);
                // Its own site's, never from https down to http (as the user's own cookies).
                let same_site =
                    next.host_str() == url.host_str() || crate::sites::same_party(&url, &next);
                if same_site && !(url.scheme() == "https" && next.scheme() != "https") {
                    add_cookies(&mut headers, &cookies);
                }
                url = next;
                continue;
            }
            let head = Head {
                status: resp.status(),
                headers: resp.headers().clone(),
                url: url.clone(),
                len: resp.content_length(),
                from: resp.remote_addr(),
            };
            let body = Stream {
                len: head.len,
                inner: Box::pin(resp.bytes_stream()),
                _lease: lease,
            };
            return Ok((head, body, headers));
        }
        Err(Error::Net {
            kind: crate::download::NetKind::Connect,
            cause: "too many redirects".to_owned(),
        })
    }
}

#[cfg(all(test, feature = "browser-connection"))]
mod tests {
    use super::imp::choose;

    fn profile_of(agent: &str) -> String {
        format!("{:?}", choose(agent))
    }

    #[test]
    fn the_profile_follows_the_browser_named() {
        let chrome = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36";
        let edge = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36 Edg/140.0.0.0";
        let firefox = "Mozilla/5.0 (X11; Linux x86_64; rv:145.0) Gecko/20100101 Firefox/145.0";
        let safari = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.1 Safari/605.1.15";
        let newer = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/999.0.0.0 Safari/537.36";
        assert!(
            profile_of(chrome).contains("Chrome142"),
            "{}",
            profile_of(chrome)
        );
        assert!(profile_of(chrome).contains("Windows"));
        assert!(profile_of(edge).contains("Edge140"), "{}", profile_of(edge));
        assert!(
            profile_of(firefox).contains("Firefox145"),
            "{}",
            profile_of(firefox)
        );
        assert!(profile_of(firefox).contains("Linux"));
        assert!(
            profile_of(safari).contains("Safari26_1"),
            "{}",
            profile_of(safari)
        );
        assert!(profile_of(safari).contains("MacOS"));
        assert!(
            profile_of(newer).contains("Chrome149"),
            "{}",
            profile_of(newer)
        );
    }

    /// #1494: Chrome 158 (Brave, Helium and the other Chromium browsers name themselves Chrome)
    /// takes the newest Chrome profile there is, and its `sec-ch-ua` names 158, as its
    /// User-Agent does; a profile of the browser's own version keeps its own.
    #[test]
    fn a_newer_chrome_takes_the_newest_profile_and_names_its_version() {
        use super::imp::{client_hints, emulation_for};
        let hints = |agent: &str| {
            emulation_for(agent)
                .headers
                .get("sec-ch-ua")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
                .unwrap_or_default()
        };
        let chrome = |v: u32| {
            format!(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{v}.0.0.0 Safari/537.36"
            )
        };
        let edge = |v: u32| format!("{} Edg/{v}.0.0.0", chrome(v));
        let newest = wreq_util::Profile::VARIANTS
            .iter()
            .filter_map(|p| format!("{p:?}").strip_prefix("Chrome")?.parse::<u32>().ok())
            .max()
            .unwrap();
        assert!(
            profile_of(&chrome(158)).contains(&format!("Chrome{newest}")),
            "{}",
            profile_of(&chrome(158))
        );
        // As Chrome 157 and Edge 157 send it on this computer (headless, their own profile).
        assert_eq!(
            hints(&chrome(157)),
            r#""Not-A.Brand";v="99", "Google Chrome";v="157", "Chromium";v="157""#
        );
        assert_eq!(
            hints(&edge(157)),
            r#""Not-A.Brand";v="99", "Microsoft Edge";v="157", "Chromium";v="157""#
        );
        assert_eq!(
            hints(&chrome(158)),
            r#""Chromium";v="158", "Not.A/Brand";v="24", "Google Chrome";v="158""#
        );
        // The profile's own, for its own version: the same words.
        let own = emulation_for(&chrome(newest));
        assert!(client_hints(&chrome(newest), super::imp::choose(&chrome(newest)).0).is_none());
        assert_eq!(
            hints(&chrome(newest)),
            own.headers.get("sec-ch-ua").unwrap().to_str().unwrap()
        );
        assert!(
            hints(&chrome(newest)).contains(&format!("\"Google Chrome\";v=\"{newest}\"")),
            "{}",
            hints(&chrome(newest))
        );
        // Chrome's highest priority in its HEADERS frame, and its Accept-Encoding (before
        // Accept-Language, as Chrome sends them).
        let e = emulation_for(&chrome(158));
        let dep = e
            .http2_options
            .as_ref()
            .and_then(|h| h.headers_stream_dependency.as_ref())
            .map(|d| format!("{d:?}"))
            .unwrap_or_default();
        assert!(dep.contains("255"), "{dep}");
        let names: Vec<&str> = e.headers.keys().map(|k| k.as_str()).collect();
        let at = |n: &str| names.iter().position(|k| *k == n);
        assert_eq!(
            e.headers
                .get("accept-encoding")
                .and_then(|v| v.to_str().ok()),
            Some("gzip, deflate, br, zstd"),
            "{names:?}"
        );
        assert!(at("accept-encoding") < at("accept-language"), "{names:?}");
        // Firefox and Safari send none; Opera's stays its profile's.
        let firefox =
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:152.0) Gecko/20100101 Firefox/152.0";
        assert!(emulation_for(firefox).headers.get("sec-ch-ua").is_none());
        let opera = format!("{} OPR/140.0.0.0", chrome(157));
        assert!(client_hints(&opera, super::imp::choose(&opera).0).is_none());
    }
}
