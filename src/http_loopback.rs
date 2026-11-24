use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024; // 16 MiB body cap
const MAX_LINE_BYTES: usize = 8 * 1024; // 8 KiB per status/header/chunk line
const MAX_HEADER_BYTES: usize = 64 * 1024; // 64 KiB total headers
const MAX_HEADER_LINES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoopbackEndpoint {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) base_path: String,
    pub(crate) allow_non_loopback: bool,
    pub(crate) raw_url: String,
}

fn contains_http_unsafe_chars(s: &str) -> bool {
    s.bytes().any(|b| b <= 0x20 || b == 0x7f)
}

impl LoopbackEndpoint {
    /// Parse an HTTP endpoint. Rejects HTTPS (no TLS stack is compiled into `kbwall`),
    /// credentials, control/whitespace/query/fragment characters, `..` base path segments,
    /// and non-loopback hosts unless `allow_non_loopback` is explicitly true.
    pub(crate) fn parse(raw_url: &str, allow_non_loopback: bool) -> Result<Self, String> {
        let trimmed = raw_url.trim().trim_end_matches('/');
        if trimmed.is_empty() {
            return Err("Engine URL must not be empty".to_string());
        }
        if contains_http_unsafe_chars(trimmed) {
            return Err(
                "Engine URL contains forbidden whitespace or control characters".to_string(),
            );
        }
        if trimmed.contains('?') || trimmed.contains('#') {
            return Err(
                "Engine URL must not contain query ('?') or fragment ('#') components".to_string(),
            );
        }
        if trimmed.starts_with("https://") {
            return Err(
                "HTTPS URLs are rejected: kbwall intentionally ships with zero TLS/internet stack to stay air-gapped on loopback HTTP"
                    .to_string(),
            );
        }
        let Some(rest) = trimmed.strip_prefix("http://") else {
            return Err(format!(
                "Unsupported engine URL scheme in '{trimmed}': only http:// is permitted"
            ));
        };
        if rest.contains('@') {
            return Err("Credentials in engine URL are not permitted".to_string());
        }

        let (authority, base_path) = match rest.find('/') {
            Some(idx) => {
                let (a, p) = rest.split_at(idx);
                (a, p.trim_end_matches('/').to_string())
            }
            None => (rest, String::new()),
        };
        if authority.is_empty() {
            return Err(format!("Missing host in engine URL '{trimmed}'"));
        }
        if base_path.split('/').any(|seg| seg == "..") {
            return Err("Engine URL base path must not contain '..' segments".to_string());
        }

        let (host, port) = parse_host_port(authority)?;
        if host.is_empty() || contains_http_unsafe_chars(&host) {
            return Err(format!("Invalid host in engine URL '{trimmed}'"));
        }
        if !allow_non_loopback && !is_loopback_host(&host) {
            return Err(format!(
                "Blocked non-loopback engine host '{host}' in '{trimmed}'. kbwall restricts traffic to loopback (127.0.0.1 / localhost / ::1) by default; set engine.allow_non_loopback = true only for trusted private networks."
            ));
        }

        Ok(Self {
            host,
            port,
            base_path,
            allow_non_loopback,
            raw_url: trimmed.to_string(),
        })
    }

