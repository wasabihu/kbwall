use crate::cjk::{
    cjk_variants, contains_cjk, extract_clause_groups, is_cjk_ideograph, PinyinCorrection,
};
use crate::config::{KbConfig, SourceConfig};
use crate::http_loopback::ReadOnlyHttpClient;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

pub(crate) const DOC_FORMATS: &[&str] = &[
    "md",
    "pdf",
    "docx",
    "pptx",
    "xlsx",
    "epub",
    "txt",
    "txt-prose",
    "html",
];

pub(crate) const CODE_FORMATS: &[&str] = &[
    "code", "py", "ts", "tsx", "js", "jsx", "mjs", "cjs", "rs", "go", "sh", "bash", "zsh", "swift",
    "java", "kt", "c", "cpp", "h", "sql", "yaml", "yml", "toml", "plist", "css",
];

const STOP_PHRASES: &[&str] = &[
    "怎么看",
    "怎么办",
    "怎么样",
    "有没有",
    "有哪些",
    "有什么",
    "是什么",
    "为什么",
    "能不能",
    "帮我找",
    "帮我查",
    "请帮我",
    "查一下",
    "找一下",
    "搜一下",
    "告诉我",
    "关于",
    "里面",
    "其中",
    "有关",
    "方面",
    "如何",
    "怎么",
    "什么",
    "哪些",
];

#[derive(Debug, Clone)]
pub(crate) struct QueryAnalysis {
    pub(crate) raw_chunks: Vec<String>,
    pub(crate) unicode_terms: Vec<String>,
    pub(crate) cjk_words: Vec<String>,
    pub(crate) bigram_groups: Vec<Vec<String>>,
    pub(crate) clause_groups: Vec<Vec<String>>,
    pub(crate) synonym_terms: Vec<String>,
    pub(crate) pinyin_corrections: Vec<PinyinCorrection>,
}

