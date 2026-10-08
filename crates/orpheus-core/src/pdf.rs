//! GUIDE §6 — PDF loader: text extraction → paragraph reconstruction →
//! footer stripping → chapter grouping. The reader only ever sees a
//! `Document`, same as with EPUB.
//!
//! Not handled yet (see `needs_ocr`): image-only/scanned PDFs, repeated
//! running headers, multi-column layouts, PDF outline → real chapters.

use std::path::Path;

use anyhow::{Context, Result};

use crate::document::{build_chapter, BlockKind, BookFormat, Document};

/// Pages bundled into one navigable chapter (`Pages 1–10`, `11–20`, …).
const PAGES_PER_CHAPTER: usize = 10;

pub fn load_pdf(path: &Path) -> Result<Document> {
    let path_str = path.to_string_lossy().to_string();
    let pages = pdf_extract::extract_text_by_pages(path)
        .with_context(|| format!("parse pdf {path_str}"))?;

    let title = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Untitled".into());
    let mut doc = Document::new(title, "Unknown".into(), path_str.clone(), BookFormat::Pdf);

    let mut chapter_idx = 0;
    for start in (0..pages.len()).step_by(PAGES_PER_CHAPTER) {
        let end = (start + PAGES_PER_CHAPTER).min(pages.len());
        let mut raw: Vec<(BlockKind, String)> = Vec::new();
        for page in &pages[start..end] {
            raw.extend(page_blocks(page));
        }
        if raw.is_empty() {
            continue;
        }
        let title = format!("Pages {}–{}", start + 1, end);
        doc.chapters
            .push(build_chapter(&doc.id, chapter_idx, title, raw));
        chapter_idx += 1;
    }

    if doc.chapters.is_empty() {
        anyhow::bail!(
            "{path_str} has no selectable text (it looks scanned or image-only) — \
             OCR is not supported yet"
        );
    }
    Ok(doc)
}

/// One page's text → (kind, text) blocks: blank lines split paragraphs,
/// wrapped lines join back together, page numbers and blank pages drop out.
fn page_blocks(page: &str) -> Vec<(BlockKind, String)> {
    let mut out = Vec::new();
    for segment in segments(page) {
        let text = join_wrapped(&segment);
        if text.is_empty() || is_page_number(&text) {
            continue;
        }
        let kind = if is_heading(&text) {
            BlockKind::Heading
        } else {
            BlockKind::Paragraph
        };
        out.push((kind, text));
    }
    out
}

