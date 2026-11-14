//! The books on this computer, and where the reader is in each one.
//!
//! Everything lives in the user's app data folder:
//!
//! ```text
//! ArcVoice/
//!   settings.json
//!   models/                   the Kokoro voice
//!   Library/
//!     library.json            list of books
//!     <book id>/
//!       book.pdf or book.epub the original file
//!       sentences.json        extracted text, one string per sentence
//!       paragraphs.json       indices of the sentences that end a paragraph
//!       position.json         where she stopped
//! ```
//!
//! The JSON uses the same field names as the Apple app, so a position file
//! written on one device can be read on another.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{epub, pdf, text};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Book {
    pub id: String,
    pub title: String,
    /// SHA-256 of the original file's bytes. The same file on another device
    /// produces the same key, which is what sync matches on.
    pub book_key: String,
    pub page_count: usize,
    pub sentence_count: usize,
    pub added_at: String,
    #[serde(default)]
    pub last_opened_at: Option<String>,
    /// Mirror of the saved position, so the library list can show progress.
    #[serde(default)]
    pub sentence_index: usize,
}

impl Book {
    pub fn progress(&self) -> f32 {
        if self.sentence_count > 1 {
            (self.sentence_index as f32 / (self.sentence_count - 1) as f32).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

/// Where the reader is in a book.
///
/// `sentence_index` is exact when both devices split the text the same way.
/// `anchor` is the sentence text itself, so a device whose PDF reader split
/// things slightly differently can still find the spot by looking for it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReadingPosition {
    pub version: u32,
    pub book_key: String,
    pub sentence_index: usize,
    pub sentence_count: usize,
    pub fraction: f64,
    pub anchor: String,
    /// ISO 8601 in UTC, for example `2026-11-14T05:12:09Z`. Strings in this
    /// form sort in time order, which is how the newer position is chosen.
    pub updated_at: String,
    pub device: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub voice: String,
    pub speed: f32,
    /// A folder shared between devices (an iCloud Drive or OneDrive folder).
    /// Positions are mirrored there so another device can pick them up.
    pub sync_folder: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { voice: "af_heart".to_string(), speed: 1.0, sync_folder: None }
    }
}

pub struct Library {
    root: PathBuf,
    pub books: Vec<Book>,
    pub settings: Settings,
}

impl Library {
    /// Opens the library in the standard app data folder.
    pub fn open_default() -> Result<Self> {
        Self::open(app_data_dir()?)
    }

