//! A small neural letter-to-sound model for words no dictionary has: the
//! invented names fiction is full of.
//!
//! This is g2p_en by Kyubyong Park and Jongseok Kim (Apache-2.0): a GRU
//! encoder-decoder trained on the CMU Pronouncing Dictionary. It reads a word
//! letter by letter and writes ARPAbet phonemes with stress marks. The weights
//! are 3 MB and a word takes well under a millisecond, so it runs anywhere.
//!
//! On a held-out sample of dictionary words its sounds differ from misaki's
//! about 19% of the time, against 31% for the old rule table, and it places
//! stress, which rules mostly cannot.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::sync::OnceLock;

const WEIGHTS: &[u8] = include_bytes!("../assets/g2p_en.npz");
const HIDDEN: usize = 256;

const PHONEMES: [&str; 74] = [
    "<pad>", "<unk>", "<s>", "</s>", "AA0", "AA1", "AA2", "AE0", "AE1", "AE2", "AH0", "AH1", "AH2", "AO0", "AO1",
    "AO2", "AW0", "AW1", "AW2", "AY0", "AY1", "AY2", "B", "CH", "D", "DH", "EH0", "EH1", "EH2", "ER0", "ER1", "ER2",
    "EY0", "EY1", "EY2", "F", "G", "HH", "IH0", "IH1", "IH2", "IY0", "IY1", "IY2", "JH", "K", "L", "M", "N", "NG",
    "OW0", "OW1", "OW2", "OY0", "OY1", "OY2", "P", "R", "S", "SH", "T", "TH", "UH0", "UH1", "UH2", "UW", "UW0", "UW1",
    "UW2", "V", "W", "Y", "Z", "ZH",
];
const START: usize = 2;
const END: usize = 3;

struct Matrix {
    rows: usize,
    columns: usize,
    values: Vec<f32>,
}

impl Matrix {
    fn row(&self, index: usize) -> &[f32] {
        &self.values[index * self.columns..(index + 1) * self.columns]
    }

    /// `self · vector + bias`, for a matrix stored row-major as (out, in).
    fn times(&self, vector: &[f32], bias: &[f32]) -> Vec<f32> {
        (0..self.rows)
            .map(|row| {
                let weights = self.row(row);
                let mut sum = bias[row];
                for (weight, value) in weights.iter().zip(vector) {
                    sum += weight * value;
                }
                sum
            })
            .collect()
    }
}

struct Gru {
    input: Matrix,
    hidden: Matrix,
    input_bias: Vec<f32>,
    hidden_bias: Vec<f32>,
}

impl Gru {
    /// One step of a PyTorch-style GRU cell (gates in the order r, z, n).
    fn step(&self, x: &[f32], h: &[f32]) -> Vec<f32> {
        let from_input = self.input.times(x, &self.input_bias);
        let from_hidden = self.hidden.times(h, &self.hidden_bias);
        let sigmoid = |value: f32| 1.0 / (1.0 + (-value).exp());
        (0..HIDDEN)
            .map(|i| {
                let reset = sigmoid(from_input[i] + from_hidden[i]);
                let update = sigmoid(from_input[HIDDEN + i] + from_hidden[HIDDEN + i]);
                let candidate = (from_input[2 * HIDDEN + i] + reset * from_hidden[2 * HIDDEN + i]).tanh();
                (1.0 - update) * candidate + update * h[i]
            })
            .collect()
    }
}

pub struct NeuralG2p {
    encoder_embedding: Matrix,
    encoder: Gru,
    decoder_embedding: Matrix,
    decoder: Gru,
    output: Matrix,
    output_bias: Vec<f32>,
}

