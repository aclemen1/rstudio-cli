//! HTTP/1.1 client over either a Unix domain socket (RStudio Server) or a
//! TCP loopback connection (RStudio Desktop on macOS/Linux). The wire format
//! is identical; only the connection setup differs.
//!
//! The HTTP envelope (request line, headers, body framing) is built once and
//! sent through whichever backend the caller picked.

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// Where the rsession is reachable from this CLI invocation.
#[derive(Debug, Clone)]
pub enum Backend {
    /// Unix domain socket — RStudio Server's
    /// `/var/run/rstudio-server/rstudio-rsession/<stream>`.
    Unix(PathBuf),
    /// TCP loopback — RStudio Desktop on macOS/Linux. The launcher (Electron)
    /// picks an ephemeral port at startup; we discover it from the rsession's
    /// argv.
    Tcp(SocketAddr),
    // TODO(linux): RStudio Desktop on Linux can also expose a Unix socket
    // via the `RS_LOCAL_PEER` env var (see rstudio source
    // SessionPosixHttpConnectionListener.cpp:96-113). On the Linux Desktop
    // build the launcher does not currently set RS_LOCAL_PEER, so this
    // PoC sticks to the TCP path that the Electron launcher exercises.
}

pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub fn request(
    backend: &Backend,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
    read_timeout: Option<Duration>,
) -> Result<HttpResponse> {
    let request_bytes = build_request(method, path, headers, body);
    let raw = match backend {
        Backend::Unix(p) => send_unix(p, &request_bytes, read_timeout)?,
        Backend::Tcp(addr) => send_tcp(*addr, &request_bytes, read_timeout)?,
    };
    parse_response(&raw)
}

fn build_request(method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut req = Vec::with_capacity(512 + body.len());
    req.extend_from_slice(format!("{method} {path} HTTP/1.1\r\n").as_bytes());
    req.extend_from_slice(b"Host: localhost\r\n");
    req.extend_from_slice(b"Connection: close\r\n");
    req.extend_from_slice(b"Accept: */*\r\n");
    for (k, v) in headers {
        req.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
    }
    req.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    req.extend_from_slice(b"\r\n");
    req.extend_from_slice(body);
    req
}

fn send_unix(path: &Path, request_bytes: &[u8], read_timeout: Option<Duration>) -> Result<Vec<u8>> {
    let mut stream = UnixStream::connect(path)
        .with_context(|| format!("connect to {}", path.display()))?;
    stream.set_read_timeout(read_timeout)?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    stream.write_all(request_bytes).context("send HTTP request")?;
    stream.shutdown(Shutdown::Write).ok();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).context("read HTTP response")?;
    Ok(buf)
}

fn send_tcp(
    addr: SocketAddr,
    request_bytes: &[u8],
    read_timeout: Option<Duration>,
) -> Result<Vec<u8>> {
    let mut stream =
        TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).with_context(|| format!("connect to {addr}"))?;
    stream.set_read_timeout(read_timeout)?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    stream.write_all(request_bytes).context("send HTTP request")?;
    stream.shutdown(Shutdown::Write).ok();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).context("read HTTP response")?;
    Ok(buf)
}

fn parse_response(buf: &[u8]) -> Result<HttpResponse> {
    let split_at = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| anyhow!("malformed HTTP response (no headers terminator)"))?;
    let head = std::str::from_utf8(&buf[..split_at]).context("HTTP headers are not UTF-8")?;
    let body = buf[split_at + 4..].to_vec();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().ok_or_else(|| anyhow!("empty HTTP response"))?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow!("invalid HTTP status line: {status_line}"))?;
    let headers = lines
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect();
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}
