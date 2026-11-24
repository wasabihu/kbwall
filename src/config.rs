use crate::cjk::PinyinVocab;
use crate::http_loopback::LoopbackEndpoint;
use serde::Deserialize;
use std::collections::HashSet;
use std::env;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

const MAX_READ_FILE_BYTES: u64 = 20 * 1024 * 1024; // 20 MiB file cap
const MAX_PASSAGE_OUTPUT_BYTES: usize = 128 * 1024; // 128 KiB MCP response cap

/// Immutable built-in secret & binary exclusion globs that cannot be weakened by user config.
pub(crate) const BUILTIN_SECRET_EXCLUDES: &[&str] = &[
    ".env",
    ".env.*",
    "*.env",
    "*.key",
    "*.pem",
    "*.p12",
    "*.pfx",
    "*.ppk",
    "*secret*",
    "*credential*",
    "*token*.json",
    "*token*.yml",
    "*token*.yaml",
    "*token*.toml",
    "id_rsa*",
    "id_ed25519*",
    "id_ecdsa*",
    "id_dsa*",
    ".npmrc",
    ".netrc",
    ".pgpass",
    "*.sqlite*",
    "*.db",
    "*.db-wal",
    "*.db-shm",
];

pub(crate) const DEFAULT_NOISE_EXCLUDES: &[&str] = &[
    "*.bak*",
    "*.zip",
    "*.tar.gz",
    "node_modules/**",
    "target/**",
    ".git/**",
    "__pycache__/**",
];

#[derive(Debug, Deserialize)]
struct RawConfig {
    #[serde(default)]
    engine: RawEngine,
    #[serde(default)]
    security: RawSecurity,
    #[serde(default)]
    query: RawQuery,
    #[serde(default)]
    groups: Vec<RawGroup>,
    #[serde(default)]
    sources: Vec<RawSource>,
}

