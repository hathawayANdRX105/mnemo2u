//! Text utilities — ports of nano-graphrag `_utils.py` / `_op.py` helpers.
//!
//! Parity with the reference is the point: ids, truncation, CSV assembly and
//! JSON recovery must produce byte-identical results so golden fixtures
//! generated from Python compare directly.

use std::sync::LazyLock;

use md5::{Digest, Md5};
use regex::Regex;
use serde_json::{Map, Value};
use tiktoken_rs::{o200k_base, CoreBPE, Rank};

use crate::core::rag::ChatMessage;

/// Field separator inside merged description/source_id strings
/// (`prompt.py:6` GRAPH_FIELD_SEP).
pub const GRAPH_FIELD_SEP: &str = "<SEP>";

pub type TextResult<T> = Result<T, TextError>;

#[derive(Debug, thiserror::Error)]
pub enum TextError {
    #[error("tokenizer error: {0}")]
    Tokenizer(String),
    #[error("json error: {0}")]
    Json(String),
    #[error("template error: {0}")]
    Template(String),
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// md5 hex digest.
pub fn md5_hex(content: &str) -> String {
    hex_lower(&Md5::digest(content.as_bytes()))
}

/// `compute_mdhash_id(content, prefix)` — `_utils.py:186`.
pub fn compute_mdhash_id(content: &str, prefix: &str) -> String {
    format!("{prefix}{}", md5_hex(content))
}

/// `compute_args_hash(model, messages)` — `_utils.py:216`.
///
/// Python hashes `str((model, messages))`; [`py_repr_str`]/[`py_repr_messages`]
/// reproduce that repr for the shape we pass (str + list of role/content dicts).
pub fn compute_args_hash(model: &str, messages: &[ChatMessage]) -> String {
    let repr = format!("({}, {})", py_repr_str(model), py_repr_messages(messages));
    md5_hex(&repr)
}

/// Python `repr()` for a string (single quotes unless the text contains an
/// apostrophe but no double quote; control characters escaped).
pub fn py_repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python `repr()` for one `{"role": ..., "content": ...}` dict.
pub fn py_repr_message(m: &ChatMessage) -> String {
    format!(
        "{{'role': {}, 'content': {}}}",
        py_repr_str(&m.role),
        py_repr_str(&m.content)
    )
}

/// Python `repr()` for a list of message dicts.
pub fn py_repr_messages(messages: &[ChatMessage]) -> String {
    let inner = messages
        .iter()
        .map(py_repr_message)
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{inner}]")
}

/// Python `repr()` for a list of strings (`['a', 'b']`) — used when a prompt
/// placeholder receives a list (e.g. `description_list`).
pub fn py_repr_str_list(values: &[String]) -> String {
    let inner = values
        .iter()
        .map(|v| py_repr_str(v))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{inner}]")
}

/// Python `repr()` for a two-element tuple of strings (`('a', 'b')`) — the
/// reference passes `(src_id, tgt_id)` as the summary name for edges.
pub fn py_repr_tuple2(first: &str, second: &str) -> String {
    format!("({}, {})", py_repr_str(first), py_repr_str(second))
}

/// `truncate_list_by_token_size` — `_utils.py:169`.
pub fn truncate_list_by_token_size<T: Clone>(
    list_data: &[T],
    key: impl Fn(&T) -> String,
    max_token_size: usize,
    tok: &Tokenizer,
) -> Vec<T> {
    if max_token_size == 0 {
        return Vec::new();
    }
    let mut tokens: usize = 0;
    for (i, data) in list_data.iter().enumerate() {
        tokens += tok.token_len(&key(data)) + 1;
        if tokens > max_token_size {
            return list_data[..i].to_vec();
        }
    }
    list_data.to_vec()
}

/// `clean_str` — `_utils.py:241` (HTML unescape + strip control characters).
pub fn clean_str(input: &str) -> String {
    html_unescape(input.trim())
        .chars()
        .filter(|c| !is_control(*c))
        .collect()
}

fn is_control(c: char) -> bool {
    matches!(c as u32, 0x00..=0x1f | 0x7f..=0x9f)
}

