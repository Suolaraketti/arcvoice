//! Kokoro text to speech, running on this machine through ONNX Runtime.
//!
//! The pipeline for one sentence:
//!   text -> spoken form (`speech_text`: times, numerals, addresses)
//!        -> phonemes (misaki, with `pronounce` for words it does not know)
//!        -> token ids (Kokoro's vocabulary)
//!        -> the model, with a voice style vector -> 24 kHz audio samples

use anyhow::{anyhow, bail, Context, Result};
use misaki_rs::{Language, G2P};
use ort::environment::Environment;
use ort::session::Session;
use ort::value::Tensor;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::pronounce::WordResolver;
use crate::speech_text;

/// Kokoro accepts at most 510 phoneme tokens per call.
const MAX_TOKENS: usize = 510;
/// Stay under the limit so a chunk never needs to be cut mid-word.
const TOKEN_BUDGET: usize = 480;
const STYLE_WIDTH: usize = 256;

/// How to run the voice model.
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Threads for one sentence. `None` lets ONNX Runtime choose.
    pub threads: Option<usize>,
    /// Hand what CoreML can run to the Neural Engine (Apple only). The GPU
    /// is left out on purpose: iOS forbids it while the app is in the
    /// background, which is where an audiobook app spends its life.
    pub coreml: bool,
    /// Where CoreML keeps its compiled model between launches.
    pub coreml_cache: Option<std::path::PathBuf>,
}

pub struct Kokoro {
    session: Session,
    // The environment must outlive the session.
    _environment: Environment,
    vocab: HashMap<char, i64>,
    /// Voice name to its style table: 510 rows of 256 numbers. The row used
    /// depends on how many tokens are being spoken.
    voices: HashMap<String, Vec<f32>>,
    american: G2P,
    british: Option<G2P>,
}

impl Kokoro {
    /// Loads the model and voices from disk. Takes a second or two.
    pub fn load(model_path: &Path, voices_path: &Path) -> Result<Self> {
        Self::load_with(model_path, voices_path, &LoadOptions::default())
    }

    pub fn load_with(model_path: &Path, voices_path: &Path, options: &LoadOptions) -> Result<Self> {
        let environment = ort::init().build().map_err(|error| anyhow!("ONNX Runtime did not start: {error}"))?;
        let mut builder = Session::builder(&environment).map_err(|error| anyhow!("{error}"))?;
        if let Some(threads) = options.threads {
            builder = builder.with_intra_threads(threads).map_err(|error| anyhow!("{error}"))?;
        }
        #[cfg(feature = "coreml")]
        if options.coreml {
            use ort::ep::coreml::{ComputeUnits, ModelFormat};
            let mut coreml = ort::ep::CoreML::default()
                .with_model_format(ModelFormat::MLProgram)
                .with_compute_units(ComputeUnits::CPUAndNeuralEngine);
            if let Some(cache) = &options.coreml_cache {
                coreml = coreml.with_model_cache_dir(cache.display());
            }
            builder = builder.with_execution_providers([coreml.build()]).map_err(|error| anyhow!("{error}"))?;
        }
        #[cfg(not(feature = "coreml"))]
        let _ = options.coreml;
        let session = builder
            .commit_from_file(model_path)
            .map_err(|error| anyhow!("could not load the voice model: {error}"))?;

        let vocab: HashMap<String, i64> = serde_json::from_str(include_str!("../assets/kokoro_vocab.json"))
            .context("the built-in vocabulary is not valid JSON")?;
        let vocab = vocab
            .into_iter()
            .filter_map(|(symbol, id)| symbol.chars().next().map(|character| (character, id)))
            .collect();

        Ok(Self {
            session,
            _environment: environment,
            vocab,
            voices: load_voices(voices_path)?,
            american: G2P::with_fallback(Language::EnglishUS, Some(Box::new(WordResolver))),
            british: None,
        })
    }

    /// The English voices, sorted. Names start with a language letter and a
    /// gender letter: `af_` American female, `bm_` British male, and so on.
    pub fn english_voices(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .voices
            .keys()
            .filter(|name| is_english_voice(name))
            .cloned()
            .collect();
        names.sort();
        names
    }

