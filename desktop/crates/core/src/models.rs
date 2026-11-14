//! Downloads the voices the first time they are needed.
//!
//! Kokoro (the default voices, about 350 MB) comes from the kokoro-onnx
//! release. Phonon (the optional expressive voices, about 68 MB) comes from
//! Gradium's Hugging Face repositories, pinned to exact revisions so an
//! upstream change never alters the voice without a deliberate update, and
//! only when an expressive voice is chosen.
//!
//! The files come from the kokoro-onnx project's GitHub release. The full
//! precision model is used on purpose: it is about 325 MB, but on a desktop
//! CPU it runs more than twice as fast as the 90 MB quantized one, and speed
//! is what makes pressing play feel instant.

use anyhow::{anyhow, bail, Context, Result};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const BASE_URL: &str = "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0";

pub struct ModelFile {
    pub name: &'static str,
    /// Used for progress before the server reports a size, and as a sanity
    /// check that a finished download is not a truncated file.
    pub approximate_bytes: u64,
}

pub const MODEL: ModelFile = ModelFile { name: "kokoro-v1.0.onnx", approximate_bytes: 325_532_387 };
pub const VOICES: ModelFile = ModelFile { name: "voices-v1.0.bin", approximate_bytes: 28_214_398 };

const PHONON_MODEL_URL: &str =
    "https://huggingface.co/gradium/phonon-31m-en/resolve/92741c5b07d085ff840e26fea8ce43110b87852f";
const PHONON_VOICES_URL: &str =
    "https://huggingface.co/gradium/phonon-voices/resolve/da6c9ec0d47be8df5c87c730a3aa97187f3229c3/voices";
const PHONON_FILES: [ModelFile; 3] = [
    ModelFile { name: "config.json", approximate_bytes: 4_634 },
    ModelFile { name: "tokenizer.json", approximate_bytes: 277_967 },
    ModelFile { name: "model.q8.gguf", approximate_bytes: 64_512_352 },
];
const PHONON_VOICE_BYTES: u64 = 256_136;

/// Where the Phonon checkpoint and its voices live.
pub fn phonon_dir(models_dir: &Path) -> PathBuf {
    models_dir.join("phonon")
}

/// A file to download: where it goes, relative to its voice's folder, where
/// it comes from, and about how big it is.
pub struct Download {
    pub path: String,
    pub url: String,
    pub bytes: u64,
}

/// Everything Phonon needs, relative to `phonon_dir`. The apps on other
/// platforms download from this same list, so they always match the engine.
pub fn phonon_downloads() -> Vec<Download> {
    let mut downloads: Vec<Download> = PHONON_FILES
        .iter()
        .map(|file| Download {
            path: file.name.to_string(),
            url: format!("{PHONON_MODEL_URL}/{}", file.name),
            bytes: file.approximate_bytes,
        })
        .collect();
    for voice in &crate::phonon::VOICES {
        let name = format!("{}.safetensors", voice.id);
        downloads.push(Download {
            path: format!("voices/{name}"),
            url: format!("{PHONON_VOICES_URL}/{name}"),
            bytes: PHONON_VOICE_BYTES,
        });
    }
    downloads
}

/// Kokoro's two files, relative to the models folder.
pub fn kokoro_downloads() -> Vec<Download> {
    [&VOICES, &MODEL]
        .iter()
        .map(|file| Download {
            path: file.name.to_string(),
            url: format!("{BASE_URL}/{}", file.name),
            bytes: file.approximate_bytes,
        })
        .collect()
}

fn phonon_targets(models_dir: &Path) -> Vec<(String, PathBuf, u64)> {
    let dir = phonon_dir(models_dir);
    phonon_downloads().into_iter().map(|d| (d.url, dir.join(d.path), d.bytes)).collect()
}

fn is_complete(path: &Path, bytes: u64) -> bool {
    fs::metadata(path).map(|meta| meta.len() >= bytes * 9 / 10).unwrap_or(false)
}

pub fn is_phonon_downloaded(models_dir: &Path) -> bool {
    phonon_targets(models_dir).iter().all(|(_, path, bytes)| is_complete(path, *bytes))
}

/// Downloads whatever of Phonon is missing. `progress` gets 0 to 1.
pub fn ensure_phonon_downloaded(models_dir: &Path, mut progress: impl FnMut(f32)) -> Result<()> {
    let targets = phonon_targets(models_dir);
    let total: u64 = targets.iter().map(|(_, _, bytes)| bytes).sum();
    let mut done = 0;
    for (url, path, bytes) in &targets {
        if !is_complete(path, *bytes) {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let base = done;
            download(url, path, |written| progress(((base + written) as f32 / total as f32).min(1.0)))
                .with_context(|| format!("could not download {}", url.rsplit('/').next().unwrap_or(url)))?;
        }
        done += bytes;
        progress((done as f32 / total as f32).min(1.0));
    }
    Ok(())
}

pub fn model_path(models_dir: &Path) -> PathBuf {
    models_dir.join(MODEL.name)
}

pub fn voices_path(models_dir: &Path) -> PathBuf {
    models_dir.join(VOICES.name)
}

pub fn is_downloaded(models_dir: &Path) -> bool {
    [&MODEL, &VOICES].iter().all(|file| {
        fs::metadata(models_dir.join(file.name))
            .map(|meta| meta.len() >= file.approximate_bytes * 9 / 10)
            .unwrap_or(false)
    })
}

/// Downloads whatever is missing. `progress` gets a value from 0 to 1.
pub fn ensure_downloaded(models_dir: &Path, mut progress: impl FnMut(f32)) -> Result<()> {
    fs::create_dir_all(models_dir)?;
    let total: u64 = MODEL.approximate_bytes + VOICES.approximate_bytes;
    let mut done: u64 = 0;
    for file in [&VOICES, &MODEL] {
        let target = models_dir.join(file.name);
        let present = fs::metadata(&target)
            .map(|meta| meta.len() >= file.approximate_bytes * 9 / 10)
            .unwrap_or(false);
        if !present {
            let base = done;
            download(&format!("{BASE_URL}/{}", file.name), &target, |bytes| {
                progress(((base + bytes) as f32 / total as f32).min(1.0));
            })
            .with_context(|| format!("could not download {}", file.name))?;
        }
        done += file.approximate_bytes;
        progress((done as f32 / total as f32).min(1.0));
    }
    Ok(())
}

fn download(url: &str, target: &Path, mut on_bytes: impl FnMut(u64)) -> Result<()> {
    let config = ureq::Agent::config_builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .build(),
        )
        .build();
    let agent: ureq::Agent = config.into();

    let mut response = agent.get(url).call().map_err(|error| anyhow!("{error}"))?;
    if !response.status().is_success() {
        bail!("the server answered {}", response.status());
    }

    // Download beside the final name and rename at the end, so a download cut
    // short is never mistaken for a finished model.
    let partial = target.with_extension("part");
    let mut file = File::create(&partial)?;
    let mut reader = response.body_mut().as_reader();
    let mut buffer = vec![0u8; 256 * 1024];
    let mut written: u64 = 0;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])?;
        written += read as u64;
        on_bytes(written);
    }
    file.flush()?;
    drop(file);
    fs::rename(&partial, target)?;
    Ok(())
}
