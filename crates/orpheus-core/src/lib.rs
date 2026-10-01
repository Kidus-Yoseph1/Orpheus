//! orpheus-core: document model, loaders, config, themes, persistence.
//!
//! GUIDE §4: every book normalizes to Document → Chapter → Block → Sentence
//! with stable sentence IDs. EPUB/PDF details never leak past the loaders.

pub mod config;
pub mod db;
pub mod document;
pub mod epub;
pub mod library;
pub mod pdf;
pub mod text;
pub mod theme;

pub use config::Config;
pub use db::LibraryDb;
pub use document::{Block, BlockKind, BookFormat, Chapter, Document, Sentence};
pub use theme::{ReaderStyle, Theme};