#[derive(Debug, Clone)]
pub(crate) struct SearchHit {
    pub(crate) index: String,
    pub(crate) target_key: String,
    pub(crate) target_name: String,
    pub(crate) group: String,
    pub(crate) format: String,
    pub(crate) rel_path: String,
    pub(crate) abs_path: String,
    pub(crate) title: String,
    pub(crate) lines: String,
    pub(crate) page: Option<u64>,
    pub(crate) pdf_pages_total: Option<u64>,
    pub(crate) score: f64,
    pub(crate) snippet: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SearchResponse {
    pub(crate) query: String,
    pub(crate) scope: String,
    pub(crate) group: String,
    pub(crate) mode_used: String,
    pub(crate) took_ms: u64,
    pub(crate) total: u64,
    pub(crate) fuzzy_used: bool,
    pub(crate) anchor_relaxed: bool,
    pub(crate) pinyin_corrections: Vec<PinyinCorrection>,
    pub(crate) hits: Vec<SearchHit>,
}

#[derive(Debug, Clone)]
pub(crate) struct SearchRequest<'a> {
    pub(crate) query: &'a str,
    pub(crate) target: Option<&'a str>,
    pub(crate) group: Option<&'a str>,
    pub(crate) scope: &'a str,
    pub(crate) include_non_default: bool,
    pub(crate) limit: usize,
    pub(crate) mode: &'a str,
    pub(crate) max_chars: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct SourceStatus {
    pub(crate) key: String,
    pub(crate) name: String,
    pub(crate) prefix: String,
    pub(crate) group: String,
    pub(crate) group_label: String,
    pub(crate) path: PathBuf,
    pub(crate) exists: bool,
    pub(crate) default_included: bool,
    pub(crate) docs: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct EngineStatus {
    pub(crate) ok: bool,
    pub(crate) url: String,
    pub(crate) total_docs: u64,
    pub(crate) sources: Vec<SourceStatus>,
}

const MAX_QUERY_CHARS: usize = 512;
const MAX_QUERY_BYTES: usize = 2048;

fn escape_es_wildcard(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if matches!(ch, '\\' | '*' | '?') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn push_unique(vec: &mut Vec<String>, item: String) {
    if !item.is_empty() && !vec.iter().any(|existing| existing == &item) {
        vec.push(item);
    }
}

fn extract_unicode_words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        let is_word_char =
            !is_cjk_ideograph(ch) && (ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.' | '+'));
        if is_word_char {
            cur.push(ch);
        } else {
            let trimmed = cur.trim_matches(|c| matches!(c, '-' | '.' | '+'));
            if trimmed.chars().count() >= 2 && trimmed.chars().any(char::is_alphanumeric) {
                push_unique(&mut out, trimmed.to_string());
            }
            cur.clear();
        }
    }
    let trimmed = cur.trim_matches(|c| matches!(c, '-' | '.' | '+'));
    if trimmed.chars().count() >= 2 && trimmed.chars().any(char::is_alphanumeric) {
        push_unique(&mut out, trimmed.to_string());
    }
    out
}

fn extract_cjk_spans(text: &str) -> Vec<String> {
    let mut spans = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        if is_cjk_ideograph(ch) {
            cur.push(ch);
        } else if !cur.is_empty() {
            spans.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        spans.push(cur);
    }
    spans
}

fn expand_synonyms(query_lower: &str, terms: &[String], groups: &[Vec<String>]) -> Vec<String> {
    let mut out = Vec::new();
    for group in groups {
        let mut matched: Option<String> = None;
        for cand in group {
            let cl = cand.trim().to_ascii_lowercase();
            if cl.is_empty() {
                continue;
            }
            let hit_in_query = if contains_cjk(&cl) {
                query_lower.contains(&cl)
            } else {
                terms.iter().any(|t| t.eq_ignore_ascii_case(&cl))
            };
            if hit_in_query {
                matched = Some(cl);
                break;
            }
        }
        if let Some(m) = matched {
            for cand in group {
                let trimmed = cand.trim();
                if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case(&m) {
                    push_unique(&mut out, trimmed.to_string());
                }
            }
        }
    }
    out
}

/// Analyze any multilingual query (English, European, Japanese, Korean, Chinese, or mixed).
/// When `enable_cjk_enhance` is true and CJK ideographs are present, automatically adds
/// Simplified/Traditional variants, homophone snapping, statutory clause variants, and bigrams.
pub(crate) fn analyze_query(query: &str, cfg: &KbConfig) -> QueryAnalysis {
    let raw_chunks: Vec<String> = query
        .split_whitespace()
        .map(str::trim)
        .filter(|s| s.chars().count() >= 2)
        .map(ToString::to_string)
        .collect();

    let has_cjk = cfg.enable_cjk_enhance && contains_cjk(query);

    let (clause_groups, cleaned) = if has_cjk {
        let mut stripped = query.to_string();
        for sp in STOP_PHRASES {
            stripped = stripped.replace(sp, " ");
        }
        extract_clause_groups(&stripped)
    } else {
        (Vec::new(), query.to_string())
    };

    let unicode_terms = extract_unicode_words(&cleaned);
    let mut cjk_words = Vec::new();
    let mut bigram_groups: Vec<Vec<String>> = Vec::new();
    let mut pinyin_corrections = Vec::new();

    if has_cjk {
        let mut seen_bg = HashSet::new();
        for span in extract_cjk_spans(&cleaned) {
            let (snapped, corrs, snapped_terms) = cfg.pinyin_vocab.snap_homophones(&span);
            for c in corrs {
                if !pinyin_corrections.contains(&c) {
                    pinyin_corrections.push(c);
                }
            }
            for st in snapped_terms {
                push_unique(&mut cjk_words, st);
            }
            if span.chars().count() >= 2 {
                push_unique(&mut cjk_words, snapped.clone());
                if snapped != span {
                    push_unique(&mut cjk_words, span.clone());
                }
            }

            let chars: Vec<char> = snapped.chars().collect();
            if chars.len() == 1 {
                if let Some(&ch) = chars.first() {
                    let s = ch.to_string();
                    if seen_bg.insert(s.clone()) {
                        bigram_groups.push(cjk_variants(&s));
                    }
                }
            } else if chars.len() >= 2 {
                for pair in chars.windows(2) {
                    let bg: String = pair.iter().collect();
                    if seen_bg.insert(bg.clone()) {
                        bigram_groups.push(cjk_variants(&bg));
                    }
                }
            }
        }
    }

    let query_lower = query.to_ascii_lowercase();
    let synonym_terms = expand_synonyms(&query_lower, &unicode_terms, &cfg.synonym_groups);

    QueryAnalysis {
        raw_chunks,
        unicode_terms,
        cjk_words,
        bigram_groups,
        clause_groups,
        synonym_terms,
        pinyin_corrections,
    }
}

fn unicode_anchor_unit(term: &str, fields: &[&str], fuzzy: bool, docs_only: bool) -> Value {
    let esc = escape_es_wildcard(term);
    let mut should = vec![
        json!({
            "multi_match": {
                "query": term,
                "fields": fields,
                "lenient": true,
                "boost": 3.0
            }
        }),
        json!({ "match_phrase": { "body": { "query": term, "boost": 3.5 } } }),
        json!({ "wildcard": { "title": { "value": format!("*{esc}*"), "case_insensitive": true, "boost": 3.5 } } }),
        json!({ "wildcard": { "ax_path": { "value": format!("*{esc}*"), "case_insensitive": true, "boost": 3.0 } } }),
    ];
    if !docs_only {
        should.push(json!({ "match_phrase": { "text": { "query": term, "boost": 2.5 } } }));
    }
    if fuzzy && term.chars().count() >= 4 && term.chars().all(|c| c.is_alphanumeric() || c == '_') {
        let low = term.to_lowercase();
        let fuzzy_fields: &[&str] = if docs_only {
            &["body", "title"]
        } else {
            &["body", "title", "defs", "text"]
        };
        for f in fuzzy_fields {
            should.push(json!({
                "fuzzy": {
                    (*f): {
                        "value": low,
                        "fuzziness": "AUTO",
                        "prefix_length": 1,
                        "boost": 2.5
                    }
                }
            }));
        }
    }
    json!({
        "bool": {
            "should": should,
            "minimum_should_match": 1
        }
    })
}

fn clause_anchor_unit(variants: &[String], docs_only: bool) -> Value {
    let mut should = Vec::new();
    for v in variants.iter().take(10) {
        let esc = escape_es_wildcard(v);
        should.push(json!({ "match_phrase": { "body": { "query": v, "boost": 4.5 } } }));
        if !docs_only {
            should.push(json!({ "match_phrase": { "text": { "query": v, "boost": 3.5 } } }));
        }
        should.push(json!({ "wildcard": { "title": { "value": format!("*{esc}*"), "case_insensitive": true, "boost": 4.0 } } }));
        should.push(json!({ "wildcard": { "ax_path": { "value": format!("*{esc}*"), "case_insensitive": true, "boost": 3.5 } } }));
    }
    json!({
        "bool": {
            "should": should,
            "minimum_should_match": 1
        }
    })
}

fn bigram_unit(
    variants: &[String],
    text_fields: &[&str],
    for_code: bool,
    docs_only: bool,
) -> Value {
    if variants.len() == 1 && variants.first().is_some_and(|v| v.chars().count() == 1) {
        return json!({
            "multi_match": {
                "query": variants.first().map_or("", String::as_str),
                "fields": text_fields,
                "lenient": true,
                "boost": 1.0
            }
        });
    }
    let mut should = Vec::new();
    let mut wc_added = 0usize;
    for v in variants.iter().take(14) {
        let len = v.chars().count();
        if len == 1 {
            should.push(json!({
                "multi_match": { "query": v, "fields": text_fields, "lenient": true, "boost": 1.0 }
            }));
            continue;
        }
        should.push(json!({ "match_phrase": { "body": { "query": v, "boost": 2.0 } } }));
        if !(for_code || docs_only) {
            should.push(json!({ "match_phrase": { "text": { "query": v, "boost": 1.5 } } }));
        }
        if wc_added < 4 && len <= 8 {
            let esc = escape_es_wildcard(v);
            should.push(json!({ "wildcard": { "title": { "value": format!("*{esc}*"), "case_insensitive": true, "boost": 2.0 } } }));
            should.push(json!({ "wildcard": { "ax_path": { "value": format!("*{esc}*"), "case_insensitive": true, "boost": 1.8 } } }));
            wc_added += 1;
        }
    }
    json!({
        "bool": {
            "should": should,
            "minimum_should_match": 1
        }
    })
}

pub(crate) fn build_search_body(
    query: &str,
    info: &QueryAnalysis,
    limit: usize,
    mode: &str,
    scope: &str,
    require_anchors: bool,
    fuzzy_terms: bool,
) -> Value {
    let bool_clause = if scope == "code" {
        let code_fields = &["defs^4", "symbols.name^3", "code^2", "body", "text"];
        let mut must_clauses: Vec<Value> = Vec::new();
        let mut code_should: Vec<Value> = vec![
            json!({ "multi_match": { "query": query, "fields": code_fields, "type": "most_fields", "lenient": true, "boost": 2.0 } }),
            json!({ "match_phrase": { "defs": { "query": query, "boost": 4.0 } } }),
            json!({ "match_phrase": { "body": { "query": query, "boost": 2.5 } } }),
        ];
        let mut core_units: Vec<Value> = Vec::new();
        for w in info.unicode_terms.iter().take(10) {
            let esc = escape_es_wildcard(w);
            core_units.push(unicode_anchor_unit(w, code_fields, fuzzy_terms, false));
            code_should.push(json!({ "wildcard": { "ax_path": { "value": format!("*{esc}*"), "case_insensitive": true, "boost": 3.5 } } }));
            code_should.push(json!({ "wildcard": { "title": { "value": format!("*{esc}*"), "case_insensitive": true, "boost": 3.0 } } }));
        }
        for bg in info.bigram_groups.iter().take(12) {
            if bg.first().is_some_and(|v| v.chars().count() >= 2) {
                core_units.push(bigram_unit(bg, code_fields, true, false));
            }
        }
        if core_units.is_empty() {
            must_clauses.push(json!({ "multi_match": { "query": query, "fields": code_fields, "lenient": true } }));
        } else {
            let msm = if require_anchors {
                if core_units.len() <= 2 {
                    core_units.len()
                } else {
                    ((core_units.len() as f64) * 0.6).ceil().max(2.0) as usize
                }
            } else if core_units.len() <= 2 {
                1
            } else {
                ((core_units.len() as f64) * 0.45).floor().max(1.0) as usize
            };
            must_clauses
                .push(json!({ "bool": { "should": core_units, "minimum_should_match": msm } }));
        }
        json!({
            "must": must_clauses,
            "should": code_should,
            "filter": [{ "terms": { "ax_format": CODE_FORMATS } }],
            "must_not": [{ "prefix": { "ax_format": "bak-" } }]
        })
    } else {
        let docs_only = scope == "docs";
        let text_fields: &[&str] = if docs_only {
            &["body^2", "title^1.5", "path"]
        } else {
            &["body^2", "text^1.5", "defs^3", "symbols.name^2"]
        };
        let mm_fields: &[&str] = if docs_only {
            &["body", "title^1.5", "path"]
        } else {
            &["body", "defs^2", "text"]
        };
        let mut must_clauses: Vec<Value> = Vec::new();
        let mut should: Vec<Value> = vec![
            json!({ "multi_match": { "query": query, "fields": mm_fields, "type": "most_fields", "lenient": true, "boost": 2.0 } }),
            json!({ "match_phrase": { "body": { "query": query, "boost": 5.0 } } }),
        ];

        let mut anchor_clauses: Vec<Value> = info
            .unicode_terms
            .iter()
            .take(8)
            .map(|w| unicode_anchor_unit(w, text_fields, fuzzy_terms, docs_only))
            .collect();
        for cg in info.clause_groups.iter().take(4) {
            let unit = clause_anchor_unit(cg, docs_only);
            should.push(unit.clone());
            anchor_clauses.push(unit);
        }

        if !anchor_clauses.is_empty() {
            if require_anchors {
                let a_msm = if anchor_clauses.len() <= 1 {
                    1
                } else {
                    anchor_clauses.len().div_ceil(2)
                };
                must_clauses.push(json!({
                    "bool": { "should": anchor_clauses, "minimum_should_match": a_msm }
                }));
            } else {
                should.extend(anchor_clauses);
            }
        }

        if !info.bigram_groups.is_empty() {
            let bg_clauses: Vec<Value> = info
                .bigram_groups
                .iter()
                .take(16)
                .map(|bg| bigram_unit(bg, text_fields, false, docs_only))
                .collect();
            let has_hard_anchor = require_anchors
                && (!info.unicode_terms.is_empty() || !info.clause_groups.is_empty());
            let b_msm = if bg_clauses.len() <= 2 {
                1
            } else {
                let min_floor = if has_hard_anchor { 1 } else { 2 };
                (((bg_clauses.len() as f64) * 0.35).floor() as usize).max(min_floor)
            };
            must_clauses.push(json!({
                "bool": { "should": bg_clauses, "minimum_should_match": b_msm }
            }));
        }

        if must_clauses.is_empty() {
            must_clauses.push(json!({
                "multi_match": { "query": query, "fields": text_fields, "lenient": true }
            }));
        }

        let mut key_phrases: Vec<String> = Vec::new();
        for w in info.cjk_words.iter().chain(info.raw_chunks.iter()) {
            for v in cjk_variants(w) {
                let len = v.chars().count();
                if (2..=24).contains(&len) {
                    push_unique(&mut key_phrases, v);
                }
            }
        }
        for cg in &info.clause_groups {
            for v in cg {
                let len = v.chars().count();
                if (2..=24).contains(&len) {
                    push_unique(&mut key_phrases, v.clone());
                }
            }
        }
        for w in &info.unicode_terms {
            let len = w.chars().count();
            if (2..=24).contains(&len) {
                push_unique(&mut key_phrases, w.clone());
            }
        }
        for sw in &info.synonym_terms {
            let len = sw.chars().count();
            if (2..=32).contains(&len) {
                push_unique(&mut key_phrases, sw.clone());
            }
        }
        for kp in key_phrases.iter().take(24) {
            should.push(json!({ "match_phrase": { "body": { "query": kp, "boost": 4.0 } } }));
            let len = kp.chars().count();
            if len <= 16 && (len >= 3 || kp.chars().next().is_some_and(char::is_alphanumeric)) {
                let esc = escape_es_wildcard(kp);
                should.push(json!({ "wildcard": { "title": { "value": format!("*{esc}*"), "case_insensitive": true, "boost": 4.5 } } }));
                should.push(json!({ "wildcard": { "ax_path": { "value": format!("*{esc}*"), "case_insensitive": true, "boost": 4.0 } } }));
            }
        }

        let mut obj = json!({
            "must": must_clauses,
            "should": should,
            "must_not": [{ "prefix": { "ax_format": "bak-" } }]
        });
        if docs_only {
            if let Some(map) = obj.as_object_mut() {
                map.insert(
                    "filter".to_string(),
                    json!([{ "terms": { "ax_format": DOC_FORMATS } }]),
                );
            }
        }
        obj
    };

    let dsl_query = if mode == "hybrid" {
        json!({
            "hybrid": {
                "queries": [
                    { "query": { "bool": bool_clause }, "weight": 1.0 },
                    { "query": { "semantic": { "field": "body", "query": query, "k": (limit * 2).max(10) } }, "weight": 0.4 }
                ]
            }
        })
    } else {
        json!({ "bool": bool_clause })
    };

    json!({
        "query": dsl_query,
        "size": limit,
        "_source": [
            "ax_path",
            "ax_format",
            "title",
            "start_line",
            "end_line",
            "line",
            "path",
            "page",
            "pdf_pages_total"
        ],
        "fields": ["_passage"]
    })
}

pub(crate) fn resolve_prefixes<'a>(
    cfg: &'a KbConfig,
    target: Option<&str>,
    group: Option<&str>,
    include_non_default: bool,
) -> Result<Vec<&'a SourceConfig>, String> {
    if let Some(t) = target.map(str::trim).filter(|s| !s.is_empty()) {
        let found = cfg.sources.iter().find(|s| s.key == t).ok_or_else(|| {
            let keys: Vec<&str> = cfg.sources.iter().map(|s| s.key.as_str()).collect();
            format!("Unknown target '{t}'. Available: {}", keys.join(", "))
        })?;
        return Ok(vec![found]);
    }
    if let Some(g) = group
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "all")
    {
        let matched: Vec<&SourceConfig> = cfg.sources.iter().filter(|s| s.group == g).collect();
        if matched.is_empty() {
            let gkeys: Vec<&str> = cfg.groups.iter().map(|gd| gd.key.as_str()).collect();
            return Err(format!(
                "Unknown or empty group '{g}'. Available: {}",
                gkeys.join(", ")
            ));
        }
        return Ok(matched);
    }
    Ok(cfg
        .sources
        .iter()
        .filter(|s| include_non_default || s.default_included)
        .collect())
}

