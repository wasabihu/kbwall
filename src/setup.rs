use crate::config::{home_dir, starter_config_template};
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn atomic_write_file(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directory '{}': {e}", parent.display()))?;
    }
    let existing_perms = fs::metadata(path).ok().map(|m| m.permissions());
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("config");
    let tmp_path = path.with_file_name(format!(".{file_name}.tmp.{}", std::process::id()));
    fs::write(&tmp_path, content)
        .map_err(|e| format!("Failed to write temp file '{}': {e}", tmp_path.display()))?;
    if let Some(perms) = existing_perms {
        drop(fs::set_permissions(&tmp_path, perms));
    }
    if let Err(e) = fs::rename(&tmp_path, path) {
        drop(fs::remove_file(&tmp_path));
        return Err(format!(
            "Failed to atomically replace '{}': {e}",
            path.display()
        ));
    }
    Ok(())
}

pub(crate) fn init_config(path: &Path, force: bool) -> Result<String, String> {
    if path.exists() && !force {
        return Err(format!(
            "Config file '{}' already exists (pass --force to overwrite)",
            path.display()
        ));
    }
    atomic_write_file(path, starter_config_template())?;
    Ok(format!("Wrote starter kbwall config -> {}", path.display()))
}

pub(crate) fn resolve_self_binary() -> PathBuf {
    if let Ok(cur) = env::current_exe() {
        if cur.is_file() {
            return cur;
        }
    }
    home_dir().join(".local/bin/kbwall")
}

pub(crate) fn wire_mcp_config_file(
    mcp_json_path: &Path,
    server_name: &str,
    kbwall_bin: &Path,
    config_path: &Path,
) -> Result<String, String> {
    let mut root: Value = if mcp_json_path.is_file() {
        let raw = fs::read_to_string(mcp_json_path)
            .map_err(|e| format!("Failed to read '{}': {e}", mcp_json_path.display()))?;
        serde_json::from_str(&raw)
            .map_err(|e| format!("Invalid JSON in '{}': {e}", mcp_json_path.display()))?
    } else {
        json!({})
    };

    if !root.is_object() {
        return Err(format!(
            "Expected JSON object in '{}'",
            mcp_json_path.display()
        ));
    }

    let entry = json!({
        "command": kbwall_bin.display().to_string(),
        "args": ["mcp", "--config", config_path.display().to_string()],
        "env": {
            "XERJ_DISABLE_FEEDBACK": "true"
        }
    });

    if let Some(obj) = root.as_object_mut() {
        let servers = obj.entry("mcpServers").or_insert_with(|| json!({}));
        if let Some(srv_map) = servers.as_object_mut() {
            srv_map.insert(server_name.to_string(), entry);
        } else {
            return Err(format!(
                "'mcpServers' in '{}' is not an object",
                mcp_json_path.display()
            ));
        }
    }

    let rendered = serde_json::to_string_pretty(&root)
        .map_err(|e| format!("Failed to serialize MCP JSON: {e}"))?;
    atomic_write_file(mcp_json_path, &format!("{rendered}\n"))?;
    Ok(format!(
        "Wired MCP server '{server_name}' -> {}",
        mcp_json_path.display()
    ))
}

pub(crate) fn detect_agent_mcp_configs() -> Vec<(&'static str, PathBuf)> {
    let home = home_dir();
    let candidates = [
        ("antigravity", home.join(".gemini/config/mcp_config.json")),
        ("claude-code", home.join(".claude.json")),
        ("cursor", home.join(".cursor/mcp.json")),
    ];
    candidates
        .into_iter()
        .filter(|(_, p)| p.is_file() || p.parent().is_some_and(Path::is_dir))
        .collect()
}