#[derive(Debug, Deserialize, Default)]
struct RawEngine {
    url: Option<String>,
    allow_non_loopback: Option<bool>,
    xerj_bin: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct RawSecurity {
    #[serde(default)]
    exclude: Vec<String>,
    allow_passage_read: Option<bool>,
    max_read_lines: Option<usize>,
}

#[derive(Debug, Deserialize, Default)]
struct RawQuery {
    default_scope: Option<String>,
    enable_cjk_enhance: Option<bool>,
    enable_fuzzy_fallback: Option<bool>,
    synonyms_file: Option<String>,
    vocab_file: Option<String>,
    #[serde(default)]
    pinyin_seeds: Vec<String>,
    #[serde(default)]
    synonyms: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct RawGroup {
    key: String,
    label: String,
}

#[derive(Debug, Deserialize)]
struct RawSource {
    key: String,
    name: String,
    path: String,
    prefix: String,
    #[serde(default = "default_group_key")]
    group: String,
    #[serde(default = "default_true")]
    default_included: bool,
    #[serde(default)]
    exclude: Vec<String>,
    plan: Option<String>,
    max_file_mb: Option<u32>,
    sync_timeout: Option<u64>,
}

fn default_group_key() -> String {
    "general".to_string()
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone)]
pub(crate) struct SourceConfig {
    pub(crate) key: String,
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) prefix: String,
    pub(crate) group: String,
    pub(crate) default_included: bool,
    pub(crate) exclude: Vec<String>,
    pub(crate) plan: Option<String>,
    pub(crate) max_file_mb: Option<u32>,
    pub(crate) sync_timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct GroupDef {
    pub(crate) key: String,
    pub(crate) label: String,
}

#[derive(Debug, Clone)]
pub(crate) struct KbConfig {
    pub(crate) config_path: PathBuf,
    pub(crate) endpoint: LoopbackEndpoint,
    pub(crate) xerj_bin: Option<PathBuf>,
    pub(crate) global_excludes: Vec<String>,
    pub(crate) allow_passage_read: bool,
    pub(crate) max_read_lines: usize,
    pub(crate) default_scope: String,
    pub(crate) enable_cjk_enhance: bool,
    pub(crate) enable_fuzzy_fallback: bool,
    pub(crate) synonym_groups: Vec<Vec<String>>,
    pub(crate) pinyin_vocab: PinyinVocab,
    pub(crate) vocab_file: Option<PathBuf>,
    pub(crate) groups: Vec<GroupDef>,
    pub(crate) sources: Vec<SourceConfig>,
}

pub(crate) fn home_dir() -> PathBuf {
    if let Ok(h) = env::var("KBWALL_HOME") {
        let trimmed = h.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(h) = env::var("HOME") {
        let trimmed = h.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(h) = env::var("USERPROFILE") {
        let trimmed = h.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    PathBuf::from(".")
}

/// Safely resolve `/Users/<USER>` on macOS only when `$USER` is a single safe username token.
#[cfg(target_os = "macos")]
pub(crate) fn safe_macos_user_home() -> Option<PathBuf> {
    let raw = env::var("USER").ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.contains("..")
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return None;
    }
    Some(Path::new("/Users").join(trimmed))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn safe_macos_user_home() -> Option<PathBuf> {
    None
}

pub(crate) fn default_config_path() -> PathBuf {
    if let Ok(p) = env::var("KBWALL_CONFIG") {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            return expand_tilde(trimmed);
        }
    }
    let candidate = home_dir().join(".config/kbwall/kb.toml");
    if candidate.is_file() {
        return candidate;
    }
    if let Some(mac_home) = safe_macos_user_home() {
        let mac_fallback = mac_home.join(".config/kbwall/kb.toml");
        if mac_fallback.is_file() {
            return mac_fallback;
        }
    }
    candidate
}

pub(crate) fn expand_tilde(raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    if trimmed == "~" {
        return home_dir();
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    PathBuf::from(trimmed)
}

fn validate_safe_identifier(value: &str, field: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("Config field '{field}' must not be empty"));
    }
    let starts_alnum = trimmed
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric());
    let all_safe = trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !starts_alnum || !all_safe || trimmed.contains("..") {
        return Err(format!(
            "Config field '{field}' has invalid identifier '{trimmed}' (must start with ASCII alphanumeric; allowed: a-z, A-Z, 0-9, '-', '_', '.')"
        ));
    }
    Ok(trimmed.to_string())
}

fn validate_cli_safe_arg(value: &str, field: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("Config field '{field}' must not be empty"));
    }
    if trimmed.starts_with('-') || trimmed.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return Err(format!(
            "Config field '{field}' rejected unsafe value '{trimmed}' (cannot start with '-' or contain control chars)"
        ));
    }
    Ok(trimmed.to_string())
}

#[derive(Debug, Deserialize)]
struct SynonymsFileDoc {
    #[serde(default)]
    groups: Vec<Vec<String>>,
}

fn load_extra_synonyms(path: &Path) -> Result<Vec<Vec<String>>, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read synonyms_file '{}': {e}", path.display()))?;
    let trimmed = content.trim();
    if trimmed.starts_with('[') {
        serde_json::from_str::<Vec<Vec<String>>>(trimmed)
            .map_err(|e| format!("Invalid JSON synonyms_file '{}': {e}", path.display()))
    } else {
        let doc: SynonymsFileDoc = toml::from_str(trimmed)
            .map_err(|e| format!("Invalid TOML synonyms_file '{}': {e}", path.display()))?;
        Ok(doc.groups)
    }
}

fn load_vocab_terms(path: &Path) -> Vec<String> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(&content).unwrap_or_default()
}

impl KbConfig {
    pub(crate) fn load_from_path(path: &Path) -> Result<Self, String> {
        let raw_text = fs::read_to_string(path).map_err(|e| {
            format!(
                "Cannot read config file '{}': {e}. Run `kbwall init` to generate a starter config.",
                path.display()
            )
        })?;
        Self::parse_toml(&raw_text, path)
    }

