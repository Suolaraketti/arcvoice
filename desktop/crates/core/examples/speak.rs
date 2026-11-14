//! A command line check of the whole pipeline, with no window involved.
//!
//!   cargo run --release --example speak -- <models dir> text "Hello there."
//!   cargo run --release --example speak -- <models dir> pdf book.pdf
//!
//! `text` writes speak.wav and prints how fast it rendered. `pdf` prints the
//! first cleaned sentences of a PDF, which is the quickest way to see how
//! well the cleanup handles a particular book.

use arcvoice_core::{models, pdf, Kokoro, SAMPLE_RATE};
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: speak <models dir> text \"words to say\" [voice]\n       speak <models dir> pdf <file.pdf>");
        std::process::exit(2);
    }
    let models_dir = std::path::Path::new(&args[1]);

    match args[2].as_str() {
        "pdf" => {
            let bytes = std::fs::read(&args[3])?;
            let book = pdf::extract(&bytes)?;
            println!("title: {:?}, pages: {}, sentences: {}", book.title, book.page_count, book.sentences.len());
            for (index, sentence) in book.sentences.iter().take(40).enumerate() {
                println!("{index:>3}  {sentence}");
            }
        }
        _ => {
            let voice = args.get(4).map(String::as_str).unwrap_or("af_heart");
            let started = Instant::now();
            let mut kokoro = Kokoro::load(&models::model_path(models_dir), &models::voices_path(models_dir))?;
            println!("loaded in {:.2}s, {} English voices", started.elapsed().as_secs_f32(), kokoro.english_voices().len());
            println!("phonemes: {}", kokoro.phonemize(&args[3], voice.starts_with('b'))?);

            let started = Instant::now();
            let samples = kokoro.synthesize(&args[3], voice, 1.0)?;
            let took = started.elapsed().as_secs_f32();
            let seconds = samples.len() as f32 / SAMPLE_RATE as f32;
            println!("{seconds:.2}s of audio in {took:.2}s ({:.1}x faster than real time)", seconds / took);
            write_wav("speak.wav", &samples)?;
            println!("wrote speak.wav");
        }
    }
    Ok(())
}

fn write_wav(path: &str, samples: &[f32]) -> std::io::Result<()> {
    let data_length = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
    bytes.extend(b"RIFF");
    bytes.extend((36 + data_length).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes()); // PCM
    bytes.extend(1u16.to_le_bytes()); // mono
    bytes.extend(SAMPLE_RATE.to_le_bytes());
    bytes.extend((SAMPLE_RATE * 2).to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(data_length.to_le_bytes());
    for sample in samples {
        bytes.extend(((sample.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, bytes)
}