impl NeuralG2p {
    /// The shared model, loaded the first time a word needs it.
    pub fn shared() -> Option<&'static NeuralG2p> {
        static MODEL: OnceLock<Option<NeuralG2p>> = OnceLock::new();
        MODEL.get_or_init(|| NeuralG2p::load().ok()).as_ref()
    }

    fn load() -> Result<Self> {
        let mut arrays = read_npz(WEIGHTS)?;
        let mut take = |name: &str| arrays.remove(name).ok_or_else(|| anyhow!("g2p weights lack {name}"));
        let matrix = |(shape, values): (Vec<usize>, Vec<f32>)| -> Result<Matrix> {
            match shape[..] {
                [rows, columns] => Ok(Matrix { rows, columns, values }),
                _ => bail!("expected a matrix, got shape {shape:?}"),
            }
        };
        let vector = |(_, values): (Vec<usize>, Vec<f32>)| values;
        Ok(Self {
            encoder_embedding: matrix(take("enc_emb")?)?,
            encoder: Gru {
                input: matrix(take("enc_w_ih")?)?,
                hidden: matrix(take("enc_w_hh")?)?,
                input_bias: vector(take("enc_b_ih")?),
                hidden_bias: vector(take("enc_b_hh")?),
            },
            decoder_embedding: matrix(take("dec_emb")?)?,
            decoder: Gru {
                input: matrix(take("dec_w_ih")?)?,
                hidden: matrix(take("dec_w_hh")?)?,
                input_bias: vector(take("dec_b_ih")?),
                hidden_bias: vector(take("dec_b_hh")?),
            },
            output: matrix(take("fc_w")?)?,
            output_bias: vector(take("fc_b")?),
        })
    }

    /// ARPAbet phonemes for a lowercase word made of the letters a to z.
    pub fn predict(&self, word: &str) -> Vec<&'static str> {
        let mut h = vec![0.0; HIDDEN];
        // Graphemes are <pad> <unk> </s> then a to z.
        let letters = word.bytes().map(|byte| match byte {
            b'a'..=b'z' => (byte - b'a') as usize + 3,
            _ => 1,
        });
        for index in letters.chain(std::iter::once(2)) {
            h = self.encoder.step(self.encoder_embedding.row(index), &h);
        }

        let mut out = Vec::new();
        let mut previous = START;
        for _ in 0..(word.len() + 6).max(20) {
            h = self.decoder.step(self.decoder_embedding.row(previous), &h);
            let logits = self.output.times(&h, &self.output_bias);
            let best = logits
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map_or(END, |(index, _)| index);
            if best == END {
                break;
            }
            if best > END {
                out.push(PHONEMES[best]);
            }
            previous = best;
        }
        out
    }
}

/// Reads every float32 array in a NumPy `.npz` archive, with its shape.
fn read_npz(bytes: &[u8]) -> Result<HashMap<String, (Vec<usize>, Vec<f32>)>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).context("g2p weights are not a zip")?;
    let mut arrays = HashMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let Some(name) = entry.name().strip_suffix(".npy").map(str::to_string) else { continue };
        let mut data = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut data)?;
        arrays.insert(name, read_npy(&data)?);
    }
    Ok(arrays)
}

fn read_npy(bytes: &[u8]) -> Result<(Vec<usize>, Vec<f32>)> {
    if bytes.len() < 12 || &bytes[..6] != b"\x93NUMPY" {
        bail!("not a NumPy array");
    }
    let (header_length, start) = match bytes[6] {
        1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10),
        _ => (u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize, 12),
    };
    let header = std::str::from_utf8(&bytes[start..start + header_length]).context("bad array header")?;
    if !header.contains("'<f4'") || header.contains("'fortran_order': True") {
        bail!("expected little-endian float32 in C order");
    }
    let shape_text = header
        .split("'shape': (")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .ok_or_else(|| anyhow!("array header has no shape"))?;
    let shape: Vec<usize> = shape_text
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().context("bad shape"))
        .collect::<Result<_>>()?;
    let values: Vec<f32> = bytes[start + header_length..]
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();
    if values.len() != shape.iter().product::<usize>() {
        bail!("array size does not match its shape");
    }
    Ok((shape, values))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predicts_like_the_reference_model() {
        let model = NeuralG2p::shared().expect("weights load");
        // Outputs of the original numpy implementation.
        assert_eq!(model.predict("pemberley").join(" "), "P EH1 M B ER0 L IY0");
        assert_eq!(model.predict("kaelith").join(" "), "K EH1 L IH0 TH");
        assert_eq!(model.predict("netherfield").join(" "), "N EH1 DH ER0 F IH2 L D");
    }
}