/// Split page text on blank lines (the extractor emits them for layout gaps).
fn segments(page: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in page.lines() {
        if line.trim().is_empty() {
            if !cur.trim().is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            if !cur.is_empty() {
                cur.push('\n');
            }
            cur.push_str(line.trim_end());
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// Rejoin hyphenated/wrapped lines into a single logical line.
fn join_wrapped(segment: &str) -> String {
    let dehyphenated = segment.replace("-\n", "");
    let joined = dehyphenated
        .lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ");
    joined.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Footer page numbers ("1", "42") — bare digits, never prose.
fn is_page_number(text: &str) -> bool {
    !text.is_empty() && text.len() <= 4 && text.chars().all(|c| c.is_ascii_digit())
}

/// Conservative heading sniff: a short line with no sentence punctuation
/// whose last word is capitalized ("Chapter One", "INTRODUCTION").
fn is_heading(text: &str) -> bool {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() || words.len() > 10 {
        return false;
    }
    if text
        .chars()
        .any(|c| matches!(c, '.' | '!' | '?' | ';' | ':'))
    {
        return false;
    }
    // Title-cased at both ends: kills digit-led table rows ("200 xC8 È")
    // and lowercase fragments ("he stopped") alike.
    let starts_upper = text.chars().next().is_some_and(|c| c.is_uppercase());
    let ends_upper = words
        .last()
        .and_then(|w| w.chars().next())
        .is_some_and(|c| c.is_uppercase());
    starts_upper && ends_upper
}

/// Heuristic: does this PDF have selectable text at all? Samples every page
/// (scanned books are image-only throughout, so the first text wins).
pub fn needs_ocr(path: &Path) -> Result<bool> {
    let pages = pdf_extract::extract_text_by_pages(path)
        .with_context(|| format!("parse pdf {}", path.display()))?;
    Ok(!pages.iter().any(|p| p.chars().any(|c| !c.is_whitespace())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::BookFormat;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    fn all_text(doc: &Document) -> String {
        doc.chapters
            .iter()
            .flat_map(|c| c.blocks.iter().map(|b| b.text.clone()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn loads_a_text_pdf_into_a_document() {
        let doc = load_pdf(&fixture("text.pdf")).expect("text.pdf must load");
        assert_eq!(doc.format, BookFormat::Pdf);
        assert_eq!(doc.title, "text", "falls back to the file stem");
        assert_eq!(doc.author, "Unknown");
        let text = all_text(&doc);
        assert!(
            text.contains("The wind moved slowly across the open desert"),
            "{text}"
        );
        assert!(doc.total_sentences() > 0);
        let (_, _, _, first) = doc.flat_sentences()[0];
        assert!(!first.speak_text.is_empty(), "TTS text is filled in");
    }

    #[test]
    fn wrapped_lines_join_into_one_paragraph() {
        let doc = load_pdf(&fixture("text.pdf")).unwrap();
        let text = all_text(&doc);
        assert!(
            text.contains("notebook, then closed the book"),
            "line wraps must become single spaces, got:\n{text}"
        );
        assert!(!text.contains("  "), "no double spaces");
    }

    #[test]
    fn standalone_page_numbers_are_dropped() {
        let doc = load_pdf(&fixture("text.pdf")).unwrap();
        let texts: Vec<&str> = doc
            .chapters
            .iter()
            .flat_map(|c| c.blocks.iter().map(|b| b.text.as_str()))
            .collect();
        assert!(!texts.contains(&"1"), "footer '1' leaked: {texts:?}");
        assert!(!texts.contains(&"2"), "footer '2' leaked: {texts:?}");
    }

    #[test]
    fn short_title_lines_become_headings() {
        let doc = load_pdf(&fixture("text.pdf")).unwrap();
        let kinds: Vec<(crate::document::BlockKind, &str)> = doc
            .chapters
            .iter()
            .flat_map(|c| c.blocks.iter().map(|b| (b.kind, b.text.as_str())))
            .collect();
        assert!(
            kinds.contains(&(crate::document::BlockKind::Heading, "Chapter One")),
            "expected a Heading block, got {kinds:?}"
        );
    }

    #[test]
    fn pages_group_into_navigable_chapters() {
        let doc = load_pdf(&fixture("text.pdf")).unwrap();
        assert_eq!(doc.chapters.len(), 1, "2 pages fit in one chapter");
        assert_eq!(doc.chapters[0].title, "Pages 1–2");
        assert_eq!(doc.chapters[0].index, 0);
    }

    #[test]
    fn loading_is_deterministic() {
        let a = load_pdf(&fixture("text.pdf")).unwrap();
        let b = load_pdf(&fixture("text.pdf")).unwrap();
        assert_eq!(a.id, b.id, "stable ids across opens");
        assert_eq!(a.flat_sentences().len(), b.flat_sentences().len());
        assert_eq!(a.flat_sentences()[0].3.id, b.flat_sentences()[0].3.id);
    }

    #[test]
    fn a_pdf_without_selectable_text_says_so() {
        let err = load_pdf(&fixture("blank.pdf")).expect_err("blank.pdf has no text");
        let msg = err.to_string();
        assert!(msg.contains("no selectable text"), "unhelpful error: {msg}");
        assert!(msg.contains("OCR"), "should name the OCR gap: {msg}");
    }

    #[test]
    fn short_fragments_stay_paragraphs() {
        // A sentence fragment without final punctuation must not become a
        // heading just because it is short: the last word stays lowercase.
        assert!(!is_heading("He stopped at the gate"));
        assert!(is_heading("Chapter One"));
        assert!(is_heading("INTRODUCTION"));
        assert!(!is_heading("he walked anyway"));
        assert!(!is_heading("It was the best of times, it was"));
    }

    #[test]
    fn reference_table_rows_are_not_headings() {
        // Seen in a real font-catalogue PDF: short rows that merely lack
        // punctuation are still prose/table data, not titles.
        assert!(!is_heading("200 xC8 È"));
        assert!(!is_heading("79 x4F O"));
        assert!(!is_heading("Table 4: Metrics"));
    }

    #[test]
    fn page_number_filter_is_narrow() {
        assert!(is_page_number("42"));
        assert!(!is_page_number("42a"));
        assert!(!is_page_number(""));
        assert!(!is_page_number("page 42"));
    }

    #[test]
    fn needs_ocr_only_for_text_less_pdfs() {
        assert!(needs_ocr(&fixture("blank.pdf")).unwrap());
        assert!(!needs_ocr(&fixture("text.pdf")).unwrap());
    }
}