fn clean_snippet(raw: &str, max_chars: usize) -> String {
    let mut compact = String::with_capacity(raw.len().min(max_chars + 16));
    let mut prev_ws = true;
    for ch in raw.chars() {
        if ch.is_whitespace() {
            if !prev_ws {
                compact.push(' ');
                prev_ws = true;
            }
        } else {
            compact.push(ch);
            prev_ws = false;
        }
    }
    let trimmed = compact.trim();
    let count = trimmed.chars().count();
    if count <= max_chars {
        return trimmed.to_string();
    }
    let cut: String = trimmed.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{cut}…")
}

fn normalize_hits(raw_resp: &Value, cfg: &KbConfig, max_chars: usize) -> Vec<SearchHit> {
    let Some(arr) = raw_resp.pointer("/hits/hits").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let idx_name = item
            .get("_index")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let src = item.get("_source").unwrap_or(&Value::Null);
        let fallback_title = match src.get("title") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
            _ => String::new(),
        };
        let passage = item
            .pointer("/fields/_passage/0")
            .and_then(|v| v.as_str().or_else(|| v.get("text").and_then(Value::as_str)))
            .unwrap_or(&fallback_title);

        let start = src
            .get("start_line")
            .or_else(|| src.get("line"))
            .and_then(Value::as_u64);
        let end = src.get("end_line").and_then(Value::as_u64).or(start);
        let lines = match (start, end) {
            (Some(s), Some(e)) if s > 0 && e > 0 && s != e => format!("{s}-{e}"),
            (Some(s), _) if s > 0 => s.to_string(),
            _ => String::new(),
        };

        let rel_path = src
            .get("ax_path")
            .or_else(|| src.get("path"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let fmt = src
            .get("ax_format")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        let mut title_str = fallback_title.trim().to_string();
        if fmt == "pdf" && (title_str.is_empty() || title_str.chars().all(|c| c.is_ascii_digit())) {
            if let Some(fname) = std::path::Path::new(&rel_path)
                .file_name()
                .and_then(|s| s.to_str())
            {
                title_str = fname.to_string();
            }
        }

        let page = src.get("page").and_then(Value::as_u64).filter(|&p| p > 0);
        let pdf_pages_total = src
            .get("pdf_pages_total")
            .and_then(Value::as_u64)
            .filter(|&p| p > 0);

        let owner = cfg.match_index_owner(&idx_name);
        // P0-2 firewall guard: never leak excluded secret files or traversal paths via snippets.
        if cfg.is_hit_path_blocked(owner, &rel_path) {
            continue;
        }
        let (target_key, target_name, group, abs_path) = match owner {
            Some(o) => {
                let abs = if rel_path.is_empty() {
                    o.path.display().to_string()
                } else {
                    o.path.join(&rel_path).display().to_string()
                };
                (o.key.clone(), o.name.clone(), o.group.clone(), abs)
            }
            None => (
                String::new(),
                idx_name.clone(),
                String::new(),
                rel_path.clone(),
            ),
        };

        let score =
            (item.get("_score").and_then(Value::as_f64).unwrap_or(0.0) * 1000.0).round() / 1000.0;

        out.push(SearchHit {
            index: idx_name,
            target_key,
            target_name,
            group,
            format: fmt,
            rel_path,
            abs_path,
            title: title_str,
            lines,
            page,
            pdf_pages_total,
            score,
            snippet: clean_snippet(passage, max_chars),
        });
    }
    out
}

