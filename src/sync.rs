use crate::cjk::is_cjk_ideograph;
use crate::config::{home_dir, safe_macos_user_home, KbConfig, SourceConfig};
use crate::http_loopback::ReadOnlyHttpClient;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::env;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const MAX_CHILD_OUTPUT_BYTES: u64 = 2 * 1024 * 1024;

pub(crate) fn find_xerj_binary(cfg: &KbConfig) -> Result<PathBuf, String> {
    if let Some(ref p) = cfg.xerj_bin {
        if p.is_file() {
            return Ok(p.clone());
        }
        return Err(format!(
            "Configured engine.xerj_bin not found: {}",
            p.display()
        ));
    }
    if let Ok(path_var) = env::var("PATH") {
        for dir in env::split_paths(&path_var) {
            let cand = dir.join("xerj");
            if cand.is_file() {
                return Ok(cand);
            }
        }
    }
    let home = home_dir();
    let mut cands = vec![
        home.join(".local/bin/xerj"),
        PathBuf::from("/opt/homebrew/bin/xerj"),
        PathBuf::from("/usr/local/bin/xerj"),
    ];
    if let Some(mac_home) = safe_macos_user_home() {
        cands.push(mac_home.join(".local/bin/xerj"));
    }
    for c in cands {
        if c.is_file() {
            return Ok(c);
        }
    }
    Err("Could not find `xerj` binary. Install via `curl -fsSL https://xerj.org/get | sh` or set engine.xerj_bin in kb.toml.".to_string())
}

fn parse_xerj_done(stdout: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("xerj-done ") {
            for tok in rest.split_whitespace() {
                if let Some((k, v)) = tok.split_once('=') {
                    map.insert(k.to_string(), v.to_string());
                }
            }
        }
    }
    map
}

struct CommandOutcome {
    code: i32,
    timed_out: bool,
    stdout: String,
    stderr: String,
}

fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> Result<CommandOutcome, String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn xerj autoindex: {e}"))?;

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let out_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(r) = stdout_pipe {
            drop(r.take(MAX_CHILD_OUTPUT_BYTES).read_to_end(&mut buf));
        }
        String::from_utf8_lossy(&buf).to_string()
    });
    let err_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(r) = stderr_pipe {
            drop(r.take(MAX_CHILD_OUTPUT_BYTES).read_to_end(&mut buf));
        }
        String::from_utf8_lossy(&buf).to_string()
    });

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let exit_code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code().unwrap_or(-1),
            Ok(None) => {
                if Instant::now() >= deadline {
                    timed_out = true;
                    drop(child.kill());
                    drop(child.wait());
                    break 124;
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(e) => {
                drop(child.kill());
                drop(child.wait());
                return Err(format!("Failed waiting for xerj autoindex: {e}"));
            }
        }
    };

    // On timeout, detach pipe reader threads rather than blocking indefinitely if a
    // grandchild inherited stdout/stderr descriptors.
    let (stdout, stderr) = if timed_out {
        (String::new(), String::new())
    } else {
        (
            out_handle.join().unwrap_or_default(),
            err_handle.join().unwrap_or_default(),
        )
    };
    Ok(CommandOutcome {
        code: exit_code,
        timed_out,
        stdout,
        stderr,
    })
}

