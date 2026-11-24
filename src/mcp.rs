use crate::config::KbConfig;
use crate::http_loopback::ReadOnlyHttpClient;
use crate::query::{
    execute_search, fetch_status, format_search_text, format_status_text, SearchRequest,
};
use serde_json::{json, Value};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;

const SERVER_NAME: &str = "kbwall";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_MCP_FRAME_BYTES: usize = 4 * 1024 * 1024;
const MAX_MCP_HEADER_LINE_BYTES: usize = 8 * 1024;
const MAX_MCP_HEADERS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameFormat {
    Ndjson,
    ContentLength,
}

pub(crate) fn tools_manifest(cfg: &KbConfig) -> Value {
    let mut group_enum: Vec<Value> = vec![Value::String("all".to_string())];
    for g in &cfg.groups {
        group_enum.push(Value::String(g.key.clone()));
    }
    let group_desc = if cfg.groups.is_empty() {
        "Optional domain group filter (or 'all')".to_string()
    } else {
        let pairs: Vec<String> = cfg
            .groups
            .iter()
            .map(|g| format!("{}({})", g.key, g.label))
            .collect();
        format!("Filter by group: all or {}", pairs.join(" / "))
    };

    let mut tools = vec![
        json!({
            "name": "kb_search",
            "description": "Search across configured local knowledge bases and projects (strictly read-only, loopback-only, multilingual with automatic offline CJK homophone/clause enhancement). Default scope='docs' filters out test code and opt-in noisy groups.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search keywords (any language; spaces separate multiple terms)" },
                    "scope": {
                        "type": "string",
                        "enum": ["docs", "code", "all"],
                        "description": "docs=documents/PDF/Markdown only (default), code=source files only, all=both"
                    },
                    "group": {
                        "type": "string",
                        "enum": group_enum,
                        "description": group_desc
                    },
                    "target": {
                        "type": "string",
                        "description": "Optional single knowledge base key (inspect available keys via kb_status)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max hits to return (1-15, default 6)"
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["bm25", "hybrid"],
                        "description": "Retrieval mode (default: bm25; hybrid auto-falls back to bm25 if engine lacks vectors)"
                    }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "kb_status",
            "description": "Check local search engine health, configured knowledge base keys, domain groups, local folder paths, and indexed passage counts (read-only).",
            "inputSchema": { "type": "object", "properties": {} }
        }),
    ];

    if cfg.allow_passage_read {
        tools.push(json!({
            "name": "kb_read",
            "description": format!(
                "Read a bounded slice of lines (max {}) from a file inside configured knowledge base directories (strictly read-only; rejects symlinks/paths outside roots and secret patterns like .env/*.key/*.pem).",
                cfg.max_read_lines
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute file path returned by kb_search" },
                    "start_line": { "type": "integer", "description": "1-based start line (default: 1)" },
                    "end_line": { "type": "integer", "description": "1-based inclusive end line (clamped to security.max_read_lines)" }
                },
                "required": ["path"]
            }
        }));
    }

    Value::Array(tools)
}

pub(crate) fn handle_jsonrpc_message(
    msg: &Value,
    cfg: &KbConfig,
    client: &ReadOnlyHttpClient,
) -> Option<Value> {
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    // Notifications (no id) must not produce a reply.
    let req_id = msg.get("id")?.clone();

    match method {
        "initialize" => {
            let client_proto = msg
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("2025-06-18");
            Some(json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {
                    "protocolVersion": client_proto,
                    "capabilities": { "tools": {} },
                    "serverInfo": {
                        "name": SERVER_NAME,
                        "version": SERVER_VERSION
                    }
                }
            }))
        }
        "ping" => Some(json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {}
        })),
        "tools/list" => Some(json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": { "tools": tools_manifest(cfg) }
        })),
        "resources/list" => Some(json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": { "resources": [] }
        })),
        "prompts/list" => Some(json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": { "prompts": [] }
        })),
        "tools/call" => {
            let name = msg
                .pointer("/params/name")
                .and_then(Value::as_str)
                .unwrap_or("");
            let args = msg
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));

            let (text, is_error) = match dispatch_tool(name, &args, cfg, client) {
                Ok(t) => (t, false),
                Err(e) => (e, true),
            };
            Some(json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {
                    "content": [{ "type": "text", "text": text }],
                    "isError": is_error
                }
            }))
        }
        other => Some(json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "error": {
                "code": -32601,
                "message": format!("Method not found: {other}")
            }
        })),
    }
}

