//! Phonon 31m: the expressive voice.
//!
//! Phonon (Gradium, built on Kyutai's Pocket TTS, Apache-2.0) generates
//! speech 80 ms at a time with a small flow-matching transformer and Kyutai's
//! Mimi codec. Measured against Kokoro on the same fiction passage, its
//! narrator voices move their pitch about 50 to 70% more, which is what
//! listeners hear as inflection, at almost the same naturalness score and
//! accuracy, while running about 14 times faster on two CPU cores and
//! downloading a fifth as much. See `README.md` for the numbers.
//!
//! Phonon reads plain text and normalizes it itself; `speech_text::prepare`
//! still runs first, because Phonon reads "Chapter IV" as "chat to wife".

use anyhow::{anyhow, Context, Result};
use ptts::checkpoint::{Checkpoint, ResolveOptions};
use ptts::preprocess::Lang;
use ptts::synth::{DeviceKind, Quant, SpeechOptions, Synth};
use std::collections::HashMap;
use std::path::Path;

use crate::{speech_text, stretch, SAMPLE_RATE};

/// A Phonon voice offered for narration: name, file id in the
/// `gradium/phonon-voices` catalog, accent and gender.
pub struct PhononVoice {
    pub name: &'static str,
    pub id: &'static str,
    pub accent: &'static str,
    pub gender: &'static str,
}

const fn voice(name: &'static str, id: &'static str, accent: &'static str, gender: &'static str) -> PhononVoice {
    PhononVoice { name, id, accent, gender }
}

/// The narrator voices, best first. Chosen by listening tests scored for
/// naturalness, pitch movement and accuracy on fiction; the catalog's many
/// customer-service and character voices are left out.
pub const VOICES: [PhononVoice; 13] = [
    voice("Harper", "4SZHfMpw-p46Ywgs", "US", "female"),
    voice("Maeve", "6PWnV0Nq4wu7RVBT", "UK", "female"),
    voice("Juniper", "NIWIOv5vTi5eRMj8", "US", "female"),
    voice("Imogen", "gDT1nz7Ie36ZhL-C", "UK", "female"),
    voice("Hadley", "bXwrkHhURL1VFJdG", "US", "female"),
    voice("Everly", "vjFyp2BcCyTlUmYb", "US", "female"),
    voice("Delia", "nGzMHMPAWGaJFHOo", "US", "female"),
    voice("Bryony", "gfP44RlHwr2kEwk7", "UK", "female"),
    voice("Hugh", "YyKcxPG7iIgS3jsK", "UK", "male"),
    voice("Spencer", "JOW0vchyswkQUMDS", "UK", "male"),
    voice("Miles", "d9XCyhjo4JhkHF1d", "US", "male"),
    voice("Trevor", "0zLApsZfVLLNaEGl", "US", "male"),
    voice("Preston", "qIxlZ7Ei8KvWlyQH", "US", "male"),
];

/// How freely the model varies its delivery. Swept on fiction: 0.3 is calm
/// and flat, 0.85 to 0.9 is the most expressive before naturalness drops,
/// 1.2 starts to wobble.
const TEMPERATURE: f32 = 0.85;

/// The fastest Phonon speaks on its own (through its `duration_delta`
/// condition) before its pace stops responding. Faster speeds are reached
/// by speeding that up with `stretch`.
const NATURAL_MAX_SPEED: f32 = 1.3;

pub struct Phonon {
    synth: Synth,
    voices: Vec<String>,
    duration_delta: Option<String>,
}