pub(crate) fn format_hit_location(hit: &SearchHit) -> String {
    let base = if hit.abs_path.is_empty() {
        &hit.rel_path
    } else {
        &hit.abs_path
    };
    if let Some(p) = hit.page {
        let mut parts = vec![match hit.pdf_pages_total {
            Some(tot) => format!("p.{p}/{tot}"),
            None => format!("p.{p}"),
        }];
        if !hit.lines.is_empty() {
            parts.push(format!("L{}", hit.lines));
        }
        format!("{base} ({})", parts.join(" · "))
    } else if !hit.lines.is_empty() {
        format!("{base}:{}", hit.lines)
    } else {
        base.clone()
    }
}

pub(crate) fn execute_search(
    cfg: &KbConfig,
    client: &ReadOnlyHttpClient,
    req: &SearchRequest<'_>,
) -> Result<SearchResponse, String> {
    let q = req.query.trim();
    if q.is_empty() {
        return Err("Query must not be empty".to_string());
    }
    if q.chars().count() > MAX_QUERY_CHARS || q.len() > MAX_QUERY_BYTES {
        return Err(format!(
            "Query exceeds safety limit (max {MAX_QUERY_CHARS} chars / {MAX_QUERY_BYTES} bytes)"
        ));
    }
    let scope = match req.scope {
        "docs" | "code" | "all" => req.scope,
        _ => cfg.default_scope.as_str(),
    };
    let sources = resolve_prefixes(cfg, req.target, req.group, req.include_non_default)?;
    if sources.is_empty() {
        return Ok(SearchResponse {
            query: q.to_string(),
            scope: scope.to_string(),
            group: req.group.unwrap_or("all").to_string(),
            mode_used: req.mode.to_string(),
            took_ms: 0,
            total: 0,
            fuzzy_used: false,
            anchor_relaxed: false,
            pinyin_corrections: Vec::new(),
            hits: Vec::new(),
        });
    }

    let index_pattern = sources
        .iter()
        .map(|s| format!("{}-*", s.prefix))
        .collect::<Vec<_>>()
        .join(",");
    let search_path =
        format!("/{index_pattern}/_search?strict_unknown_fields=false&ignore_unavailable=true");

    let info = analyze_query(q, cfg);
    let mut mode_used = req.mode.to_string();
    let timeout = Duration::from_secs(30);

    let run = |use_mode: &str, require_anchors: bool, fuzzy: bool| {
        let body = build_search_body(q, &info, req.limit, use_mode, scope, require_anchors, fuzzy);
        client.post_search_json(&search_path, &body, timeout)
    };

    let mut raw = match run(&mode_used, true, false) {
        Ok(v) => v,
        Err(e) if req.mode == "hybrid" && e.status == 400 => {
            mode_used = "bm25_fallback".to_string();
            run("bm25", true, false).map_err(|err| err.to_string())?
        }
        Err(e) => return Err(e.to_string()),
    };

    let mut hits = normalize_hits(&raw, cfg, req.max_chars);
    let has_anchors = !info.unicode_terms.is_empty() || !info.clause_groups.is_empty();
    let has_fuzzy_eligible = cfg.enable_fuzzy_fallback
        && info
            .unicode_terms
            .iter()
            .any(|t| t.chars().count() >= 4 && t.chars().all(|c| c.is_alphanumeric() || c == '_'));
    let active_mode = if mode_used == "bm25_fallback" {
        "bm25"
    } else {
        mode_used.as_str()
    };

    let code_multi_unit =
        scope == "code" && (info.unicode_terms.len() + info.bigram_groups.len()) >= 2;
    let mut anchor_relaxed = false;
    let mut fuzzy_used = false;
    if hits.is_empty()
        && ((scope != "code" && has_anchors && !info.bigram_groups.is_empty()) || code_multi_unit)
    {
        anchor_relaxed = true;
        fuzzy_used = has_fuzzy_eligible;
        if let Ok(r2) = run(active_mode, false, has_fuzzy_eligible) {
            raw = r2;
            hits = normalize_hits(&raw, cfg, req.max_chars);
        }
    } else if hits.is_empty() && has_fuzzy_eligible {
        fuzzy_used = true;
        if let Ok(r2) = run(active_mode, true, true) {
            raw = r2;
            hits = normalize_hits(&raw, cfg, req.max_chars);
        }
    }

    let took_ms = raw.get("took").and_then(Value::as_u64).unwrap_or(0);
    let total = raw
        .pointer("/hits/total/value")
        .and_then(Value::as_u64)
        .unwrap_or(hits.len() as u64);

    Ok(SearchResponse {
        query: q.to_string(),
        scope: scope.to_string(),
        group: req.group.unwrap_or("all").to_string(),
        mode_used,
        took_ms,
        total,
        fuzzy_used,
        anchor_relaxed,
        pinyin_corrections: info.pinyin_corrections,
        hits,
    })
}

