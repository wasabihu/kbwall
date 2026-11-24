# kbwall

[![CI](https://github.com/wasabihu/kbwall/actions/workflows/ci.yml/badge.svg)](https://github.com/wasabihu/kbwall/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Zero Unsafe](https://img.shields.io/badge/unsafe-forbid-22c55e.svg)](Cargo.toml)
[![Zero TLS / Air-Gapped](https://img.shields.io/badge/network-loopback%20only-0ea5e9.svg)](SECURITY.md)

**English** · [简体中文](README.zh-CN.md)

**An air-gapped, read-only Rust MCP firewall for [`xerj`](https://github.com/xerj-org/xerj) and Elasticsearch-compatible local knowledge bases — ~1 MB single binary, zero Tokio, zero TLS, multilingual out of the box with offline CJK enhancement.**

---

## Why `kbwall`?

Local search engines like [`xerj-org/xerj`](https://github.com/xerj-org/xerj) are remarkably fast at indexing Markdown, PDFs, Office files, and codebases in a single binary. Their full built-in MCP server is built for all-purpose developer workflows — including index writes/deletes, corpus feedback, and automated field-report PRs.

When pointing Claude Code, Cursor, Codex, or Antigravity at a machine holding **private notes, financial filings, legal contracts, and proprietary repositories**, you often want a **strict least-privilege, read-only boundary**:

| Guarantee | What `kbwall` Enforces |
| :--- | :--- |
| **Air-Gapped by Construction** | Ships with **zero TLS libraries** and restricts traffic to loopback HTTP (`127.0.0.1` / `localhost` / `::1`) by default — architecturally incapable of phoning home or submitting outbound PRs. |
| **Compile-Time Read-Only MCP** | Exposes only **3 read-only tools** (`kb_search`, `kb_status`, optional `kb_read`). Its HTTP client only exposes `GET` and `POST .../_search` (no `PUT`, `DELETE`, or `/_bulk`). |
| **Immutable Secret Exclusions** | Always blocks `.env`, `.env.*`, `*.key`, `*.pem`, `*secret*`, `*credential*`, `*token*.json`, SSH keys, and `*.sqlite*` across both `kbwall sync` and `kb_read` (canonicalized anti-symlink confinement). |
| **Multilingual + First-Class Offline CJK** | Native Unicode word tokenization + fuzzy fallback for English & European/non-Latin languages; when CJK characters appear, automatically activates an embedded 88 KB offline table covering **all 20,992 BMP CJK ideographs** (`U+4E00..=U+9FFF`) for Simplified↔Traditional folding, homophone pinyin correction, and statutory clause variants (`第3条` ↔ `第三条`) identically on macOS, Linux, and Windows. |
| **Multi-Vault Noise Control** | Organize dozens of folders into domain `groups`, filter `scope = "docs" | "code" | "all"`, and keep chat/inbox folders opt-in (`default_included = false`). |
| **Sub-4 ms MCP Startup** | ~1.0 MB stripped Rust binary (`#![forbid(unsafe_code)]`, zero Tokio), `~3.7 ms` MCP cold `initialize`. |

---

## 60-Second Quickstart

### 1. Install `xerj` (the underlying search engine) & `kbwall`

```bash
# Install XERJ (if not already installed)
curl -fsSL https://xerj.org/get | sh
xerj --insecure --detach

# Build & install kbwall from source (Rust >= 1.88)
cargo install --git https://github.com/wasabihu/kbwall
```

### 2. Initialize & Edit `~/.config/kbwall/kb.toml`

```bash
kbwall init
# Edit ~/.config/kbwall/kb.toml to declare your folders under [[sources]]
kbwall check
kbwall sync
```

### 3. Wire into Your AI Coding Agent

```bash
# Auto-detect & wire Antigravity / Claude Code / Cursor
kbwall setup

# Or wire Claude Code / Codex CLI directly
claude mcp add kbwall -- ~/.local/bin/kbwall mcp
codex mcp add kbwall -- ~/.local/bin/kbwall mcp
```

---

## Exposed MCP Tools (Strictly Read-Only)

1. **`kb_search`**: Search across configured sources with `query`, `scope` (`docs` / `code` / `all`), `group`, `target`, `limit` (`1..=15`), and `mode` (`bm25` / `hybrid` with automatic BM25 fallback). Returns exact file paths, line ranges, PDF page numbers (`p.211/257`), and snippets.
2. **`kb_status`**: Inspect local engine health, total indexed passages, domain groups, and configured source paths.
3. **`kb_read`** *(disable via `security.allow_passage_read = false`)*: Read a bounded slice of lines (default max 80) from a whitelisted file inside configured `[[sources]]` directories. Canonicalizes paths to block `..` traversal, symlink escapes, binary files, and built-in/custom secret exclusion globs.

---

## CLI Commands

```bash
kbwall check                                # Validate kb.toml, loopback policy, and source directories
kbwall status [--json]                      # Show engine health and per-source indexed passage counts
kbwall search "query" [--scope docs|code|all] [--group <g>] [--target <t>] [--limit 8] [--json]
kbwall read /path/to/doc.md --start-line 1 --end-line 40
kbwall sync [--target <key>] [--group <g>] [--dry-run] [--json]
kbwall mcp [--config /path/to/kb.toml]      # Run read-only stdio MCP server
kbwall init [--force]                       # Generate starter ~/.config/kbwall/kb.toml
kbwall setup [--name kbwall] [--mcp-file <path>]
```

---

## Part of the Least-Privilege Local AI Agent Stack

`kbwall` pairs cleanly with companion open-source projects:

- **[`xerj-org/xerj`](https://github.com/xerj-org/xerj)** — High-speed single-binary Rust search engine powering indexing and BM25/hybrid retrieval underneath `kbwall`.
- **[`wasabihu/sessfork`](https://github.com/wasabihu/sessfork)** — Sister zero-Tokio Rust MCP (`sessfork-mcp`) for least-privilege, zero-focus-theft authenticated web browsing via Moli/Chrome.
- **[`wasabihu/vericorpus`](https://github.com/wasabihu/vericorpus)** — Audit-grade statutory, financial, and clinical corpora & CJK retrieval benchmarks for XERJ/ES engines.
- **[`wasabihu/AKReport`](https://github.com/wasabihu/AKReport)** — Batch-download A-share/HKEX annual report PDFs straight into a `kbwall` research source for 100% private local AI financial research.
- **[`wasabihu/claimcite`](https://github.com/wasabihu/claimcite)** — Verify that every citation and quote in AI-assisted writing matches real published literature.

---

## Security & Attribution

- See [SECURITY.md](SECURITY.md) for the full threat model and security invariants.
- Embedded offline CJK Simplified↔Traditional and Pinyin lookup tables (`src/data/`) are derived from Unicode CLDR/Unihan data (Unicode-3.0 permissive license).
- Independent community project; not affiliated with or endorsed by The XERJ Authors or Elastic.

## License

[MIT](LICENSE)