    pub(crate) fn parse_toml(raw_text: &str, path: &Path) -> Result<Self, String> {
        let raw: RawConfig = toml::from_str(raw_text)
            .map_err(|e| format!("Invalid TOML in '{}': {e}", path.display()))?;

        let url_str = env::var("KBWALL_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or(raw.engine.url)
            .unwrap_or_else(|| "http://127.0.0.1:9200".to_string());
        let allow_non_loopback = raw.engine.allow_non_loopback.unwrap_or(false);
        let endpoint = LoopbackEndpoint::parse(&url_str, allow_non_loopback)?;

        let xerj_bin = env::var("XERJ_BIN")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or(raw.engine.xerj_bin.filter(|s| !s.trim().is_empty()))
            .map(|s| expand_tilde(&s));

        let mut seen_ex = HashSet::new();
        let mut global_excludes = Vec::new();
        for pat in BUILTIN_SECRET_EXCLUDES
            .iter()
            .chain(DEFAULT_NOISE_EXCLUDES.iter())
            .map(|s| (*s).to_string())
        {
            if seen_ex.insert(pat.clone()) {
                global_excludes.push(pat);
            }
        }
        for user_pat in raw.security.exclude {
            let clean = validate_cli_safe_arg(&user_pat, "security.exclude")?;
            if seen_ex.insert(clean.clone()) {
                global_excludes.push(clean);
            }
        }

        let default_scope = match raw.query.default_scope.as_deref().unwrap_or("docs").trim() {
            "docs" => "docs".to_string(),
            "code" => "code".to_string(),
            "all" => "all".to_string(),
            other => {
                return Err(format!(
                    "Invalid query.default_scope '{other}': expected 'docs', 'code', or 'all'"
                ))
            }
        };

        let mut synonym_groups = raw.query.synonyms;
        if let Some(syn_file) = raw
            .query
            .synonyms_file
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let syn_path = expand_tilde(syn_file);
            if syn_path.is_file() {
                synonym_groups.extend(load_extra_synonyms(&syn_path)?);
            }
        }

        let mut seen_keys = HashSet::new();
        let mut sources = Vec::with_capacity(raw.sources.len());
        for src in raw.sources {
            let key = validate_safe_identifier(&src.key, "sources.key")?;
            if !seen_keys.insert(key.clone()) {
                return Err(format!("Duplicate source key '{key}' in config"));
            }
            let prefix = validate_safe_identifier(&src.prefix, "sources.prefix")?;
            let group = validate_safe_identifier(&src.group, "sources.group")?;
            let mut src_excludes = Vec::with_capacity(src.exclude.len());
            for ep in src.exclude {
                src_excludes.push(validate_cli_safe_arg(&ep, "sources.exclude")?);
            }
            let plan = match src.plan.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                Some(p) => Some(validate_cli_safe_arg(p, "sources.plan")?),
                None => None,
            };
            sources.push(SourceConfig {
                key,
                name: src.name.trim().to_string(),
                path: expand_tilde(&src.path),
                prefix,
                group,
                default_included: src.default_included,
                exclude: src_excludes,
                plan,
                max_file_mb: src.max_file_mb,
                sync_timeout_secs: src.sync_timeout.unwrap_or(300).clamp(10, 3600),
            });
        }

        let mut groups: Vec<GroupDef> = raw
            .groups
            .into_iter()
            .map(|g| GroupDef {
                key: g.key.trim().to_string(),
                label: g.label.trim().to_string(),
            })
            .collect();
        for src in &sources {
            if !groups.iter().any(|g| g.key == src.group) {
                groups.push(GroupDef {
                    key: src.group.clone(),
                    label: src.group.clone(),
                });
            }
        }

        let vocab_file = raw
            .query
            .vocab_file
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(expand_tilde);

        let mut vocab_terms: Vec<String> = raw.query.pinyin_seeds;
        for src in &sources {
            for span in extract_cjk_name_spans(&src.name) {
                vocab_terms.push(span);
            }
        }
        if let Some(ref vf) = vocab_file {
            vocab_terms.extend(load_vocab_terms(vf));
        }
        let pinyin_vocab = PinyinVocab::build(vocab_terms.iter().map(String::as_str));