pub(crate) fn fetch_status(cfg: &KbConfig, client: &ReadOnlyHttpClient) -> EngineStatus {
    let ok = client.health_check(Duration::from_secs(2));
    let indices: Vec<Value> = if ok {
        client
            .get_json("/_cat/indices?format=json", Duration::from_secs(3))
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let mut docs_by_owner: std::collections::HashMap<&str, u64> =
        std::collections::HashMap::with_capacity(cfg.sources.len());
    for row in &indices {
        let Some(idx_name) = row.get("index").and_then(Value::as_str) else {
            continue;
        };
        let Some(owner) = cfg.match_index_owner(idx_name) else {
            continue;
        };
        let count = row
            .get("docs.count")
            .and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
            })
            .unwrap_or(0);
        let entry = docs_by_owner.entry(owner.key.as_str()).or_insert(0);
        *entry = entry.saturating_add(count);
    }

    let mut total_docs = 0u64;
    let mut sources = Vec::with_capacity(cfg.sources.len());
    for src in &cfg.sources {
        let docs = docs_by_owner.get(src.key.as_str()).copied().unwrap_or(0);
        total_docs = total_docs.saturating_add(docs);
        sources.push(SourceStatus {
            key: src.key.clone(),
            name: src.name.clone(),
            prefix: src.prefix.clone(),
            group: src.group.clone(),
            group_label: cfg.group_label(&src.group).to_string(),
            path: src.path.clone(),
            exists: src.path.is_dir(),
            default_included: src.default_included,
            docs,
        });
    }

    EngineStatus {
        ok,
        url: client.url().to_string(),
        total_docs,
        sources,
    }
}

