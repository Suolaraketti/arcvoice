//! The silences between sentences, which decide whether a long listen feels
//! like a narrator or a machine.
//!
//! Kokoro renders each sentence with an unpredictable bit of silence at either
//! end. Left alone, the gaps between sentences wander from a blink to a long
//! hang. So each sentence's own silence is trimmed to a fixed, short margin
//! and a deliberate pause is added: short within a paragraph, longer where a
//! paragraph ends, longest around a heading. The lengths follow common
//! audiobook practice and shrink as the reading speed goes up.

use crate::text;

/// What comes after a sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Break {
    Sentence,
    Paragraph,
}

/// Seconds of silence to put after `sentence` at `speed`.
pub fn pause_after(sentence: &str, after: Break, speed: f32) -> f32 {
    let trimmed = sentence.trim_end_matches(['"', '\'', '\u{201D}', '\u{2019}', ')', ' ']);
    let seconds = if text::is_heading(sentence) {
        1.1
    } else if after == Break::Paragraph {
        0.75
    } else if trimmed.ends_with(['\u{2026}', '\u{2014}', '\u{2013}']) || trimmed.ends_with("...") {
        // Trailing off, or cut off mid-thought: let it hang a moment.
        0.55
    } else if trimmed.ends_with([',', ';', ':']) {
        // Only happens when a very long sentence was split at a clause.
        0.25
    } else {
        0.42
    };
    seconds / speed.clamp(0.5, 2.0)
}

/// Samples quieter than this are treated as silence. Kokoro's breaths and
/// the tails of words sit well above it.
const SILENCE: f32 = 0.006;
/// Silence kept before the first sound, so a plosive keeps its attack.
const LEAD_SECONDS: f32 = 0.02;
/// Silence kept after the last sound, so a word's decay is not clipped.
const TAIL_SECONDS: f32 = 0.06;

/// The part of a rendered sentence worth playing: its silent ends cut down to
/// short, fixed margins.
pub fn trim_silence(samples: &[f32], sample_rate: u32) -> &[f32] {
    let first = samples.iter().position(|sample| sample.abs() > SILENCE);
    let last = samples.iter().rposition(|sample| sample.abs() > SILENCE);
    let (Some(first), Some(last)) = (first, last) else {
        return &samples[..0];
    };
    let lead = (LEAD_SECONDS * sample_rate as f32) as usize;
    let tail = (TAIL_SECONDS * sample_rate as f32) as usize;
    let start = first.saturating_sub(lead);
    let end = (last + 1 + tail).min(samples.len());
    &samples[start..end]
}

/// A rendered sentence ready to queue: trimmed, with its pause after it, and
/// with a few milliseconds of fade at each edge so no click is heard.
pub fn finish_sentence(samples: &[f32], sample_rate: u32, sentence: &str, after: Break, speed: f32) -> Vec<f32> {
    let spoken = trim_silence(samples, sample_rate);
    let pause = (pause_after(sentence, after, speed) * sample_rate as f32) as usize;
    let mut out = Vec::with_capacity(spoken.len() + pause);
    out.extend_from_slice(spoken);
    let fade = ((0.004 * sample_rate as f32) as usize).min(out.len() / 2);
    for index in 0..fade {
        let gain = index as f32 / fade as f32;
        out[index] *= gain;
        let from_end = out.len() - 1 - index;
        out[from_end] *= gain;
    }
    out.resize(out.len() + pause, 0.0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pauses_follow_the_text() {
        assert!(pause_after("Chapter Three", Break::Sentence, 1.0) > 1.0);
        assert_eq!(pause_after("It was over.", Break::Paragraph, 1.0), 0.75);
        assert_eq!(pause_after("It was over.", Break::Sentence, 1.0), 0.42);
        assert_eq!(pause_after("\u{201C}I don\u{2019}t\u{2014}\u{201D}", Break::Sentence, 1.0), 0.55);
        assert_eq!(pause_after("It was over.", Break::Sentence, 2.0), 0.21);
    }

    #[test]
    fn trims_silence_to_fixed_margins() {
        let rate = 1_000;
        let mut samples = vec![0.0; 300];
        samples.extend(vec![0.5; 100]);
        samples.extend(vec![0.0; 500]);
        let trimmed = trim_silence(&samples, rate);
        // 20 ms lead + 100 sound + 60 ms tail at 1 kHz.
        assert_eq!(trimmed.len(), 20 + 100 + 60);
        assert!(trim_silence(&[0.0; 50], rate).is_empty());
    }

    #[test]
    fn finished_sentence_has_its_pause() {
        let rate = 1_000;
        let mut samples = vec![0.0; 100];
        samples.extend(vec![0.5; 100]);
        let finished = finish_sentence(&samples, rate, "Done.", Break::Paragraph, 1.0);
        assert_eq!(finished.len(), 20 + 100 + 750);
        assert_eq!(finished[0], 0.0);
    }
}