    /// Renders text to audio samples at 24 kHz. Long text is split into
    /// pieces the model can take and the audio is joined.
    pub fn synthesize(&mut self, text: &str, voice: &str, speed: f32) -> Result<Vec<f32>> {
        let voice = if self.voices.contains_key(voice) { voice } else { "af_heart" };
        let british = voice.starts_with('b');
        let mut audio = Vec::new();
        for tokens in self.token_chunks(text, british)? {
            if tokens.is_empty() {
                continue;
            }
            audio.extend(self.run(&tokens, voice, speed)?);
        }
        Ok(audio)
    }

    /// The phoneme string for a piece of text. Exposed for tests and tools.
    pub fn phonemize(&mut self, text: &str, british: bool) -> Result<String> {
        let g2p = if british {
            self.british
                .get_or_insert_with(|| G2P::with_fallback(Language::EnglishGB, Some(Box::new(WordResolver))))
        } else {
            &self.american
        };
        let text = speech_text::prepare(text);
        match g2p.g2p(&text) {
            Ok((phonemes, _)) => Ok(phonemes),
            // Something in the sentence defeated the phonemizer. Say every
            // word it can rather than skipping the whole sentence.
            Err(_) => {
                let words: Vec<String> = text
                    .split_whitespace()
                    .filter_map(|word| g2p.g2p(word).ok().map(|(phonemes, _)| phonemes))
                    .filter(|phonemes| !phonemes.is_empty())
                    .collect();
                if words.is_empty() {
                    bail!("could not phonemize \"{text}\"");
                }
                Ok(words.join(" "))
            }
        }
    }

    fn tokens(&mut self, text: &str, british: bool) -> Result<Vec<i64>> {
        let phonemes = self.phonemize(text, british)?;
        Ok(phonemes.chars().filter_map(|character| self.vocab.get(&character).copied()).collect())
    }

    /// Token ids for the text, split so every piece fits in one model call.
    fn token_chunks(&mut self, text: &str, british: bool) -> Result<Vec<Vec<i64>>> {
        let tokens = self.tokens(text, british)?;
        if tokens.len() <= TOKEN_BUDGET {
            return Ok(vec![tokens]);
        }
        // Too long for one call: cut the text near its middle, at a clause
        // break if there is one, and handle each half the same way.
        match split_point(text) {
            Some(point) => {
                let (left, right) = text.split_at(point);
                let mut chunks = self.token_chunks(left, british)?;
                chunks.extend(self.token_chunks(right, british)?);
                Ok(chunks)
            }
            // One enormous unbroken word. Cut the tokens themselves.
            None => Ok(tokens.chunks(TOKEN_BUDGET).map(<[i64]>::to_vec).collect()),
        }
    }

    fn run(&mut self, tokens: &[i64], voice: &str, speed: f32) -> Result<Vec<f32>> {
        let count = tokens.len().min(MAX_TOKENS);
        let table = self.voices.get(voice).ok_or_else(|| anyhow!("unknown voice {voice}"))?;
        // Kokoro's reference code picks the style row for the phoneme count
        // minus one: `voice[len(phonemes) - 1]`.
        let row = count.saturating_sub(1).min(table.len() / STYLE_WIDTH - 1);
        let style = table[row * STYLE_WIDTH..(row + 1) * STYLE_WIDTH].to_vec();

        // The model expects a padding token at each end.
        let mut ids = Vec::with_capacity(count + 2);
        ids.push(0);
        ids.extend_from_slice(&tokens[..count]);
        ids.push(0);

        let inputs = ort::inputs![
            "tokens" => Tensor::from_array(([1usize, ids.len()], ids)).map_err(|e| anyhow!("{e}"))?,
            "style" => Tensor::from_array(([1usize, STYLE_WIDTH], style)).map_err(|e| anyhow!("{e}"))?,
            "speed" => Tensor::from_array(([1usize], vec![speed.clamp(0.5, 2.0)])).map_err(|e| anyhow!("{e}"))?,
        ];
        let outputs = self.session.run(inputs).map_err(|error| anyhow!("the voice model failed: {error}"))?;
        let (_, samples) = outputs["audio"]
            .try_extract_tensor::<f32>()
            .map_err(|error| anyhow!("unexpected model output: {error}"))?;
        Ok(samples.to_vec())
    }
}