    fn host_header(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    /// Resolve socket addresses using `(host, port)` tuple (native IPv6 support)
    /// and enforce post-resolution loopback verification to defeat DNS rebinding.
    pub(crate) fn resolve_addrs(&self) -> Result<Vec<SocketAddr>, String> {
        let addrs: Vec<SocketAddr> = (self.host.as_str(), self.port)
            .to_socket_addrs()
            .map_err(|e| format!("Failed to resolve '{}:{}': {e}", self.host, self.port))?
            .collect();
        if addrs.is_empty() {
            return Err(format!(
                "No socket address resolved for '{}:{}'",
                self.host, self.port
            ));
        }
        if !self.allow_non_loopback {
            for addr in &addrs {
                if !addr.ip().is_loopback() {
                    return Err(format!(
                        "Blocked DNS/hosts resolution of '{}' to non-loopback IP {} (air-gap guard)",
                        self.host,
                        addr.ip()
                    ));
                }
            }
        }
        Ok(addrs)
    }

    fn build_full_path(&self, path: &str) -> Result<String, HttpStatusError> {
        if !path.starts_with('/') || contains_http_unsafe_chars(path) || path.contains('#') {
            return Err(HttpStatusError {
                status: 400,
                body: format!("Invalid request path '{path}'"),
            });
        }
        let full = if self.base_path.is_empty() {
            path.to_string()
        } else {
            format!("{}{path}", self.base_path)
        };
        if contains_http_unsafe_chars(&full) || full.contains('#') {
            return Err(HttpStatusError {
                status: 400,
                body: "Constructed request path contains forbidden characters".to_string(),
            });
        }
        let route = full.split('?').next().unwrap_or("");
        if route.split('/').any(|seg| seg == "..") {
            return Err(HttpStatusError {
                status: 400,
                body: "Request path must not contain '..' segments".to_string(),
            });
        }
        Ok(full)
    }
}

fn parse_host_port(authority: &str) -> Result<(String, u16), String> {
    if let Some(after_open) = authority.strip_prefix('[') {
        let Some(close_idx) = after_open.find(']') else {
            return Err(format!("Unclosed IPv6 literal in '{authority}'"));
        };
        let (host_part, rest) = after_open.split_at(close_idx);
        let tail = rest.strip_prefix(']').unwrap_or("");
        let port = if tail.is_empty() {
            80
        } else if let Some(p_str) = tail.strip_prefix(':') {
            p_str
                .parse::<u16>()
                .map_err(|_| format!("Invalid port '{p_str}' in '{authority}'"))?
        } else {
            return Err(format!("Malformed IPv6 authority '{authority}'"));
        };
        return Ok((host_part.to_string(), port));
    }

    match authority.rsplit_once(':') {
        Some((h, p_str)) if !h.is_empty() => {
            let port = p_str
                .parse::<u16>()
                .map_err(|_| format!("Invalid port '{p_str}' in '{authority}'"))?;
            Ok((h.to_string(), port))
        }
        _ => Ok((authority.to_string(), 80)),
    }
}

pub(crate) fn is_loopback_host(host: &str) -> bool {
    let lower = host.trim().to_ascii_lowercase();
    if lower == "localhost" {
        return true;
    }
    lower.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Compile-time read-only HTTP/1.1 client for XERJ / Elasticsearch-compatible loopback engines.
/// Intentionally exposes ONLY `get_json` and `post_search_json` (no PUT/DELETE/bulk writes).
#[derive(Debug, Clone)]
pub(crate) struct ReadOnlyHttpClient {
    endpoint: LoopbackEndpoint,
}

#[derive(Debug)]
pub(crate) struct HttpStatusError {
    pub(crate) status: u16,
    pub(crate) body: String,
}

impl std::fmt::Display for HttpStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {} from search engine: {}", self.status, self.body)
    }
}

impl ReadOnlyHttpClient {
    pub(crate) fn new(endpoint: LoopbackEndpoint) -> Self {
        Self { endpoint }
    }

    pub(crate) fn url(&self) -> &str {
        &self.endpoint.raw_url
    }

    pub(crate) fn health_check(&self, timeout: Duration) -> bool {
        self.get_json("/", timeout).is_ok()
    }

    pub(crate) fn get_json(&self, path: &str, timeout: Duration) -> Result<Value, String> {
        let full_path = self
            .endpoint
            .build_full_path(path)
            .map_err(|e| e.to_string())?;
        let resp = self
            .send_request("GET", &full_path, None, timeout)
            .map_err(|e| e.to_string())?;
        serde_json::from_slice(&resp)
            .map_err(|e| format!("Invalid JSON response for GET {path}: {e}"))
    }