pub(crate) fn sync_source(
    cfg: &KbConfig,
    client: &ReadOnlyHttpClient,
    src: &SourceConfig,
    dry_run: bool,
) -> Value {
    let canonical_path = match fs::canonicalize(&src.path) {
        Ok(p) if p.is_dir() => p,
        Ok(p) => {
            return json!({
                "ok": false,
                "target": src.key,
                "prefix": src.prefix,
                "error": format!("Source path is not a directory: {}", p.display())
            });
        }
        Err(e) => {
            return json!({
                "ok": false,
                "target": src.key,
                "prefix": src.prefix,
                "error": format!("Directory does not exist or cannot be accessed ({}): {e}", src.path.display())
            });
        }
    };
    if canonical_path.to_string_lossy().starts_with('-') {
        return json!({
            "ok": false,
            "target": src.key,
            "prefix": src.prefix,
            "error": format!("Refusing source path starting with '-': {}", canonical_path.display())
        });
    }
    let binary = match find_xerj_binary(cfg) {
        Ok(b) => b,
        Err(e) => {
            return json!({ "ok": false, "target": src.key, "prefix": src.prefix, "error": e });
        }
    };
    if !client.health_check(Duration::from_secs(2)) {
        return json!({
            "ok": false,
            "target": src.key,
            "prefix": src.prefix,
            "error": format!("Search engine is offline at {}", client.url())
        });
    }

    let mut seen = HashSet::new();
    let mut excludes = Vec::new();
    for pat in cfg.global_excludes.iter().chain(src.exclude.iter()) {
        if seen.insert(pat.clone()) {
            excludes.push(pat.clone());
        }
    }

    let timeout = Duration::from_secs(src.sync_timeout_secs);
    let run_once = |fresh: bool| -> Result<CommandOutcome, String> {
        let mut cmd = Command::new(&binary);
        cmd.arg("autoindex")
            .arg(&canonical_path)
            .arg("--url")
            .arg(client.url())
            .arg("--prefix")
            .arg(&src.prefix)
            .arg("--no-graph")
            .arg("--quiet")
            .arg("--yes")
            .arg("--disable-feedback")
            .env("XERJ_DISABLE_FEEDBACK", "true");
        if let Some(ref plan) = src.plan {
            cmd.arg("--plan").arg(plan);
        }
        if let Some(mb) = src.max_file_mb {
            cmd.arg("--max-file-mb").arg(mb.to_string());
        }
        for pat in &excludes {
            cmd.arg("--exclude").arg(pat);
        }
        if dry_run {
            cmd.arg("--dry-run");
        }
        if fresh {
            cmd.arg("--fresh");
        }
        run_with_timeout(&mut cmd, timeout)
    };

    let timeout_json = || {
        json!({
            "ok": false,
            "target": src.key,
            "name": src.name,
            "prefix": src.prefix,
            "group": src.group,
            "exit_code": 124,
            "reason": "timeout",
            "error": format!("autoindex timed out after {}s", src.sync_timeout_secs)
        })
    };

    let mut outcome = match run_once(false) {
        Ok(o) => o,
        Err(e) => {
            return json!({ "ok": false, "target": src.key, "prefix": src.prefix, "error": e });
        }
    };
    if outcome.timed_out {
        return timeout_json();
    }

    let mut done = parse_xerj_done(&outcome.stdout);
    if outcome.code == 3 && done.get("reason").map(String::as_str) == Some("stale-resume-plan") {
        match run_once(true) {
            Ok(o2) => {
                outcome = o2;
                if outcome.timed_out {
                    return timeout_json();
                }
                done = parse_xerj_done(&outcome.stdout);
            }
            Err(e) => {
                return json!({ "ok": false, "target": src.key, "prefix": src.prefix, "error": e });
            }
        }
    }

    let ok = outcome.code == 0
        || (outcome.code == 3 && done.get("ok").map(String::as_str) != Some("false"));
    let err_tail = if ok {
        String::new()
    } else {
        let raw = if outcome.stderr.trim().is_empty() {
            outcome.stdout.trim()
        } else {
            outcome.stderr.trim()
        };
        raw.chars()
            .rev()
            .take(400)
            .collect::<String>()
            .chars()
            .rev()
            .collect()
    };

    json!({
        "ok": ok,
        "target": src.key,
        "name": src.name,
        "prefix": src.prefix,
        "group": src.group,
        "exit_code": outcome.code,
        "reason": done.get("reason").cloned().unwrap_or_default(),
        "summary": done,
        "error": err_tail
    })
}

/// Refresh the optional local CJK homophone vocabulary file from indexed document titles.
pub(crate) fn refresh_vocab_cache(cfg: &KbConfig, client: &ReadOnlyHttpClient) {
    let Some(ref vocab_path) = cfg.vocab_file else {
        return;
    };
    let prefixes: Vec<String> = cfg
        .sources
        .iter()
        .map(|s| format!("{}-*", s.prefix))
        .collect();
    if prefixes.is_empty() {
        return;
    }
    let path = format!(
        "/{}/_search?strict_unknown_fields=false&ignore_unavailable=true",
        prefixes.join(",")
    );
    let body = json!({
        "size": 400,
        "_source": ["title"],
        "query": { "match_all": {} }
    });
    let Ok(resp) = client.post_search_json(&path, &body, Duration::from_secs(10)) else {
        return;
    };
    let mut terms: BTreeSet<String> = BTreeSet::new();
    if let Some(hits) = resp.pointer("/hits/hits").and_then(Value::as_array) {
        for h in hits {
            if let Some(title) = h.pointer("/_source/title").and_then(Value::as_str) {
                let mut cur = String::new();
                for ch in title.chars() {
                    if is_cjk_ideograph(ch) {
                        cur.push(ch);
                    } else {
                        let len = cur.chars().count();
                        if (2..=5).contains(&len) {
                            terms.insert(cur.clone());
                        }
                        cur.clear();
                    }
                }
                let len = cur.chars().count();
                if (2..=5).contains(&len) {
                    terms.insert(cur);
                }
            }
        }
    }
    if terms.is_empty() {
        return;
    }
    if let Some(parent) = vocab_path.parent() {
        drop(fs::create_dir_all(parent));
    }
    let list: Vec<String> = terms.into_iter().take(2000).collect();
    if let Ok(encoded) = serde_json::to_string_pretty(&list) {
        let file_name = vocab_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("vocab.json");
        let tmp_path =
            vocab_path.with_file_name(format!(".{file_name}.tmp.{}", std::process::id()));
        if fs::write(&tmp_path, encoded).is_ok() && fs::rename(&tmp_path, vocab_path).is_err() {
            drop(fs::remove_file(&tmp_path));
        }
    }
}
