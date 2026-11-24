use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

const CJK_START: u32 = 0x4E00;
const CJK_END: u32 = 0x9FFF;

const PINYIN_SYLLABLES_RAW: &str = include_str!("data/pinyin_syllables.txt");
const PINYIN_IDX_BYTES: &[u8] = include_bytes!("data/pinyin_idx.bin");
const CJK_S2T_BYTES: &[u8] = include_bytes!("data/cjk_s2t.bin");
const CJK_T2S_BYTES: &[u8] = include_bytes!("data/cjk_t2s.bin");

struct CjkTables {
    syllables: Vec<&'static str>,
    s2t: Vec<(u32, u32)>,
    t2s: Vec<(u32, u32)>,
}

fn parse_pairs(bytes: &[u8]) -> Vec<(u32, u32)> {
    let mut out = Vec::with_capacity(bytes.len() / 8);
    for &[a0, a1, a2, a3, b0, b1, b2, b3] in bytes.as_chunks::<8>().0 {
        let a = u32::from_le_bytes([a0, a1, a2, a3]);
        let b = u32::from_le_bytes([b0, b1, b2, b3]);
        out.push((a, b));
    }
    out
}

fn tables() -> &'static CjkTables {
    static TABLES: OnceLock<CjkTables> = OnceLock::new();
    TABLES.get_or_init(|| CjkTables {
        syllables: PINYIN_SYLLABLES_RAW.split('\n').collect(),
        s2t: parse_pairs(CJK_S2T_BYTES),
        t2s: parse_pairs(CJK_T2S_BYTES),
    })
}

pub(crate) fn is_cjk_ideograph(ch: char) -> bool {
    let cp = ch as u32;
    (CJK_START..=CJK_END).contains(&cp)
}

pub(crate) fn contains_cjk(text: &str) -> bool {
    text.chars().any(is_cjk_ideograph)
}

pub(crate) fn char_pinyin(ch: char) -> Option<&'static str> {
    let cp = ch as u32;
    if !(CJK_START..=CJK_END).contains(&cp) {
        return None;
    }
    let off = ((cp - CJK_START) as usize) * 2;
    let lo = *PINYIN_IDX_BYTES.get(off)?;
    let hi = *PINYIN_IDX_BYTES.get(off + 1)?;
    let idx = usize::from(u16::from_le_bytes([lo, hi]));
    if idx == 0 {
        return None;
    }
    tables()
        .syllables
        .get(idx)
        .copied()
        .filter(|s| !s.is_empty())
}

pub(crate) fn to_pinyin_exact(text: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for ch in text.chars() {
        let py = char_pinyin(ch)?;
        parts.push(py);
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

fn map_chars(text: &str, pairs: &[(u32, u32)]) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        let cp = ch as u32;
        if let Ok(idx) = pairs.binary_search_by_key(&cp, |&(k, _)| k) {
            if let Some(&(_, mapped)) = pairs.get(idx) {
                if let Some(mc) = char::from_u32(mapped) {
                    out.push(mc);
                    continue;
                }
            }
        }
        out.push(ch);
    }
    out
}

pub(crate) fn to_traditional(text: &str) -> String {
    map_chars(text, &tables().s2t)
}

pub(crate) fn to_simplified(text: &str) -> String {
    map_chars(text, &tables().t2s)
}