/// Minimal HTML entity unescape (named set + numeric). The reference uses
/// `html.unescape`; only the entities occurring in real documents matter here.
pub fn html_unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'&' {
            if let Some(end) = s[i..].find(';') {
                let entity = &s[i + 1..i + end];
                // HTML5 entity names are case-sensitive as a set but include
                // uppercase variants for the common ones (`&AMP;`), and the
                // reference upcases attributes before unescaping.
                let name = entity.to_ascii_lowercase();
                let replacement = match name.as_str() {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" | "#39" => Some('\''),
                    "nbsp" => Some('\u{a0}'),
                    _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                        u32::from_str_radix(&entity[2..], 16)
                            .ok()
                            .and_then(char::from_u32)
                    }
                    _ if entity.starts_with('#') => {
                        entity[1..].parse::<u32>().ok().and_then(char::from_u32)
                    }
                    _ => None,
                };
                if let Some(ch) = replacement {
                    out.push(ch);
                    i += end + 1;
                    continue;
                }
            }
        }
        let ch = s[i..].chars().next().expect("valid utf-8");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// `split_string_by_multi_markers` — `_utils.py:219`.
///
/// The markers are literals, so the split is a plain leftmost scan: at each
/// position the first marker that matches wins, exactly like the reference's
/// `re.split` on escaped markers joined by `|`. A regex cannot be used here —
/// this engine's `regex::escape` emits `\<`/`\>`, which are word-boundary
/// assertions rather than literals, so any delimiter containing `<` or `>`
/// (the tuple delimiter `<|#|>` among them) would never match.
pub fn split_string_by_multi_markers(content: &str, markers: &[&str]) -> Vec<String> {
    if markers.is_empty() {
        return vec![content.to_string()];
    }
    let mut parts: Vec<String> = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < content.len() {
        let hit = markers
            .iter()
            .find(|marker| !marker.is_empty() && content[i..].starts_with(**marker));
        match hit {
            Some(marker) => {
                parts.push(content[start..i].trim().to_string());
                i += marker.len();
                start = i;
            }
            None => {
                let ch = content[i..].chars().next().expect("valid utf-8");
                i += ch.len_utf8();
            }
        }
    }
    parts.push(content[start..].trim().to_string());
    parts.retain(|part| !part.is_empty());
    parts
}

/// `is_float_regex` — `_utils.py:216`.
pub fn is_float_regex(value: &str) -> bool {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[-+]?[0-9]*\.?[0-9]+$").expect("valid regex"));
    RE.is_match(value)
}

/// `enclose_string_with_quotes` — `_utils.py:227` for string cells
/// (numbers are formatted by callers before reaching the CSV assembler).
pub fn enclose_string_with_quotes(content: &str) -> String {
    let stripped = content.trim().trim_matches('\'').trim_matches('"');
    format!("\"{stripped}\"")
}

/// One CSV cell.
///
/// The reference distinguishes numbers from text: `enclose_string_with_quotes`
/// (`_utils.py:230-238`) returns `str(n)` **bare** for anything that is an
/// `int`/`float`, and only text cells get wrapped in quotes. Getting this wrong
/// changes the report context byte for byte, so cells carry their kind.
#[derive(Debug, Clone, PartialEq)]
pub enum CsvCell {
    Text(String),
    Int(i64),
    Float(f64),
}

impl CsvCell {
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }

    pub fn int(value: i64) -> Self {
        Self::Int(value)
    }

    pub fn float(value: f64) -> Self {
        Self::Float(value)
    }

    /// `enclose_string_with_quotes`: numbers bare, text quoted.
    fn cell(&self) -> String {
        match self {
            Self::Text(value) => enclose_string_with_quotes(value),
            Self::Int(value) => value.to_string(),
            Self::Float(value) => py_repr_float(*value),
        }
    }

    /// `format_row` in `_pack_single_community_describe` (`_op.py:549`): every
    /// cell quoted and inner quotes doubled, comma-joined — this is the form
    /// the truncation budget measures.
    fn measurement(&self) -> String {
        match self {
            Self::Text(value) => format!("\"{}\"", value.replace('"', "\"\"")),
            Self::Int(value) => format!("\"{value}\""),
            Self::Float(value) => format!("\"{}\"", py_repr_float(*value).replace('"', "\"\"")),
        }
    }
}