        Ok(Self {
            config_path: path.to_path_buf(),
            endpoint,
            xerj_bin,
            global_excludes,
            allow_passage_read: raw.security.allow_passage_read.unwrap_or(true),
            max_read_lines: raw.security.max_read_lines.unwrap_or(80).clamp(5, 400),
            default_scope,
            enable_cjk_enhance: raw.query.enable_cjk_enhance.unwrap_or(true),
            enable_fuzzy_fallback: raw.query.enable_fuzzy_fallback.unwrap_or(true),
            synonym_groups,
            pinyin_vocab,
            vocab_file,
            groups,
            sources,
        })
    }

    pub(crate) fn group_label<'a>(&'a self, key: &'a str) -> &'a str {
        self.groups
            .iter()
            .find(|g| g.key == key)
            .map_or(key, |g| g.label.as_str())
    }

    pub(crate) fn match_index_owner(&self, index_name: &str) -> Option<&SourceConfig> {
        let mut best: Option<&SourceConfig> = None;
        let mut best_len: usize = 0;
        for src in &self.sources {
            if let Some(rest) = index_name.strip_prefix(&src.prefix) {
                if rest.starts_with('-') && src.prefix.len() >= best_len {
                    best = Some(src);
                    best_len = src.prefix.len();
                }
            }
        }
        best
    }

    /// Check whether a search-hit relative path is excluded by built-in secret rules,
    /// global exclusions, or the owning (or any nested) source's exclusion rules.
    /// Also rejects absolute paths or `..` traversal components (P0-2 firewall guard).
    pub(crate) fn is_hit_path_blocked(&self, owner: Option<&SourceConfig>, rel_path: &str) -> bool {
        let trimmed = rel_path.trim();
        if trimmed.is_empty() {
            return false;
        }
        let normalized = trimmed.replace('\\', "/");
        if normalized.starts_with('/') {
            return true;
        }
        let p = Path::new(&normalized);
        if p.is_absolute()
            || p.components().any(|c| {
                matches!(
                    c,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return true;
        }
        let file_name = normalized.rsplit('/').next().unwrap_or(&normalized);

        for pat in BUILTIN_SECRET_EXCLUDES
            .iter()
            .copied()
            .chain(self.global_excludes.iter().map(String::as_str))
        {
            if path_matches_glob(&normalized, file_name, pat) {
                return true;
            }
        }
        if let Some(src) = owner {
            for pat in &src.exclude {
                if path_matches_glob(&normalized, file_name, pat) {
                    return true;
                }
            }
            // Also check any overlapping parent/child configured sources against the full path.
            let candidate_abs = src.path.join(p);
            for other in &self.sources {
                if other.key == src.key {
                    continue;
                }
                if let Ok(other_rel) = candidate_abs.strip_prefix(&other.path) {
                    let other_rel_str = other_rel.to_string_lossy().replace('\\', "/");
                    for pat in &other.exclude {
                        if path_matches_glob(&other_rel_str, file_name, pat) {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    /// Resolve a requested file path for read-only passage inspection (`kb_read`).
    /// Enforces:
    /// 1. `allow_passage_read` is enabled;
    /// 2. canonical path resides strictly inside at least one configured source root;
    /// 3. ALL matching (parent and nested child) source exclusion rules + built-in/global
    ///    secret exclusions allow the file (defeating nested-source exclusion bypass).
    pub(crate) fn validate_read_path(
        &self,
        requested: &Path,
    ) -> Result<(PathBuf, &SourceConfig), String> {
        if !self.allow_passage_read {
            return Err(
                "Passage reading (kb_read) is disabled by security.allow_passage_read = false"
                    .to_string(),
            );
        }
        let canonical = fs::canonicalize(requested)
            .map_err(|e| format!("Cannot resolve file '{}': {e}", requested.display()))?;

        let mut matching_sources: Vec<(&SourceConfig, PathBuf, usize)> = Vec::new();
        for src in &self.sources {
            let Ok(root_canon) = fs::canonicalize(&src.path) else {
                continue;
            };
            if let Ok(rel) = canonical.strip_prefix(&root_canon) {
                matching_sources.push((src, rel.to_path_buf(), root_canon.as_os_str().len()));
            }
        }
        if matching_sources.is_empty() {
            return Err(format!(
                "Access denied: '{}' is outside all configured kb.toml source directories",
                requested.display()
            ));
        }

        let file_name = canonical
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();

        // Check global/built-in AND every matching source's exclusions (union = deny-wins).
        for (src, rel_path, _) in &matching_sources {
            let rel_str = rel_path.to_string_lossy().replace('\\', "/");
            for pat in BUILTIN_SECRET_EXCLUDES
                .iter()
                .copied()
                .chain(self.global_excludes.iter().map(String::as_str))
                .chain(src.exclude.iter().map(String::as_str))
            {
                if path_matches_glob(&rel_str, &file_name, pat) {
                    return Err(format!(
                        "Access denied: '{rel_str}' matches security exclusion pattern '{pat}'"
                    ));
                }
            }
        }

        // Pick the most-specific (longest canonical root) source for display attribution.
        matching_sources.sort_by_key(|&(_, _, root_len)| std::cmp::Reverse(root_len));
        let owner = matching_sources
            .first()
            .map(|(s, _, _)| *s)
            .ok_or_else(|| "No matching source".to_string())?;
        Ok((canonical, owner))
    }

    pub(crate) fn read_passage(
        &self,
        requested: &Path,
        start_line: usize,
        end_line: Option<usize>,
    ) -> Result<String, String> {
        let (canonical, src) = self.validate_read_path(requested)?;
        let mut opts = OpenOptions::new();
        opts.read(true);
        #[cfg(unix)]
        {
            // O_NOFOLLOW defeats TOCTOU symlink swap; O_NONBLOCK prevents blocking on FIFOs.
            // Using standard POSIX numeric constants keeps `#![forbid(unsafe_code)]` with zero libc dep.
            #[cfg(any(
                target_os = "macos",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd"
            ))]
            const FLAGS: i32 = 0x0100 | 0x0004; // O_NOFOLLOW | O_NONBLOCK on BSD/macOS
            #[cfg(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "x86")))]
            const FLAGS: i32 = 0o0400_000 | 0o0004_000; // O_NOFOLLOW | O_NONBLOCK on Linux x86/x86_64
            #[cfg(all(target_os = "linux", any(target_arch = "aarch64", target_arch = "arm")))]
            const FLAGS: i32 = 0o0100_000 | 0o0004_000; // O_NOFOLLOW | O_NONBLOCK on Linux arm/aarch64
            #[cfg(not(any(
                target_os = "macos",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                all(
                    target_os = "linux",
                    any(
                        target_arch = "x86_64",
                        target_arch = "x86",
                        target_arch = "aarch64",
                        target_arch = "arm"
                    )
                )
            )))]
            const FLAGS: i32 = 0;
            if FLAGS != 0 {
                opts.custom_flags(FLAGS);
            }
        }
        let file = opts
            .open(&canonical)
            .map_err(|e| format!("Failed to open '{}': {e}", canonical.display()))?;
        let meta = file
            .metadata()
            .map_err(|e| format!("Failed to stat '{}': {e}", canonical.display()))?;
        if !meta.is_file() {
            return Err(format!(
                "Requested path '{}' is not a regular file",
                canonical.display()
            ));
        }
        if meta.len() > MAX_READ_FILE_BYTES {
            return Err("Refused to read file larger than 20 MiB".to_string());
        }

        let mut raw_bytes = Vec::new();
        file.take(MAX_READ_FILE_BYTES + 1)
            .read_to_end(&mut raw_bytes)
            .map_err(|e| format!("Failed to read '{}': {e}", canonical.display()))?;
        if raw_bytes.len() as u64 > MAX_READ_FILE_BYTES {
            return Err("Refused to read file larger than 20 MiB".to_string());
        }
        if raw_bytes.iter().take(4096).any(|&b| b == 0) {
            return Err(format!(
                "Refused to read binary file '{}' via text passage reader",
                canonical.display()
            ));
        }

        let text = String::from_utf8_lossy(&raw_bytes);
        let lines: Vec<&str> = text.lines().collect();
        let total_lines = lines.len();
        if total_lines == 0 {
            return Ok(format!(
                "File: {} [{}] (empty file)\n",
                canonical.display(),
                src.name
            ));
        }

        let start = start_line.max(1).min(total_lines);
        let max_end = start.saturating_add(self.max_read_lines.saturating_sub(1));
        let end = end_line
            .unwrap_or(start.saturating_add(39))
            .max(start)
            .min(max_end)
            .min(total_lines);

        let mut out = format!(
            "File: {} [{}] (lines {}-{} of {})\n```\n",
            canonical.display(),
            src.name,
            start,
            end,
            total_lines
        );
        let mut truncated_bytes = false;
        for (offset, line) in lines
            .get((start - 1)..end)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let formatted = format!("{:>5} | {line}\n", start + offset);
            if out.len().saturating_add(formatted.len()) > MAX_PASSAGE_OUTPUT_BYTES {
                truncated_bytes = true;
                break;
            }
            if write!(out, "{formatted}").is_err() {
                break;
            }
        }
        if truncated_bytes {
            out.push_str("... [truncated at 128 KiB output safety cap]\n");
        }
        out.push_str("```\n");
        Ok(out)
    }
}

