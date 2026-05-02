use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};

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
    socket_path: &Path,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
    read_timeout: Option<Duration>,
) -> Result<HttpResponse> {
    let mut stream = UnixStream::connect(socket_path)
        .with_context(|| format!("connect to {}", socket_path.display()))?;
    stream.set_read_timeout(read_timeout)?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;

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

    stream.write_all(&req).context("send HTTP request")?;
    stream.shutdown(Shutdown::Write).ok();

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).context("read HTTP response")?;

    parse_response(&buf)
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
