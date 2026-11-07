//! TLS for `https://` and `wss://` hubs (ADR 0005): rustls with the
//! webpki root store, plus the PEM certificates in `KLARK_HUB_CA_FILE` (a
//! private ingress's CA or a self-signed hub). No OpenSSL.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

pub const CA_FILE_ENV: &str = "KLARK_HUB_CA_FILE";

/// The client config: public roots + `KLARK_HUB_CA_FILE`.
pub fn client_config() -> Result<Arc<ClientConfig>, String> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(path) = std::env::var_os(CA_FILE_ENV).filter(|p| !p.is_empty()) {
        let shown = std::path::Path::new(&path).display().to_string();
        let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(&path)
            .map_err(|e| format!("{CA_FILE_ENV} {shown}: {e}"))?
            .collect::<Result<_, _>>()
            .map_err(|e| format!("{CA_FILE_ENV} {shown}: {e}"))?;
        if certs.is_empty() {
            return Err(format!("{CA_FILE_ENV} {shown}: no PEM certificates"));
        }
        for c in certs {
            roots
                .add(c)
                .map_err(|e| format!("{CA_FILE_ENV} {shown}: {e}"))?;
        }
    }
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
    Ok(Arc::new(config))
}

/// A plain or TLS connection to the hub.
pub enum Conn {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Conn {
    /// Wraps `tcp` in TLS for `host` when `tls` is set.
    pub fn new(tcp: TcpStream, host: &str, tls: bool) -> Result<Self, String> {
        if !tls {
            return Ok(Conn::Plain(tcp));
        }
        let name = ServerName::try_from(host.to_string()).map_err(|e| format!("{host}: {e}"))?;
        let conn = ClientConnection::new(client_config()?, name).map_err(|e| e.to_string())?;
        Ok(Conn::Tls(Box::new(StreamOwned::new(conn, tcp))))
    }
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(s) => s.read(buf),
            // A server that closes without close_notify (common behind
            // proxies; framing is Content-Length / Connection: close) is EOF.
            Conn::Tls(s) => match s.read(buf) {
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(0),
                r => r,
            },
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(s) => s.write(buf),
            Conn::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Conn::Plain(s) => s.flush(),
            Conn::Tls(s) => s.flush(),
        }
    }
}