fn extract_cjk_name_spans(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in name.chars() {
        if crate::cjk::is_cjk_ideograph(ch) {
            cur.push(ch);
        } else {
            let len = cur.chars().count();
            if (2..=6).contains(&len) {
                out.push(cur.clone());
            }
            cur.clear();
        }
    }
    let len = cur.chars().count();
    if (2..=6).contains(&len) {
        out.push(cur);
    }
    out
}

/// Segment-aware Unicode case-insensitive glob matcher supporting `*`, `?`,
/// trailing `/` directories, leading `./`/`/`, and arbitrary `**` (0..N segments).
pub(crate) fn path_matches_glob(rel_path: &str, file_name: &str, pattern: &str) -> bool {
    let mut pat = pattern.trim().replace('\\', "/").to_lowercase();
    while let Some(stripped) = pat.strip_prefix("./") {
        pat = stripped.to_string();
    }
    pat = pat.trim_start_matches('/').to_string();
    if pat.is_empty() {
        return false;
    }
    if pat.ends_with('/') {
        pat.push_str("**");
    }

    let rel_norm = rel_path
        .trim()
        .replace('\\', "/")
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_lowercase();
    let name_norm = file_name.trim().to_lowercase();

    if !pat.contains('/') {
        if simple_glob_match(&pat, &name_norm) {
            return true;
        }
        return rel_norm.split('/').any(|seg| simple_glob_match(&pat, seg));
    }

    let pattern_parts: Vec<&str> = pat.split('/').filter(|s| !s.is_empty()).collect();
    let target_parts: Vec<&str> = rel_norm.split('/').filter(|s| !s.is_empty()).collect();
    if match_segments(&pattern_parts, &target_parts) {
        return true;
    }
    // For directory-glob patterns like `dir/**` or `*private*/**`, also match nested occurrences.
    if pattern_parts.last() == Some(&"**") && pattern_parts.first() != Some(&"**") {
        let mut with_prefix = Vec::with_capacity(pattern_parts.len() + 1);
        with_prefix.push("**");
        with_prefix.extend_from_slice(&pattern_parts);
        return match_segments(&with_prefix, &target_parts);
    }
    false
}

