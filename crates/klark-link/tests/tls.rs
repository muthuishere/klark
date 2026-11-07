//! https and wss against a local rustls server whose self-signed certificate
//! (tests/fixtures/localhost-*.pem) is trusted through KLARK_HUB_CA_FILE.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn trust_fixture() {
    // Every test in this binary sets the same value.
    std::env::set_var(klark_link::tls::CA_FILE_ENV, fixture("localhost-cert.pem"));
}

fn server_config() -> Arc<ServerConfig> {
    let certs = CertificateDer::pem_file_iter(fixture("localhost-cert.pem"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let key = PrivateKeyDer::from_pem_file(fixture("localhost-key.pem")).unwrap();
    Arc::new(
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .unwrap(),
    )
}

/// An https server answering each request with the next JSON body; returns
/// the port and the request lines it saw.
fn https_server(answers: Vec<&'static str>) -> (u16, std::thread::JoinHandle<Vec<String>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let cfg = server_config();
    let t = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for body in answers {
            let (tcp, _) = l.accept().unwrap();
            let conn = ServerConnection::new(cfg.clone()).unwrap();
            let mut s = StreamOwned::new(conn, tcp);
            let mut r = BufReader::new(&mut s);
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            seen.push(line.trim().to_string());
            let mut len = 0;
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                if h == "\r\n" {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
            }
            let mut b = vec![0u8; len];
            r.read_exact(&mut b).unwrap();
            write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            s.conn.send_close_notify();
            let _ = s.flush();
        }
        seen
    });
    (port, t)
}

#[test]
fn https_device_flow_start_and_poll() {
    trust_fixture();
    let (port, t) = https_server(vec![
        r#"{"code":"ABCD-EFGH","poll_id":"p1","url":"https://localhost/x","expires_in":60,"interval":0}"#,
        r#"{"status":"approved","token":"tok","machine":"box"}"#,
    ]);
    let base = format!("https://localhost:{port}");
    let t0 = Duration::from_secs(5);
    let start = klark_link::http::request(
        "POST",
        &format!("{base}/api/connect/start"),
        None,
        Some(&serde_json::json!({"name": "box"})),
        t0,
    )
    .unwrap();
    assert!(start.ok(), "{start:?}");
    assert!(start.body.contains("ABCD-EFGH"));
    let poll = klark_link::http::request(
        "POST",
        &format!("{base}/api/connect/poll"),
        None,
        Some(&serde_json::json!({"poll_id": "p1"})),
        t0,
    )
    .unwrap();
    assert!(poll.body.contains("approved"));
    let seen = t.join().unwrap();
    assert_eq!(seen[0], "POST /api/connect/start HTTP/1.1");
    assert_eq!(seen[1], "POST /api/connect/poll HTTP/1.1");
}

#[test]
fn https_refuses_a_hub_cert_without_the_ca_file() {
    // nextest runs each test in its own process, so this env is ours.
    std::env::remove_var(klark_link::tls::CA_FILE_ENV);
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let cfg = server_config();
    std::thread::spawn(move || {
        let (tcp, _) = l.accept().unwrap();
        let mut s = StreamOwned::new(ServerConnection::new(cfg).unwrap(), tcp);
        let _ = s.read(&mut [0u8; 16]);
    });
    let e = klark_link::http::request(
        "GET",
        &format!("https://localhost:{port}/healthz"),
        None,
        None,
        Duration::from_secs(5),
    )
    .unwrap_err();
    assert!(e.to_lowercase().contains("certificate"), "{e}");
}

#[test]
fn wss_handshake_carries_the_token() {
    trust_fixture();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let cfg = server_config();
    let t = std::thread::spawn(move || {
        let (tcp, _) = l.accept().unwrap();
        let s = StreamOwned::new(ServerConnection::new(cfg).unwrap(), tcp);
        let mut auth = String::new();
        let mut ws = tungstenite::accept_hdr(
            s,
            |req: &tungstenite::handshake::server::Request,
             resp: tungstenite::handshake::server::Response| {
                auth = req
                    .headers()
                    .get("Authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                Ok(resp)
            },
        )
        .unwrap();
        ws.send(tungstenite::Message::text("hi")).unwrap();
        auth
    });
    let mut ws =
        klark_link::connect_hub(&format!("wss://localhost:{port}/agent"), "tok", "box").unwrap();
    let msg = loop {
        match ws.read() {
            Ok(m) => break m,
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue
            }
            Err(e) => panic!("{e}"),
        }
    };
    assert_eq!(msg.into_text().unwrap(), "hi");
    assert_eq!(t.join().unwrap(), "Bearer tok");
}
