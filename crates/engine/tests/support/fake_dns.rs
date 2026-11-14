//! A small DNS server on 127.0.0.1, plain over TCP (MDM asks plain DNS over TCP only), for the
//! tests that choose it in Options → Connection → DNS (#36): the names they look up are answered
//! here, never by the system's DNS or the network. It answers `*.mdm.test` with 127.0.0.1 and
//! every other name with NXDOMAIN ("no such name"), at once; or, silent, never answers.
//!
//! Shared by the test files that use it (`#[path]`), each with only some of it.
#![allow(dead_code)]

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The answer to DNS query `q`: an A record 127.0.0.1 for names ending in `.mdm.test` (others:
/// NXDOMAIN), and the name asked; None for a query it can't read.
fn answer(q: &[u8]) -> Option<(Vec<u8>, String)> {
    if q.len() < 12 {
        return None;
    }
    // The question: labels up to the zero byte, then type and class.
    let mut i = 12;
    let mut labels = Vec::new();
    while i < q.len() && q[i] != 0 {
        let l = q[i] as usize;
        labels.push(String::from_utf8_lossy(q.get(i + 1..i + 1 + l)?).to_lowercase());
        i += 1 + l;
    }
    let qend = i + 5;
    if qend > q.len() {
        return None;
    }
    let qtype = u16::from_be_bytes([q[i + 1], q[i + 2]]);
    let name = labels.join(".");
    let ours = name.ends_with(".mdm.test");
    let mut r = Vec::with_capacity(64);
    r.extend_from_slice(&q[..2]); // the id
    // A response, recursion available; NXDOMAIN when the name isn't ours.
    r.extend_from_slice(if ours { &[0x81, 0x80] } else { &[0x81, 0x83] });
    let answers: u16 = (ours && qtype == 1).into();
    r.extend_from_slice(&[0, 1]);
    r.extend_from_slice(&answers.to_be_bytes());
    r.extend_from_slice(&[0, 0, 0, 0]);
    r.extend_from_slice(&q[12..qend]);
    if answers == 1 {
        // The name (a pointer to the question's), A, IN, 60 s, 127.0.0.1.
        r.extend_from_slice(&[0xC0, 0x0C, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 127, 0, 0, 1]);
    }
    Some((r, name))
}

/// The fake DNS server: its address, each name it was asked, and the address each connection
/// came from.
pub struct FakeDns {
    pub addr: SocketAddr,
    pub asked: Arc<Mutex<Vec<String>>>,
    pub peers: Arc<Mutex<Vec<IpAddr>>>,
}

/// A DNS server answering as [`answer`].
pub async fn fake_dns() -> FakeDns {
    serve(true).await
}

/// A DNS server that takes every query and never answers (a DNS the network swallows).
pub async fn silent_dns() -> FakeDns {
    serve(false).await
}

async fn serve(answers: bool) -> FakeDns {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let log = asked.clone();
    let peers = Arc::new(Mutex::new(Vec::new()));
    let peers2 = peers.clone();
    tokio::spawn(async move {
        while let Ok((mut conn, peer)) = listener.accept().await {
            peers2.lock().unwrap().push(peer.ip());
            let log = log.clone();
            tokio::spawn(async move {
                // Each message has a 2-byte length in front (RFC 1035 4.2.2).
                loop {
                    let mut len = [0u8; 2];
                    if conn.read_exact(&mut len).await.is_err() {
                        return;
                    }
                    let mut q = vec![0u8; u16::from_be_bytes(len) as usize];
                    if conn.read_exact(&mut q).await.is_err() {
                        return;
                    }
                    let Some((r, name)) = answer(&q) else { return };
                    log.lock().unwrap().push(name);
                    if !answers {
                        continue;
                    }
                    let mut out = (r.len() as u16).to_be_bytes().to_vec();
                    out.extend_from_slice(&r);
                    if conn.write_all(&out).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    FakeDns { addr, asked, peers }
}