fn dispatch_tool(
    name: &str,
    args: &Value,
    cfg: &KbConfig,
    client: &ReadOnlyHttpClient,
) -> Result<String, String> {
    match name {
        "kb_search" => {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "Missing required non-empty parameter 'query'".to_string())?;
            let scope = args
                .get("scope")
                .and_then(Value::as_str)
                .unwrap_or(cfg.default_scope.as_str());
            let group = args
                .get("group")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty() && *s != "all");
            let target = args
                .get("target")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .map_or(6, |v| v.clamp(1, 15) as usize);
            let mode = match args.get("mode").and_then(Value::as_str).unwrap_or("bm25") {
                "hybrid" => "hybrid",
                _ => "bm25",
            };
            let include_non_default = group.is_some() || target.is_some();

            let resp = execute_search(
                cfg,
                client,
                &SearchRequest {
                    query,
                    target,
                    group,
                    scope,
                    include_non_default,
                    limit,
                    mode,
                    max_chars: 320,
                },
            )?;
            Ok(format_search_text(&resp))
        }
        "kb_status" => {
            let st = fetch_status(cfg, client);
            Ok(format_status_text(&st))
        }
        "kb_read" => {
            let path_str = args
                .get("path")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "Missing required parameter 'path'".to_string())?;
            let start_line = args
                .get("start_line")
                .and_then(Value::as_u64)
                .map_or(1, |v| v.clamp(1, 1_000_000) as usize);
            let end_line = args
                .get("end_line")
                .and_then(Value::as_u64)
                .map(|v| v.clamp(1, 1_000_000) as usize);
            cfg.read_passage(Path::new(path_str), start_line, end_line)
        }
        other => Err(format!("Unknown read-only tool '{other}'")),
    }
}

fn read_bounded_line<R: BufRead>(reader: &mut R, max_bytes: usize) -> io::Result<Option<Vec<u8>>> {
    let mut out = Vec::new();
    loop {
        let avail = reader.fill_buf()?;
        if avail.is_empty() {
            return Ok(if out.is_empty() { None } else { Some(out) });
        }
        let nl_pos = avail.iter().position(|&b| b == b'\n');
        let take = nl_pos.map_or(avail.len(), |p| p + 1);
        if out.len().saturating_add(take) > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("MCP input line exceeds {max_bytes}-byte safety cap"),
            ));
        }
        if let Some(chunk) = avail.get(..take) {
            out.extend_from_slice(chunk);
        }
        reader.consume(take);
        if nl_pos.is_some() {
            return Ok(Some(out));
        }
    }
}

fn trim_ascii_ws(mut bytes: &[u8]) -> &[u8] {
    while let Some((&first, rest)) = bytes.split_first() {
        if first.is_ascii_whitespace() {
            bytes = rest;
        } else {
            break;
        }
    }
    while let Some((&last, rest)) = bytes.split_last() {
        if last.is_ascii_whitespace() {
            bytes = rest;
        } else {
            break;
        }
    }
    bytes
}

fn parse_header_line(line: &[u8]) -> Option<(&str, &str)> {
    let s = std::str::from_utf8(line).ok()?;
    let (k, v) = s.split_once(':')?;
    Some((k.trim(), v.trim()))
}

