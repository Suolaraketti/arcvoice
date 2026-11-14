//! The shared engine behind the ArcVoice desktop app: read a PDF, clean the
//! text, split it into sentences, and speak them with Kokoro on this machine.

pub mod fallback;
pub mod library;
pub mod models;
pub mod pdf;
pub mod text;
pub mod tts;

pub use library::{Book, Library, PreparedImport, ReadingPosition, Settings};
pub use tts::Kokoro;

/// Kokoro always produces 24 kHz mono audio.
pub const SAMPLE_RATE: u32 = 24_000;