/// Python `str(float)`: `6.0` prints as `6.0`, not `6`.
pub fn py_repr_float(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}

fn csv_row(cells: &[CsvCell]) -> String {
    cells
        .iter()
        .map(CsvCell::cell)
        .collect::<Vec<_>>()
        .join(",\t")
}

/// `list_of_list_to_csv` — `_utils.py:234` (`",\t"` join, `"\n"` rows).
pub fn list_of_list_to_csv(rows: &[Vec<CsvCell>]) -> String {
    rows.iter()
        .map(|row| csv_row(row))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `format_row` — the truncation measurement key (`_op.py:549`).
pub fn csv_measurement_row(cells: &[CsvCell]) -> String {
    cells
        .iter()
        .map(CsvCell::measurement)
        .collect::<Vec<_>>()
        .join(",")
}

/// `extract_first_complete_json` — `_utils.py:34` (brace-stack scanner).
pub fn extract_first_complete_json(s: &str) -> Option<Value> {
    let bytes = s.as_bytes();
    let mut stack: Vec<usize> = Vec::new();
    let mut first_start: Option<usize> = None;
    for (i, b) in bytes.iter().enumerate() {
        match b {
            b'{' => {
                stack.push(i);
                if first_start.is_none() {
                    first_start = Some(i);
                }
            }
            b'}' => {
                if let Some(_start) = stack.pop() {
                    if stack.is_empty() {
                        let start = first_start.expect("start recorded with first '{'");
                        let json_str = s[start..=i].replace('\n', "");
                        // Reference returns immediately: parse result or None.
                        return serde_json::from_str::<Value>(&json_str).ok();
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// `parse_value` — `_utils.py:64` (lenient scalar conversion).
pub fn parse_value(value: &str) -> Value {
    let v = value.trim();
    match v {
        "null" => Value::Null,
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => {
            if v.contains('.') {
                v.parse::<f64>()
                    .map(Value::from)
                    .unwrap_or_else(|_| Value::String(v.trim_matches('"').to_string()))
            } else {
                v.parse::<i64>()
                    .map(Value::from)
                    .unwrap_or_else(|_| Value::String(v.trim_matches('"').to_string()))
            }
        }
    }
}

/// `extract_values_from_json` — `_utils.py:79` (regex fallback for
/// non-standard JSON; nested objects recurse).
pub fn extract_values_from_json(json_string: &str) -> Map<String, Value> {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)(?P<key>"?\w+"?)\s*:\s*(?P<value>\{[^}]*\}|".*?"|[^,}]+)"#)
            .expect("valid regex")
    });
    let mut extracted = Map::new();
    for caps in RE.captures_iter(json_string) {
        let key = caps["key"].trim_matches('"').to_string();
        let raw = caps["value"].trim().to_string();
        let value = if raw.starts_with('{') && raw.ends_with('}') {
            Value::Object(extract_values_from_json(&raw))
        } else {
            parse_value(&raw)
        };
        extracted.insert(key, value);
    }
    extracted
}

/// `convert_response_to_json` — `_utils.py:105`.
pub fn convert_response_to_json(response: &str) -> Option<Value> {
    if let Some(v) = extract_first_complete_json(response) {
        return Some(v);
    }
    let fallback = extract_values_from_json(response);
    if fallback.is_empty() {
        None
    } else {
        Some(Value::Object(fallback))
    }
}

/// Minimal `str.format`-equivalent for filling prompt templates.
///
/// Replaces `{name}` with the matching value and un-doubles `{{`/`}}` exactly
/// like Python's `str.format` (the reference calls `.format(**context)` on the
/// raw templates, which contain doubled braces as literals).
pub fn fill_template(template: &str, vars: &[(&str, &str)]) -> TextResult<String> {
    let mut out = String::with_capacity(template.len() + 64);
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        match chars[i] {
            '{' => {
                if chars.get(i + 1) == Some(&'{') {
                    out.push('{');
                    i += 2;
                    continue;
                }
                let mut j = i + 1;
                while j < chars.len() && chars[j] != '}' {
                    j += 1;
                }
                if j >= chars.len() {
                    out.push('{');
                    i += 1;
                    continue;
                }
                let name: String = chars[i + 1..j].iter().collect();
                match vars.iter().find(|(key, _)| *key == name) {
                    Some((_, value)) => out.push_str(value),
                    None => {
                        return Err(TextError::Template(format!(
                            "unknown placeholder {{{name}}}"
                        )));
                    }
                }
                i = j + 1;
            }
            '}' => {
                if chars.get(i + 1) == Some(&'}') {
                    out.push('}');
                    i += 2;
                    continue;
                }
                out.push('}');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    Ok(out)
}

/// `TokenizerWrapper` — `_utils.py:123`.
/// The reference defaults to tiktoken `encoding_for_model("gpt-4o")`, which is
/// `o200k_base`; parity tests pin that choice.
pub struct Tokenizer {
    bpe: std::sync::Arc<CoreBPE>,
}

impl Clone for Tokenizer {
    fn clone(&self) -> Self {
        Self {
            bpe: self.bpe.clone(),
        }
    }
}

impl Tokenizer {
    pub fn for_gpt_4o() -> TextResult<Self> {
        o200k_base()
            .map(|bpe| Self {
                bpe: std::sync::Arc::new(bpe),
            })
            .map_err(|e| TextError::Tokenizer(e.to_string()))
    }

    pub fn encode(&self, text: &str) -> Vec<Rank> {
        self.bpe.encode_ordinary(text)
    }

    /// Decode tokens to text.
    ///
    /// Python's tiktoken decodes with `errors="replace"`, and token windows can
    /// split a multi-byte character, so strict UTF-8 validation would reject
    /// inputs the reference happily chunks — mirror the reference instead.
    pub fn decode(&self, tokens: &[Rank]) -> TextResult<String> {
        let bytes = self
            .bpe
            .decode_bytes(tokens)
            .map_err(|e| TextError::Tokenizer(e.to_string()))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub fn decode_batch(&self, batch: &[Vec<Rank>]) -> TextResult<Vec<String>> {
        batch.iter().map(|tokens| self.decode(tokens)).collect()
    }

    pub fn token_len(&self, text: &str) -> usize {
        self.encode(text).len()
    }
}

/// `sanitize_text_for_encoding` + `normalize_extracted_info`
/// (LightRAG `utils.py:5838-5971`, sanitize at :5973-6022).
///
/// The pipeline is: HTML unescape, surrogate/control strip, then the
/// normalisation passes below, then the outer-quote strip, then the optional
/// inner-quote / non-breaking-space pass, then the numeric filters.
///
/// Deviation from the reference: the three CJK/ASCII spacing rules are written
/// with capture groups instead of lookaround (the `regex` crate has none), and
/// the full-width translation is an explicit `chars().map()` table rather than
/// `str.translate` — same result, Rust-expressible.
pub fn normalize_extracted_info(name: &str, remove_inner_quotes: bool) -> String {
    // sanitize_text_for_encoding: unescape, drop surrogates/control chars, trim.
    let mut text = html_unescape(name.trim());
    text.retain(|c| !matches!(c as u32, 0x00..=0x1f | 0x7f..=0x9f));
    let text = text.trim().to_string();

    // HTML paragraph/line-break tags.
    let text = TAG_PATTERN.replace_all(&text, "").to_string();

    // Full-width letters, digits and a few symbols -> ASCII.
    let text: String = text.chars().map(fold_full_width).collect();
    // Chinese punctuation that the reference maps 1:1.
    let text = text
        .replace('－', "-")
        .replace('＋', "+")
        .replace('／', "/")
        .replace('＊', "*")
        .replace('（', "(")
        .replace('）', ")")
        .replace('　', " ");

    // Spaces glued between CJK characters, and between CJK and ASCII/digits/
    // symbols, are not meaningful separators in the reference's model.
    let text = strip_cjk_spaces(&text);

    // Matching outer quotes only, and only when the enclosed text has none.
    let mut text = strip_outer_quotes(&text);

    if remove_inner_quotes {
        text = text
            .replace(['“', '”', '‘', '’'], "")
            .replace(['\u{00a0}', '\u{202f}'], " ");
        // Quotes hugging CJK characters go too (reference regex pair).
        text = strip_cjk_adjacent_quotes(&text);
    }

    let text = text.trim().to_string();
    if text.len() < 3 && text.bytes().all(|b| b.is_ascii_digit()) {
        return String::new();
    }
    if text.len() < 6 && is_dots_and_digits(&text) {
        return String::new();
    }
    text
}

/// `sanitize_and_normalize_extracted_text` (`utils.py:5808`).
pub fn sanitize_and_normalize_extracted_text(
    input_text: &str,
    remove_inner_quotes: bool,
) -> String {
    if input_text.is_empty() {
        return String::new();
    }
    normalize_extracted_info(input_text, remove_inner_quotes)
}

/// `normalize_entity_name` (`utils.py:5833`): always the inner-quote pass.
pub fn normalize_entity_name(input_text: &str) -> String {
    sanitize_and_normalize_extracted_text(input_text, true)
}

/// `<p>`, `<br>` and their closed forms (`normalize_extracted_info` head).
static TAG_PATTERN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)</?p\s*/?>|</?br\s*/?>").expect("tag pattern compiles")
});

static CJK_CJK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"([\u{4e00}-\u{9fa5}])\s+([\u{4e00}-\u{9fa5}])")
        .expect("cjk pattern compiles")
});