fn read_next_frame<R: BufRead>(reader: &mut R) -> io::Result<Option<(FrameFormat, Vec<u8>)>> {
    loop {
        let Some(line) = read_bounded_line(reader, MAX_MCP_FRAME_BYTES)? else {
            return Ok(None);
        };
        let trimmed = trim_ascii_ws(&line);
        if trimmed.is_empty() {
            continue;
        }

        let is_lsp_header = !trimmed.starts_with(b"{")
            && !trimmed.starts_with(b"[")
            && parse_header_line(trimmed).is_some_and(|(k, _)| {
                k.eq_ignore_ascii_case("content-length") || k.eq_ignore_ascii_case("content-type")
            });

        if !is_lsp_header {
            return Ok(Some((FrameFormat::Ndjson, trimmed.to_vec())));
        }

        let mut content_length: Option<usize> = None;
        if let Some((k, v)) = parse_header_line(trimmed) {
            if k.eq_ignore_ascii_case("content-length") {
                let parsed = v.parse::<usize>().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "Invalid Content-Length value")
                })?;
                content_length = Some(parsed);
            }
        }

        let mut header_count = 1usize;
        loop {
            let Some(hdr_line) = read_bounded_line(reader, MAX_MCP_HEADER_LINE_BYTES)? else {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Unexpected EOF inside MCP Content-Length headers",
                ));
            };
            let hdr_trimmed = trim_ascii_ws(&hdr_line);
            if hdr_trimmed.is_empty() {
                break;
            }
            header_count += 1;
            if header_count > MAX_MCP_HEADERS {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Too many MCP frame headers",
                ));
            }
            if let Some((k, v)) = parse_header_line(hdr_trimmed) {
                if k.eq_ignore_ascii_case("content-length") {
                    let parsed = v.parse::<usize>().map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "Invalid Content-Length value")
                    })?;
                    if let Some(prev) = content_length {
                        if prev != parsed {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "Conflicting Content-Length headers",
                            ));
                        }
                    }
                    content_length = Some(parsed);
                }
            }
        }

        let len = content_length.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "Missing Content-Length header")
        })?;
        if len == 0 || len > MAX_MCP_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("MCP Content-Length {len} out of bounds (1..={MAX_MCP_FRAME_BYTES})"),
            ));
        }

        let mut body = vec![0u8; len];
        reader.read_exact(&mut body)?;
        return Ok(Some((FrameFormat::ContentLength, body)));
    }
}

fn write_frame<W: Write>(out: &mut W, fmt: FrameFormat, encoded: &str) -> io::Result<()> {
    match fmt {
        FrameFormat::Ndjson => writeln!(out, "{encoded}")?,
        FrameFormat::ContentLength => {
            write!(out, "Content-Length: {}\r\n\r\n{encoded}", encoded.len())?;
        }
    }
    out.flush()
}

fn serve_io<R: BufRead, W: Write>(
    reader: &mut R,
    out: &mut W,
    cfg: &KbConfig,
    client: &ReadOnlyHttpClient,
) {
    loop {
        match read_next_frame(reader) {
            Ok(Some((fmt, payload))) => {
                let msg: Value = match serde_json::from_slice(&payload) {
                    Ok(v) => v,
                    Err(e) => {
                        let err_resp = json!({
                            "jsonrpc": "2.0",
                            "id": Value::Null,
                            "error": { "code": -32700, "message": format!("Parse error: {e}") }
                        });
                        if let Ok(encoded) = serde_json::to_string(&err_resp) {
                            if write_frame(out, fmt, &encoded).is_err() {
                                break;
                            }
                        }
                        continue;
                    }
                };
                if let Some(resp) = handle_jsonrpc_message(&msg, cfg, client) {
                    if let Ok(encoded) = serde_json::to_string(&resp) {
                        if write_frame(out, fmt, &encoded).is_err() {
                            break;
                        }
                    }
                }
            }
            Ok(None) => break,
            Err(e) => {
                if e.kind() == io::ErrorKind::InvalidData {
                    let err_resp = json!({
                        "jsonrpc": "2.0",
                        "id": Value::Null,
                        "error": { "code": -32700, "message": format!("Framing error: {e}") }
                    });
                    if let Ok(encoded) = serde_json::to_string(&err_resp) {
                        drop(write_frame(out, FrameFormat::Ndjson, &encoded));
                    }
                }
                break;
            }
        }
    }
}

