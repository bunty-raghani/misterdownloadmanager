//! The certificates secure connections (HTTPS, FTPS, DNS over TLS or HTTPS) check servers with:
//! the system's, and Mozilla's ([`mozilla_roots`], the ones Firefox carries) where the system
//! lacks a server's root (#1546). A system without any (a minimal Linux without the
//! `ca-certificates` package, #264) still gives every client: plain HTTP and FTP work, and each
//! secure connection fails with [`NO_ROOTS`], which the engine reports as
//! `Error::NoCertificates`. The torrent session's HTTP clients take the same setup
//! (mdm-torrent), so a session still starts there: DHT, peers and plain HTTP trackers work,
//! HTTPS trackers fail with [`NO_ROOTS`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};

/// Why a secure connection failed on a system without certificates: the error its handshake
/// gives, in English (the command line and the event log); the interfaces word it themselves.
pub const NO_ROOTS: &str = "this system has no trusted certificates to check the server with \
                            (install the ca-certificates package)";

/// The crypto provider MDM's own TLS configurations use (the one reqwest uses too).
pub(crate) fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// Whether the system has certificates to check servers with. Windows and macOS always do
/// (their verifiers ask the system at each connection); Linux reads its CA bundle, which can be
/// missing. Asked again until it's found, so installing the certificates while MDM runs is seen
/// by the next download.
pub fn system_has_roots() -> bool {
    static FOUND: AtomicBool = AtomicBool::new(false);
    if FOUND.load(Ordering::Relaxed) {
        return true;
    }
    let found = rustls_platform_verifier::Verifier::new(provider()).is_ok();
    if found {
        FOUND.store(true, Ordering::Relaxed);
    }
    found
}

/// Mozilla's trusted root certificates (CCADB's list for websites, as Firefox carries it:
/// webpki-root-certs), which MDM's verifier trusts besides the system's (#1546). A Windows that
/// hasn't fetched a root yet (Windows downloads the roots it trusts from Windows Update the first
/// time a site needs one, and behind a proxy that often fails: Let's Encrypt's ISRG Root X1 and
/// X2 on an old Windows 10) refused such sites with "unknown issuer" while Firefox opened them.
///
/// How the system's verifier (rustls-platform-verifier) takes them: on Windows they are asked
/// only when Windows' own chain ends in a root it doesn't have or doesn't trust, and that chain
/// is then built and checked by Windows again with them (its policy, the same revocation check);
/// on macOS they are anchors beside the system's, in the same trust evaluation; on Linux they join
/// the system's CA bundle (usually Mozilla's list too) in one store. So the system's own roots
/// stay trusted (an antivirus's or a company's HTTPS inspection keeps working), and a root Mozilla
/// has is trusted even where the system's store lacks it or was told not to trust it.
///
/// A root Mozilla trusts only for some names (its name constraints: webpki-roots carries them,
/// the plain certificates here don't) is left out, so it is never trusted for more than Mozilla
/// trusts it ([`name_constrained`]).
pub fn mozilla_roots() -> impl Iterator<Item = CertificateDer<'static>> {
    webpki_root_certs::TLS_SERVER_ROOT_CERTS
        .iter()
        .filter(|c| !name_constrained(c))
        .cloned()
}

/// Whether Mozilla's list limits the root certificate `cert` to some names: a webpki-roots
/// anchor with name constraints whose subject and public key are both `cert`'s.
fn name_constrained(cert: &[u8]) -> bool {
    let has = |part: &[u8]| !part.is_empty() && cert.windows(part.len()).any(|w| w == part);
    webpki_roots::TLS_SERVER_ROOTS.iter().any(|a| {
        a.name_constraints.is_some()
            && has(a.subject.as_ref())
            && has(a.subject_public_key_info.as_ref())
    })
}

/// [`mozilla_roots`] as reqwest takes them, made once.
fn mozilla_certificates() -> Vec<reqwest::Certificate> {
    static CERTS: std::sync::OnceLock<Vec<reqwest::Certificate>> = std::sync::OnceLock::new();
    CERTS
        .get_or_init(|| {
            mozilla_roots()
                .filter_map(|c| reqwest::Certificate::from_der(&c).ok())
                .collect()
        })
        .clone()
}

