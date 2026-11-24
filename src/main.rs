mod cjk;
mod config;
mod http_loopback;
mod mcp;
mod query;
mod setup;
mod sync;

use config::{default_config_path, expand_tilde, KbConfig};
use http_loopback::ReadOnlyHttpClient;
use query::{execute_search, fetch_status, format_search_text, format_status_text, SearchRequest};
use serde_json::json;
use std::env;
use std::path::{Path, PathBuf};
use std::process;

fn print_help() {
    println!(
        "kbwall {} — Air-gapped, read-only Rust MCP firewall for XERJ & ES-compatible local KBs\n\n\
         USAGE:\n\
           kbwall <COMMAND> [OPTIONS]\n\n\
         COMMANDS:\n\
           mcp       Start read-only stdio MCP server (kb_search, kb_status, kb_read)\n\
           search    Search configured knowledge bases from the terminal\n\
           status    Check engine health and indexed passage counts\n\
           read      Read a whitelisted file slice (enforces root & secret exclusions)\n\
           sync      Sync configured sources via local `xerj autoindex` (feedback disabled)\n\
           check     Validate kb.toml security rules and configured source paths\n\
           init      Generate a starter kb.toml configuration file\n\
           setup     Wire `kbwall mcp` into detected AI coding agents\n\n\
         GLOBAL OPTIONS:\n\
           -c, --config <PATH>   Path to kb.toml (default: ~/.config/kbwall/kb.toml)\n\
           -V, --version         Print version\n\
           -h, --help            Print help",
        env!("CARGO_PKG_VERSION")
    );
}

fn extract_config_flag(args: &[String]) -> (PathBuf, Vec<String>) {
    let mut cfg_path: Option<PathBuf> = None;
    let mut rest = Vec::with_capacity(args.len());
    let mut i = 0usize;
    while i < args.len() {
        let Some(arg) = args.get(i) else {
            break;
        };
        if let Some(val) = arg.strip_prefix("--config=") {
            cfg_path = Some(expand_tilde(val));
            i += 1;
        } else if (arg == "--config" || arg == "-c") && i + 1 < args.len() {
            if let Some(next) = args.get(i + 1) {
                cfg_path = Some(expand_tilde(next));
            }
            i += 2;
        } else {
            rest.push(arg.clone());
            i += 1;
        }
    }
    (cfg_path.unwrap_or_else(default_config_path), rest)
}

