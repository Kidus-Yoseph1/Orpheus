//! GUIDE §6 — PDF support stub (Milestone 4).
//!
//! V1 M1+2 focuses on EPUB. This stub keeps the `Document` boundary intact
//! so the reader never branches on format specifics.

use std::path::Path;

use anyhow::Result;

use crate::document::Document;

pub fn load_pdf(_path: &Path) -> Result<Document> {
    anyhow::bail!(
        "PDF support lands in Milestone 4. \
         Planned pipeline: text blocks → paragraph reconstruction → \
         header/footer stripping → column detection → OCR fallback (Tesseract)."
    )
}

/// Heuristic: does this PDF have selectable text at all?
/// Real implementation will sample pages via a PDF extractor.
pub fn needs_ocr(_path: &Path) -> Result<bool> {
    Ok(false)
}