fn match_segments(pat: &[&str], path: &[&str]) -> bool {
    let (m, n) = (pat.len(), path.len());
    let mut dp = vec![vec![false; n + 1]; m + 1];
    if let Some(row0) = dp.get_mut(0) {
        if let Some(cell) = row0.get_mut(0) {
            *cell = true;
        }
    }
    for i in 1..=m {
        if pat.get(i - 1) == Some(&"**") {
            let prev = dp
                .get(i - 1)
                .and_then(|r| r.first().copied())
                .unwrap_or(false);
            if let Some(row) = dp.get_mut(i) {
                if let Some(cell) = row.get_mut(0) {
                    *cell = prev;
                }
            }
        }
    }
    for i in 1..=m {
        let p_seg = pat.get(i - 1).copied().unwrap_or("");
        for j in 1..=n {
            let matched = if p_seg == "**" {
                let zero = dp
                    .get(i - 1)
                    .and_then(|r| r.get(j).copied())
                    .unwrap_or(false);
                let more = dp
                    .get(i)
                    .and_then(|r| r.get(j - 1).copied())
                    .unwrap_or(false);
                zero || more
            } else {
                let t_seg = path.get(j - 1).copied().unwrap_or("");
                let prev = dp
                    .get(i - 1)
                    .and_then(|r| r.get(j - 1).copied())
                    .unwrap_or(false);
                prev && simple_glob_match(p_seg, t_seg)
            };
            if let Some(row) = dp.get_mut(i) {
                if let Some(cell) = row.get_mut(j) {
                    *cell = matched;
                }
            }
        }
    }
    dp.get(m).and_then(|r| r.get(n).copied()).unwrap_or(false)
}

