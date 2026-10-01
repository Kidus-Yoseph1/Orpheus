//! GUIDE §24 — text normalization + sentence segmentation + TTS preprocessing.
//!
//! Pipeline: cleanup → paragraph reconstruction → sentence segmentation
//! → abbreviation handling → pronunciation replacements → TTS.

use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

static WS_RE: OnceLock<Regex> = OnceLock::new();
static MULTI_BLANK_RE: OnceLock<Regex> = OnceLock::new();

fn ws_re() -> &'static Regex {
    WS_RE.get_or_init(|| Regex::new(r"[ \t\u{00A0}]+").unwrap())
}

fn multi_blank_re() -> &'static Regex {
    MULTI_BLANK_RE.get_or_init(|| Regex::new(r"\n{3,}").unwrap())
}

/// Normalize raw extracted text: entities, whitespace, hyphenated line breaks.
pub fn normalize_text(raw: &str) -> String {
    // Decode HTML entities first.
    let decoded = html_escape::decode_html_entities(raw);
    let mut s = decoded.into_owned();
    // De-hyphenate line breaks: "sand-\nwich" → "sandwich".
    s = s.replace("-\n", "");
    // Collapse intra-line whitespace.
    s = ws_re().replace_all(&s, " ").into_owned();
    // Normalize newlines.
    s = s.replace("\r\n", "\n").replace('\r', "\n");
    s = multi_blank_re().replace_all(&s, "\n\n").into_owned();
    // Trim each line, drop empty artifacts.
    s.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Split a paragraph into sentences. Handles abbreviations, dialogue, quotes.
pub fn segment_sentences(paragraph: &str) -> Vec<String> {
    let text = paragraph.trim();
    if text.is_empty() {
        return Vec::new();
    }
    // Protect known abbreviations before splitting on [.?!].
    let protected = protect_abbreviations(text);
    let mut out = Vec::new();
    let mut start = 0usize;
    let chars: Vec<char> = protected.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '.' || c == '!' || c == '?' {
            // Consume runs like "..." "?!" and closing quotes.
            let mut j = i + 1;
            while j < chars.len() && (chars[j] == '.' || chars[j] == '!' || chars[j] == '?') {
                j += 1;
            }
            while j < chars.len() && matches!(chars[j], '"' | '\'' | '”' | '’' | ')' | '»') {
                j += 1;
            }
            // Boundary if next is whitespace/end (avoids mid-word splits).
            let boundary = j >= chars.len()
                || chars[j].is_whitespace()
                || (j < chars.len() && chars[j] == '\n');
            if boundary {
                let raw: String = chars[start..j].iter().collect();
                let restored = restore_abbreviations(raw.trim());
                if !restored.is_empty() {
                    out.push(restored);
                }
                // Skip whitespace to next sentence start.
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                start = j;
                i = j;
                continue;
            }
            i = j;
            continue;
        }
        i += 1;
    }
    if start < chars.len() {
        let tail: String = chars[start..].iter().collect();
        let restored = restore_abbreviations(tail.trim());
        if !restored.is_empty() {
            out.push(restored);
        }
    }
    // Fallback: never return empty for non-empty input.
    if out.is_empty() {
        out.push(restore_abbreviations(text));
    }
    out
}

const ABBR: &[(&str, &str)] = &[
    ("Mr.", "Mr§"),
    ("Mrs.", "Mrs§"),
    ("Ms.", "Ms§"),
    ("Dr.", "Dr§"),
    ("St.", "St§"),
    ("Prof.", "Prof§"),
    ("Sr.", "Sr§"),
    ("Jr.", "Jr§"),
    ("vs.", "vs§"),
    ("etc.", "etc§"),
    ("e.g.", "e§g§"),
    ("i.e.", "i§e§"),
];

fn protect_abbreviations(s: &str) -> String {
    let mut out = s.to_string();
    for (from, to) in ABBR {
        out = out.replace(from, to);
    }
    out
}

fn restore_abbreviations(s: &str) -> String {
    let mut out = s.to_string();
    for (from, to) in ABBR {
        out = out.replace(to, from);
    }
    out
}

/// GUIDE §24–25: produce speakable text — strip artifacts, expand
/// abbreviations, apply user pronunciation dictionary.
pub fn preprocess_for_tts(sentence: &str) -> String {
    preprocess_for_tts_with_dict(sentence, empty_dict())
}

static EMPTY_DICT: OnceLock<HashMap<String, String>> = OnceLock::new();
fn empty_dict() -> &'static HashMap<String, String> {
    EMPTY_DICT.get_or_init(HashMap::new)
}

pub fn preprocess_for_tts_with_dict(sentence: &str, dict: &HashMap<String, String>) -> String {
    let mut s = sentence.trim().to_string();
    // Drop page-number-only lines ("12", "— 47 —").
    if s.chars()
        .all(|c| c.is_ascii_digit() || c.is_whitespace() || c == '—' || c == '-')
    {
        if s.chars().any(|c| c.is_ascii_digit()) && s.len() < 8 {
            return String::new();
        }
    }
    // Expand common TTS-hostile tokens.
    for (from, to) in [("&", " and "), ("@", " at "), ("%", " percent ")] {
        s = s.replace(from, to);
    }
    // User pronunciation overrides (whole-word).
    for (word, pron) in dict {
        // Simple case-sensitive whole-word replacement.
        let pattern = format!(r"\b{}\b", regex::escape(word));
        if let Ok(re) = Regex::new(&pattern) {
            s = re.replace_all(&s, pron.as_str()).into_owned();
        }
    }
    ws_re().replace_all(&s, " ").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_simple_sentences() {
        let v = segment_sentences("Paul looked toward the horizon. The wind carried sand.");
        assert_eq!(v.len(), 2);
        assert!(v[0].contains("Paul"));
    }

    #[test]
    fn protects_abbreviations() {
        let v = segment_sentences("Dr. Smith went home. He slept.");
        assert_eq!(v.len(), 2, "got: {v:?}");
        assert!(v[0].starts_with("Dr."));
    }

    #[test]
    fn dialogue_boundaries() {
        let v = segment_sentences("\"We should go.\" Jessica said nothing.");
        assert!(v.len() >= 2, "got: {v:?}");
    }

    #[test]
    fn normalizes_entities_and_hyphens() {
        let s = normalize_text("sand-\nwich &amp; dust");
        assert_eq!(s, "sandwich & dust");
    }

    #[test]
    fn pronunciation_dict_applies() {
        let mut d = HashMap::new();
        d.insert("Cthulhu".into(), "kuh-THOO-loo".into());
        let s = preprocess_for_tts_with_dict("Cthulhu waits.", &d);
        assert!(s.contains("kuh-THOO-loo"), "got: {s}");
    }
}
