//! A minimal blocking HTTP/1.1 client for the hub's small JSON API
//! (`/api/token`, `/api/agents`, `/api/messages`, ADR 0009).
//!
//! `http://` and `https://` (rustls, see `tls`; ADR 0005).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};

use crate::tls::Conn;
use std::time::Duration;

/// The hub's HTTP base (`http://host:port`) from any hub URL a user or the link
/// holds: `ws://hub:8080/agent` -> `http://hub:8080`, `wss` -> `https`,
/// `http(s)://hub:8080/anything` -> its origin.
pub fn hub_http_base(hub_url: &str) -> Result<String, String> {
    let mut url = url::Url::parse(hub_url.trim()).map_err(|e| format!("hub url {hub_url}: {e}"))?;
    let scheme = match url.scheme() {
        "ws" | "http" => "http",
        "wss" | "https" => "https",
        other => {
            return Err(format!(
                "hub url scheme {other} is not ws, wss, http or https"
            ))
        }
    };
    url.set_scheme(scheme)
        .map_err(|_| "cannot set url scheme".to_string())?;
    url.set_path("");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.as_str().trim_end_matches('/').to_string())
}

#[derive(Debug, Clone, PartialEq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

impl HttpResponse {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// `{"error": "..."}` from the body if present, else the body or status.
    pub fn error_text(&self) -> String {
        serde_json::from_str::<serde_json::Value>(&self.body)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                let body = self.body.trim();
                if body.is_empty() {
                    format!("HTTP {}", self.status)
                } else {
                    format!("HTTP {}: {body}", self.status)
                }
            })
    }
}

/// One request: `method url` with an optional bearer token and JSON body.
pub fn request(
    method: &str,
    url: &str,
    bearer: Option<&str>,
    json_body: Option<&serde_json::Value>,
    timeout: Duration,
) -> Result<HttpResponse, String> {
    let (mut stream, hostport, path) = open(url, timeout)?;
    let body = json_body.map(|v| v.to_string()).unwrap_or_default();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {hostport}\r\nAccept: application/json\r\nConnection: close\r\nUser-Agent: klark-msg\r\n"
    );
    if let Some(token) = bearer {
        if token.contains(['\r', '\n']) {
            return Err("token contains invalid header characters".into());
        }
        req.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    if json_body.is_some() {
        req.push_str("Content-Type: application/json\r\n");
    }
    req.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    req.push_str(&body);
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("send to {hostport}: {e}"))?;

    let mut raw = Vec::new();
    BufReader::new(stream)
        .read_to_end(&mut raw)
        .map_err(|e| format!("read from {hostport}: {e}"))?;
    parse_response(&raw)
}

/// Open an http or https connection for `url`; returns (stream, host:port, path?query).
fn open(url: &str, timeout: Duration) -> Result<(Conn, String, String), String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("url {url}: {e}"))?;
    let tls = match parsed.scheme() {
        "http" => false,
        "https" => true,
        other => return Err(format!("{url}: unsupported scheme {other}")),
    };
    let host = parsed.host_str().ok_or_else(|| format!("{url}: no host"))?;
    let port = parsed.port_or_known_default().unwrap_or(80);
    // Every resolved address in turn (localhost is often ::1 first).
    let addrs: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("resolve {host}:{port}: {e}"))?
        .collect();
    let mut last = format!("resolve {host}:{port}: no address");
    let mut stream = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last = format!("connect {host}:{port}: {e}"),
        }
    }
    let stream = stream.ok_or(last)?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();
    let mut path = parsed.path().to_string();
    if let Some(q) = parsed.query() {
        path.push('?');
        path.push_str(q);
    }
    let host_only = host.trim_start_matches('[').trim_end_matches(']');
    let conn = Conn::new(stream, host_only, tls)?;
    Ok((conn, format!("{host}:{port}"), path))
}

fn auth_header(bearer: Option<&str>) -> Result<String, String> {
    match bearer {
        Some(t) if t.contains(['\r', '\n']) => {
            Err("token contains invalid header characters".into())
        }
        Some(t) => Ok(format!("Authorization: Bearer {t}\r\n")),
        None => Ok(String::new()),
    }
}