pub(crate) fn format_search_text(resp: &SearchResponse) -> String {
    if resp.hits.is_empty() {
        return format!(
            "No matches found for '{}' (scope={}, group={}). Try broadening keywords or scope='all'.",
            resp.query, resp.scope, resp.group
        );
    }
    let mut notes = Vec::new();
    if !resp.pinyin_corrections.is_empty() {
        let pairs: Vec<String> = resp
            .pinyin_corrections
            .iter()
            .take(3)
            .map(|c| format!("{}→{}", c.from, c.to))
            .collect();
        notes.push(format!("CJK homophone: {}", pairs.join(", ")));
    }
    if resp.fuzzy_used {
        notes.push("fuzzy fallback".to_string());
    }
    if resp.anchor_relaxed {
        notes.push("relaxed anchors".to_string());
    }
    let note_str = if notes.is_empty() {
        String::new()
    } else {
        format!(" [{}]", notes.join("; "))
    };

    let mut lines = vec![format!(
        "Matched {} total (showing {}, {} ms){}:",
        resp.total,
        resp.hits.len(),
        resp.took_ms,
        note_str
    )];
    for (idx, h) in resp.hits.iter().enumerate() {
        let loc = format_hit_location(h);
        let fmt = if h.format.is_empty() {
            String::new()
        } else {
            format!(" [{}]", h.format)
        };
        lines.push(format!(
            "{}. [{}] {}{} (score={:.3})",
            idx + 1,
            h.target_name,
            h.title,
            fmt,
            h.score
        ));
        lines.push(format!("   path: {loc}"));
        lines.push(format!("   snippet: {}", h.snippet));
    }
    lines.join("\n")
}