    /// Strictly permits POST requests targeting an Elasticsearch/XERJ `/_search` endpoint only.
    pub(crate) fn post_search_json(
        &self,
        path: &str,
        body: &Value,
        timeout: Duration,
    ) -> Result<Value, HttpStatusError> {
        let full_path = self.endpoint.build_full_path(path)?;
        let route = full_path.split('?').next().unwrap_or("");
        if !route.ends_with("/_search") {
            return Err(HttpStatusError {
                status: 403,
                body: format!("kbwall read-only guard blocked non-search POST path '{full_path}'"),
            });
        }
        let encoded = serde_json::to_vec(body).map_err(|e| HttpStatusError {
            status: 0,
            body: format!("Failed to serialize search body: {e}"),
        })?;
        let resp = self.send_request("POST", &full_path, Some(&encoded), timeout)?;
        serde_json::from_slice(&resp).map_err(|e| HttpStatusError {
            status: 0,
            body: format!("Invalid JSON response for POST {path}: {e}"),
        })
    }

    fn send_request(
        &self,
        method: &str,
        full_path: &str,
        body: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<Vec<u8>, HttpStatusError> {
        let deadline = Instant::now() + timeout;
        let host_hdr = self.endpoint.host_header();
        if contains_http_unsafe_chars(&host_hdr) || contains_http_unsafe_chars(full_path) {
            return Err(HttpStatusError {
                status: 400,
                body: "Request headers or path contain forbidden characters".to_string(),
            });
        }
        let addrs = self
            .endpoint
            .resolve_addrs()
            .map_err(|e| HttpStatusError { status: 0, body: e })?;

        let mut last_err = String::new();
        let mut connected: Option<TcpStream> = None;
        for addr in addrs {
            let rem = deadline.saturating_duration_since(Instant::now());
            if rem.is_zero() {
                last_err = "connect timed out".to_string();
                break;
            }
            match TcpStream::connect_timeout(&addr, rem) {
                Ok(s) => {
                    connected = Some(s);
                    break;
                }
                Err(e) => {
                    last_err = e.to_string();
                }
            }
        }
        let mut stream = connected.ok_or_else(|| HttpStatusError {
            status: 0,
            body: format!("Failed to connect to {}: {last_err}", self.endpoint.raw_url),
        })?;

        let rem = deadline.saturating_duration_since(Instant::now());
        if rem.is_zero() {
            return Err(HttpStatusError {
                status: 0,
                body: "HTTP request timed out".to_string(),
            });
        }
        stream
            .set_read_timeout(Some(rem))
            .map_err(|e| HttpStatusError {
                status: 0,
                body: format!("Failed to set read timeout: {e}"),
            })?;
        stream
            .set_write_timeout(Some(rem))
            .map_err(|e| HttpStatusError {
                status: 0,
                body: format!("Failed to set write timeout: {e}"),
            })?;

        let req_head = if let Some(payload) = body {
            format!(
                "{method} {full_path} HTTP/1.1\r\nHost: {host_hdr}\r\nUser-Agent: kbwall/{}\r\nAccept: application/json\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                env!("CARGO_PKG_VERSION"),
                payload.len()
            )
        } else {
            format!(
                "{method} {full_path} HTTP/1.1\r\nHost: {host_hdr}\r\nUser-Agent: kbwall/{}\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
                env!("CARGO_PKG_VERSION")
            )
        };

        stream
            .write_all(req_head.as_bytes())
            .and_then(|()| {
                if let Some(payload) = body {
                    stream.write_all(payload)
                } else {
                    Ok(())
                }
            })
            .and_then(|()| stream.flush())
            .map_err(|e| HttpStatusError {
                status: 0,
                body: format!("HTTP write failed: {e}"),
            })?;

        read_http_response(stream, deadline)
    }
}

fn read_line_limited<R: BufRead>(
    reader: &mut R,
    max_bytes: usize,
    deadline: Instant,
) -> Result<Option<String>, HttpStatusError> {
    if Instant::now() >= deadline {
        return Err(HttpStatusError {
            status: 0,
            body: "HTTP response read timed out".to_string(),
        });
    }
    let mut buf = Vec::with_capacity(128);
    let n = reader
        .take((max_bytes + 1) as u64)
        .read_until(b'\n', &mut buf)
        .map_err(|e| HttpStatusError {
            status: 0,
            body: format!("Failed to read HTTP line: {e}"),
        })?;
    if n == 0 {
        return Ok(None);
    }
    if buf.len() > max_bytes {
        return Err(HttpStatusError {
            status: 0,
            body: format!("HTTP line exceeded safety cap ({max_bytes} bytes)"),
        });
    }
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

fn read_http_response(stream: TcpStream, deadline: Instant) -> Result<Vec<u8>, HttpStatusError> {
    let mut reader = BufReader::new(stream);
    let Some(status_line) = read_line_limited(&mut reader, MAX_LINE_BYTES, deadline)? else {
        return Err(HttpStatusError {
            status: 0,
            body: "Empty HTTP response from search engine".to_string(),
        });
    };
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| HttpStatusError {
            status: 0,
            body: format!("Malformed HTTP status line: '{}'", status_line.trim()),
        })?;

    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    let mut header_bytes = 0usize;
    let mut header_lines = 0usize;
    loop {
        let Some(line) = read_line_limited(&mut reader, MAX_LINE_BYTES, deadline)? else {
            return Err(HttpStatusError {
                status: 0,
                body: "Premature EOF while reading HTTP response headers".to_string(),
            });
        };
        if line == "\r\n" || line == "\n" {
            break;
        }
        header_lines += 1;
        header_bytes = header_bytes.saturating_add(line.len());
        if header_lines > MAX_HEADER_LINES || header_bytes > MAX_HEADER_BYTES {
            return Err(HttpStatusError {
                status: 0,
                body: "HTTP response headers exceeded safety limit".to_string(),
            });
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim();
            if key == "content-length" {
                if let Ok(len) = val.parse::<usize>() {
                    content_length = Some(len);
                }
            } else if key == "transfer-encoding" && val.to_ascii_lowercase().contains("chunked") {
                chunked = true;
            }
        }
    }

    let body = if chunked {
        read_chunked_body(&mut reader, deadline)?
    } else if let Some(len) = content_length {
        if len > MAX_RESPONSE_BYTES {
            return Err(HttpStatusError {
                status: 0,
                body: format!("Response exceeded safety limit ({MAX_RESPONSE_BYTES} bytes)"),
            });
        }
        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf).map_err(|e| HttpStatusError {
            status: 0,
            body: format!("Failed to read HTTP body: {e}"),
        })?;
        buf
    } else {
        let mut buf = Vec::new();
        reader
            .take((MAX_RESPONSE_BYTES + 1) as u64)
            .read_to_end(&mut buf)
            .map_err(|e| HttpStatusError {
                status: 0,
                body: format!("Failed to read HTTP stream body: {e}"),
            })?;
        if buf.len() > MAX_RESPONSE_BYTES {
            return Err(HttpStatusError {
                status: 0,
                body: format!("Response exceeded safety limit ({MAX_RESPONSE_BYTES} bytes)"),
            });
        }
        buf
    };