fn simple_glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star_p, mut star_t) = (None, 0usize);

    while ti < t.len() {
        if pi < p.len() && (p.get(pi) == Some(&'?') || p.get(pi) == t.get(ti)) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p.get(pi) == Some(&'*') {
            star_p = Some(pi);
            star_t = ti;
            pi += 1;
        } else if let Some(sp) = star_p {
            pi = sp + 1;
            star_t += 1;
            ti = star_t;
        } else {
            return false;
        }
    }
    while pi < p.len() && p.get(pi) == Some(&'*') {
        pi += 1;
    }
    pi == p.len()
}

pub(crate) fn starter_config_template() -> &'static str {
    r#"# kbwall configuration — Air-gapped Read-Only MCP Firewall for XERJ / Elasticsearch
# Docs: https://github.com/wasabihu/kbwall

[engine]
# Loopback-only by default (127.0.0.1 / localhost / [::1]).
# HTTPS and remote hosts are blocked unless allow_non_loopback = true.
url = "http://127.0.0.1:9200"
# Optional explicit path to local `xerj` binary (auto-detected if empty)
xerj_bin = ""

[security]
# Built-in secret exclusions (.env, *.env, *.key, *.pem, *secret*, *credential*, *token*.json, *.sqlite*)
# are ALWAYS enforced and cannot be weakened. Add any extra exclusions below:
exclude = [
  "archive/**",
  "private/**"
]
allow_passage_read = true
max_read_lines = 80

[query]
default_scope = "docs"          # docs | code | all
enable_cjk_enhance = true       # Auto-activates only on CJK/mixed queries (zero overhead for pure Latin/other scripts)
enable_fuzzy_fallback = true    # Auto-retries with fuzziness=AUTO on 0-hit queries
# Optional external synonyms file (TOML with `groups = [["termA", "termB"]]` or JSON list)
synonyms_file = ""
# Optional local vocabulary JSON for CJK homophone snapping (auto-updated by `kbwall sync`)
vocab_file = "~/.config/kbwall/vocab.json"

[[groups]]
key = "notes"
label = "Notes & Docs"

[[groups]]
key = "research"
label = "Research & PDFs"

[[groups]]
key = "code"
label = "Codebases"

