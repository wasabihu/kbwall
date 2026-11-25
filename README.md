# kbwall

[![CI](https://github.com/wasabihu/kbwall/actions/workflows/ci.yml/badge.svg)](https://github.com/wasabihu/kbwall/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Zero Unsafe](https://img.shields.io/badge/unsafe-forbid-22c55e.svg)](Cargo.toml)
[![Zero TLS / Air-Gapped](https://img.shields.io/badge/network-loopback%20only-0ea5e9.svg)](SECURITY.md)
[![wasabihu/kbwall MCP server](https://glama.ai/mcp/servers/wasabihu/kbwall/badges/score.svg)](https://glama.ai/mcp/servers/wasabihu/kbwall)

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

# Option A: Install prebuilt ~1 MB binary (macOS & Linux, verifies SHA-256, no Rust needed)
curl -fsSL https://raw.githubusercontent.com/wasabihu/kbwall/main/install.sh | sh
# (Windows: download kbwall-x86_64-pc-windows-msvc.zip from GitHub Releases)

# Option B: Build from source (Rust >= 1.88)
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

1. **`kb_search`**: Search across configured sources with `query`, `scope` (`docs` / `code` / `all`), `group`, `target`, `limit` (`1..=15`), and `mode` (`bm25` / `hybrid` with filter pushdown into every sub-query leg and automatic BM25 fallback). Returns exact file paths, line ranges, PDF page numbers (`p.211/257`), canonical `xerj://` evidence handles (`cite`), and snippets.
2. **`kb_status`**: Inspect local engine health, total indexed passages, domain groups, and configured source paths.
3. **`kb_read`** *(disable via `security.allow_passage_read = false`)*: Read a bounded slice of lines (default max 80) from a whitelisted file inside configured `[[sources]]` directories. Canonicalizes paths to block `..` traversal, symlink escapes, binary files, and built-in/custom secret exclusion globs.

---

## Verifiable `xerj://` Evidence Handles

Every eligible `kb_search` hit (in MCP output, `kbwall search`, and `kbwall search --json`) emits a canonical, host-free evidence handle matching the shared [`wasabihu/claimcite`](https://github.com/wasabihu/claimcite) span protocol byte-for-byte:

```text
xerj://<percent_encoded_index>/<percent_encoded_doc_id>#b=<byte_start>-<byte_end>&f=<field>
```

- **No `host:port` in document URIs**: citations stay portable across machines and never leak internal network topology into reports or manuscripts.
- **Strict UTF-8 byte boundaries**: `#b=<byte_start>-<byte_end>` always snaps to verified UTF-8 character boundaries in `_source.<field>` (preferring XERJ's verified `_passage` byte offsets when present and falling back to exact/longest-chunk span location for `hybrid` or non-`_passage` hits).
- **Zero-hallucination round-trip**: copy any verbatim quote alongside its `cite: xerj://...` handle into Markdown and run `claimcite check --xerj-url http://127.0.0.1:9200` to verify every quote byte-for-byte against your local XERJ node.

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

## Verified XERJ Open Ecosystem

`kbwall` sits at the read-only retrieval firewall layer of an end-to-end, local-first open stack:

| Stage | Project / Model | Role |
| :--- | :--- | :--- |
| **1. Ingest** | **[`wasabihu/sessfork`](https://github.com/wasabihu/sessfork)** | Zero-Tokio Rust MCP (`sessfork-mcp`) & authenticated web ingestion into local XERJ without foreground focus theft. |
| **1b. Filings** | **[`wasabihu/AKReport`](https://github.com/wasabihu/AKReport)** | Batch-download A-share/HKEX annual report PDFs directly into a private local `kbwall` source. |
| **2. Search Engine** | **[`xerj-org/xerj`](https://github.com/xerj-org/xerj)** | Single-binary Rust search engine powering indexing, BM25, HNSW/kNN, and hybrid fusion underneath `kbwall`. |
| **2b. Open Models (HF)** | **[`wasabihu/xerj-bilingual-fin-law-384`](https://huggingface.co/wasabihu/xerj-bilingual-fin-law-384)** (MIT)<br>**[`wasabihu/VeriCorpus-Reranker-0.6B`](https://huggingface.co/wasabihu/VeriCorpus-Reranker-0.6B)** (Apache-2.0) | Bilingual finance/law 384-d embeddings and clean-room 0.6B cross-encoder reranker for local neural/hybrid retrieval. |
| **3. Read-Only Firewall** | **`wasabihu/kbwall`** *(this repo)* | ~1 MB air-gapped, read-only Rust MCP firewall enforcing secret confinement, hybrid filter pushdown, and canonical `xerj://` evidence handles. |
| **4. Verification & Eval** | **[`wasabihu/claimcite`](https://github.com/wasabihu/claimcite)**<br>**[`wasabihu/vericorpus`](https://github.com/wasabihu/vericorpus)** | Zero-dependency citation & `xerj://` UTF-8 byte-span verifier + audit-grade statutory/financial/clinical corpora & CJK retrieval benchmarks. |

---

## Security & Attribution

- See [SECURITY.md](SECURITY.md) for the full threat model and security invariants.
- Embedded offline CJK Simplified↔Traditional and Pinyin lookup tables (`src/data/`) are derived from Unicode CLDR/Unihan data (Unicode-3.0 permissive license).
- Independent community project; not affiliated with or endorsed by The XERJ Authors or Elastic.

## License

[MIT](LICENSE)