/// `PUT url` streaming the file at `path` as the body, with Content-Length.
/// The file is never held in memory.
pub fn put_file(
    url: &str,
    bearer: Option<&str>,
    path: &std::path::Path,
    content_type: &str,
    timeout: Duration,
) -> Result<HttpResponse, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let len = file
        .metadata()
        .map_err(|e| format!("{}: {e}", path.display()))?
        .len();
    let (mut stream, hostport, req_path) = open(url, timeout)?;
    let head = format!(
        "PUT {req_path} HTTP/1.1\r\nHost: {hostport}\r\nAccept: application/json\r\nConnection: close\r\nUser-Agent: klark-offer\r\n{}Content-Type: {content_type}\r\nContent-Length: {len}\r\n\r\n",
        auth_header(bearer)?
    );
    stream
        .write_all(head.as_bytes())
        .map_err(|e| format!("send to {hostport}: {e}"))?;
    let sent = std::io::copy(&mut (&mut file).take(len), &mut stream)
        .map_err(|e| format!("send to {hostport}: {e}"))?;
    if sent != len {
        return Err(format!("{}: file shrank while sending", path.display()));
    }
    let mut raw = Vec::new();
    BufReader::new(stream)
        .read_to_end(&mut raw)
        .map_err(|e| format!("read from {hostport}: {e}"))?;
    parse_response(&raw)
}

/// `GET url` streaming a 2xx body into `out`, refusing more than `max` bytes.
/// Returns the number of body bytes written; a non-2xx is an error carrying
/// the hub's error text.
pub fn get_to_writer(
    url: &str,
    bearer: Option<&str>,
    out: &mut dyn Write,
    max: u64,
    timeout: Duration,
) -> Result<u64, String> {
    let (mut stream, hostport, req_path) = open(url, timeout)?;
    let head = format!(
        "GET {req_path} HTTP/1.1\r\nHost: {hostport}\r\nConnection: close\r\nUser-Agent: klark-expose\r\n{}\r\n",
        auth_header(bearer)?
    );
    stream
        .write_all(head.as_bytes())
        .map_err(|e| format!("send to {hostport}: {e}"))?;
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .map_err(|e| format!("read from {hostport}: {e}"))?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("malformed status line: {}", status_line.trim()))?;
    let (mut chunked, mut length) = (false, None::<u64>);
    loop {
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| format!("read from {hostport}: {e}"))?;
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
            if k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked") {
                chunked = true;
            } else if k == "content-length" {
                length = v.parse().ok();
            }
        }
    }
    if !(200..300).contains(&status) {
        let mut body = Vec::new();
        let _ = reader.take(64 * 1024).read_to_end(&mut body);
        let raw = HttpResponse {
            status,
            body: String::from_utf8_lossy(&body).into_owned(),
        };
        return Err(raw.error_text());
    }
    if length.is_some_and(|n| n > max) {
        return Err(format!(
            "body of {} bytes exceeds {max}",
            length.unwrap_or(0)
        ));
    }
    let mut total = 0u64;
    let mut copy = |src: &mut dyn Read, n: Option<u64>, total: &mut u64| -> Result<(), String> {
        let limit = n.unwrap_or(max + 1);
        let got = std::io::copy(&mut src.take(limit.min(max + 1 - *total)), out)
            .map_err(|e| format!("read body: {e}"))?;
        *total += got;
        if *total > max {
            return Err(format!("body exceeds {max} bytes"));
        }
        if n.is_some_and(|n| got < n) {
            return Err("truncated body".into());
        }
        Ok(())
    };
    if chunked {
        loop {
            let mut size_line = String::new();
            reader
                .read_line(&mut size_line)
                .map_err(|e| format!("read body: {e}"))?;
            let size = u64::from_str_radix(size_line.trim().split(';').next().unwrap_or(""), 16)
                .map_err(|_| format!("bad chunk size: {}", size_line.trim()))?;
            if size == 0 {
                break;
            }
            copy(&mut reader, Some(size), &mut total)?;
            let mut crlf = [0u8; 2];
            let _ = reader.read_exact(&mut crlf);
        }
    } else {
        copy(&mut reader, length, &mut total)?;
    }
    Ok(total)
}

