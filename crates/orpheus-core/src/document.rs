//! GUIDE §4 — normalized document representation.
//!
//! Stable IDs: sha256(book_id + chapter_idx + block_idx + sentence_idx + text)
//! truncated to 16 hex chars. Deterministic across re-opens so resume,
//! cache keys, and bookmarks stay valid.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: String,
    pub title: String,
    pub author: String,
    pub source_path: String,
    pub format: BookFormat,
    pub chapters: Vec<Chapter>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BookFormat {
    Epub,
    Pdf,
}

impl BookFormat {
    pub fn from_path(path: &str) -> Option<Self> {
        let lower = path.to_lowercase();
        if lower.ends_with(".epub") {
            Some(Self::Epub)
        } else if lower.ends_with(".pdf") {
            Some(Self::Pdf)
        } else {
            None
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Epub => "epub",
            Self::Pdf => "pdf",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chapter {
    pub id: String,
    pub index: usize,
    pub title: String,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub id: String,
    pub index: usize,
    pub kind: BlockKind,
    pub text: String,
    pub sentences: Vec<Sentence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockKind {
    Heading,
    Paragraph,
    Quote,
    List,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sentence {
    pub id: String,
    pub index: usize,
    pub text: String,
    /// Narration-ready text after preprocessing (GUIDE §24). Defaults to `text`.
    pub speak_text: String,
}

impl Document {
    pub fn new(title: String, author: String, source_path: String, format: BookFormat) -> Self {
        let id = stable_id(&format!("{source_path}|{title}|{author}"));
        Self {
            id,
            title,
            author,
            source_path,
            format,
            chapters: Vec::new(),
        }
    }

    pub fn total_sentences(&self) -> usize {
        self.chapters
            .iter()
            .map(|c| c.blocks.iter().map(|b| b.sentences.len()).sum::<usize>())
            .sum()
    }

    /// Flat sentence list with (chapter_idx, block_idx, sentence_idx).
    pub fn flat_sentences(&self) -> Vec<(usize, usize, usize, &Sentence)> {
        let mut out = Vec::new();
        for (ci, ch) in self.chapters.iter().enumerate() {
            for (bi, b) in ch.blocks.iter().enumerate() {
                for (si, s) in b.sentences.iter().enumerate() {
                    out.push((ci, bi, si, s));
                }
            }
        }
        out
    }

    /// 0.0–1.0 progress for a position.
    pub fn progress_for(&self, chapter_idx: usize, global_sentence: usize) -> f32 {
        let total = self.total_sentences().max(1) as f32;
        let _ = chapter_idx;
        (global_sentence as f32 / total).clamp(0.0, 1.0)
    }
}

/// Build a chapter from raw (title, blocks) where each block is (kind, text).
pub fn build_chapter(
    doc_id: &str,
    index: usize,
    title: String,
    raw_blocks: Vec<(BlockKind, String)>,
) -> Chapter {
    let id = stable_id(&format!("{doc_id}|ch|{index}|{title}"));
    let mut blocks = Vec::with_capacity(raw_blocks.len());
    for (bi, (kind, text)) in raw_blocks.into_iter().enumerate() {
        let block_id = stable_id(&format!("{id}|b|{bi}|{text}"));
        let sentence_texts = crate::text::segment_sentences(&text);
        let sentences = sentence_texts
            .into_iter()
            .enumerate()
            .map(|(si, t)| {
                let sid = stable_id(&format!("{block_id}|s|{si}|{t}"));
                Sentence {
                    id: sid,
                    index: si,
                    speak_text: crate::text::preprocess_for_tts(&t),
                    text: t,
                }
            })
            .collect();
        blocks.push(Block {
            id: block_id,
            index: bi,
            kind,
            text,
            sentences,
        });
    }
    Chapter {
        id,
        index,
        title,
        blocks,
    }
}

pub fn stable_id(input: &str) -> String {
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    let digest = h.finalize();
    hex_encode(&digest[..8])
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}
