//! GUIDE §5 — EPUB loader, first-class format.
//!
//! Manual OPF/spine parsing (zip + quick-xml) + HTML extraction (scraper)
//! so we control chapter order, block kinds, and normalization.
//! Wrapped behind `load_epub`; no EPUB details leak to reader/TTS.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use scraper::{Html, Selector};
use zip::ZipArchive;

use crate::document::{build_chapter, BlockKind, BookFormat, Document};

pub fn load_epub(path: &Path) -> Result<Document> {
    let path_str = path.to_string_lossy().to_string();
    let file = File::open(path).with_context(|| format!("open epub {path_str}"))?;
    let mut zip = ZipArchive::new(file).context("parse epub zip")?;

    let opf_path = find_opf_path(&mut zip).context("find OPF")?;
    let opf_xml = read_zip_file(&mut zip, &opf_path)?;
    let opf = parse_opf(&opf_xml).context("parse OPF")?;
    let base = opf_base_dir(&opf_path);

    let mut doc = Document::new(
        opf.title.clone().unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "Untitled".into())
        }),
        opf.author.clone().unwrap_or_else(|| "Unknown".into()),
        path_str.clone(),
        BookFormat::Epub,
    );

    let mut chapter_idx = 0;
    for idref in &opf.spine {
        let Some(item) = opf.manifest.get(idref) else {
            continue;
        };
        if !is_xhtml(&item.media_type, &item.href) {
            continue;
        }
        let full = join_zip_path(&base, &item.href);
        let html = match read_zip_file(&mut zip, &full) {
            Ok(h) => h,
            Err(_) => continue,
        };
        let (title, blocks) = extract_blocks(&html, chapter_idx);
        if blocks.is_empty() {
            continue;
        }
        doc.chapters
            .push(build_chapter(&doc.id, chapter_idx, title, blocks));
        chapter_idx += 1;
    }

    if doc.chapters.is_empty() {
        anyhow::bail!("no readable chapters found in {path_str}");
    }
    Ok(doc)
}

// --- OPF plumbing ----------------------------------------------------------

struct OpfItem {
    href: String,
    media_type: String,
}

struct Opf {
    title: Option<String>,
    author: Option<String>,
    manifest: std::collections::HashMap<String, OpfItem>,
    spine: Vec<String>,
}

fn find_opf_path(zip: &mut ZipArchive<File>) -> Result<String> {
    let mut container = String::new();
    zip.by_name("META-INF/container.xml")
        .context("epub missing META-INF/container.xml")?
        .read_to_string(&mut container)?;
    // Minimal parse: look for full-path="...opf".
    let re = regex::Regex::new(r#"full-path="([^"]+)""#).expect("static regex compiles");
    re.captures(&container)
        .and_then(|c| c.get(1))
        .map(|m| html_escape::decode_html_entities(m.as_str()).into_owned())
        .context("container.xml has no rootfile full-path")
}

fn read_zip_file(zip: &mut ZipArchive<File>, name: &str) -> Result<String> {
    // Zip names are percent-decoded rarely; try raw then decoded.
    let decoded = percent_decode(name);
    let candidates = [name.to_string(), decoded];
    for cand in &candidates {
        if let Ok(mut f) = zip.by_name(cand) {
            let mut s = String::new();
            f.read_to_string(&mut s)
                .with_context(|| format!("read {cand}"))?;
            return Ok(s);
        }
    }
    anyhow::bail!("epub entry not found: {name}")
}

fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut bytes = s.as_bytes().iter().peekable();
    while let Some(&b) = bytes.next() {
        if b == b'%' {
            let h: Vec<u8> = bytes.by_ref().take(2).copied().collect();
            if h.len() == 2 {
                if let Ok(hex) = std::str::from_utf8(&h) {
                    if let Ok(v) = u8::from_str_radix(hex, 16) {
                        out.push(v as char);
                        continue;
                    }
                }
                out.push('%');
                out.extend(h.into_iter().map(|c| c as char));
            } else {
                out.push('%');
            }
        } else {
            out.push(b as char);
        }
    }
    out
}