[[sources]]
key = "my_notes"
name = "Personal Notes"
path = "~/Documents/notes"
prefix = "kb-notes"
group = "notes"
default_included = true
exclude = ["drafts/private/**"]
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_nested_sources_and_blocks_secrets_and_flag_injection() {
        let tmp = env::temp_dir().join(format!("kbwall_cfg_test_{}", std::process::id()));
        let parent_dir = tmp.join("docs");
        let child_dir = parent_dir.join("work");
        let payroll_dir = child_dir.join("payroll");
        let outside = tmp.join("outside");
        drop(fs::remove_dir_all(&tmp));
        fs::create_dir_all(&payroll_dir).unwrap();
        fs::create_dir_all(&outside).unwrap();

        let good_doc = child_dir.join("guide.md");
        fs::write(&good_doc, "# Hello\nLine 2\nLine 3\n").unwrap();
        let payroll_doc = payroll_dir.join("salaries.md");
        fs::write(&payroll_doc, "confidential").unwrap();
        let prod_env = parent_dir.join("prod.env");
        fs::write(&prod_env, "API_KEY=topsecret").unwrap();
        let outside_doc = outside.join("leak.txt");
        fs::write(&outside_doc, "outside secret").unwrap();

        let toml_cfg = format!(
            r#"
[engine]
url = "http://127.0.0.1:9200"

[[sources]]
key = "parent"
name = "Parent Docs"
path = "{}"
prefix = "kb-parent"
group = "notes"

[[sources]]
key = "work"
name = "Work Docs"
path = "{}"
prefix = "kb-work"
group = "notes"
exclude = ["payroll/**"]
"#,
            parent_dir.display().to_string().replace('\\', "\\\\"),
            child_dir.display().to_string().replace('\\', "\\\\")
        );
        let cfg = KbConfig::parse_toml(&toml_cfg, Path::new("test.toml")).unwrap();

        let passage = cfg.read_passage(&good_doc, 1, Some(3)).unwrap();
        assert!(passage.contains("# Hello"));
        assert!(passage.contains("Line 3"));

        // Nested child exclusion MUST still block even when parent source appears first!
        let err_nested = cfg.validate_read_path(&payroll_doc).unwrap_err();
        assert!(err_nested.contains("payroll/**"));

        // `*.env` built-in secret exclusion must block `prod.env`!
        let err_env = cfg.validate_read_path(&prod_env).unwrap_err();
        assert!(err_env.contains("exclusion pattern"));

        // Search hit firewall must also block `prod.env`, `payroll/salaries.md`, and `../escape`!
        let parent_src = cfg.sources.first();
        assert!(cfg.is_hit_path_blocked(parent_src, "prod.env"));
        assert!(cfg.is_hit_path_blocked(parent_src, "work/payroll/salaries.md"));
        assert!(cfg.is_hit_path_blocked(parent_src, "/etc/passwd"));
        assert!(cfg.is_hit_path_blocked(parent_src, "../outside/leak.txt"));
        assert!(!cfg.is_hit_path_blocked(parent_src, "work/guide.md"));

        let err_out = cfg.validate_read_path(&outside_doc).unwrap_err();
        assert!(err_out.contains("outside all configured"));

        // Leading `-` in prefix or exclude must be rejected (flag injection guard).
        let bad_prefix = toml_cfg.replace("kb-work", "--evil");
        assert!(KbConfig::parse_toml(&bad_prefix, Path::new("bad.toml")).is_err());

        let bad_excl = toml_cfg.replace("payroll/**", "--delete-all");
        assert!(KbConfig::parse_toml(&bad_excl, Path::new("bad2.toml")).is_err());

        drop(fs::remove_dir_all(&tmp));
    }

    #[test]
    fn segment_glob_handles_arbitrary_double_star_wildcards_and_trailing_slashes() {
        assert!(path_matches_glob(".env.local", ".env.local", ".env.*"));
        assert!(path_matches_glob("prod.env", "prod.env", "*.env"));
        assert!(path_matches_glob(
            "sub/my_secret_key.txt",
            "my_secret_key.txt",
            "*secret*"
        ));
        assert!(path_matches_glob(
            "my-private-dir/a.md",
            "a.md",
            "*private*/**"
        ));
        assert!(path_matches_glob(
            "docs/secret.md",
            "secret.md",
            "docs/**/secret.md"
        ));
        assert!(path_matches_glob(
            "docs/a/b/secret.md",
            "secret.md",
            "docs/**/secret.md"
        ));
        assert!(path_matches_glob("private/notes/a.md", "a.md", "private/"));
        assert!(path_matches_glob(
            "private/notes/a.md",
            "a.md",
            "./private/**"
        ));
        assert!(!path_matches_glob("docs/guide.md", "guide.md", "*.key"));
    }
}