pub(crate) fn format_status_text(st: &EngineStatus) -> String {
    let state = if st.ok { "online" } else { "offline" };
    let mut lines = vec![format!(
        "Engine: {} ({}) · Total indexed passages: {}",
        st.url, state, st.total_docs
    )];
    for s in &st.sources {
        if !s.exists && s.docs == 0 {
            continue;
        }
        let flag = if s.default_included { "" } else { " (opt-in)" };
        lines.push(format!(
            "- {} | {} [{}]{} | {} docs | {}",
            s.key,
            s.name,
            s.group_label,
            flag,
            s.docs,
            s.path.display()
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn test_config() -> KbConfig {
        KbConfig::parse_toml(
            r#"
[engine]
url = "http://127.0.0.1:9200"

[query]
pinyin_seeds = ["商誉减值", "归母净利润"]
synonyms = [["revenue", "营业收入", "营收"]]

[[sources]]
key = "docs"
name = "Docs"
path = "/tmp"
prefix = "kb-docs"
group = "notes"
"#,
            Path::new("test.toml"),
        )
        .unwrap()
    }

    #[test]
    fn analyzes_multilingual_unicode_and_cjk_mixed_queries() {
        let cfg = test_config();

        // European Unicode words (French/German accents) + English preserved natively.
        let eu = analyze_query("München résumé Architecture", &cfg);
        assert!(eu.unicode_terms.contains(&"München".to_string()));
        assert!(eu.unicode_terms.contains(&"résumé".to_string()));
        assert!(eu.unicode_terms.contains(&"Architecture".to_string()));
        assert_eq!(eu.bigram_groups.len(), 0);

        // CJK homophone + clause + bilingual synonym expansion.
        let zh = analyze_query("第3条 商誉减直 revenue", &cfg);
        assert_eq!(zh.clause_groups.len(), 1);
        assert!(zh
            .clause_groups
            .first()
            .is_some_and(|g| g.contains(&"第三条".to_string())));
        assert_eq!(zh.pinyin_corrections.len(), 1);
        assert_eq!(
            zh.pinyin_corrections.first().map(|c| c.to.as_str()),
            Some("商誉减值")
        );
        assert!(zh.synonym_terms.contains(&"营业收入".to_string()));
    }

    #[test]
    fn builds_docs_and_code_scope_filters_distinctly() {
        let cfg = test_config();
        let info = analyze_query("handle_jsonrpc_message", &cfg);
        let docs_dsl = build_search_body(
            "handle_jsonrpc_message",
            &info,
            6,
            "bm25",
            "docs",
            true,
            false,
        );
        let code_dsl = build_search_body(
            "handle_jsonrpc_message",
            &info,
            6,
            "bm25",
            "code",
            true,
            false,
        );

        assert!(docs_dsl.to_string().contains("\"md\""));
        assert!(code_dsl.to_string().contains("\"rs\""));
        assert!(code_dsl.to_string().contains("defs^4"));

        // 2-term code query requires both terms strictly before fallback relaxation.
        let two_info = analyze_query("def sync_target", &cfg);
        let strict_code =
            build_search_body("def sync_target", &two_info, 5, "bm25", "code", true, false);
        let relaxed_code =
            build_search_body("def sync_target", &two_info, 5, "bm25", "code", false, true);
        assert_eq!(
            strict_code
                .pointer("/query/bool/must/0/bool/minimum_should_match")
                .and_then(Value::as_u64),
            Some(2)
        );
        assert_eq!(
            relaxed_code
                .pointer("/query/bool/must/0/bool/minimum_should_match")
                .and_then(Value::as_u64),
            Some(1)
        );
    }

    #[test]
    fn escapes_es_wildcard_metacharacters() {
        assert_eq!(escape_es_wildcard(r"a*b?c\d"), r"a\*b\?c\\d");
        let cfg = test_config();
        let info = analyze_query("foo*bar?baz", &cfg);
        let dsl = build_search_body("foo*bar?baz", &info, 5, "bm25", "docs", true, false);
        let s = dsl.to_string();
        assert!(!s.contains("\"*foo*bar?baz*\""));
    }

    #[test]
    fn normalize_hits_drops_secret_and_traversal_paths() {
        let cfg = test_config();
        let raw = json!({
            "hits": {
                "hits": [
                    {
                        "_index": "kb-docs-v1",
                        "_score": 10.0,
                        "_source": { "ax_path": ".env.production", "ax_format": "txt", "title": "env" },
                        "fields": { "_passage": ["API_KEY=leaked"] }
                    },
                    {
                        "_index": "kb-docs-v1",
                        "_score": 9.0,
                        "_source": { "ax_path": "../../etc/passwd", "ax_format": "txt", "title": "passwd" },
                        "fields": { "_passage": ["root:x:0:0"] }
                    },
                    {
                        "_index": "kb-docs-v1",
                        "_score": 8.0,
                        "_source": { "ax_path": "notes/readme.md", "ax_format": "md", "title": "Readme" },
                        "fields": { "_passage": ["Safe content"] }
                    }
                ]
            }
        });
        let hits = normalize_hits(&raw, &cfg, 200);
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits.first().map(|h| h.rel_path.as_str()),
            Some("notes/readme.md")
        );
    }

    #[test]
    fn execute_search_rejects_oversized_query() {
        let cfg = test_config();
        let client = ReadOnlyHttpClient::new(cfg.endpoint.clone());
        let huge = "a".repeat(MAX_QUERY_CHARS + 1);
        let err = execute_search(
            &cfg,
            &client,
            &SearchRequest {
                query: &huge,
                target: None,
                group: None,
                scope: "docs",
                include_non_default: false,
                limit: 5,
                mode: "bm25",
                max_chars: 200,
            },
        )
        .unwrap_err();
        assert!(err.contains("safety limit"));
    }
}
