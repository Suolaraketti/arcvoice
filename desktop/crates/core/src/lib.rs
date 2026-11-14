//! The shared engine behind the ArcVoice desktop app: read a PDF or EPUB, clean the
//! text, split it into sentences, and speak them with Kokoro on this machine.

pub mod epub;
pub mod fallback;
pub mod g2p_nn;
pub mod library;
pub mod narrator;
pub mod pacing;
pub mod models;
pub mod pdf;
pub mod phonon;
pub mod pronounce;
pub mod speech_text;
pub mod stretch;
pub mod text;
pub mod tts;

pub use library::{Book, Library, PreparedImport, ReadingPosition, Settings};
pub use narrator::{Narrator, VoiceChoice};
pub use phonon::Phonon;
pub use tts::{Kokoro, LoadOptions};

/// Kokoro always produces 24 kHz mono audio.
pub const SAMPLE_RATE: u32 = 24_000;
