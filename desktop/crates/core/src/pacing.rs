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
    /// The first part of a sentence spoken ahead of the rest so playback
    /// starts sooner. Only a breath follows it.
    Clause,
}

/// Seconds of silence to put after `sentence` at `speed`.
pub fn pause_after(sentence: &str, after: Break, speed: f32) -> f32 {
    let trimmed = sentence.trim_end_matches(['"', '\'', '\u{201D}', '\u{2019}', ')', ' ']);
    let seconds = if after == Break::Clause {
        // A comma's breath, or almost none when cut between words.
        if trimmed.ends_with([',', ';', ':', '\u{2014}', '\u{2013}']) { 0.2 } else { 0.05 }
    } else if text::is_heading(sentence) {
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

/// The speaking level every sentence is drawn toward: the RMS of its voiced
/// part, in dBFS. About where Kokoro and most Phonon voices sit already.
const TARGET_LEVEL_DB: f32 = -21.0;
/// How much of the gap to the target is closed. Not all of it: a whispered
/// line should stay quieter than a shouted one, just not by a startling jump.
const LEVEL_CORRECTION: f32 = 0.6;
/// The most any sentence is turned up or down.
const MAX_LEVEL_CHANGE_DB: f32 = 4.0;

/// The RMS level of the voiced 20 ms frames of `samples`, in dBFS.
fn speech_level_db(samples: &[f32], sample_rate: u32) -> Option<f32> {
    let frame = (0.02 * sample_rate as f32) as usize;
    let mut sum = 0.0f64;
    let mut count = 0usize;
    for chunk in samples.chunks(frame.max(1)) {
        let energy = chunk.iter().map(|s| (*s as f64) * (*s as f64)).sum::<f64>() / chunk.len() as f64;
        // Frames above -40 dBFS are speech; below is breath and silence.
        if energy > 1e-4 {
            sum += energy * chunk.len() as f64;
            count += chunk.len();
        }
    }
    (count > 0).then(|| (10.0 * (sum / count as f64).log10()) as f32)
}

/// Draws a sentence's loudness toward a common level, so sentences and
/// voices sit at an even volume over hours of listening, with a soft limit
/// so turning one up can never clip.
pub fn even_out_loudness(samples: &mut [f32], sample_rate: u32) {
    let Some(level) = speech_level_db(samples, sample_rate) else { return };
    let change = ((TARGET_LEVEL_DB - level) * LEVEL_CORRECTION).clamp(-MAX_LEVEL_CHANGE_DB, MAX_LEVEL_CHANGE_DB);
    let gain = 10f32.powf(change / 20.0);
    for sample in samples.iter_mut() {
        let value = *sample * gain;
        // Above 0.9 bend smoothly toward 1.0 instead of clipping.
        *sample = if value.abs() <= 0.9 {
            value
        } else {
            value.signum() * (0.9 + 0.1 * ((value.abs() - 0.9) / 0.1).tanh())
        };
    }
}

/// A rendered sentence ready to queue: trimmed, evened out in loudness, with
/// its pause after it, and a few milliseconds of fade at each edge so no
/// click is heard.
pub fn finish_sentence(samples: &[f32], sample_rate: u32, sentence: &str, after: Break, speed: f32) -> Vec<f32> {
    let spoken = trim_silence(samples, sample_rate);
    let pause = (pause_after(sentence, after, speed) * sample_rate as f32) as usize;
    let mut out = Vec::with_capacity(spoken.len() + pause);
    out.extend_from_slice(spoken);
    even_out_loudness(&mut out, sample_rate);
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
        assert_eq!(pause_after("When it was over,", Break::Clause, 1.0), 0.2);
        assert_eq!(pause_after("When it was", Break::Clause, 1.0), 0.05);
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
    fn loudness_moves_toward_the_target_but_not_all_the_way() {
        let rate = 24_000;
        let tone = |amplitude: f32| -> Vec<f32> {
            (0..rate).map(|i| amplitude * (std::f32::consts::TAU * 200.0 * i as f32 / rate as f32).sin()).collect()
        };
        let level = |x: &[f32]| speech_level_db(x, rate).unwrap();
        // A quiet sentence (about -33 dB) comes up, by the capped 4 dB.
        let mut quiet = tone(0.03);
        let before = level(&quiet);
        even_out_loudness(&mut quiet, rate);
        assert!((level(&quiet) - before - 4.0).abs() < 0.2, "{} -> {}", before, level(&quiet));
        // One near the target barely moves.
        let mut normal = tone(0.125);
        let before = level(&normal);
        even_out_loudness(&mut normal, rate);
        assert!((level(&normal) - before).abs() < 1.0);
        // A loud one comes down and never clips.
        let mut loud = tone(0.99);
        even_out_loudness(&mut loud, rate);
        assert!(loud.iter().all(|s| s.abs() <= 1.0));
        assert!(level(&loud) < -3.0);
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