/// A response head: status and how its body is framed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Head {
    pub status: u16,
    pub chunked: bool,
    pub length: Option<u64>,
    /// The server closes after this response (`Connection: close`).
    pub close: bool,
}

/// Read a status line and headers.
pub fn read_head(r: &mut impl BufRead) -> Result<Head, String> {
    let mut status_line = String::new();
    if r.read_line(&mut status_line)
        .map_err(|e| format!("read: {e}"))?
        == 0
    {
        return Err("connection closed".into());
    }
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("malformed status line: {}", status_line.trim()))?;
    let mut head = Head {
        status,
        chunked: false,
        length: None,
        close: false,
    };
    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line).map_err(|e| format!("read: {e}"))?;
        if n == 0 || line == "\r\n" || line == "\n" {
            if matches!(head.status, 204 | 304) || (100..200).contains(&head.status) {
                head.length = Some(0);
            }
            return Ok(head);
        }
        if let Some((k, v)) = line.split_once(':') {
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_ascii_lowercase());
            match k.as_str() {
                "transfer-encoding" if v.contains("chunked") => head.chunked = true,
                "content-length" => head.length = v.parse().ok(),
                "connection" if v.contains("close") => head.close = true,
                _ => {}
            }
        }
    }
}

/// A chunked body as a plain byte stream, read as it arrives (for held
/// responses such as the hub's event stream).
pub struct Chunked<R: BufRead> {
    inner: R,
    left: u64,
    done: bool,
}

impl<R: BufRead> Chunked<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            left: 0,
            done: false,
        }
    }
}

impl<R: BufRead> Read for Chunked<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::io::{Error, ErrorKind};
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        if self.left == 0 {
            let mut line = String::new();
            // The CRLF that ends the previous chunk, then the size line.
            while line.trim().is_empty() {
                line.clear();
                if self.inner.read_line(&mut line)? == 0 {
                    return Err(Error::new(ErrorKind::UnexpectedEof, "stream ended"));
                }
            }
            self.left = u64::from_str_radix(line.trim().split(';').next().unwrap_or(""), 16)
                .map_err(|_| Error::new(ErrorKind::InvalidData, "bad chunk size"))?;
            if self.left == 0 {
                self.done = true;
                return Ok(0);
            }
        }
        let want = buf.len().min(self.left as usize);
        let n = self.inner.read(&mut buf[..want])?;
        if n == 0 {
            return Err(Error::new(ErrorKind::UnexpectedEof, "stream ended"));
        }
        self.left -= n as u64;
        Ok(n)
    }
}

fn read_body(r: &mut impl BufRead, head: &Head) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    if head.chunked {
        Chunked::new(&mut *r)
            .read_to_end(&mut body)
            .map_err(|e| format!("read body: {e}"))?;
    } else if let Some(n) = head.length {
        body.resize(n as usize, 0);
        r.read_exact(&mut body)
            .map_err(|e| format!("read body: {e}"))?;
    } else {
        r.read_to_end(&mut body)
            .map_err(|e| format!("read body: {e}"))?;
    }
    Ok(body)
}

/// One HTTP/1.1 connection to a hub, kept alive across requests: the link's
/// pushes reuse it instead of paying a TCP (and TLS) handshake each.
pub struct KeepAlive {
    base: String,
    timeout: Duration,
    conn: Option<(BufReader<Conn>, String)>,
}