fn cmd_search(cfg: &KbConfig, args: &[String]) -> i32 {
    let mut query_parts: Vec<String> = Vec::new();
    let mut target: Option<String> = None;
    let mut group: Option<String> = None;
    let mut scope = cfg.default_scope.clone();
    let mut mode = "bm25".to_string();
    let mut limit: usize = 8;
    let mut max_chars: usize = 260;
    let mut include_all = false;
    let mut as_json = false;

    let mut i = 0usize;
    while i < args.len() {
        let Some(a) = args.get(i) else {
            break;
        };
        match a.as_str() {
            "--target" if i + 1 < args.len() => {
                target = args.get(i + 1).cloned();
                i += 2;
            }
            "--group" if i + 1 < args.len() => {
                group = args.get(i + 1).cloned();
                i += 2;
            }
            "--scope" if i + 1 < args.len() => {
                if let Some(v) = args.get(i + 1) {
                    scope.clone_from(v);
                }
                i += 2;
            }
            "--mode" if i + 1 < args.len() => {
                if let Some(v) = args.get(i + 1) {
                    mode.clone_from(v);
                }
                i += 2;
            }
            "--limit" if i + 1 < args.len() => {
                if let Some(v) = args.get(i + 1).and_then(|s| s.parse::<usize>().ok()) {
                    limit = v.clamp(1, 30);
                }
                i += 2;
            }
            "--max-chars" if i + 1 < args.len() => {
                if let Some(v) = args.get(i + 1).and_then(|s| s.parse::<usize>().ok()) {
                    max_chars = v.clamp(60, 4000);
                }
                i += 2;
            }
            "--include-opt-in" | "--all-sources" => {
                include_all = true;
                i += 1;
            }
            "--json" => {
                as_json = true;
                i += 1;
            }
            other => {
                query_parts.push(other.to_string());
                i += 1;
            }
        }
    }

    let query = query_parts.join(" ");
    if query.trim().is_empty() {
        eprintln!("Usage: kbwall search <query> [--scope docs|code|all] [--group <g>] [--target <t>] [--limit N] [--json]");
        return 2;
    }
    let client = ReadOnlyHttpClient::new(cfg.endpoint.clone());
    let include_non_default = include_all || group.is_some() || target.is_some();
    match execute_search(
        cfg,
        &client,
        &SearchRequest {
            query: &query,
            target: target.as_deref(),
            group: group.as_deref(),
            scope: &scope,
            include_non_default,
            limit,
            mode: &mode,
            max_chars,
        },
    ) {
        Ok(resp) => {
            if as_json {
                let hits_json: Vec<_> = resp
                    .hits
                    .iter()
                    .map(|h| {
                        json!({
                            "index": h.index,
                            "target": h.target_key,
                            "target_name": h.target_name,
                            "group": h.group,
                            "format": h.format,
                            "path": h.rel_path,
                            "abs_path": h.abs_path,
                            "title": h.title,
                            "lines": h.lines,
                            "page": h.page,
                            "pdf_pages_total": h.pdf_pages_total,
                            "score": h.score,
                            "snippet": h.snippet,
                        })
                    })
                    .collect();
                let payload = json!({
                    "ok": true,
                    "query": resp.query,
                    "scope": resp.scope,
                    "group": resp.group,
                    "mode_used": resp.mode_used,
                    "took_ms": resp.took_ms,
                    "total": resp.total,
                    "fuzzy_used": resp.fuzzy_used,
                    "anchor_relaxed": resp.anchor_relaxed,
                    "pinyin_corrections": resp.pinyin_corrections.iter().map(|c| json!({"from": c.from, "to": c.to})).collect::<Vec<_>>(),
                    "hits": hits_json,
                });
                if let Ok(s) = serde_json::to_string_pretty(&payload) {
                    println!("{s}");
                }
            } else {
                println!("{}", format_search_text(&resp));
            }
            0
        }
        Err(e) => {
            eprintln!("kbwall search error: {e}");
            1
        }
    }
}

fn cmd_status(cfg: &KbConfig, args: &[String]) -> i32 {
    let as_json = args.iter().any(|a| a == "--json");
    let client = ReadOnlyHttpClient::new(cfg.endpoint.clone());
    let st = fetch_status(cfg, &client);
    if as_json {
        let srcs: Vec<_> = st
            .sources
            .iter()
            .map(|s| {
                json!({
                    "key": s.key,
                    "name": s.name,
                    "prefix": s.prefix,
                    "group": s.group,
                    "group_label": s.group_label,
                    "path": s.path.display().to_string(),
                    "exists": s.exists,
                    "default_included": s.default_included,
                    "docs": s.docs,
                })
            })
            .collect();
        let payload = json!({
            "ok": st.ok,
            "url": st.url,
            "total_docs": st.total_docs,
            "sources": srcs,
        });
        if let Ok(s) = serde_json::to_string_pretty(&payload) {
            println!("{s}");
        }
    } else {
        println!("{}", format_status_text(&st));
    }
    i32::from(!st.ok)
}

