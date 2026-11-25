# kbwall

[![CI](https://github.com/wasabihu/kbwall/actions/workflows/ci.yml/badge.svg)](https://github.com/wasabihu/kbwall/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Zero Unsafe](https://img.shields.io/badge/unsafe-forbid-22c55e.svg)](Cargo.toml)
[![Zero TLS / Air-Gapped](https://img.shields.io/badge/network-loopback%20only-0ea5e9.svg)](SECURITY.md)
[![wasabihu/kbwall MCP server](https://glama.ai/mcp/servers/wasabihu/kbwall/badges/score.svg)](https://glama.ai/mcp/servers/wasabihu/kbwall)

[English](README.md) · **简体中文**

**面向 [`xerj`](https://github.com/xerj-org/xerj) 与 Elasticsearch 兼容本地知识库的架构级断网、纯只读 Rust MCP 防火墙 —— 约 1 MB 单二进制、零 Tokio、零 TLS、全语种开箱即用，内置全字库离线中文增强。**

---

## 为什么做 `kbwall`？

[`xerj-org/xerj`](https://github.com/xerj-org/xerj) 能用单个二进制极速索引本地 Markdown、PDF、Office 与代码库；它内置的官方完整 MCP 面向全能读写开发工作流（含索引增删、语料反馈与自动提 PR）。

当你把 Claude Code、Cursor、Codex 或 Antigravity 接入装满**私人笔记、上市公司财报、法务合同与私有代码**的电脑时，往往需要一层**严格最小权限的只读安全边界**：

| 核心保障 | `kbwall` 的硬约束 |
| :--- | :--- |
| **架构级断网（Air-Gapped by Construction）** | **零 TLS 网络库依赖**，默认只允许回环地址（`127.0.0.1` / `localhost` / `::1`）明文 HTTP —— 从编译层面杜绝静默联网、遥测上报或自动提 PR。 |
| **编译级只读 MCP** | 仅暴露 **3 个纯只读工具**（`kb_search`、`kb_status`、可选 `kb_read`）；内部 HTTP 客户端只实现 `GET` 与 `POST .../_search`，无任何 `PUT`/`DELETE`/`/_bulk` 写入能力。 |
| **不可弱化的密钥排除表** | 在 `kbwall sync` 与 `kb_read` 两端强制拦截 `.env`、`.env.*`、`*.key`、`*.pem`、`*secret*`、`*credential*`、`*token*.json`、SSH 私钥与 `*.sqlite*`，并对路径做规范化解析防软链逃逸。 |
| **全语种通用 + 全字库离线中文增强** | 英文及各类 Unicode 外文原生走词元锚点 + 拼写容错；遇到中文或中英混排时自动激活内置 88 KB 离线字表（覆盖 **全部 20,992 个 BMP 汉字 `U+4E00..=U+9FFF`**），实现**繁简双向归一、词表同音拼音纠偏、法条/章节数字变体（`第3条` ↔ `第三条`）**，macOS / Linux / Windows 三端完全一致。 |
| **多知识库分层降噪** | 用一份 `kb.toml` 统一管理几十个目录，支持领域 `group`、`docs / code / all` 作用域隔离，以及 `default_included = false`（如收件箱/聊天记录默认不干扰常规检索）。 |
| **极速极轻** | 约 1.0 MB Release 单二进制（`#![forbid(unsafe_code)]`、零 Tokio），实测 MCP 冷启动 `initialize` 约 **`3.7 ms`**。 |

---

## 60 秒快速上手

### 1. 安装底层引擎 `xerj` 与 `kbwall`

```bash
# 安装并后台启动本机 XERJ（若尚未安装）
curl -fsSL https://xerj.org/get | sh
xerj --insecure --detach

# 方式 A（推荐）：一键安装 ~1 MB 预编译单二进制（macOS / Linux，自动校验 SHA-256，无需安装 Rust）
curl -fsSL https://raw.githubusercontent.com/wasabihu/kbwall/main/install.sh | sh
# （Windows 用户可直接到 GitHub Releases 下载 kbwall-x86_64-pc-windows-msvc.zip）

# 方式 B：从源码编译安装（Rust >= 1.88）
cargo install --git https://github.com/wasabihu/kbwall
```

### 2. 生成并配置 `~/.config/kbwall/kb.toml`

```bash
kbwall init
# 编辑 ~/.config/kbwall/kb.toml，在 [[sources]] 下添加你的本地知识库目录
kbwall check
kbwall sync
```

### 3. 一键接入你的 AI 编程工具

```bash
# 自动检测并写入 Antigravity / Claude Code / Cursor 配置
kbwall setup

# 或直接用 CLI 接入 Claude Code / Codex
claude mcp add kbwall -- ~/.local/bin/kbwall mcp
codex mcp add kbwall -- ~/.local/bin/kbwall mcp
```

---

## 暴露的 3 个纯只读 MCP 工具

1. **`kb_search`**：跨配置知识库检索，支持 `query`、`scope`（`docs` / `code` / `all`）、`group`、`target`、`limit`（`1..=15`）与 `mode`（`bm25` / `hybrid`，所有过滤条件均下推至 `hybrid` 各子分支，无向量时自动回退 BM25），返回精确路径、起止行号、PDF 页码（如 `p.211/257`）、标准 `xerj://` 证据句柄（`cite`）与摘录。
2. **`kb_status`**：查看本机引擎在线状态、总索引片段数、领域分组及各分库路径。
3. **`kb_read`**（可通过 `security.allow_passage_read = false` 关闭）：按白名单只读查看命中文件的上下文片段（默认单次最多 80 行），强制拦截目录外路径、符号链接逃逸、二进制文件与敏感密钥文件。

---

## 可核验 `xerj://` 证据句柄（零幻觉引用闭环）

每条合规的 `kb_search` 命中结果均会输出与 [`wasabihu/claimcite`](https://github.com/wasabihu/claimcite) 逐字节对齐的无宿主证据句柄：

```text
xerj://<percent_encoded_index>/<percent_encoded_doc_id>#b=<byte_start>-<byte_end>&f=<field>
```

- **不含 `host:port`**：引用天然跨机可迁移，不在文档与报告中暴露内网拓扑。
- **严格 UTF-8 字节边界**：优先复用 XERJ 核验过的 `_passage` 字节偏移，混合检索或无 `_passage` 命中时自动对齐到合法 UTF-8 字符边界。
- **本地闭环验真**：在 Markdown 中引用原文与 `cite: xerj://...`，运行 `claimcite check --xerj-url http://127.0.0.1:9200` 即可对每一处引文完成字节级核验。

---

## Verified XERJ Open Ecosystem（已验证开源生态链）

| 环节 | 项目 / 模型 | 职责 |
| :--- | :--- | :--- |
| **1. 采集入库** | **[`wasabihu/sessfork`](https://github.com/wasabihu/sessfork)** | 零 Tokio Rust MCP（`sessfork-mcp`）与免抢焦点已登录网页安全采集入库。 |
| **1b. 财报源** | **[`wasabihu/AKReport`](https://github.com/wasabihu/AKReport)** | 批量下载 A 股/港股历年年报 PDF，直接接入 `kbwall` 私有财报库。 |
| **2. 检索引擎** | **[`xerj-org/xerj`](https://github.com/xerj-org/xerj)** | 单二进制 Rust 搜索引擎，承担全格式索引、BM25、HNSW/kNN 与混合检索。 |
| **2b. 开源模型（HF）** | **[`wasabihu/xerj-bilingual-fin-law-384`](https://huggingface.co/wasabihu/xerj-bilingual-fin-law-384)**（MIT）<br>**[`wasabihu/VeriCorpus-Reranker-0.6B`](https://huggingface.co/wasabihu/VeriCorpus-Reranker-0.6B)**（Apache-2.0） | 中英双语金融/法律 384 维嵌入模型与 0.6B 交叉编码器精排模型。 |
| **3. 只读防火墙** | **`wasabihu/kbwall`** *（本仓）* | ~1 MB 架构级断网只读 Rust MCP 防火墙，执行密钥隔离、混合过滤下推并输出 `xerj://` 证据句柄。 |
| **4. 验真与基准** | **[`wasabihu/claimcite`](https://github.com/wasabihu/claimcite)**<br>**[`wasabihu/vericorpus`](https://github.com/wasabihu/vericorpus)** | 零依赖 `xerj://` 字节跨度引文核查器 + 合规法条/财报/临床语料与中文检索基准。 |

---

## 安全与开源协议

- 完整安全模型与架构红线见 [SECURITY.md](SECURITY.md)。
- 独立社区项目，与 XERJ 官方及 Elastic 无隶属或背书关系。
- 开源协议：[MIT](LICENSE)。