impl KeepAlive {
    pub fn new(base: &str, timeout: Duration) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            timeout,
            conn: None,
        }
    }

    /// `method base+path` with extra header lines (each ending in CRLF) and a
    /// body. A reused connection the server closed meanwhile is retried once
    /// on a fresh one, so callers must make retries harmless (the link's
    /// pushes carry a sequence number the hub dedupes on).
    pub fn send(
        &mut self,
        method: &str,
        path: &str,
        headers: &str,
        body: &[u8],
    ) -> Result<(u16, Vec<u8>), String> {
        let mut attempt = 0;
        loop {
            let fresh = self.conn.is_none();
            if fresh {
                let (conn, hostport, _) = open(&format!("{}{path}", self.base), self.timeout)?;
                self.conn = Some((BufReader::new(conn), hostport));
            }
            match self.try_send(method, path, headers, body) {
                Ok((head, body)) => {
                    if head.close || (!head.chunked && head.length.is_none()) {
                        self.conn = None;
                    }
                    return Ok((head.status, body));
                }
                Err(e) => {
                    self.conn = None;
                    attempt += 1;
                    if fresh || attempt > 1 {
                        return Err(e);
                    }
                }
            }
        }
    }

    fn try_send(
        &mut self,
        method: &str,
        path: &str,
        headers: &str,
        body: &[u8],
    ) -> Result<(Head, Vec<u8>), String> {
        let (r, hostport) = self.conn.as_mut().ok_or("not connected")?;
        let mut req = format!(
            "{method} {path} HTTP/1.1\r\nHost: {hostport}\r\n{headers}Content-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        req.extend_from_slice(body);
        let w = r.get_mut();
        w.write_all(&req)
            .and_then(|_| w.flush())
            .map_err(|e| format!("send to {hostport}: {e}"))?;
        let head = read_head(r)?;
        let body = read_body(r, &head)?;
        Ok((head, body))
    }
}

/// A held-open body, read as it arrives.
pub type Stream = Box<dyn BufRead + Send>;

/// `GET url` whose response is held open and read as it arrives (the hub's
/// event stream). A non-2xx is `Err((status, error text))`; a failure before
/// any response is status 0.
pub fn open_stream(url: &str, headers: &str, timeout: Duration) -> Result<Stream, (u16, String)> {
    let (conn, hostport, path) = open(url, timeout).map_err(|e| (0, e))?;
    let mut r = BufReader::new(conn);
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {hostport}\r\nUser-Agent: klark-link\r\nAccept: application/x-ndjson\r\n{headers}\r\n"
    );
    let w = r.get_mut();
    w.write_all(req.as_bytes())
        .and_then(|_| w.flush())
        .map_err(|e| (0, format!("send to {hostport}: {e}")))?;
    let head = read_head(&mut r).map_err(|e| (0, e))?;
    if !(200..300).contains(&head.status) {
        let body = read_body(&mut r, &head).unwrap_or_default();
        let text = HttpResponse {
            status: head.status,
            body: String::from_utf8_lossy(&body).into_owned(),
        }
        .error_text();
        return Err((head.status, text));
    }
    Ok(if head.chunked {
        Box::new(BufReader::new(Chunked::new(r)))
    } else {
        Box::new(r)
    })
}

