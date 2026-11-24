# Security Policy

`kbwall` is designed as a **least-privilege, read-only, air-gapped MCP firewall** in front of local search engines (such as XERJ or Elasticsearch-compatible nodes).

## Core Security Invariants

1. **Zero `unsafe` Rust (`#![forbid(unsafe_code)]`)**: Enforced at compile time in `Cargo.toml`.
2. **Air-Gapped by Construction**: `kbwall` intentionally links **zero TLS libraries** and restricts engine traffic to loopback HTTP (`127.0.0.1` / `localhost` / `::1`) unless `engine.allow_non_loopback = true` is explicitly configured. DNS resolution validates that **every** resolved socket address is loopback before connecting (defeating DNS rebinding / dual-stack bypass).
3. **Compile-Time Read-Only HTTP Client**: The internal HTTP client only exposes `GET` and `POST .../_search`, validates `base_path` and final request paths against traversal/query/fragment injection, and enforces strict response line/header/body size and wall-clock deadline caps. Write endpoints (`PUT`, `DELETE`, `/_bulk`, `/_doc`) cannot be invoked by the MCP server.
4. **No Outbound Telemetry or PR Submission**: `kbwall` never phones home, never registers feedback/PR tools in MCP, and always invokes `xerj autoindex` with `--disable-feedback` and `XERJ_DISABLE_FEEDBACK=true`.
5. **Strict Path & Secret Confinement (`kb_search` & `kb_read`)**: Immutable built-in secret exclusions (`.env`, `.env.*`, `*.env`, `*.key`, `*.pem`, `*.p12`, `*.pfx`, `*.ppk`, `*secret*`, `*credential*`, `*token*.json/yml/yaml/toml`, SSH private keys, `.npmrc`, `.netrc`, `.pgpass`, `*.sqlite*`, `*.db*`) plus user exclusions are enforced across `kbwall sync`, `kb_search` hit normalization (dropping blocked or traversal paths before snippets leave the server), and `kb_read` (single `O_NOFOLLOW | O_NONBLOCK` open, canonical root confinement across nested sources with deny-wins semantics, 20 MiB file cap, and 128 KiB output cap).
6. **Bounded MCP & Query Processing**: Queries are length-capped and wildcard-escaped before entering Elasticsearch DSL; MCP stdio supports both NDJSON and `Content-Length` framing with a 4 MiB frame cap.

## Reporting a Vulnerability

Please report security issues privately via GitHub Security Advisories on `wasabihu/kbwall` rather than opening a public issue.