fn parse_opf(xml: &str) -> Result<Opf> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut title: Option<String> = None;
    let mut author: Option<String> = None;
    let mut manifest = std::collections::HashMap::new();
    let mut spine: Vec<String> = Vec::new();
    let mut current_text_element: Option<String> = None;
    let mut current_text = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                let local = name.rsplit(':').next().unwrap_or(&name).to_lowercase();
                match local.as_str() {
                    "item" => {
                        let (mut id, mut href, mut media) = (None, None, None);
                        for attr in e.attributes().flatten() {
                            let k = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
                            let v = String::from_utf8_lossy(&attr.value).into_owned();
                            match k.as_str() {
                                "id" => id = Some(v),
                                "href" => href = Some(v),
                                "media-type" => media = Some(v),
                                _ => {}
                            }
                        }
                        if let (Some(id), Some(href), Some(media)) = (id, href, media) {
                            manifest.insert(
                                id,
                                OpfItem {
                                    href: html_escape::decode_html_entities(&href).into_owned(),
                                    media_type: media,
                                },
                            );
                        }
                    }
                    "itemref" => {
                        for attr in e.attributes().flatten() {
                            let k = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
                            if k == "idref" {
                                let v = String::from_utf8_lossy(&attr.value).into_owned();
                                // Skip non-linear (cover etc.)? Keep linear!=no only.
                                spine.push(v);
                            }
                        }
                    }
                    "title" | "creator" => {
                        current_text_element = Some(local);
                        current_text.clear();
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(e)) => {
                if current_text_element.is_some() {
                    // quick-xml 0.37: unescape entities; fall back to raw bytes.
                    let s = e.unescape().unwrap_or_default().into_owned();
                    current_text.push_str(&s);
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                let local = name.rsplit(':').next().unwrap_or(&name).to_lowercase();
                match local.as_str() {
                    "title" => {
                        if current_text_element.as_deref() == Some("title") && title.is_none() {
                            title = Some(current_text.trim().to_string());
                        }
                        current_text_element = None;
                    }
                    "creator" => {
                        if current_text_element.as_deref() == Some("creator") && author.is_none() {
                            author = Some(current_text.trim().to_string());
                        }
                        current_text_element = None;
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => anyhow::bail!("opf xml error: {e}"),
            _ => {}
        }
        buf.clear();
    }
    Ok(Opf {
        title: title.filter(|t| !t.is_empty()),
        author: author.filter(|a| !a.is_empty()),
        manifest,
        spine,
    })
}

fn opf_base_dir(opf_path: &str) -> String {
    match opf_path.rfind('/') {
        Some(i) => opf_path[..=i].to_string(),
        None => String::new(),
    }
}

fn join_zip_path(base: &str, href: &str) -> String {
    // href may contain ../ segments; resolve minimally.
    let combined = format!("{base}{href}");
    let mut parts: Vec<&str> = Vec::new();
    for seg in combined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn is_xhtml(media_type: &str, href: &str) -> bool {
    media_type.contains("html")
        || media_type.contains("xhtml")
        || href.to_lowercase().ends_with(".xhtml")
        || href.to_lowercase().ends_with(".html")
        || href.to_lowercase().ends_with(".htm")
}

// --- HTML → blocks -----------------------------------------------------------

fn extract_blocks(html: &str, chapter_idx: usize) -> (String, Vec<(BlockKind, String)>) {
    let doc = Html::parse_document(html);
    // Remove hidden/script/style content.
    let mut blocks: Vec<(BlockKind, String)> = Vec::new();

    let heading_sel = Selector::parse("h1, h2, h3, h4, h5, h6").unwrap();
    let para_sel = Selector::parse("p").unwrap();
    let quote_sel = Selector::parse("blockquote").unwrap();
    let li_sel = Selector::parse("li").unwrap();

    // Walk in document order over these selectors.
    let all_sel = Selector::parse("h1, h2, h3, h4, h5, h6, p, blockquote, li").unwrap();
    for el in doc.select(&all_sel) {
        let name = el.value().name().to_lowercase();
        let kind = if name.starts_with('h') {
            BlockKind::Heading
        } else if name == "blockquote" {
            BlockKind::Quote
        } else if name == "li" {
            BlockKind::List
        } else {
            BlockKind::Paragraph
        };
        let text = collect_text(el);
        let norm = crate::text::normalize_text(&text);
        if norm.is_empty() || is_boilerplate(&norm) {
            continue;
        }
        blocks.push((kind, norm));
    }

    // Fallback: no <p> markup (some EPUBs use bare divs) — use body text.
    if blocks.is_empty() {
        if let Ok(body_sel) = Selector::parse("body") {
            if let Some(body) = doc.select(&body_sel).next() {
                let text = collect_text(body);
                for para in text.split("\n\n") {
                    let norm = crate::text::normalize_text(para);
                    if !norm.is_empty() && !is_boilerplate(&norm) {
                        blocks.push((BlockKind::Paragraph, norm));
                    }
                }
            }
        }
    }

    // Chapter title = first heading, else toc-ish fallback.
    let title = blocks
        .iter()
        .find(|(k, _)| *k == BlockKind::Heading)
        .map(|(_, t)| truncate(t, 80))
        .or_else(|| blocks.first().map(|(_, t)| truncate(t, 80)))
        .unwrap_or_else(|| format!("Chapter {}", chapter_idx + 1));

    // Keep headings out of narration flow? No — keep them, TTS reads titles.
    let _ = (heading_sel, para_sel, quote_sel, li_sel);
    (title, blocks)
}

fn collect_text(el: scraper::ElementRef<'_>) -> String {
    el.text().collect::<Vec<_>>().join(" ")
}

fn is_boilerplate(s: &str) -> bool {
    // Skip page-number-only / tiny artifacts.
    if s.len() < 3 {
        return true;
    }
    if s.chars().all(|c| c.is_numeric() || c.is_whitespace()) && s.len() < 8 {
        return true;
    }
    false
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_minimal_epub(path: &Path) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("mimetype", opts).unwrap();
        zip.write_all(b"application/epub+zip").unwrap();
        zip.start_file("META-INF/container.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
        )
        .unwrap();
        zip.start_file("OEBPS/content.opf", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Dune Test</dc:title><dc:creator>Frank Herbert</dc:creator></metadata><manifest><item id="ch1" href="ch1.xhtml" media-type="application/xhtml+xml"/><item id="ch2" href="ch2.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="ch1"/><itemref idref="ch2"/></spine></package>"#,
        )
        .unwrap();
        zip.start_file("OEBPS/ch1.xhtml", opts).unwrap();
        zip.write_all(
            br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><h1>Chapter One</h1><p>The desert was vast and silent. Paul looked toward the horizon.</p><p>"We should go," he said. Jessica said nothing.</p></body></html>"#,
        )
        .unwrap();
        zip.start_file("OEBPS/ch2.xhtml", opts).unwrap();
        zip.write_all(
            br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><h1>Chapter Two</h1><p>Dr. Smith arrived. The wind carried sand across the rocks.</p></body></html>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn loads_minimal_epub_with_stable_ids() {
        let dir = std::env::temp_dir().join(format!("orpheus-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let epub_path = dir.join("test.epub");
        make_minimal_epub(&epub_path);

        let doc = load_epub(&epub_path).expect("load epub");
        assert_eq!(doc.title, "Dune Test");
        assert_eq!(doc.author, "Frank Herbert");
        assert_eq!(doc.chapters.len(), 2);
        assert_eq!(doc.chapters[0].title, "Chapter One");
        assert!(
            doc.total_sentences() >= 6,
            "sentences: {}",
            doc.total_sentences()
        );

        // Stable IDs: reload → same ids.
        let doc2 = load_epub(&epub_path).unwrap();
        assert_eq!(doc.id, doc2.id);
        assert_eq!(
            doc.chapters[0].blocks[0].sentences[0].id,
            doc2.chapters[0].blocks[0].sentences[0].id
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