impl Phonon {
    /// Loads a checkpoint folder (`config.json`, `tokenizer.json`,
    /// `model.q8.gguf`) and the voice files in its `voices/` folder.
    pub fn load(dir: &Path, threads: Option<usize>) -> Result<Self> {
        if let Some(threads) = threads {
            xn::set_num_threads(threads);
        }
        let checkpoint = Checkpoint::resolve(dir, ResolveOptions { quant: Quant::Q80, weights: None })
            .map_err(|error| anyhow!("the Phonon voice is incomplete: {error}"))?;
        let mut synth = checkpoint
            .builder(Lang::En)
            .device(DeviceKind::Cpu)
            .temperature(TEMPERATURE)
            .build()
            .map_err(|error| anyhow!("the Phonon voice could not be loaded: {error}"))?;
        let mut voices = Vec::new();
        for voice in &VOICES {
            let path = dir.join("voices").join(format!("{}.safetensors", voice.id));
            if path.is_file() && synth.add_voice_file(voice.name, &path).is_ok() {
                voices.push(voice.name.to_string());
            }
        }
        if voices.is_empty() {
            return Err(anyhow!("no Phonon voices were found in {}", dir.display()));
        }
        Ok(Self { synth, voices, duration_delta: None })
    }

    /// The voices that loaded, best first.
    pub fn voices(&self) -> &[String] {
        &self.voices
    }

    /// Renders text at 24 kHz in `voice` at `speed` (0.5 to 2).
    pub fn synthesize(&mut self, text: &str, voice: &str, speed: f32) -> Result<Vec<f32>> {
        let voice = if self.voices.iter().any(|name| name == voice) { voice } else { &self.voices[0] };
        let speed = speed.clamp(0.5, 2.0);
        let natural = speed.min(NATURAL_MAX_SPEED);
        let delta = format!("{:.2}", duration_delta_for(natural));
        if self.duration_delta.as_deref() != Some(delta.as_str()) {
            self.synth
                .set_conditions(HashMap::from([("duration_delta".to_string(), delta.clone())]))
                .map_err(|error| anyhow!("could not set the reading speed: {error}"))?;
            self.duration_delta = Some(delta);
        }

        let text = speech_text::prepare(text);
        if !crate::narrator::has_words(&text) {
            return Ok(crate::narrator::scene_break(speed));
        }
        // The same sentence always gets the same take, so going back a
        // sentence replays it as it was rather than a new reading of it.
        let options = SpeechOptions::default().voice(voice).temperature(TEMPERATURE).seed(seed_for(&text));
        let samples = self
            .synth
            .say_with(&text, &options)
            .map_err(|error| anyhow!("the voice could not read that: {error}"))
            .with_context(|| format!("reading \"{text}\""))?;
        debug_assert_eq!(self.synth.sample_rate(), SAMPLE_RATE);
        Ok(if speed > natural { stretch::speed_up(&samples, speed / natural, SAMPLE_RATE) } else { samples })
    }
}

/// Phonon's `duration_delta` for a speaking speed, from measurements: 0 is
/// its normal pace, +0.3 about 0.86x, +1.0 about 0.8x, -0.3 about 1.25x and
/// -0.45 about 1.3x. In between, linear.
fn duration_delta_for(speed: f32) -> f32 {
    const POINTS: [(f32, f32); 5] = [(0.8, 1.0), (0.86, 0.3), (1.0, 0.0), (1.25, -0.3), (1.3, -0.45)];
    let speed = speed.clamp(POINTS[0].0, POINTS[POINTS.len() - 1].0);
    for pair in POINTS.windows(2) {
        let ((s0, d0), (s1, d1)) = (pair[0], pair[1]);
        if speed <= s1 {
            return d0 + (d1 - d0) * (speed - s0) / (s1 - s0);
        }
    }
    POINTS[POINTS.len() - 1].1
}

fn seed_for(text: &str) -> u64 {
    // FNV-1a: stable across runs and platforms, unlike the std hasher.
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_maps_to_duration_delta() {
        assert_eq!(duration_delta_for(1.0), 0.0);
        assert!((duration_delta_for(0.8) - 1.0).abs() < 1e-6);
        assert!((duration_delta_for(1.3) + 0.45).abs() < 1e-6);
        assert!(duration_delta_for(1.1) < 0.0 && duration_delta_for(1.1) > -0.3);
        // Beyond the natural range it holds at the end.
        assert!((duration_delta_for(2.0) + 0.45).abs() < 1e-6);
    }

    #[test]
    fn seeds_are_stable() {
        assert_eq!(seed_for("It was over."), seed_for("It was over."));
        assert_ne!(seed_for("It was over."), seed_for("It was not over."));
    }
}