/// `builder` with the TLS it can have here: reqwest's own (the system's verifier, with
/// [`mozilla_roots`] besides) when the system has certificates, else one whose every secure
/// connection fails with [`NO_ROOTS`] (reqwest's would refuse to build the client at all, plain
/// HTTP included). MDM's own clients get it through `net::bind_client`; the torrent session's
/// (trackers, UPnP) through librqbit's `SessionOptions::configure_http_client` (mdm-torrent).
pub fn configure(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    configure_with(builder, system_has_roots(), mozilla_certificates())
}

/// `fallback`: the roots trusted besides the system's (Mozilla's; the tests give a test CA in
/// their place, a root the system doesn't have).
fn configure_with(
    builder: reqwest::ClientBuilder,
    has_roots: bool,
    fallback: Vec<reqwest::Certificate>,
) -> reqwest::ClientBuilder {
    if has_roots {
        builder.tls_certs_merge(fallback)
    } else {
        builder.tls_backend_preconfigured(no_roots_config())
    }
}

/// A client configuration for a system without certificates: every server is refused.
pub(crate) fn no_roots_config() -> rustls::ClientConfig {
    let provider = provider();
    let verifier = Arc::new(NoRoots(provider.clone()));
    rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("the default protocol versions are supported")
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth()
}

/// Whether `e` (or one of its causes) is a secure connection refused for want of certificates.
pub fn is_no_roots(e: &(dyn std::error::Error + 'static)) -> bool {
    let mut next = Some(e);
    while let Some(e) = next {
        if e.to_string().contains(NO_ROOTS) {
            return true;
        }
        next = e.source();
    }
    false
}

/// Why a server's certificate was refused (`Error::Certificate`): what a browser warns about
/// too. MDM has no way around it: the user is told what is wrong with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CertProblem {
    /// Its validity ended.
    Expired,
    /// Its validity hasn't begun (or this computer's clock is wrong).
    NotYetValid,
    /// No certificate the system trusts vouches for it: signed by itself, or by an issuer the
    /// system doesn't know.
    Untrusted,
    /// It is for other names than the server's.
    WrongName,
    /// Its issuer took it back.
    Revoked,
    /// It isn't made for a website (its extended key usage leaves out TLS servers).
    NotForWebsites,
    /// Refused for another reason (a bad signature, an encoding).
    Other,
}

impl std::fmt::Display for CertProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            CertProblem::Expired => "the server's certificate has expired",
            CertProblem::NotYetValid => "the server's certificate is not valid yet",
            CertProblem::Untrusted => {
                "the server's certificate is not trusted (self-signed or from an unknown issuer)"
            }
            CertProblem::WrongName => "the server's certificate is for another name",
            CertProblem::Revoked => "the server's certificate was revoked",
            CertProblem::NotForWebsites => "the server's certificate isn't made for websites",
            CertProblem::Other => "the server's certificate was refused",
        })
    }
}

