//! GUIDE §19 — SQLite library: books, positions, bookmarks, voices,
//! TTS model registry (mirrors the on-disk `models/<id>/` store).

use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct BookRow {
    pub id: String,
    pub path: String,
    pub title: String,
    pub author: String,
    pub format: String,
    pub last_opened_at: Option<String>,
    pub chapter_idx: i64,
    pub sentence_idx: i64,
    pub progress: f64,
}

#[derive(Debug, Clone)]
pub struct BookmarkRow {
    pub id: i64,
    pub book_id: String,
    pub chapter_idx: i64,
    pub sentence_idx: i64,
    pub note: String,
    pub created_at: String,
}

/// Mirror of a `ModelStore` manifest for fast listing without disk scans.
#[derive(Debug, Clone)]
pub struct TtsModelRow {
    pub id: String,
    pub backend: String,
    pub hf_repo: String,
    pub revision: String,
    pub local_path: String,
    pub total_bytes: i64,
    pub installed_at: String,
}

/// A cloned/custom voice: normalized reference audio + metadata.
#[derive(Debug, Clone)]
pub struct VoiceRow {
    pub id: String,
    pub name: String,
    pub model_id: String,
    pub reference_path: String,
    pub created_at: String,
}

pub struct LibraryDb {
    conn: Connection,
}