static CJK_ASCII: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"([\u{4e00}-\u{9fa5}])\s+([a-zA-Z0-9\(\)\[\]@#$%!&\*\-=+_])|([a-zA-Z0-9\(\)\[\]@#$%!&\*\-=+_])\s+([\u{4e00}-\u{9fa5}])",
    )
    .expect("cjk/ascii pattern compiles")
});

/// Full-width -> ASCII single-char table (the reference's two `str.translate`
/// tables plus the symbol replacements it applies unconditionally).
fn fold_full_width(c: char) -> char {
    match c {
        'Ａ'..='Ｚ' => char::from_u32(c as u32 - 'Ａ' as u32 + 'A' as u32).unwrap_or(c),
        'ａ'..='ｚ' => char::from_u32(c as u32 - 'ａ' as u32 + 'a' as u32).unwrap_or(c),
        '０'..='９' => char::from_u32(c as u32 - '０' as u32 + '0' as u32).unwrap_or(c),
        '—' => '-',
        _ => c,
    }
}

/// `re.sub(r"(?<=[CJK])\s+(?=[CJK])", "", name)` and the mixed pair — the
/// regex crate has no lookaround, so the whitespace run is captured and the
/// rule re-applied until it stops matching (a single pass cannot see past a
/// run of three or more spaces).
fn strip_cjk_spaces(text: &str) -> String {
    let mut current = text.to_string();
    loop {
        let collapsed = CJK_CJK.replace_all(&current, "$1$2").to_string();
        let collapsed = CJK_ASCII
            .replace_all(&collapsed, |caps: &regex::Captures<'_>| {
                match (caps.get(1), caps.get(2)) {
                    (Some(left), Some(right)) => format!("{}{}", left.as_str(), right.as_str()),
                    _ => match (caps.get(3), caps.get(4)) {
                        (Some(left), Some(right)) => {
                            format!("{}{}", left.as_str(), right.as_str())
                        }
                        _ => caps
                            .get(0)
                            .map(|whole| whole.as_str().to_string())
                            .unwrap_or_default(),
                    },
                }
            })
            .to_string();
        if collapsed == current {
            return current;
        }
        current = collapsed;
    }
}

