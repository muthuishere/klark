//! A minimal blocking HTTP/1.1 client for the hub's small JSON API
//! (`/api/token`, `/api/agents`, `/api/messages`, ADR 0009).
//!
//! Plain `http://` only: plain HTTP is the decided deployment (ADR 0005), and
//! nothing else in the tree is an HTTP client. `https://` returns a clear error.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
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
    let parsed = url::Url::parse(url).map_err(|e| format!("url {url}: {e}"))?;
    match parsed.scheme() {
        "http" => {}
        "https" => {
            return Err(format!(
                "{url}: https is not supported by klark msg yet; the hub is plain http (ADR 0005)"
            ))
        }
        other => return Err(format!("{url}: unsupported scheme {other}")),
    }
    let host = parsed.host_str().ok_or_else(|| format!("{url}: no host"))?;
    let port = parsed.port_or_known_default().unwrap_or(80);
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("resolve {host}:{port}: {e}"))?
        .next()
        .ok_or_else(|| format!("resolve {host}:{port}: no address"))?;
    let mut stream = TcpStream::connect_timeout(&addr, timeout)
        .map_err(|e| format!("connect {host}:{port}: {e}"))?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();

    let mut path = parsed.path().to_string();
    if let Some(q) = parsed.query() {
        path.push('?');
        path.push_str(q);
    }
    let body = json_body.map(|v| v.to_string()).unwrap_or_default();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nAccept: application/json\r\nConnection: close\r\nUser-Agent: klark-msg\r\n"
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
        .map_err(|e| format!("send to {host}:{port}: {e}"))?;

    let mut raw = Vec::new();
    BufReader::new(stream)
        .read_to_end(&mut raw)
        .map_err(|e| format!("read from {host}:{port}: {e}"))?;
    parse_response(&raw)
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
    fn https_is_refused_clearly() {
        let e = request(
            "GET",
            "https://h/api/agents",
            None,
            None,
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(e.contains("https is not supported"), "{e}");
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
