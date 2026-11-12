use std::io::Write;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use clap::Parser;
use mdm_labserver::{Config, Faults, LabServer};

/// HTTP Range test server with a request log and fault injection.
#[derive(Parser)]
struct Args {
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:8090")]
    bind: SocketAddr,
    /// Default faults, query-string syntax, e.g. "rate=1M&max_conns=2".
    /// Per-URL query parameters override these.
    #[arg(long, default_value = "")]
    faults: String,
    /// Also append every log entry as a JSON line to this file.
    #[arg(long)]
    log_file: Option<std::path::PathBuf>,
    /// Serve the real files of this folder at /files/NAME (archives for the link grabber's tests).
    #[arg(long)]
    files: Option<std::path::PathBuf>,
    /// Also serve the same generated files over FTP on this address (ftp://ADDR/gen/8M/a.bin).
    #[arg(long)]
    ftp: Option<SocketAddr>,
    /// The FTP server's TLS: "explicit" (ftpes://, AUTH TLS) or "implicit" (ftps://). Its
    /// certificate comes from a test CA made at start, so clients must be told to trust it.
    #[arg(long, requires = "ftp")]
    ftp_tls: Option<String>,
    /// The login FTP sessions must give, "user:password" (else any).
    #[arg(long, requires = "ftp")]
    ftp_login: Option<String>,
    /// Bytes per second per FTP transfer, e.g. 2M.
    #[arg(long, requires = "ftp")]
    ftp_rate: Option<String>,
    /// A folder of .torrent files for /rss/dir.xml (the RSS tests' downloads).
    #[arg(long)]
    rss_dir: Option<std::path::PathBuf>,
    /// Serve HTTPS (as localhost, from a test CA made at start, written next to the log or
    /// printed) instead of HTTP: `browser_only` then looks at the TLS handshake too.
    #[arg(long)]
    tls: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let faults = Faults::parse(&args.faults).context("--faults")?;
    let file = match &args.log_file {
        Some(p) => Some(Mutex::new(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)?,
        )),
        None => None,
    };
    let on_log = Arc::new(move |e: &mdm_labserver::LogEntry| {
        let line = serde_json::to_string(e).expect("log entry serializes");
        println!("{line}");
        if let Some(f) = &file {
            let _ = writeln!(f.lock().unwrap(), "{line}");
        }
    });
    let mut server = LabServer::start(Config {
        bind: args.bind,
        faults,
        on_log: Some(on_log),
        files: args.files.clone(),
        rss_dir: args.rss_dir,
        tls: args.tls,
    })
    .await?;
    eprintln!(
        "labserver listening on {}  (GET / for help)",
        server.url("")
    );
    if let Some(ca) = server.ca_der() {
        let path =
            std::env::temp_dir().join(format!("mdm-labserver-ca-{}.der", server.addr().port()));
        std::fs::write(&path, ca)?;
        eprintln!("its test CA's certificate (DER): {}", path.display());
    }
    let _ftp = match args.ftp {
        Some(bind) => {
            use mdm_labserver::ftp::{FtpConfig, FtpServer, FtpTls};
            let tls = match args.ftp_tls.as_deref() {
                None => FtpTls::None,
                Some("explicit") => FtpTls::Explicit,
                Some("implicit") => FtpTls::Implicit,
                Some(other) => anyhow::bail!("--ftp-tls: {other:?} (explicit or implicit)"),
            };
            let login = match args.ftp_login.as_deref() {
                Some(l) => Some(
                    l.split_once(':')
                        .map(|(u, p)| (u.to_owned(), p.to_owned()))
                        .context("--ftp-login: user:password")?,
                ),
                None => None,
            };
            let rate = match args.ftp_rate.as_deref() {
                Some(r) => Some(mdm_labserver::content::parse_size(r).context("--ftp-rate")?),
                None => None,
            };
            let ftp = FtpServer::start(
                bind,
                FtpConfig {
                    login,
                    tls,
                    rate,
                    ..FtpConfig::default()
                },
            )
            .await?;
            eprintln!("labserver FTP on {}", ftp.url("/gen/8M/a.bin"));
            Some(ftp)
        }
        None => None,
    };
    server.wait().await;
    Ok(())
}
