//! Downloads the Kokoro voice the first time the app runs.
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
