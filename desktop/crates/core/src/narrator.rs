//! Every voice the app offers, from both engines, behind one interface.
//!
//! Phonon voices are the default: more expressive, faster and a smaller
//! download. Kokoro's voices stay available as "classic" voices for anyone
//! who prefers their steadier read; they are downloaded only when wanted.
//! Voice ids say which engine they belong to: Phonon ids start with
//! `phonon/`, and Kokoro ids are Kokoro's own names (`af_heart`), as saved by
//! earlier versions of the app.

use anyhow::{anyhow, Result};

use crate::phonon::{self, Phonon};
use crate::tts::{voice_display_name, Kokoro};

pub const PHONON_PREFIX: &str = "phonon/";
pub const DEFAULT_VOICE: &str = "phonon/Harper";

/// A voice as the voice picker shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceChoice {
    pub id: String,
    /// "Harper (US, female)".
    pub label: String,
    /// True for Kokoro's classic voices.
    pub classic: bool,
}

pub fn is_classic_voice(id: &str) -> bool {
    !id.starts_with(PHONON_PREFIX)
}

#[derive(Default)]
pub struct Narrator {
    phonon: Option<Phonon>,
    kokoro: Option<Kokoro>,
}

impl Narrator {
    pub fn new(phonon: Option<Phonon>, kokoro: Option<Kokoro>) -> Self {
        Self { phonon, kokoro }
    }

    pub fn set_kokoro(&mut self, kokoro: Kokoro) {
        self.kokoro = Some(kokoro);
    }

    pub fn has_kokoro(&self) -> bool {
        self.kokoro.is_some()
    }

    pub fn has_phonon(&self) -> bool {
        self.phonon.is_some()
    }

    /// Phonon's voices first, best first, then Kokoro's classic voices.
    pub fn voices(&self) -> Vec<VoiceChoice> {
        let mut choices = Vec::new();
        if let Some(phonon) = &self.phonon {
            for name in phonon.voices() {
                let info = phonon::VOICES.iter().find(|voice| voice.name == name);
                let label = match info {
                    Some(info) => format!("{name} ({}, {})", info.accent, info.gender),
                    None => name.clone(),
                };
                choices.push(VoiceChoice { id: format!("{PHONON_PREFIX}{name}"), label, classic: false });
            }
        }
        if let Some(kokoro) = &self.kokoro {
            for name in kokoro.english_voices() {
                choices.push(VoiceChoice { label: voice_display_name(&name), id: name, classic: true });
            }
        }
        choices
    }

    /// The voice to use when the saved one is not available.
    pub fn fallback_voice(&self) -> String {
        if self.phonon.is_some() {
            DEFAULT_VOICE.to_string()
        } else {
            "af_heart".to_string()
        }
    }

    /// Renders text at 24 kHz. A voice whose engine is not loaded falls back
    /// to the other engine's default rather than failing.
    pub fn synthesize(&mut self, text: &str, voice: &str, speed: f32) -> Result<Vec<f32>> {
        // A scene break ("* * *", "***", a lone dash) has nothing to say.
        // Phonon refuses such text, so it becomes a beat of silence; the
        // pause that follows it is added by `pacing` as for any sentence.
        if !has_words(text) {
            return Ok(scene_break(speed));
        }
        match (voice.strip_prefix(PHONON_PREFIX), &mut self.phonon, &mut self.kokoro) {
            (Some(name), Some(phonon), _) => phonon.synthesize(text, name, speed),
            (None, _, Some(kokoro)) => kokoro.synthesize(text, voice, speed),
            (_, Some(phonon), None) => phonon.synthesize(text, phonon::VOICES[0].name, speed),
            (_, None, Some(kokoro)) => kokoro.synthesize(text, "af_heart", speed),
            (_, None, None) => Err(anyhow!("no voice is loaded")),
        }
    }

    /// True when `synthesize` has a voice to render with.
    pub fn is_ready(&self) -> bool {
        self.phonon.is_some() || self.kokoro.is_some()
    }

    /// The phonemes Kokoro would use, for diagnosing a pronunciation.
    pub fn phonemize(&mut self, text: &str, british: bool) -> Result<String> {
        match &mut self.kokoro {
            Some(kokoro) => kokoro.phonemize(text, british),
            None => Err(anyhow!("phonemes are only available for the classic voices")),
        }
    }
}

/// True when the text has something to pronounce: a letter or a digit.
pub fn has_words(text: &str) -> bool {
    text.chars().any(char::is_alphanumeric)
}

/// The silence that stands in for a scene break.
pub fn scene_break(speed: f32) -> Vec<f32> {
    vec![0.0; (0.6 / speed.max(0.5) * crate::SAMPLE_RATE as f32) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_breaks_are_silence_not_errors() {
        let mut narrator = Narrator::default();
        for text in ["* * *", "***", "\u{2014}", "...", "#", "~"] {
            let samples = narrator.synthesize(text, DEFAULT_VOICE, 1.0).unwrap();
            assert!(!samples.is_empty() && samples.iter().all(|&s| s == 0.0), "{text}");
        }
        // Real text with no voice loaded is still an error.
        assert!(narrator.synthesize("Chapter 3", DEFAULT_VOICE, 1.0).is_err());
    }
}