    if !(200..300).contains(&status) {
        let snippet = String::from_utf8_lossy(&body);
        let compact: String = snippet.chars().take(400).collect();
        return Err(HttpStatusError {
            status,
            body: compact,
        });
    }
    Ok(body)
}

fn read_chunked_body<R: BufRead>(
    reader: &mut R,
    deadline: Instant,
) -> Result<Vec<u8>, HttpStatusError> {
    let mut out = Vec::new();
    loop {
        let Some(size_line) = read_line_limited(reader, MAX_LINE_BYTES, deadline)? else {
            return Err(HttpStatusError {
                status: 0,
                body: "Premature EOF in chunked HTTP body before terminating 0-chunk".to_string(),
            });
        };
        let hex_part = size_line.split(';').next().unwrap_or("").trim();
        let chunk_len = usize::from_str_radix(hex_part, 16).map_err(|_| HttpStatusError {
            status: 0,
            body: format!("Invalid chunked encoding size '{hex_part}'"),
        })?;
        if chunk_len == 0 {
            // Drain optional trailer lines bounded by header cap until empty line.
            for _ in 0..=MAX_HEADER_LINES {
                let Some(trailer) = read_line_limited(reader, MAX_LINE_BYTES, deadline)? else {
                    break;
                };
                if trailer == "\r\n" || trailer == "\n" {
                    break;
                }
            }
            break;
        }
        if out.len().saturating_add(chunk_len) > MAX_RESPONSE_BYTES {
            return Err(HttpStatusError {
                status: 0,
                body: format!(
                    "Chunked response exceeded safety limit ({MAX_RESPONSE_BYTES} bytes)"
                ),
            });
        }
        let start = out.len();
        out.resize(start + chunk_len, 0);
        if let Some(slice) = out.get_mut(start..start + chunk_len) {
            reader.read_exact(slice).map_err(|e| HttpStatusError {
                status: 0,
                body: format!("Failed to read chunk payload: {e}"),
            })?;
        }
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf).map_err(|e| HttpStatusError {
            status: 0,
            body: format!("Failed to read chunk terminator: {e}"),
        })?;
        if crlf != *b"\r\n" {
            return Err(HttpStatusError {
                status: 0,
                body: "Malformed chunked HTTP terminator (expected CRLF)".to_string(),
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn enforces_loopback_blocks_smuggling_base_path_and_resolves_ipv6() {
        assert!(LoopbackEndpoint::parse("http://127.0.0.1:9200", false).is_ok());
        assert!(LoopbackEndpoint::parse("http://localhost:9200", false).is_ok());
        let v6 = LoopbackEndpoint::parse("http://[::1]:9200", false).unwrap();
        let v6_addrs = v6.resolve_addrs().unwrap();
        assert!(v6_addrs.iter().all(|a| a.ip().is_loopback()));

        // Reject query/fragment/CRLF/traversal in engine URL (P0-1 regression guard).
        assert!(LoopbackEndpoint::parse("http://127.0.0.1:9200/_bulk?x=", false).is_err());
        assert!(LoopbackEndpoint::parse("http://127.0.0.1:9200/a#b", false).is_err());
        assert!(LoopbackEndpoint::parse("http://127.0.0.1:9200/a\r\nDELETE /x", false).is_err());
        assert!(LoopbackEndpoint::parse("http://127.0.0.1:9200/a/../b", false).is_err());
        assert!(LoopbackEndpoint::parse("https://127.0.0.1:9200", true).is_err());
        assert!(LoopbackEndpoint::parse("http://example.com:9200", false).is_err());
        assert!(LoopbackEndpoint::parse("http://10.0.0.5:9200", true).is_ok());

        let client = ReadOnlyHttpClient::new(
            LoopbackEndpoint::parse("http://127.0.0.1:9200", false).unwrap(),
        );
        let err = client
            .post_search_json("/_bulk", &serde_json::json!({}), Duration::from_secs(1))
            .unwrap_err();
        assert_eq!(err.status, 403);
        assert!(err.body.contains("read-only guard"));
    }

    #[test]
    fn parses_chunked_and_rejects_truncated_chunked_responses() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let srv = thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut req_buf = [0u8; 1024];
            drop(sock.read(&mut req_buf));
            let resp = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n6\r\n{\"ok\":\r\n5\r\ntrue}\r\n0\r\n\r\n";
            sock.write_all(resp.as_bytes()).unwrap();

            // Second connection: truncated chunked stream (missing terminating 0 chunk).
            let (mut sock2, _) = listener.accept().unwrap();
            drop(sock2.read(&mut req_buf));
            sock2
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n")
                .unwrap();
        });

        let ep = LoopbackEndpoint::parse(&format!("http://127.0.0.1:{port}"), false).unwrap();
        let client = ReadOnlyHttpClient::new(ep);
        let val = client.get_json("/", Duration::from_secs(2)).unwrap();
        assert_eq!(val.get("ok").and_then(Value::as_bool), Some(true));

        let err = client.get_json("/", Duration::from_secs(2)).unwrap_err();
        assert!(err.contains("Premature EOF"));
        srv.join().unwrap();
    }
}