/// Parse a full HTTP/1.1 response (Content-Length, chunked, or read-to-close).
pub fn parse_response(raw: &[u8]) -> Result<HttpResponse, String> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("malformed HTTP response: no header end")?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let rest = &raw[split + 4..];
    let mut lines = head.lines();
    let status_line = lines.next().ok_or("malformed HTTP response: empty")?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("malformed status line: {status_line}"))?;
    let mut chunked = false;
    let mut length: Option<usize> = None;
    for line in lines {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        if k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked") {
            chunked = true;
        } else if k == "content-length" {
            length = v.parse().ok();
        }
    }
    let body = if chunked {
        dechunk(rest)?
    } else if let Some(n) = length {
        rest[..n.min(rest.len())].to_vec()
    } else {
        rest.to_vec()
    };
    Ok(HttpResponse {
        status,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

fn dechunk(mut data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let mut reader = BufReader::new(data);
        let mut size_line = String::new();
        let consumed = reader
            .read_line(&mut size_line)
            .map_err(|e| e.to_string())?;
        if consumed == 0 {
            return Ok(out);
        }
        let size = usize::from_str_radix(size_line.trim().split(';').next().unwrap_or(""), 16)
            .map_err(|_| format!("bad chunk size: {}", size_line.trim()))?;
        data = &data[consumed..];
        if size == 0 {
            return Ok(out);
        }
        if data.len() < size {
            return Err("truncated chunked body".into());
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size..];
        if data.starts_with(b"\r\n") {
            data = &data[2..];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_base_from_any_hub_url() {
        assert_eq!(
            hub_http_base("ws://hub:8080/agent").unwrap(),
            "http://hub:8080"
        );
        assert_eq!(
            hub_http_base("wss://hub.example.com/agent").unwrap(),
            "https://hub.example.com"
        );
        assert_eq!(
            hub_http_base("http://10.0.0.5:8080/").unwrap(),
            "http://10.0.0.5:8080"
        );
        assert_eq!(
            hub_http_base("http://h:8080/token?x=1").unwrap(),
            "http://h:8080"
        );
        assert!(hub_http_base("ftp://h/").is_err());
        assert!(hub_http_base("not a url").is_err());
    }

    #[test]
    fn parses_content_length_and_chunked() {
        let r = parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}junk").unwrap();
        assert_eq!(
            r,
            HttpResponse {
                status: 200,
                body: "{}".into()
            }
        );
        let r = parse_response(
            b"HTTP/1.1 404 Not Found\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"er\r\na\r\nror\":\"no\"}\r\n0\r\n\r\n",
        )
        .unwrap();
        assert_eq!(r.status, 404);
        assert_eq!(r.error_text(), "no");
    }

    #[test]
    fn put_streams_a_file_with_content_length() {
        use std::net::TcpListener;
        let path = std::env::temp_dir().join(format!("klark-put-{}.bin", std::process::id()));
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &data).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut head = String::new();
            let mut len = 0usize;
            loop {
                let mut l = String::new();
                r.read_line(&mut l).unwrap();
                if let Some(v) = l.strip_prefix("Content-Length: ") {
                    len = v.trim().parse().unwrap();
                }
                head.push_str(&l);
                if l == "\r\n" {
                    break;
                }
            }
            let mut body = vec![0u8; len];
            r.read_exact(&mut body).unwrap();
            let mut s = s;
            s.write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 13\r\n\r\n{\"sha256\":\"x\"}")
                .unwrap();
            (head, body)
        });
        let r = put_file(
            &format!("http://127.0.0.1:{port}/api/assets?pane=p1&name=a%20b.bin"),
            Some("mtok"),
            &path,
            "application/octet-stream",
            Duration::from_secs(5),
        )
        .unwrap();
        let (head, body) = server.join().unwrap();
        assert!(
            head.starts_with("PUT /api/assets?pane=p1&name=a%20b.bin HTTP/1.1\r\n"),
            "{head}"
        );
        assert!(head.contains("Authorization: Bearer mtok\r\n"));
        assert!(head.contains("Content-Length: 200000\r\n"));
        assert_eq!(body, data);
        assert_eq!(r.status, 201);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn get_error_status_is_an_error() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            s.write_all(
                b"HTTP/1.1 403 Forbidden\r\nContent-Length: 20\r\n\r\n{\"error\":\"not yours\"}",
            )
            .unwrap();
        });
        let mut sink = Vec::new();
        let e = get_to_writer(
            &format!("http://127.0.0.1:{port}/api/assets/x"),
            Some("t"),
            &mut sink,
            10,
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert_eq!(e, "not yours");
    }

    #[test]
    fn round_trip_against_a_local_server() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 18\r\n\r\n{\"delivered\":true}")
                .unwrap();
            req
        });
        let body = serde_json::json!({"to": "m/s/a", "text": "hi"});
        let r = request(
            "POST",
            &format!("http://127.0.0.1:{port}/api/messages"),
            Some("tok"),
            Some(&body),
            Duration::from_secs(5),
        )
        .unwrap();
        let req = server.join().unwrap();
        assert!(req.starts_with("POST /api/messages HTTP/1.1\r\n"), "{req}");
        assert!(req.contains("Authorization: Bearer tok\r\n"));
        assert!(req.ends_with(&body.to_string()));
        assert!(r.ok());
    }
}