    pub fn open(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(root.join("Library"))
            .with_context(|| format!("could not create {}", root.display()))?;
        let books = read_json(&root.join("Library").join("library.json")).unwrap_or_default();
        let settings = read_json(&root.join("settings.json")).unwrap_or_default();
        Ok(Self { root, books, settings })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn models_dir(&self) -> PathBuf {
        self.root.join("models")
    }

    fn book_dir(&self, id: &str) -> PathBuf {
        self.root.join("Library").join(id)
    }

    fn save_index(&self) -> Result<()> {
        write_json(&self.root.join("Library").join("library.json"), &self.books)
    }

    pub fn save_settings(&self) -> Result<()> {
        write_json(&self.root.join("settings.json"), &self.settings)
    }

    /// Books with the most recently opened first.
    pub fn sorted_books(&self) -> Vec<Book> {
        let mut books = self.books.clone();
        books.sort_by(|a, b| {
            let a_time = a.last_opened_at.as_ref().unwrap_or(&a.added_at);
            let b_time = b.last_opened_at.as_ref().unwrap_or(&b.added_at);
            b_time.cmp(a_time)
        });
        books
    }

    /// Copies a PDF or EPUB into the library and extracts its text. Importing
    /// a file that is already in the library returns the existing book.
    pub fn import_book(&mut self, source: &Path) -> Result<Book> {
        let prepared = PreparedImport::read(source)?;
        self.add(prepared)
    }

    /// Stores a book whose text has already been extracted. This part is
    /// quick; the slow extraction happens in `PreparedImport::read`, which
    /// needs no access to the library and can run on another thread.
    pub fn add(&mut self, prepared: PreparedImport) -> Result<Book> {
        if let Some(existing) = self.books.iter().find(|book| book.book_key == prepared.book_key) {
            return Ok(existing.clone());
        }
        let id = prepared.book_key[..16].to_string();
        let dir = self.book_dir(&id);
        fs::create_dir_all(&dir)?;
        fs::write(dir.join(format!("book.{}", prepared.extension)), &prepared.bytes)?;
        write_json(&dir.join("sentences.json"), &prepared.sentences)?;
        write_json(&dir.join("paragraphs.json"), &prepared.paragraph_ends)?;

        let book = Book {
            id,
            title: prepared.title,
            book_key: prepared.book_key,
            page_count: prepared.page_count,
            sentence_count: prepared.sentences.len(),
            added_at: now_iso8601(),
            last_opened_at: None,
            sentence_index: 0,
        };
        self.books.push(book.clone());
        self.save_index()?;
        Ok(book)
    }

    pub fn sentences(&self, book: &Book) -> Result<Vec<String>> {
        read_json(&self.book_dir(&book.id).join("sentences.json"))
            .ok_or_else(|| anyhow!("the text for \"{}\" is missing; import the book again", book.title))
    }

    /// Indices of the sentences that end a paragraph. Books added before
    /// paragraphs were tracked have none, and are paced sentence by sentence.
    pub fn paragraph_ends(&self, book: &Book) -> Vec<usize> {
        read_json(&self.book_dir(&book.id).join("paragraphs.json")).unwrap_or_default()
    }

    pub fn mark_opened(&mut self, id: &str) {
        if let Some(book) = self.books.iter_mut().find(|book| book.id == id) {
            book.last_opened_at = Some(now_iso8601());
            let _ = self.save_index();
        }
    }

    pub fn delete(&mut self, id: &str) -> Result<()> {
        let dir = self.book_dir(id);
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        self.books.retain(|book| book.id != id);
        self.save_index()
    }

    fn sync_position_path(&self, book_key: &str) -> Option<PathBuf> {
        self.settings
            .sync_folder
            .as_ref()
            .map(|folder| folder.join(format!("{book_key}.position.json")))
    }

    /// The saved position for a book: whichever is newer of the local file and
    /// the copy in the sync folder.
    pub fn position(&self, book: &Book) -> Option<ReadingPosition> {
        let local: Option<ReadingPosition> = read_json(&self.book_dir(&book.id).join("position.json"));
        let synced: Option<ReadingPosition> = self
            .sync_position_path(&book.book_key)
            .and_then(|path| read_json(&path));
        match (local, synced) {
            (Some(local), Some(synced)) => Some(if synced.updated_at > local.updated_at { synced } else { local }),
            (local, synced) => local.or(synced),
        }
    }

    /// The sentence to start from, given this device's own split of the text.
    pub fn resume_index(&self, book: &Book, sentences: &[String]) -> usize {
        match self.position(book) {
            Some(position) => relocate(&position, sentences),
            None => book.sentence_index.min(sentences.len().saturating_sub(1)),
        }
    }

    /// Called every time the reader moves to a new sentence. The file is tiny,
    /// so writing it each time means a crash never costs more than a sentence.
    pub fn save_position(&mut self, id: &str, sentence_index: usize, anchor: &str) {
        let Some(slot) = self.books.iter().position(|book| book.id == id) else { return };
        let book = self.books[slot].clone();
        let position = ReadingPosition {
            version: 1,
            book_key: book.book_key.clone(),
            sentence_index,
            sentence_count: book.sentence_count,
            fraction: if book.sentence_count > 1 {
                sentence_index as f64 / (book.sentence_count - 1) as f64
            } else {
                0.0
            },
            anchor: anchor.chars().take(200).collect(),
            updated_at: now_iso8601(),
            device: device_name(),
        };
        let _ = write_json(&self.book_dir(id).join("position.json"), &position);
        if let Some(path) = self.sync_position_path(&book.book_key) {
            let _ = write_json(&path, &position);
        }
        if self.books[slot].sentence_index != sentence_index {
            self.books[slot].sentence_index = sentence_index;
            let _ = self.save_index();
        }
    }
}

/// A PDF or EPUB that has been read and cleaned, ready to be added to the
/// library.
pub struct PreparedImport {
    pub title: String,
    pub book_key: String,
    pub page_count: usize,
    pub sentences: Vec<String>,
    pub paragraph_ends: Vec<usize>,
    /// "pdf" or "epub", decided by the file's contents, not its name.
    extension: &'static str,
    bytes: Vec<u8>,
}

impl PreparedImport {
    /// Reads the file and extracts its text. This is the slow part of an
    /// import, a few seconds for a long novel.
    pub fn read(source: &Path) -> Result<Self> {
        let bytes = fs::read(source).with_context(|| format!("could not read {}", source.display()))?;
        let book_key = hex(&Sha256::digest(&bytes));
        let (title, page_count, sentences, paragraph_ends, extension) = if epub::looks_like_epub(&bytes) {
            let extracted = epub::extract(&bytes)?;
            (extracted.title, extracted.page_count, extracted.sentences, extracted.paragraph_ends, "epub")
        } else {
            let extracted = pdf::extract(&bytes)?;
            (extracted.title, extracted.page_count, extracted.sentences, extracted.paragraph_ends, "pdf")
        };
        let fallback_title = source
            .file_stem()
            .map(|stem| stem.to_string_lossy().replace('_', " "))
            .unwrap_or_else(|| "Untitled".to_string());
        Ok(Self {
            title: title.unwrap_or(fallback_title),
            book_key,
            page_count,
            sentences,
            paragraph_ends,
            extension,
            bytes,
        })
    }
}

/// Finds where a saved position falls in this device's sentence list.
///
/// If the counts match, the index is trusted. Otherwise the anchor text is
/// searched for, preferring the match closest to the saved fraction, and the
/// fraction alone is the last resort.
pub fn relocate(position: &ReadingPosition, sentences: &[String]) -> usize {
    if sentences.is_empty() {
        return 0;
    }
    let last = sentences.len() - 1;
    if position.sentence_count == sentences.len() {
        return position.sentence_index.min(last);
    }
    let expected = (position.fraction.clamp(0.0, 1.0) * last as f64).round() as usize;
    let anchor = normalize_for_matching(&position.anchor);
    if anchor.chars().count() >= 12 {
        let probe: String = anchor.chars().take(60).collect();
        let best = sentences
            .iter()
            .enumerate()
            .filter(|(_, sentence)| {
                let candidate = normalize_for_matching(sentence);
                candidate.starts_with(&probe) || probe.starts_with(&candidate) && candidate.chars().count() >= 12
            })
            .min_by_key(|(index, _)| index.abs_diff(expected));
        if let Some((index, _)) = best {
            return index;
        }
    }
    expected.min(last)
}

fn normalize_for_matching(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

pub fn app_data_dir() -> Result<PathBuf> {
    // The local folder, not the roaming one: the voice model is hundreds of
    // megabytes and has no business following a Windows profile around.
    let base = dirs::data_local_dir().ok_or_else(|| anyhow!("could not find the app data folder"))?;
    Ok(base.join("ArcVoice"))
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let data = fs::read(path).ok()?;
    serde_json::from_slice(&data).ok()
}

/// Writes to a temporary file first, then renames, so a crash halfway through
/// never leaves a half-written file behind.
fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(value)?)?;
    fs::rename(&temporary, path)?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "Desktop".to_string())
}

