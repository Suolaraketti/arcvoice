//! Drives the playback engine with no window: import a PDF, play, seek,
//! pause, and print what happens. Used to check the engine end to end.
//!
//!   cargo run --release --example engine_smoke -- <data dir> <file.pdf>
//!
//! The Kokoro model must already be in `<data dir>/models`.

use arcvoice::engine::{Command, Engine, VoiceStatus};
use arcvoice_core::Library;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mut library = Library::open(PathBuf::from(&args[1]))?;
    let book = library.import_pdf(&PathBuf::from(&args[2]))?;
    let sentences = library.sentences(&book)?;
    let start = library.resume_index(&book, &sentences);
    println!("book \"{}\": {} sentences, resuming at {start}", book.title, sentences.len());

    let library = Arc::new(Mutex::new(library));
    let engine = Engine::start(library.clone(), || {});

    let began = Instant::now();
    while engine.snapshot().voice_status != VoiceStatus::Ready {
        if let VoiceStatus::Failed(reason) = engine.snapshot().voice_status {
            anyhow::bail!(reason);
        }
        sleep(Duration::from_millis(100));
    }
    println!("voice ready after {:.1}s, {} voices", began.elapsed().as_secs_f32(), engine.snapshot().voices.len());

    engine.send(Command::Open { book: book.clone(), sentences, start });
    engine.send(Command::Toggle);
    watch(&engine, "playing", 14);

    engine.send(Command::Seek(40));
    watch(&engine, "after seek to 40", 8);

    engine.send(Command::Toggle);
    watch(&engine, "paused", 2);

    let saved = library.lock().unwrap().position(&book);
    println!("saved position: {:?}", saved.map(|position| (position.sentence_index, position.anchor)));
    Ok(())
}

fn watch(engine: &Engine, label: &str, seconds: u64) {
    for second in 1..=seconds {
        sleep(Duration::from_secs(1));
        let snapshot = engine.snapshot();
        println!(
            "{label} {second:>2}s: sentence {:>3} playing={} buffering={} error={:?}",
            snapshot.current_index, snapshot.playing, snapshot.buffering, snapshot.error
        );
    }
}