fn cmd_read(cfg: &KbConfig, args: &[String]) -> i32 {
    let mut path_arg: Option<String> = None;
    let mut start_line: usize = 1;
    let mut end_line: Option<usize> = None;
    let mut i = 0usize;
    while i < args.len() {
        let Some(a) = args.get(i) else {
            break;
        };
        match a.as_str() {
            "--start" | "--start-line" if i + 1 < args.len() => {
                if let Some(v) = args.get(i + 1).and_then(|s| s.parse::<usize>().ok()) {
                    start_line = v;
                }
                i += 2;
            }
            "--end" | "--end-line" if i + 1 < args.len() => {
                end_line = args.get(i + 1).and_then(|s| s.parse::<usize>().ok());
                i += 2;
            }
            other if path_arg.is_none() => {
                path_arg = Some(other.to_string());
                i += 1;
            }
            _ => i += 1,
        }
    }
    let Some(p) = path_arg else {
        eprintln!("Usage: kbwall read <path> [--start-line N] [--end-line M]");
        return 2;
    };
    match cfg.read_passage(Path::new(&p), start_line, end_line) {
        Ok(out) => {
            print!("{out}");
            0
        }
        Err(e) => {
            eprintln!("kbwall read error: {e}");
            1
        }
    }
}

fn cmd_sync(cfg: &KbConfig, args: &[String]) -> i32 {
    let mut target: Option<String> = None;
    let mut group: Option<String> = None;
    let mut dry_run = false;
    let mut as_json = false;
    let mut i = 0usize;
    while i < args.len() {
        let Some(a) = args.get(i) else {
            break;
        };
        match a.as_str() {
            "--target" if i + 1 < args.len() => {
                target = args.get(i + 1).cloned();
                i += 2;
            }
            "--group" if i + 1 < args.len() => {
                group = args.get(i + 1).cloned();
                i += 2;
            }
            "--dry-run" => {
                dry_run = true;
                i += 1;
            }
            "--json" => {
                as_json = true;
                i += 1;
            }
            _ => i += 1,
        }
    }

    let selected: Vec<_> = cfg
        .sources
        .iter()
        .filter(|s| {
            if let Some(ref t) = target {
                s.key == *t
            } else if let Some(ref g) = group {
                s.group == *g && s.path.is_dir()
            } else {
                s.path.is_dir()
            }
        })
        .collect();
    if selected.is_empty() {
        eprintln!("No matching existing sources found to sync.");
        return 1;
    }

    let client = ReadOnlyHttpClient::new(cfg.endpoint.clone());
    let results: Vec<_> = selected
        .iter()
        .map(|s| sync::sync_source(cfg, &client, s, dry_run))
        .collect();
    let all_ok = results
        .iter()
        .all(|r| r.get("ok").and_then(serde_json::Value::as_bool) == Some(true));
    if !dry_run {
        sync::refresh_vocab_cache(cfg, &client);
    }

    if as_json {
        if let Ok(s) = serde_json::to_string_pretty(&json!({ "ok": all_ok, "results": results })) {
            println!("{s}");
        }
    } else {
        for r in &results {
            let ok = r.get("ok").and_then(serde_json::Value::as_bool) == Some(true);
            let mark = if ok { "OK" } else { "ERR" };
            let t = r
                .get("target")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("-");
            let p = r
                .get("prefix")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("-");
            let detail = r
                .get("error")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .or_else(|| r.get("reason").and_then(serde_json::Value::as_str))
                .unwrap_or("");
            println!("[{mark}] {t} ({p}) {detail}");
        }
    }
    i32::from(!all_ok)
}

fn cmd_check(cfg: &KbConfig) -> i32 {
    let client = ReadOnlyHttpClient::new(cfg.endpoint.clone());
    let healthy = client.health_check(std::time::Duration::from_secs(2));
    println!("Config: {}", cfg.config_path.display());
    println!(
        "Engine: {} ({})",
        cfg.endpoint.raw_url,
        if healthy { "online" } else { "offline" }
    );
    println!(
        "Security: read-only MCP, loopback HTTP (zero TLS), {} global excludes, kb_read={} (max {} lines)",
        cfg.global_excludes.len(),
        cfg.allow_passage_read,
        cfg.max_read_lines
    );
    println!(
        "Query: default_scope={}, cjk_enhance={}, fuzzy_fallback={}, synonym_groups={}",
        cfg.default_scope,
        cfg.enable_cjk_enhance,
        cfg.enable_fuzzy_fallback,
        cfg.synonym_groups.len()
    );
    let existing = cfg.sources.iter().filter(|s| s.path.is_dir()).count();
    println!(
        "Sources: {}/{} directories present",
        existing,
        cfg.sources.len()
    );
    0
}

