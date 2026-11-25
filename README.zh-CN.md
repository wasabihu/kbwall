# kbwall

[![CI](https://github.com/wasabihu/kbwall/actions/workflows/ci.yml/badge.svg)](https://github.com/wasabihu/kbwall/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Zero Unsafe](https://img.shields.io/badge/unsafe-forbid-22c55e.svg)](Cargo.toml)
[![Zero TLS / Air-Gapped](https://img.shields.io/badge/network-loopback%20only-0ea5e9.svg)](SECURITY.md)

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

1. **`kb_search`**：跨配置知识库检索，支持 `query`、`scope`（`docs` / `code` / `all`）、`group`、`target`、`limit`（`1..=15`）与 `mode`（`bm25` / `hybrid`，无向量时自动回退 BM25），返回精确路径、起止行号、PDF 页码（如 `p.211/257`）与摘录。
2. **`kb_status`**：查看本机引擎在线状态、总索引片段数、领域分组及各分库路径。
3. **`kb_read`**（可通过 `security.allow_passage_read = false` 关闭）：按白名单只读查看命中文件的上下文片段（默认单次最多 80 行），强制拦截目录外路径、符号链接逃逸、二进制文件与敏感密钥文件。

---

## 生态联动（最小权限本地 AI 基础设施栈）

- **[`xerj-org/xerj`](https://github.com/xerj-org/xerj)**：底层单二进制 Rust 搜索引擎，负责全格式索引与 BM25/混合检索。
- **[`wasabihu/sessfork`](https://github.com/wasabihu/sessfork)**：姊妹篇零 Tokio Rust MCP（`sessfork-mcp`），负责安全无抢焦点访问外部已登录网页。
- **[`wasabihu/vericorpus`](https://github.com/wasabihu/vericorpus)**：面向 XERJ/ES 的合规多语种法条/财报/临床语料与检索基准。
- **[`wasabihu/AKReport`](https://github.com/wasabihu/AKReport)**：批量下载 A 股/港股历年年报 PDF，配合 `kbwall` 5 行配置秒变 100% 纯本地私有 AI 财报投研库。
- **[`wasabihu/claimcite`](https://github.com/wasabihu/claimcite)**：逐条核查 AI 写作里的公开学术引用是否真实且与原文一致。

---

## 安全与开源协议

- 完整安全模型与架构红线见 [SECURITY.md](SECURITY.md)。
- 独立社区项目，与 XERJ 官方及 Elastic 无隶属或背书关系。
- 开源协议：[MIT](LICENSE)。