fn strip_outer_quotes(text: &str) -> String {
    let mut name = text.to_string();
    for (open, close) in [
        ('"', '"'),
        ('\'', '\''),
        ('“', '”'),
        ('‘', '’'),
        ('《', '》'),
    ] {
        if name.chars().count() >= 2 && name.starts_with(open) && name.ends_with(close) {
            let inner: String = name
                .chars()
                .skip(1)
                .take(name.chars().count() - 2)
                .collect();
            if !inner.contains(open) && !inner.contains(close) {
                name = inner;
            }
        }
    }
    name
}

/// The reference's two "quotes adjacent to CJK" regexes, expressed as capture
/// groups for the same reason as `strip_cjk_spaces`.
fn strip_cjk_adjacent_quotes(text: &str) -> String {
    static BEFORE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"['\"]+([\u{4e00}-\u{9fa5}])"#).expect("quote-before pattern compiles")
    });
    static AFTER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"([\u{4e00}-\u{9fa5}])['\"]+"#).expect("quote-after pattern compiles")
    });
    let current = BEFORE.replace_all(text, "$1").to_string();
    AFTER.replace_all(&current, "$1").to_string()
}

/// `should_filter_by_dots` (`utils.py:5958`): digits and dots only, with at
/// least one dot.
fn is_dots_and_digits(text: &str) -> bool {
    text.contains('.') && text.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

/// `time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(ts))` — the reference
/// renders `created_at` in local time. We have no timezone database, so the
/// rendering is UTC and the caller documents the substitution (the value is
/// display-only provenance, never compared).
pub fn format_local_datetime(timestamp: i64) -> String {
    let days = timestamp.div_euclid(86_400);
    let seconds_of_day = timestamp.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

/// Howard Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u64, u64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (year + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mdhash_matches_python_md5() {
        // md5("hello") == 5d41402abc4b2a76b9719d911017c592
        assert_eq!(
            compute_mdhash_id("hello", "doc-"),
            "doc-5d41402abc4b2a76b9719d911017c592"
        );
    }

    #[test]
    fn py_repr_handles_quotes_and_controls() {
        assert_eq!(py_repr_str("hi"), "'hi'");
        assert_eq!(py_repr_str("it's"), "\"it's\"");
        assert_eq!(py_repr_str("a\nb"), "'a\\nb'");
    }

    #[test]
    fn csv_rows_match_reference_format() {
        // Numbers stay bare, text gets quoted — the reference's
        // `enclose_string_with_quotes` branches on the cell type.
        let rows = vec![
            vec![CsvCell::text("id"), CsvCell::text("entity")],
            vec![CsvCell::int(0), CsvCell::text("ACME")],
        ];
        assert_eq!(
            list_of_list_to_csv(&rows),
            "\"id\",\t\"entity\"\n0,\t\"ACME\""
        );
        assert_eq!(csv_measurement_row(&rows[1]), "\"0\",\"ACME\"");
    }

    #[test]
    fn json_recovery_paths() {
        let v = convert_response_to_json("{\n  \"a\": 1\n}").expect("primary path");
        assert_eq!(v["a"], 1);
        let v = convert_response_to_json("text {\"a\": 1} tail").expect("embedded object");
        assert_eq!(v["a"], 1);
        let v = convert_response_to_json("a: 1, b: \"x\"").expect("fallback path");
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], "x");
    }

    #[test]
    fn split_and_clean() {
        assert_eq!(
            split_string_by_multi_markers("a ## b <|COMPLETE|> ", &["##", "<|COMPLETE|>"]),
            vec!["a", "b"]
        );
        assert_eq!(clean_str("  A&#39;B &amp; C\x07 "), "A'B & C");
    }

    #[test]
    fn truncate_by_tokens() {
        let tok = Tokenizer::for_gpt_4o().expect("tokenizer");
        let items = vec!["one", "two", "three"];
        let kept = truncate_list_by_token_size(&items, |s| s.to_string(), 3, &tok);
        assert!(
            kept.len() < items.len(),
            "budget of 3 tokens must cut the list"
        );
        let kept_all = truncate_list_by_token_size(&items, |s| s.to_string(), 1000, &tok);
        assert_eq!(kept_all.len(), 3);
    }
}