fn cmd_setup(cfg_path: &Path, args: &[String]) -> i32 {
    let mut server_name = "kbwall".to_string();
    let mut explicit_file: Option<PathBuf> = None;
    let mut i = 0usize;
    while i < args.len() {
        let Some(a) = args.get(i) else {
            break;
        };
        match a.as_str() {
            "--name" if i + 1 < args.len() => {
                if let Some(v) = args.get(i + 1) {
                    server_name.clone_from(v);
                }
                i += 2;
            }
            "--mcp-file" if i + 1 < args.len() => {
                if let Some(v) = args.get(i + 1) {
                    explicit_file = Some(expand_tilde(v));
                }
                i += 2;
            }
            _ => i += 1,
        }
    }
    let self_bin = setup::resolve_self_binary();
    if let Some(target_file) = explicit_file {
        return match setup::wire_mcp_config_file(&target_file, &server_name, &self_bin, cfg_path) {
            Ok(msg) => {
                println!("{msg}");
                0
            }
            Err(e) => {
                eprintln!("kbwall setup error: {e}");
                1
            }
        };
    }

    let targets = setup::detect_agent_mcp_configs();
    if targets.is_empty() {
        println!(
            "No AI agent MCP configs auto-detected. Add manually:\n  command: {}\n  args: [\"mcp\", \"--config\", \"{}\"]",
            self_bin.display(),
            cfg_path.display()
        );
        return 0;
    }
    let mut had_err = false;
    for (agent, path) in targets {
        match setup::wire_mcp_config_file(&path, &server_name, &self_bin, cfg_path) {
            Ok(msg) => println!("[{agent}] {msg}"),
            Err(e) => {
                eprintln!("[{agent}] error: {e}");
                had_err = true;
            }
        }
    }
    i32::from(had_err)
}

fn main() {
    let raw_args: Vec<String> = env::args().skip(1).collect();
    if raw_args.is_empty() || raw_args.iter().any(|a| a == "-h" || a == "--help") {
        print_help();
        return;
    }
    if raw_args.len() == 1
        && raw_args
            .first()
            .is_some_and(|a| a == "-V" || a == "--version")
    {
        println!("kbwall {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let (cfg_path, args) = extract_config_flag(&raw_args);
    let Some(cmd) = args.first().map(String::as_str) else {
        print_help();
        return;
    };
    let sub_args = args.get(1..).unwrap_or(&[]);

    if cmd == "init" {
        let force = sub_args.iter().any(|a| a == "--force");
        match setup::init_config(&cfg_path, force) {
            Ok(msg) => println!("{msg}"),
            Err(e) => {
                eprintln!("kbwall init error: {e}");
                process::exit(1);
            }
        }
        return;
    }

    if cmd == "setup" {
        process::exit(cmd_setup(&cfg_path, sub_args));
    }

    let cfg = match KbConfig::load_from_path(&cfg_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("kbwall config error: {e}");
            process::exit(1);
        }
    };

    let rc = match cmd {
        "mcp" => mcp::run_stdio_server(&cfg),
        "search" => cmd_search(&cfg, sub_args),
        "status" => cmd_status(&cfg, sub_args),
        "read" => cmd_read(&cfg, sub_args),
        "sync" => cmd_sync(&cfg, sub_args),
        "check" => cmd_check(&cfg),
        other => {
            eprintln!("Unknown command '{other}'. Run `kbwall --help`.");
            2
        }
    };
    process::exit(rc);
}
