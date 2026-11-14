//! Speeding speech up without raising its pitch.
//!
//! A voice that can set its own pace (Kokoro, or Phonon within its range)
//! always sounds best doing so. This covers the rest: when the listener asks
//! for 1.75x and the voice tops out near 1.3x, the remaining factor is made up
//! here with WSOLA (waveform similarity overlap-add). Short overlapping slices
//! are taken from the speech at a faster stride than they are laid down, each
//! one nudged to where it lines up best with the last, so the result is
//! shorter with no change of pitch and no chipmunk effect. It is what podcast
//! players do, and it sounds clean on a single voice.

/// `samples` played `factor` times faster (factor above 1).
pub fn speed_up(samples: &[f32], factor: f32, sample_rate: u32) -> Vec<f32> {
    let rate = sample_rate as f32;
    let frame = (0.030 * rate) as usize;
    let hop_out = frame / 2;
    let tolerance = (0.012 * rate) as usize;
    if factor <= 1.01 || samples.len() < frame * 4 {
        return samples.to_vec();
    }
    let window: Vec<f32> = (0..frame)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / frame as f32).cos())
        .collect();

    let out_length = (samples.len() as f32 / factor) as usize + frame;
    let mut out = vec![0.0f32; out_length];
    let mut weight = vec![0.0f32; out_length];
    let mut out_position = 0usize;
    let mut previous: Option<usize> = None;

    loop {
        let nominal = (out_position as f32 * factor) as usize;
        if nominal + frame + tolerance >= samples.len() || out_position + frame >= out_length {
            break;
        }
        let chosen = match previous {
            None => nominal,
            Some(previous) => {
                // Where the last slice would naturally have continued.
                let target = previous + hop_out;
                let low = nominal.saturating_sub(tolerance);
                let high = (nominal + tolerance).min(samples.len() - frame);
                let mut best = nominal;
                let mut best_score = f32::MIN;
                for candidate in low..=high {
                    if target + hop_out > samples.len() {
                        break;
                    }
                    let mut score = 0.0;
                    // Every other sample is plenty to find the alignment.
                    for i in (0..hop_out).step_by(2) {
                        score += samples[candidate + i] * samples[target + i];
                    }
                    if score > best_score {
                        best_score = score;
                        best = candidate;
                    }
                }
                best
            }
        };
        for i in 0..frame {
            out[out_position + i] += samples[chosen + i] * window[i];
            weight[out_position + i] += window[i];
        }
        previous = Some(chosen);
        out_position += hop_out;
    }

    let end = (out_position + hop_out).min(out_length);
    out.truncate(end);
    for (sample, weight) in out.iter_mut().zip(&weight) {
        if *weight > 1e-3 {
            *sample /= weight;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortens_by_the_factor_and_keeps_pitch() {
        let rate = 24_000;
        // Two seconds of a 220 Hz tone.
        let tone: Vec<f32> =
            (0..rate * 2).map(|i| (std::f32::consts::TAU * 220.0 * i as f32 / rate as f32).sin() * 0.5).collect();
        let faster = speed_up(&tone, 1.5, rate);
        let expected = tone.len() as f32 / 1.5;
        assert!((faster.len() as f32 - expected).abs() < expected * 0.05, "{} vs {expected}", faster.len());
        // Pitch is unchanged: count zero crossings over the middle stretch.
        let crossings = |x: &[f32]| x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f32;
        let middle = &faster[rate as usize / 4..rate as usize / 4 + rate as usize / 2];
        let per_second = crossings(middle) * 2.0;
        assert!((per_second - 220.0).abs() < 8.0, "{per_second} Hz");
    }

    #[test]
    fn leaves_normal_speed_alone() {
        let samples = vec![0.25; 10_000];
        assert_eq!(speed_up(&samples, 1.0, 24_000), samples);
    }
}