pub(crate) fn run_stdio_server(cfg: &KbConfig) -> i32 {
    let client = ReadOnlyHttpClient::new(cfg.endpoint.clone());
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut out = io::BufWriter::new(stdout.lock());
    serve_io(&mut reader, &mut out, cfg, &client);
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_cfg(allow_read: bool) -> KbConfig {
        KbConfig::parse_toml(
            &format!(
                r#"
[engine]
url = "http://127.0.0.1:9200"

[security]
allow_passage_read = {allow_read}

[[sources]]
key = "notes"
name = "Notes"
path = "/tmp"
prefix = "kb-notes"
group = "notes"
"#
            ),
            Path::new("test.toml"),
        )
        .unwrap()
    }

    #[test]
    fn exposes_only_read_only_tools_and_never_write_or_feedback_tools() {
        let cfg = sample_cfg(true);
        let client = ReadOnlyHttpClient::new(cfg.endpoint.clone());
        let list = handle_jsonrpc_message(
            &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
            &cfg,
            &client,
        )
        .unwrap();
        let names: Vec<&str> = list
            .pointer("/result/tools")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .filter_map(|t| t.get("name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, vec!["kb_search", "kb_status", "kb_read"]);
        assert!(!names.contains(&"xerj_field_report"));
        assert!(!names.contains(&"kb_delete"));

        let cfg_no_read = sample_cfg(false);
        let list2 = handle_jsonrpc_message(
            &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            &cfg_no_read,
            &client,
        )
        .unwrap();
        let names2: Vec<&str> = list2
            .pointer("/result/tools")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .filter_map(|t| t.get("name").and_then(Value::as_str))
            .collect();
        assert_eq!(names2, vec!["kb_search", "kb_status"]);
    }

    #[test]
    fn rejects_empty_query_and_unknown_tools_fail_closed() {
        let cfg = sample_cfg(true);
        let client = ReadOnlyHttpClient::new(cfg.endpoint.clone());
        let resp = handle_jsonrpc_message(
            &json!({
                "jsonrpc":"2.0","id":3,"method":"tools/call",
                "params":{"name":"kb_search","arguments":{"query":"   "}}
            }),
            &cfg,
            &client,
        )
        .unwrap();
        assert_eq!(
            resp.pointer("/result/isError").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn supports_both_content_length_and_ndjson_stdio_framing() {
        let cfg = sample_cfg(true);
        let client = ReadOnlyHttpClient::new(cfg.endpoint.clone());

        // 1) Content-Length framed messages without trailing newlines.
        let req1 = r#"{"jsonrpc":"2.0","id":10,"method":"ping"}"#;
        let req2 = r#"{"jsonrpc":"2.0","id":11,"method":"tools/list"}"#;
        let wire = format!(
            "Content-Length: {}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{}Content-Length: {}\r\n\r\n{}",
            req1.len(),
            req1,
            req2.len(),
            req2
        );
        let mut reader = io::Cursor::new(wire.into_bytes());
        let mut out_buf = Vec::new();
        serve_io(&mut reader, &mut out_buf, &cfg, &client);

        let mut resp_reader = io::Cursor::new(out_buf);
        let (f1, b1) = read_next_frame(&mut resp_reader).unwrap().unwrap();
        let (f2, b2) = read_next_frame(&mut resp_reader).unwrap().unwrap();
        assert_eq!(f1, FrameFormat::ContentLength);
        assert_eq!(f2, FrameFormat::ContentLength);
        let v1: Value = serde_json::from_slice(&b1).unwrap();
        let v2: Value = serde_json::from_slice(&b2).unwrap();
        assert_eq!(v1.get("id").and_then(Value::as_u64), Some(10));
        assert_eq!(v2.get("id").and_then(Value::as_u64), Some(11));

        // 2) NDJSON framing.
        let nd = format!("{req1}\n");
        let mut nd_reader = io::Cursor::new(nd.into_bytes());
        let mut nd_out = Vec::new();
        serve_io(&mut nd_reader, &mut nd_out, &cfg, &client);
        let nd_str = String::from_utf8(nd_out).unwrap();
        assert!(!nd_str.starts_with("Content-Length:"));
        let v3: Value = serde_json::from_str(nd_str.trim()).unwrap();
        assert_eq!(v3.get("id").and_then(Value::as_u64), Some(10));
    }
}