/// Whether `e` (or one of its causes) is a server's certificate refused by the verifier, and
/// why. An `io::Error` hides the error it wraps from `source()` (it gives that error's own
/// source): it is looked into as well.
pub fn certificate_problem(e: &(dyn std::error::Error + 'static)) -> Option<CertProblem> {
    use rustls::CertificateError as C;
    if let Some(rustls::Error::InvalidCertificate(c)) = e.downcast_ref::<rustls::Error>() {
        return Some(match c {
            C::Expired | C::ExpiredContext { .. } => CertProblem::Expired,
            C::NotValidYet | C::NotValidYetContext { .. } => CertProblem::NotYetValid,
            C::UnknownIssuer => CertProblem::Untrusted,
            C::NotValidForName | C::NotValidForNameContext { .. } => CertProblem::WrongName,
            C::Revoked => CertProblem::Revoked,
            C::InvalidPurpose | C::InvalidPurposeContext { .. } => CertProblem::NotForWebsites,
            C::Other(other) => of_platform(&other.to_string()),
            _ => CertProblem::Other,
        });
    }
    // What an io::Error wraps (hyper's connector wraps one io::Error in another).
    let wrapped = e
        .downcast_ref::<std::io::Error>()
        .and_then(|io| io.get_ref())
        .and_then(|inner| certificate_problem(inner));
    wrapped.or_else(|| e.source().and_then(certificate_problem))
}

/// The platform verifier's own words for what rustls has no variant for (`CertificateError::Other`).
/// On macOS (and iOS) it names only a few of the system's reasons and gives the others as
/// "<the system's description>: <its OSStatus code>", an expired certificate among them. On
/// macOS and Android a certificate not for TLS servers is its `EkuError`, by its words.
fn of_platform(said: &str) -> CertProblem {
    if said == "certificate had invalid extensions" {
        return CertProblem::NotForWebsites;
    }
    let code = said
        .rsplit_once(": ")
        .and_then(|(_, code)| code.trim().parse::<i64>().ok());
    match code {
        // errSecCertificateExpired, CSSMERR_TP_CERT_EXPIRED
        Some(-67818 | -2147409654) => CertProblem::Expired,
        // errSecCertificateNotValidYet, CSSMERR_TP_CERT_NOT_VALID_YET
        Some(-67819 | -2147409653) => CertProblem::NotYetValid,
        // errSecNotTrusted, errSecCreateChainFailed
        Some(-67843 | -25318) => CertProblem::Untrusted,
        // errSecHostNameMismatch
        Some(-67602) => CertProblem::WrongName,
        // errSecInvalidExtendedKeyUsage
        Some(-67609) => CertProblem::NotForWebsites,
        // errSecCertificateRevoked
        Some(-67820) => CertProblem::Revoked,
        _ => CertProblem::Other,
    }
}

/// The verifier of a system without certificates: no server can be checked, so none is trusted.
#[derive(Debug)]
struct NoRoots(Arc<CryptoProvider>);

fn refused() -> rustls::Error {
    rustls::Error::General(NO_ROOTS.to_owned())
}

impl ServerCertVerifier for NoRoots {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Err(refused())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(refused())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(refused())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdm_labserver::{Config, LabServer, ftp};

    /// A client of a system without certificates.
    fn without_roots() -> reqwest::Client {
        configure_with(reqwest::Client::builder(), false, Vec::new())
            .build()
            .expect("a client builds without certificates")
    }

    #[tokio::test]
    async fn without_certificates_plain_http_still_downloads() {
        let lab = LabServer::start(Config::default()).await.unwrap();
        let body = without_roots()
            .get(lab.url("/gen/16K?seed=3"))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(body.len(), 16 * 1024);
    }

    #[tokio::test]
    async fn without_certificates_https_says_why() {
        // An implicit-FTPS server opens with a TLS handshake, as an HTTPS server does: the
        // client sees its certificate and refuses it before any HTTP.
        let server = ftp::FtpServer::start(
            "127.0.0.1:0".parse().unwrap(),
            ftp::FtpConfig {
                tls: ftp::FtpTls::Implicit,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let e = without_roots()
            .get(format!("https://{}/", server.addr()))
            .send()
            .await
            .unwrap_err();
        assert!(is_no_roots(&e), "{e:?}");
        let e = crate::Error::from(e);
        assert!(matches!(e, crate::Error::NoCertificates), "{e:?}");
        assert_eq!(e.to_string(), NO_ROOTS);
    }

    /// A client of this system (its verifier, as MDM's downloads have it), trusting the lab's
    /// test CA besides.
    fn trusting(ca: Vec<u8>) -> reqwest::Client {
        let config = crate::ftp::client_config(&[ca]).unwrap();
        reqwest::Client::builder()
            .tls_backend_preconfigured(config)
            .build()
            .unwrap()
    }

    /// A refused certificate says why (#762): expired, self-signed, for another name; the
    /// system's own verifier decides (Windows', macOS's, or the CA bundle's on Linux).
    #[tokio::test]
    async fn a_refused_certificate_says_why() {
        use mdm_labserver::Cert;
        for (cert, want) in [
            (Cert::Expired, CertProblem::Expired),
            (Cert::SelfSigned, CertProblem::Untrusted),
            (Cert::WrongName, CertProblem::WrongName),
        ] {
            let lab = LabServer::start(Config {
                tls: true,
                cert,
                ..Config::default()
            })
            .await
            .unwrap();
            let url = format!("https://localhost:{}/gen/16K", lab.addr().port());
            let e = trusting(lab.ca_der().unwrap())
                .get(&url)
                .send()
                .await
                .unwrap_err();
            assert_eq!(certificate_problem(&e), Some(want), "{cert:?}: {e:?}");
            let e = crate::Error::from(e);
            assert!(
                matches!(e, crate::Error::Certificate(p) if p == want),
                "{cert:?}: {e:?}"
            );
        }
        // The good one downloads.
        let lab = LabServer::start(Config {
            tls: true,
            ..Config::default()
        })
        .await
        .unwrap();
        let url = format!("https://localhost:{}/gen/16K", lab.addr().port());
        let body = trusting(lab.ca_der().unwrap())
            .get(&url)
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(body.len(), 16 * 1024);
    }

    /// A client as `configure` makes it, with `fallback` in Mozilla's roots' place.
    fn configured_with(fallback: Vec<Vec<u8>>) -> reqwest::Client {
        let fallback = fallback
            .iter()
            .map(|der| reqwest::Certificate::from_der(der).unwrap())
            .collect();
        configure_with(reqwest::Client::builder(), true, fallback)
            .build()
            .unwrap()
    }

    /// #1546: a server whose chain ends in a root the system doesn't have, but Mozilla's list
    /// does (the lab's test CA stands for it: it is in no system's store). Without the fallback
    /// the system refuses it as from an unknown issuer, as an old Windows 10 refused Let's
    /// Encrypt's; with it, the chain is accepted, and a bad certificate is still refused for
    /// what is wrong with it, by the system's own verifier (Windows' chain engine checks the
    /// chain again with the extra roots, macOS's trust evaluation, webpki on Linux).
    #[tokio::test]
    async fn a_root_only_mozilla_has_is_trusted_and_bad_certificates_still_refused() {
        use mdm_labserver::Cert;
        let good = LabServer::start(Config {
            tls: true,
            ..Config::default()
        })
        .await
        .unwrap();
        let url = format!("https://localhost:{}/gen/16K", good.addr().port());
        let ca = good.ca_der().unwrap();

        // The system alone: unknown issuer.
        let e = configured_with(Vec::new())
            .get(&url)
            .send()
            .await
            .unwrap_err();
        assert_eq!(
            certificate_problem(&e),
            Some(CertProblem::Untrusted),
            "{e:?}"
        );

        // With the fallback's roots: it downloads.
        let body = configured_with(vec![ca])
            .get(&url)
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(body.len(), 16 * 1024);

        // Still refused, each for its own reason, with the fallback trusting the lab's CA.
        for (cert, want) in [
            (Cert::Expired, CertProblem::Expired),
            (Cert::SelfSigned, CertProblem::Untrusted),
            (Cert::WrongName, CertProblem::WrongName),
        ] {
            let lab = LabServer::start(Config {
                tls: true,
                cert,
                ..Config::default()
            })
            .await
            .unwrap();
            let url = format!("https://localhost:{}/gen/16K", lab.addr().port());
            let e = configured_with(vec![lab.ca_der().unwrap()])
                .get(&url)
                .send()
                .await
                .unwrap_err();
            assert_eq!(certificate_problem(&e), Some(want), "{cert:?}: {e:?}");
        }
    }

    /// The fallback is Mozilla's whole list, every certificate of which this system's verifier
    /// takes: one it refused would leave every client unbuilt. It holds the roots of the chain
    /// #1546's server sends (Let's Encrypt's YE1, Root YE, cross-signed by ISRG Root X2, itself
    /// by ISRG Root X1).
    #[test]
    fn the_fallback_is_mozillas_list_and_the_system_takes_it() {
        let all = webpki_root_certs::TLS_SERVER_ROOT_CERTS.len();
        assert!(all > 100, "{all}");
        // The two crates are the same list (their versions go together).
        assert_eq!(webpki_roots::TLS_SERVER_ROOTS.len(), all);
        let constrained = webpki_roots::TLS_SERVER_ROOTS
            .iter()
            .filter(|a| a.name_constraints.is_some())
            .count();
        assert_eq!(mozilla_roots().count(), all - constrained);
        assert_eq!(mozilla_certificates().len(), all - constrained);
        for name in [&b"ISRG Root X1"[..], b"ISRG Root X2"] {
            assert!(
                mozilla_roots().any(|c| c.windows(name.len()).any(|w| w == name)),
                "{}",
                String::from_utf8_lossy(name)
            );
        }
        configure(reqwest::Client::builder())
            .build()
            .expect("the system's verifier takes Mozilla's roots");
        crate::ftp::client_config(&[]).expect("so does MDM's own TLS configuration");
    }

    /// A root Mozilla trusts only for some names is left out of the fallback: its plain
    /// certificate would be trusted for every name. Mozilla's list has one today, TUBITAK's,
    /// limited to `.tr`.
    #[test]
    fn name_constrained_roots_are_left_out() {
        let tubitak = &b"TUBITAK Kamu SM SSL Kok Sertifikasi - Surum 1"[..];
        let has = |c: &[u8], s: &[u8]| c.windows(s.len()).any(|w| w == s);
        let constrained: Vec<_> = webpki_roots::TLS_SERVER_ROOTS
            .iter()
            .filter(|a| a.name_constraints.is_some())
            .collect();
        assert!(!constrained.is_empty());
        assert!(constrained.iter().any(|a| has(a.subject.as_ref(), tubitak)));
        assert!(
            webpki_root_certs::TLS_SERVER_ROOT_CERTS
                .iter()
                .any(|c| has(c, tubitak)),
            "the plain list has it"
        );
        assert!(
            mozilla_roots().all(|c| !has(&c, tubitak)),
            "the fallback doesn't"
        );
        // Each constrained anchor is exactly one certificate of the plain list, left out.
        for a in &constrained {
            let matching = webpki_root_certs::TLS_SERVER_ROOT_CERTS
                .iter()
                .filter(|c| {
                    has(c, a.subject.as_ref()) && has(c, a.subject_public_key_info.as_ref())
                })
                .count();
            assert_eq!(matching, 1);
        }
        // An unconstrained root stays.
        assert!(mozilla_roots().any(|c| has(&c, b"ISRG Root X1")));
    }

    #[test]
    fn a_wrapped_certificate_error_is_found() {
        let e = std::io::Error::other(rustls::Error::InvalidCertificate(
            rustls::CertificateError::Expired,
        ));
        assert_eq!(certificate_problem(&e), Some(CertProblem::Expired));
        // As macOS's verifier gives what rustls has no variant for, and an EKU it refused.
        let other =
            |s: &str| {
                std::io::Error::other(rustls::Error::InvalidCertificate(
                    rustls::CertificateError::Other(rustls::OtherError(std::sync::Arc::from(
                        Box::<dyn std::error::Error + Send + Sync>::from(s.to_owned()),
                    ))),
                ))
            };
        for (said, want) in [
            (
                "“localhost” certificate is expired: -67818",
                CertProblem::Expired,
            ),
            (
                "“localhost” certificate is not yet valid: -67819",
                CertProblem::NotYetValid,
            ),
            (
                "“localhost” certificate is not trusted: -67843",
                CertProblem::Untrusted,
            ),
            (
                "certificate had invalid extensions",
                CertProblem::NotForWebsites,
            ),
            ("something new: -1", CertProblem::Other),
            ("no code at all", CertProblem::Other),
        ] {
            assert_eq!(certificate_problem(&other(said)), Some(want), "{said}");
        }
        let e = std::io::Error::other(rustls::Error::InvalidCertificate(
            rustls::CertificateError::InvalidPurpose,
        ));
        assert_eq!(certificate_problem(&e), Some(CertProblem::NotForWebsites));
        let e = std::io::Error::other(rustls::Error::General("handshake".into()));
        assert_eq!(certificate_problem(&e), None);
        assert_eq!(certificate_problem(&std::io::Error::other(refused())), None);
    }

    #[test]
    fn other_tls_failures_are_not_mistaken_for_it() {
        let e = std::io::Error::other(rustls::Error::InvalidCertificate(
            rustls::CertificateError::UnknownIssuer,
        ));
        assert!(!is_no_roots(&e));
        assert!(is_no_roots(&std::io::Error::other(refused())));
    }
}