pub fn is_english_voice(name: &str) -> bool {
    ["af_", "am_", "bf_", "bm_"].iter().any(|prefix| name.starts_with(prefix))
}

/// "af_heart" to "Heart (US, female)".
pub fn voice_display_name(name: &str) -> String {
    let Some((code, label)) = name.split_once('_') else { return name.to_string() };
    let mut letters = code.chars();
    let accent = match letters.next() {
        Some('a') => "US",
        Some('b') => "UK",
        _ => code,
    };
    let gender = if letters.next() == Some('f') { "female" } else { "male" };
    let mut label_chars = label.chars();
    let label = match label_chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + label_chars.as_str(),
        None => String::new(),
    };
    format!("{label} ({accent}, {gender})")
}

/// A byte offset near the middle of the text where it can be cut: after a
/// clause mark if possible, otherwise at a space.
fn split_point(text: &str) -> Option<usize> {
    let middle = text.len() / 2;
    let distance = |index: &usize| index.abs_diff(middle);
    let after = |predicate: fn(char) -> bool| {
        text.char_indices()
            .filter(|(index, character)| {
                predicate(*character) && *index > 0 && index + character.len_utf8() < text.len()
            })
            .map(|(index, character)| index + character.len_utf8())
            .min_by_key(distance)
    };
    let clause = after(|c| matches!(c, ',' | ';' | ':' | '\u{2014}' | '\u{2013}'));
    // Only take a clause break that is reasonably central; a comma three
    // words in would leave the other piece just as oversized.
    match clause {
        Some(point) if point.abs_diff(middle) <= text.len() / 4 => Some(point),
        _ => after(|c| c == ' '),
    }
}

/// Reads the voices file, which is a NumPy `.npz`: a zip of one array per voice.
fn load_voices(path: &Path) -> Result<HashMap<String, Vec<f32>>> {
    let file = File::open(path).with_context(|| format!("could not open {}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("the voices file is damaged")?;
    let mut voices = HashMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let Some(name) = entry.name().strip_suffix(".npy").map(str::to_string) else { continue };
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes)?;
        voices.insert(name, parse_npy_f32(&bytes)?);
    }
    if voices.is_empty() {
        bail!("the voices file has no voices in it");
    }
    Ok(voices)
}

/// Parses a NumPy `.npy` array of little-endian 32-bit floats.
fn parse_npy_f32(bytes: &[u8]) -> Result<Vec<f32>> {
    if bytes.len() < 12 || &bytes[..6] != b"\x93NUMPY" {
        bail!("not a NumPy array");
    }
    let (header_length, data_start) = match bytes[6] {
        1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10),
        _ => (u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize, 12),
    };
    let data_start = data_start + header_length;
    let header = String::from_utf8_lossy(bytes.get(..data_start).ok_or_else(|| anyhow!("truncated array"))?);
    if !header.contains("<f4") {
        bail!("voice data is not 32-bit float");
    }
    let data = &bytes[data_start..];
    if data.len() % (4 * STYLE_WIDTH) != 0 {
        bail!("voice data has an unexpected size");
    }
    Ok(data
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_voices_for_people() {
        assert_eq!(voice_display_name("af_heart"), "Heart (US, female)");
        assert_eq!(voice_display_name("bm_george"), "George (UK, male)");
        assert!(is_english_voice("am_adam"));
        assert!(!is_english_voice("jf_alpha"));
    }

    #[test]
    fn splits_long_text_at_a_central_clause() {
        let text = "one two three four five six, seven eight nine ten eleven twelve";
        let point = split_point(text).unwrap();
        assert_eq!(&text[..point], "one two three four five six,");
        assert_eq!(split_point("unbroken"), None);
    }

    #[test]
    fn parses_numpy_arrays() {
        let mut bytes = b"\x93NUMPY\x01\x00".to_vec();
        let header = b"{'descr': '<f4', 'fortran_order': False, 'shape': (1, 1, 256), }\n";
        bytes.extend((header.len() as u16).to_le_bytes());
        bytes.extend(header);
        for value in 0..256 {
            bytes.extend((value as f32).to_le_bytes());
        }
        let parsed = parse_npy_f32(&bytes).unwrap();
        assert_eq!(parsed.len(), 256);
        assert_eq!(parsed[255], 255.0);
    }
}