/// The current time as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn now_iso8601() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    iso8601_from_unix(seconds)
}

pub fn iso8601_from_unix(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let time = seconds % 86_400;
    // Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
    let year = year_of_era + era * 400 + if month <= 2 { 1 } else { 0 };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3_600,
        (time % 3_600) / 60,
        time % 60
    )
}

/// Re-exported so callers do not need the text module for one check.
pub fn is_heading(sentence: &str) -> bool {
    text::is_heading(sentence)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_dates() {
        assert_eq!(iso8601_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601_from_unix(1_794_632_529), "2026-11-14T05:02:09Z");
        assert_eq!(iso8601_from_unix(951_782_400), "2000-02-29T00:00:00Z");
    }

    fn position(index: usize, count: usize, anchor: &str) -> ReadingPosition {
        ReadingPosition {
            version: 1,
            book_key: "k".into(),
            sentence_index: index,
            sentence_count: count,
            fraction: index as f64 / (count - 1) as f64,
            anchor: anchor.into(),
            updated_at: "2026-11-14T05:02:09Z".into(),
            device: "test".into(),
        }
    }

    #[test]
    fn relocates_by_anchor_when_counts_differ() {
        let sentences: Vec<String> = (0..100)
            .map(|i| format!("Sentence number {i} of the story goes right here."))
            .collect();
        // Same split: trust the index.
        assert_eq!(relocate(&position(40, 100, "anything at all"), &sentences), 40);
        // Another device split the book into 120 sentences; find ours by text.
        let other = position(70, 120, "Sentence number 57 of the story goes right here.");
        assert_eq!(relocate(&other, &sentences), 57);
        // No usable anchor: fall back to the fraction.
        let vague = position(60, 120, "");
        assert_eq!(relocate(&vague, &sentences), 50);
    }

    #[test]
    fn reads_position_written_by_the_apple_app() {
        let json = r#"{"anchor":"She ran.","bookKey":"abc","device":"iPhone","fraction":0.5,
            "sentenceCount":11,"sentenceIndex":5,"updatedAt":"2026-11-14T05:02:09Z","version":1}"#;
        let parsed: ReadingPosition = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.sentence_index, 5);
        assert_eq!(parsed.device, "iPhone");
    }
}