pub(crate) fn cjk_variants(text: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(3);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return out;
    }
    out.push(trimmed.to_string());
    if contains_cjk(trimmed) {
        let s = to_simplified(trimmed);
        if !out.contains(&s) {
            out.push(s);
        }
        let t = to_traditional(trimmed);
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

const COMMON_SKIP_WORDS: &[&str] = &[
    "机制", "市场", "现在", "方式", "公司", "问题", "方法", "时间", "事件", "实践", "实现", "实例",
    "结果", "原因", "情况", "数据", "内容", "产品", "服务", "系统", "方案", "报告", "文档", "代码",
    "功能", "用户", "客户", "团队", "工作", "管理", "运营", "投资", "研究", "分析", "评估", "测试",
    "配置", "模型", "知识", "资料", "规则", "索引", "检索", "查询", "答案", "标准", "版本",
];

#[derive(Debug, Clone, Default)]
pub(crate) struct PinyinVocab {
    by_pinyin: HashMap<String, Vec<String>>,
    exact: HashSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PinyinCorrection {
    pub(crate) from: String,
    pub(crate) to: String,
}

impl PinyinVocab {
    pub(crate) fn build<'a>(terms: impl IntoIterator<Item = &'a str>) -> Self {
        let skip: HashSet<&str> = COMMON_SKIP_WORDS.iter().copied().collect();
        let mut exact: HashSet<String> = skip.iter().map(|s| (*s).to_string()).collect();
        let mut by_pinyin: HashMap<String, Vec<String>> = HashMap::new();

        for raw in terms {
            let term = raw.trim();
            let char_len = term.chars().count();
            if !(2..=6).contains(&char_len) || !term.chars().all(is_cjk_ideograph) {
                continue;
            }
            if skip.contains(term) {
                continue;
            }
            for v in cjk_variants(term) {
                exact.insert(v);
            }
            let Some(py) = to_pinyin_exact(term) else {
                continue;
            };
            let entry = by_pinyin.entry(py).or_default();
            if !entry.iter().any(|existing| existing == term) {
                entry.push(term.to_string());
            }
        }
        Self { by_pinyin, exact }
    }

    pub(crate) fn snap_homophones(
        &self,
        span: &str,
    ) -> (String, Vec<PinyinCorrection>, Vec<String>) {
        if self.by_pinyin.is_empty() {
            return (span.to_string(), Vec::new(), Vec::new());
        }
        let chars: Vec<char> = span.chars().collect();
        let n = chars.len();
        if n < 2 {
            return (span.to_string(), Vec::new(), Vec::new());
        }

        let mut out = String::with_capacity(span.len());
        let mut corrections = Vec::new();
        let mut snapped_terms = Vec::new();
        let mut i = 0usize;

        while i < n {
            let mut matched = false;
            let max_w = (n - i).min(6);
            for w in (2..=max_w).rev() {
                let seg: String = chars.get(i..i + w).into_iter().flatten().copied().collect();
                if self.exact.contains(&seg) {
                    out.push_str(&seg);
                    i += w;
                    matched = true;
                    break;
                }
                if let Some(py) = to_pinyin_exact(&seg) {
                    if let Some(candidates) = self.by_pinyin.get(&py) {
                        if let Some(best) = candidates.first() {
                            if best != &seg {
                                out.push_str(best);
                                corrections.push(PinyinCorrection {
                                    from: seg,
                                    to: best.clone(),
                                });
                                snapped_terms.push(best.clone());
                                i += w;
                                matched = true;
                                break;
                            }
                        }
                    }
                }
            }
            if !matched {
                if let Some(&ch) = chars.get(i) {
                    out.push(ch);
                }
                i += 1;
            }
        }

        (out, corrections, snapped_terms)
    }
}

/// Convert an integer `1..=9999` to compact Chinese numerals (e.g. `12` -> `十二`, `105` -> `一百零五`).
pub(crate) fn int_to_zh(n: u32) -> Option<String> {
    const DIGITS: [char; 10] = ['零', '一', '二', '三', '四', '五', '六', '七', '八', '九'];
    const UNITS: [Option<char>; 4] = [None, Some('十'), Some('百'), Some('千')];

    if n == 0 || n > 9999 {
        return None;
    }

    let parts = [n / 1000, (n / 100) % 10, (n / 10) % 10, n % 10];
    let mut out = String::new();
    let mut need_zero = false;

    for (idx, &d) in parts.iter().enumerate() {
        let pos = 3 - idx;
        if d == 0 {
            if !out.is_empty() {
                need_zero = true;
            }
            continue;
        }
        if need_zero {
            out.push('零');
            need_zero = false;
        }
        if !(pos == 1 && d == 1 && out.is_empty()) {
            if let Some(&dc) = DIGITS.get(d as usize) {
                out.push(dc);
            }
        }
        if let Some(Some(u)) = UNITS.get(pos) {
            out.push(*u);
        }
    }
    Some(out)
}

/// Parse Chinese numerals (`一`..`九千九百九十九`, including `两`/`〇`) to integer.
pub(crate) fn zh_to_int(text: &str) -> Option<u32> {
    let mut total: u32 = 0;
    let mut cur: u32 = 0;
    let mut seen_any = false;

    for ch in text.chars() {
        let digit = match ch {
            '零' | '〇' => Some(0),
            '一' => Some(1),
            '二' | '两' | '兩' => Some(2),
            '三' => Some(3),
            '四' => Some(4),
            '五' => Some(5),
            '六' => Some(6),
            '七' => Some(7),
            '八' => Some(8),
            '九' => Some(9),
            _ => None,
        };
        if let Some(d) = digit {
            cur = d;
            seen_any = true;
            continue;
        }
        let unit = match ch {
            '十' => Some(10),
            '百' => Some(100),
            '千' => Some(1000),
            _ => None,
        };
        if let Some(u) = unit {
            let factor = if cur == 0 { 1 } else { cur };
            total = total.checked_add(factor.checked_mul(u)?)?;
            cur = 0;
            seen_any = true;
            continue;
        }
        return None;
    }
    if !seen_any {
        return None;
    }
    let val = total.checked_add(cur)?;
    if val == 0 {
        None
    } else {
        Some(val)
    }
}

/// Extract Chinese statutory/section clauses like `第3条` / `第十五章` / `第18C章`
/// into bidirectional Arabic + Chinese + Simplified/Traditional variant groups.
pub(crate) fn extract_clause_groups(query: &str) -> (Vec<Vec<String>>, String) {
    let chars: Vec<char> = query.chars().collect();
    let n = chars.len();
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut cleaned = String::with_capacity(query.len());
    let mut i = 0usize;

    while i < n {
        if chars.get(i) == Some(&'第') {
            let mut j = i + 1;
            let mut num_str = String::new();
            let mut is_ascii_num = false;
            let mut is_zh_num = false;

            while let Some(&c) = chars.get(j) {
                if c.is_ascii_digit() && !is_zh_num {
                    is_ascii_num = true;
                    num_str.push(c);
                    j += 1;
                } else if !is_ascii_num
                    && matches!(
                        c,
                        '零' | '〇'
                            | '一'
                            | '二'
                            | '两'
                            | '兩'
                            | '三'
                            | '四'
                            | '五'
                            | '六'
                            | '七'
                            | '八'
                            | '九'
                            | '十'
                            | '百'
                            | '千'
                    )
                {
                    is_zh_num = true;
                    num_str.push(c);
                    j += 1;
                } else {
                    break;
                }
            }

            let mut suffix_let = String::new();
            if is_ascii_num {
                if let Some(&c) = chars.get(j) {
                    if c.is_ascii_alphabetic() {
                        suffix_let.push(c.to_ascii_uppercase());
                        j += 1;
                    }
                }
            }

            let unit_ch = chars.get(j).copied();
            let is_unit = matches!(
                unit_ch,
                Some('条' | '條' | '章' | '节' | '節' | '款' | '项' | '項')
            );

            if !num_str.is_empty() && is_unit {
                let parsed_num = if is_ascii_num {
                    num_str.parse::<u32>().ok().filter(|&v| v > 0 && v <= 9999)
                } else {
                    zh_to_int(&num_str).filter(|&v| v <= 9999)
                };
                if let (Some(num), Some(unit)) = (parsed_num, unit_ch) {
                    let mut variants = Vec::new();
                    let unit_str = unit.to_string();
                    for u_var in cjk_variants(&unit_str) {
                        let arab = format!("第{num}{suffix_let}{u_var}");
                        if !variants.contains(&arab) {
                            variants.push(arab);
                        }
                        if suffix_let.is_empty() {
                            if let Some(zh) = int_to_zh(num) {
                                let zh_form = format!("第{zh}{u_var}");
                                if !variants.contains(&zh_form) {
                                    variants.push(zh_form);
                                }
                            }
                        }
                    }
                    groups.push(variants);
                    cleaned.push(' ');
                    i = j + 1;
                    continue;
                }
            }
        }
        if let Some(&ch) = chars.get(i) {
            cleaned.push(ch);
        }
        i += 1;
    }

    (groups, cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_pinyin_and_simplified_traditional_offline() {
        assert_eq!(to_pinyin_exact("知识库").as_deref(), Some("zhi shi ku"));
        assert_eq!(
            to_pinyin_exact("商誉减值").as_deref(),
            Some("shang yu jian zhi")
        );
        assert_eq!(to_pinyin_exact("知识abc"), None);
        assert_eq!(to_traditional("简体中文知识库"), "簡體中文知識庫");
        assert_eq!(to_simplified("繁體中文知識庫"), "繁体中文知识库");

        let vars = cjk_variants("知识库");
        assert!(vars.contains(&"知识库".to_string()));
        assert!(vars.contains(&"知識庫".to_string()));
    }

    #[test]
    fn snaps_homophones_while_preserving_common_words() {
        let vocab = PinyinVocab::build(["商誉减值", "净利润", "归母净利润"]);
        let (fixed, corrs, snapped) = vocab.snap_homophones("商誉减直和机制分析");
        assert_eq!(fixed, "商誉减值和机制分析");
        assert_eq!(corrs.len(), 1);
        assert_eq!(corrs.first().map(|c| c.from.as_str()), Some("商誉减直"));
        assert_eq!(corrs.first().map(|c| c.to.as_str()), Some("商誉减值"));
        assert_eq!(snapped, vec!["商誉减值".to_string()]);
    }

    #[test]
    fn extracts_bidirectional_clause_variants() {
        assert_eq!(int_to_zh(12).as_deref(), Some("十二"));
        assert_eq!(int_to_zh(105).as_deref(), Some("一百零五"));
        assert_eq!(zh_to_int("一百零五"), Some(105));
        assert_eq!(zh_to_int("十二"), Some(12));

        let (groups, rest) = extract_clause_groups("公司法第18条与第十五章规定");
        assert_eq!(groups.len(), 2);
        let g0 = groups.first().expect("first clause group");
        let g1 = groups.get(1).expect("second clause group");
        assert!(g0.contains(&"第18条".to_string()));
        assert!(g0.contains(&"第十八条".to_string()));
        assert!(g0.contains(&"第十八條".to_string()));
        assert!(g1.contains(&"第15章".to_string()));
        assert!(g1.contains(&"第十五章".to_string()));
        assert!(rest.contains("公司法"));
    }
}