impl LibraryDb {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS books (
                id TEXT PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                title TEXT NOT NULL,
                author TEXT NOT NULL DEFAULT 'Unknown',
                format TEXT NOT NULL DEFAULT 'epub',
                added_at TEXT NOT NULL DEFAULT (datetime('now')),
                last_opened_at TEXT
            );
            CREATE TABLE IF NOT EXISTS positions (
                book_id TEXT PRIMARY KEY REFERENCES books(id) ON DELETE CASCADE,
                chapter_idx INTEGER NOT NULL DEFAULT 0,
                sentence_idx INTEGER NOT NULL DEFAULT 0,
                progress REAL NOT NULL DEFAULT 0.0,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS bookmarks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                book_id TEXT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
                chapter_idx INTEGER NOT NULL,
                sentence_idx INTEGER NOT NULL,
                note TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS voices (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                model_id TEXT NOT NULL,
                reference_path TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS tts_models (
                id TEXT PRIMARY KEY,
                backend TEXT NOT NULL DEFAULT '',
                hf_repo TEXT NOT NULL DEFAULT '',
                revision TEXT NOT NULL DEFAULT '',
                local_path TEXT NOT NULL DEFAULT '',
                total_bytes INTEGER NOT NULL DEFAULT 0,
                installed_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            ",
        )?;
        Ok(())
    }

    pub fn upsert_book(
        &self,
        id: &str,
        path: &str,
        title: &str,
        author: &str,
        format: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO books (id, path, title, author, format, last_opened_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))
             ON CONFLICT(path) DO UPDATE SET
               title=excluded.title, author=excluded.author,
               format=excluded.format, last_opened_at=datetime('now')",
            params![id, path, title, author, format],
        )?;
        Ok(())
    }

    pub fn save_position(
        &self,
        book_id: &str,
        chapter_idx: i64,
        sentence_idx: i64,
        progress: f64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO positions (book_id, chapter_idx, sentence_idx, progress, updated_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))
             ON CONFLICT(book_id) DO UPDATE SET
               chapter_idx=excluded.chapter_idx, sentence_idx=excluded.sentence_idx,
               progress=excluded.progress, updated_at=datetime('now')",
            params![book_id, chapter_idx, sentence_idx, progress],
        )?;
        // Keep books.last_opened_at fresh for Continue Reading ordering.
        self.conn.execute(
            "UPDATE books SET last_opened_at=datetime('now') WHERE id=?1",
            params![book_id],
        )?;
        Ok(())
    }

    pub fn get_position(&self, book_id: &str) -> Result<Option<(i64, i64, f64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT chapter_idx, sentence_idx, progress FROM positions WHERE book_id=?1",
        )?;
        let mut rows = stmt.query(params![book_id])?;
        if let Some(r) = rows.next()? {
            Ok(Some((r.get(0)?, r.get(1)?, r.get(2)?)))
        } else {
            Ok(None)
        }
    }

    pub fn recent_books(&self, limit: i64) -> Result<Vec<BookRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT b.id, b.path, b.title, b.author, b.format, b.last_opened_at,
                    COALESCE(p.chapter_idx,0), COALESCE(p.sentence_idx,0), COALESCE(p.progress,0.0)
             FROM books b LEFT JOIN positions p ON p.book_id=b.id
             ORDER BY b.last_opened_at DESC NULLS LAST LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| {
            Ok(BookRow {
                id: r.get(0)?,
                path: r.get(1)?,
                title: r.get(2)?,
                author: r.get(3)?,
                format: r.get(4)?,
                last_opened_at: r.get(5)?,
                chapter_idx: r.get(6)?,
                sentence_idx: r.get(7)?,
                progress: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn remove_book(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM books WHERE id=?1", params![id])?;
        Ok(())
    }

    pub fn add_bookmark(
        &self,
        book_id: &str,
        chapter_idx: i64,
        sentence_idx: i64,
        note: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO bookmarks (book_id, chapter_idx, sentence_idx, note) VALUES (?1,?2,?3,?4)",
            params![book_id, chapter_idx, sentence_idx, note],
        )?;
        Ok(())
    }

    pub fn bookmarks_for(&self, book_id: &str) -> Result<Vec<BookmarkRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, book_id, chapter_idx, sentence_idx, note, created_at
             FROM bookmarks WHERE book_id=?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![book_id], |r| {
            Ok(BookmarkRow {
                id: r.get(0)?,
                book_id: r.get(1)?,
                chapter_idx: r.get(2)?,
                sentence_idx: r.get(3)?,
                note: r.get(4)?,
                created_at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Record a pulled model (mirrors the store manifest on disk).
    pub fn upsert_tts_model(
        &self,
        id: &str,
        backend: &str,
        hf_repo: &str,
        revision: &str,
        local_path: &str,
        total_bytes: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO tts_models (id, backend, hf_repo, revision, local_path, total_bytes, installed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))
             ON CONFLICT(id) DO UPDATE SET
               backend=excluded.backend, hf_repo=excluded.hf_repo,
               revision=excluded.revision, local_path=excluded.local_path,
               total_bytes=excluded.total_bytes, installed_at=datetime('now')",
            params![id, backend, hf_repo, revision, local_path, total_bytes],
        )?;
        Ok(())
    }

    pub fn list_tts_models(&self) -> Result<Vec<TtsModelRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, backend, hf_repo, revision, local_path, total_bytes, installed_at
             FROM tts_models ORDER BY installed_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(TtsModelRow {
                id: r.get(0)?,
                backend: r.get(1)?,
                hf_repo: r.get(2)?,
                revision: r.get(3)?,
                local_path: r.get(4)?,
                total_bytes: r.get(5)?,
                installed_at: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn remove_tts_model(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM tts_models WHERE id=?1", params![id])?;
        Ok(())
    }

    pub fn add_voice(
        &self,
        id: &str,
        name: &str,
        model_id: &str,
        reference_path: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO voices (id, name, model_id, reference_path) VALUES (?1,?2,?3,?4)
             ON CONFLICT(id) DO UPDATE SET
               name=excluded.name, model_id=excluded.model_id,
               reference_path=excluded.reference_path",
            params![id, name, model_id, reference_path],
        )?;
        Ok(())
    }

    pub fn list_voices(&self, model_id: &str) -> Result<Vec<VoiceRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, model_id, reference_path, created_at
             FROM voices WHERE model_id=?1 ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![model_id], |r| {
            Ok(VoiceRow {
                id: r.get(0)?,
                name: r.get(1)?,
                model_id: r.get(2)?,
                reference_path: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Every registered reference clip, regardless of model. A sample is a
    /// wav on disk — it is usable by any engine (kokoro via OpenVoice,
    /// chatterbox natively), so voices must not be locked to one model.
    pub fn list_all_voices(&self) -> Result<Vec<VoiceRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, model_id, reference_path, created_at
             FROM voices ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(VoiceRow {
                id: r.get(0)?,
                name: r.get(1)?,
                model_id: r.get(2)?,
                reference_path: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn remove_voice(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM voices WHERE id=?1", params![id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_roundtrip() {
        let db = LibraryDb::in_memory().unwrap();
        db.upsert_book("b1", "/books/dune.epub", "Dune", "Frank Herbert", "epub")
            .unwrap();
        db.save_position("b1", 3, 42, 0.47).unwrap();
        let pos = db.get_position("b1").unwrap().unwrap();
        assert_eq!(pos, (3, 42, 0.47));
        let recent = db.recent_books(10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].title, "Dune");
        db.add_bookmark("b1", 3, 42, "test").unwrap();
        assert_eq!(db.bookmarks_for("b1").unwrap().len(), 1);
    }

    #[test]
    fn voices_are_model_agnostic() {
        let db = LibraryDb::in_memory().unwrap();
        db.add_voice("clarke", "Clarke", "kokoro", "/data/voices/clarke.wav")
            .unwrap();
        // A reference clip is a wav on disk: switching to chatterbox-turbo
        // must not make it vanish (that left the reader with a fake
        // "<model>-default" voice the worker rejects).
        let all = db.list_all_voices().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "clarke");
        assert!(db.list_voices("chatterbox-turbo").unwrap().is_empty());
        assert_eq!(db.list_voices("kokoro").unwrap().len(), 1);
    }

    #[test]
    fn voice_library_roundtrip() {
        let db = LibraryDb::in_memory().unwrap();
        assert!(db.list_voices("kokoro").unwrap().is_empty());
        db.add_voice("sarah", "Sarah", "kokoro", "/data/voices/sarah.wav")
            .unwrap();
        db.add_voice("deep", "Deep", "kokoro", "/data/voices/deep.wav")
            .unwrap();
        let rows = db.list_voices("kokoro").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "Sarah");
        assert!(db.list_voices("chatterbox").unwrap().is_empty());
        db.remove_voice("sarah").unwrap();
        assert_eq!(db.list_voices("kokoro").unwrap().len(), 1);
    }

    #[test]
    fn tts_model_registry_roundtrip() {
        let db = LibraryDb::in_memory().unwrap();
        db.upsert_tts_model(
            "kokoro",
            "kokoro",
            "hexgrad/Kokoro-82M",
            "abc123",
            "/data/models/kokoro",
            320_000_000,
        )
        .unwrap();
        let rows = db.list_tts_models().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].hf_repo, "hexgrad/Kokoro-82M");
        // Re-pull updates revision in place.
        db.upsert_tts_model(
            "kokoro",
            "kokoro",
            "hexgrad/Kokoro-82M",
            "def456",
            "/data/models/kokoro",
            321_000_000,
        )
        .unwrap();
        let rows = db.list_tts_models().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].revision, "def456");
        db.remove_tts_model("kokoro").unwrap();
        assert!(db.list_tts_models().unwrap().is_empty());
    }
}
