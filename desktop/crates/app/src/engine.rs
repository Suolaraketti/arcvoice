//! Playback, kept off the window's thread so reading never stalls when the
//! window is minimized or busy.
//!
//! Two threads do the work:
//!
//! - The **voice thread** owns the Kokoro model. It downloads and loads the
//!   model at startup, then renders one sentence at a time on request.
//! - The **controller thread** owns the audio output. It keeps a few rendered
//!   sentences queued ahead of the one being spoken, tracks which sentence is
//!   playing by watching the queue drain, and saves the position as it moves.
//!
//! The window sends `Command`s in and reads a `Snapshot` out.

use arcvoice_core::pacing::{self, Break};
use arcvoice_core::narrator::is_classic_voice;
use arcvoice_core::{models, Book, Kokoro, Library, Narrator, Phonon, VoiceChoice, SAMPLE_RATE};
use rodio::buffer::SamplesBuffer;
use rodio::{OutputStream, OutputStreamBuilder, Sink};
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// How many sentences to keep rendered beyond the one being spoken.
const LOOKAHEAD: usize = 3;
/// A short breath between sentences, in seconds. Headings get a longer one.

#[derive(Debug, Clone, PartialEq)]
pub enum VoiceStatus {
    Downloading(f32),
    Loading,
    Ready,
    Failed(String),
}

/// What the window needs to draw the player.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub voice_status: VoiceStatus,
    pub voices: Vec<VoiceChoice>,
    pub book: Option<Book>,
    pub sentences: Arc<Vec<String>>,
    pub current_index: usize,
    pub playing: bool,
    pub finished: bool,
    /// True while the sentence that should be playing is still being rendered.
    pub buffering: bool,
    pub error: Option<String>,
}

pub enum Command {
    Open { book: Book, sentences: Vec<String>, start: usize },
    Close,
    Toggle,
    Seek(usize),
    Skip(i64),
    SetVoice(String),
    SetSpeed(f32),
}

enum Message {
    Command(Command),
    VoiceStatus(VoiceStatus, Vec<VoiceChoice>),
    Rendered { generation: u64, index: usize, result: Result<Vec<f32>, String> },
}

struct RenderRequest {
    generation: u64,
    index: usize,
    text: String,
    voice: String,
    speed: f32,
}

/// The window's handle on the engine.
pub struct Engine {
    sender: Sender<Message>,
    snapshot: Arc<Mutex<Snapshot>>,
}

impl Engine {
    /// Starts both threads. `wake` is called whenever the snapshot changes, so
    /// the window can redraw.
    pub fn start(library: Arc<Mutex<Library>>, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let (voice, speed, models_dir) = {
            let library = library.lock().unwrap();
            (library.settings.voice.clone(), library.settings.speed, library.models_dir())
        };
        let snapshot = Arc::new(Mutex::new(Snapshot {
            voice_status: VoiceStatus::Loading,
            voices: Vec::new(),
            book: None,
            sentences: Arc::new(Vec::new()),
            current_index: 0,
            playing: false,
            finished: false,
            buffering: false,
            error: None,
        }));
        let (sender, receiver) = mpsc::channel::<Message>();
        let (render_sender, render_receiver) = mpsc::channel::<RenderRequest>();
        let generation = Arc::new(AtomicU64::new(0));
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);

        {
            let sender = sender.clone();
            let generation = generation.clone();
            thread::Builder::new()
                .name("voice".into())
                .spawn(move || voice_thread(models_dir, render_receiver, sender, generation))
                .expect("could not start the voice thread");
        }
        {
            let snapshot = snapshot.clone();
            thread::Builder::new()
                .name("controller".into())
                .spawn(move || {
                    Controller::new(library, snapshot, render_sender, generation, wake, voice, speed).run(receiver)
                })
                .expect("could not start the controller thread");
        }
        Self { sender, snapshot }
    }

    pub fn send(&self, command: Command) {
        let _ = self.sender.send(Message::Command(command));
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.lock().unwrap().clone()
    }
}

// ---------------------------------------------------------------------------
// Voice thread
// ---------------------------------------------------------------------------

fn voice_thread(
    models_dir: std::path::PathBuf,
    requests: Receiver<RenderRequest>,
    messages: Sender<Message>,
    generation: Arc<AtomicU64>,
) {
    let report = |status: VoiceStatus, voices: Vec<VoiceChoice>| {
        let _ = messages.send(Message::VoiceStatus(status, voices));
    };
    // Reporting every downloaded chunk would flood the channel.
    let download_progress = || {
        let mut last_reported = -1.0f32;
        move |fraction: f32| {
            if fraction - last_reported >= 0.005 {
                last_reported = fraction;
                report(VoiceStatus::Downloading(fraction), Vec::new());
            }
        }
    };
    let threads = std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(1, 4));

    // Phonon first: the default voice, and a small download.
    if !models::is_phonon_downloaded(&models_dir) {
        report(VoiceStatus::Downloading(0.0), Vec::new());
        if let Err(error) = models::ensure_phonon_downloaded(&models_dir, download_progress()) {
            report(VoiceStatus::Failed(format!("The voice could not be downloaded: {error:#}")), Vec::new());
            return;
        }
    }
    report(VoiceStatus::Loading, Vec::new());
    let phonon = match Phonon::load(&models::phonon_dir(&models_dir), Some(threads)) {
        Ok(phonon) => Some(phonon),
        Err(error) => {
            report(VoiceStatus::Failed(format!("The voice could not be loaded: {error:#}")), Vec::new());
            return;
        }
    };
    // Kokoro's classic voices too, when an earlier version downloaded them.
    let load_kokoro = || {
        let options = arcvoice_core::LoadOptions { threads: Some(threads), ..Default::default() };
        Kokoro::load_with(&models::model_path(&models_dir), &models::voices_path(&models_dir), &options)
    };
    let kokoro = if models::is_downloaded(&models_dir) { load_kokoro().ok() } else { None };
    let mut narrator = Narrator::new(phonon, kokoro);
    // The first render is slower than the rest. Take that hit now.
    let _ = narrator.synthesize("Ready.", arcvoice_core::narrator::DEFAULT_VOICE, 1.0);
    report(VoiceStatus::Ready, narrator.voices());

    while let Ok(request) = requests.recv() {
        // A seek since this was asked for makes it stale. Skip the work.
        if request.generation != generation.load(Ordering::SeqCst) {
            continue;
        }
        // A classic voice was chosen and Kokoro is not here yet: fetch it.
        if is_classic_voice(&request.voice) && !narrator.has_kokoro() {
            report(VoiceStatus::Downloading(0.0), narrator.voices());
            let loaded = models::ensure_downloaded(&models_dir, download_progress())
                .and_then(|()| {
                    report(VoiceStatus::Loading, narrator.voices());
                    load_kokoro()
                });
            match loaded {
                Ok(kokoro) => narrator.set_kokoro(kokoro),
                Err(error) => {
                    let message = format!("The classic voices could not be downloaded: {error:#}");
                    report(VoiceStatus::Failed(message), narrator.voices());
                    continue;
                }
            }
            report(VoiceStatus::Ready, narrator.voices());
        }
        // One sentence the voice cannot read must not stop the book: it is
        // logged and passed over with a beat of silence.
        let result = match narrator.synthesize(&request.text, &request.voice, request.speed) {
            Ok(samples) => Ok(samples),
            Err(error) if narrator.is_ready() => {
                eprintln!("skipped sentence {}: {error:#}", request.index);
                Ok(arcvoice_core::narrator::scene_break(request.speed))
            }
            Err(error) => Err(format!("{error:#}")),
        };
        let _ = messages.send(Message::Rendered { generation: request.generation, index: request.index, result });
    }
}

// ---------------------------------------------------------------------------
// Controller thread
// ---------------------------------------------------------------------------

struct Controller {
    library: Arc<Mutex<Library>>,
    snapshot: Arc<Mutex<Snapshot>>,
    render: Sender<RenderRequest>,
    generation: Arc<AtomicU64>,
    wake: Arc<dyn Fn() + Send + Sync>,

    // Audio. The stream has to stay alive for as long as sound should play.
    _stream: Option<OutputStream>,
    sink: Option<Sink>,
    audio_error: Option<String>,

    voice_ready: bool,
    voice: String,
    speed: f32,

    book: Option<Book>,
    sentences: Arc<Vec<String>>,
    /// Sentences after which a paragraph ends, for a longer pause.
    paragraph_ends: HashSet<usize>,
    /// The sentence being spoken, or the next to speak while paused.
    position: usize,
    playing: bool,
    finished: bool,
    /// Play was pressed before the voice finished loading.
    play_when_ready: bool,

    /// Sentences handed to the audio queue and not yet finished, oldest first.
    queued: VecDeque<usize>,
    next_to_render: usize,
    /// The sentence the voice thread is working on right now.
    pending: Option<usize>,
    error: Option<String>,
}

impl Controller {
    fn new(
        library: Arc<Mutex<Library>>,
        snapshot: Arc<Mutex<Snapshot>>,
        render: Sender<RenderRequest>,
        generation: Arc<AtomicU64>,
        wake: Arc<dyn Fn() + Send + Sync>,
        voice: String,
        speed: f32,
    ) -> Self {
        let (stream, sink, audio_error) = match OutputStreamBuilder::open_default_stream() {
            Ok(stream) => {
                let sink = Sink::connect_new(stream.mixer());
                sink.pause();
                (Some(stream), Some(sink), None)
            }
            Err(error) => (None, None, Some(format!("No audio output was found: {error}"))),
        };
        Self {
            library,
            snapshot,
            render,
            generation,
            wake,
            _stream: stream,
            sink,
            audio_error,
            voice_ready: false,
            voice,
            speed,
            book: None,
            sentences: Arc::new(Vec::new()),
            paragraph_ends: HashSet::new(),
            position: 0,
            playing: false,
            finished: false,
            play_when_ready: false,
            queued: VecDeque::new(),
            next_to_render: 0,
            pending: None,
            error: None,
        }
    }

    fn run(mut self, messages: Receiver<Message>) {
        let mut voice_status = VoiceStatus::Loading;
        let mut voices: Vec<VoiceChoice> = Vec::new();
        loop {
            // Wake often while playing to notice sentences finishing; rarely
            // when idle so a paused app uses no CPU.
            let wait = Duration::from_millis(if self.playing { 30 } else { 500 });
            let mut changed = false;
            match messages.recv_timeout(wait) {
                Ok(Message::Command(command)) => {
                    self.handle(command);
                    changed = true;
                }
                Ok(Message::VoiceStatus(status, names)) => {
                    if status == VoiceStatus::Ready {
                        self.voice_ready = true;
                        voices = names;
                        if self.play_when_ready {
                            self.play_when_ready = false;
                            self.play();
                        }
                    }
                    voice_status = status;
                    changed = true;
                }
                Ok(Message::Rendered { generation, index, result }) => {
                    self.rendered(generation, index, result);
                    changed = true;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }

            changed |= self.advance();
            self.request_more();

            if changed {
                let buffering = self.playing && self.queued.is_empty();
                {
                    let mut snapshot = self.snapshot.lock().unwrap();
                    snapshot.voice_status = voice_status.clone();
                    snapshot.voices = voices.clone();
                    snapshot.book = self.book.clone();
                    snapshot.sentences = self.sentences.clone();
                    snapshot.current_index = self.position;
                    snapshot.playing = self.playing;
                    snapshot.finished = self.finished;
                    snapshot.buffering = buffering;
                    snapshot.error = self.error.clone().or_else(|| self.audio_error.clone());
                }
                (self.wake)();
            }
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Open { book, sentences, start } => {
                self.flush();
                self.paragraph_ends = self.library.lock().unwrap().paragraph_ends(&book).into_iter().collect();
                self.playing = false;
                self.finished = false;
                self.error = None;
                self.position = start.min(sentences.len().saturating_sub(1));
                self.next_to_render = self.position;
                self.sentences = Arc::new(sentences);
                self.book = Some(book);
            }
            Command::Close => {
                self.flush();
                self.playing = false;
                self.finished = false;
                self.book = None;
                self.sentences = Arc::new(Vec::new());
                self.position = 0;
                self.next_to_render = 0;
            }
            Command::Toggle => {
                if self.playing {
                    self.pause()
                } else {
                    self.play()
                }
            }
            Command::Seek(index) => self.seek(index),
            Command::Skip(delta) => {
                let target = (self.position as i64 + delta).max(0) as usize;
                self.seek(target);
            }
            Command::SetVoice(voice) => {
                if voice != self.voice {
                    self.voice = voice;
                    // Throw away audio already rendered in the old voice.
                    self.seek(self.position);
                }
            }
            Command::SetSpeed(speed) => {
                if (speed - self.speed).abs() > f32::EPSILON {
                    self.speed = speed;
                    self.seek(self.position);
                }
            }
        }
    }

    fn play(&mut self) {
        if self.book.is_none() || self.sentences.is_empty() {
            return;
        }
        if !self.voice_ready {
            self.play_when_ready = true;
            return;
        }
        if self.finished {
            self.flush();
            self.finished = false;
            self.position = 0;
            self.next_to_render = 0;
        }
        self.error = None;
        self.playing = true;
        if let Some(sink) = &self.sink {
            sink.play();
        }
    }

    fn pause(&mut self) {
        self.play_when_ready = false;
        if !self.playing {
            return;
        }
        self.playing = false;
        if let Some(sink) = &self.sink {
            sink.pause();
        }
        self.save_position();
    }

    /// Jumps to a sentence. Keeps playing if it was playing.
    fn seek(&mut self, index: usize) {
        if self.sentences.is_empty() {
            return;
        }
        self.flush();
        self.finished = false;
        self.position = index.min(self.sentences.len() - 1);
        self.next_to_render = self.position;
        self.save_position();
        if self.playing {
            if let Some(sink) = &self.sink {
                sink.play();
            }
        }
    }

    /// Drops everything queued or in flight. Bumping the generation makes any
    /// render still on its way back get ignored.
    fn flush(&mut self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.queued.clear();
        self.pending = None;
        if let Some(sink) = &self.sink {
            // `clear` also pauses the sink; `seek` and `play` restart it.
            sink.clear();
        }
    }

    /// Notices sentences that have finished playing and moves the position
    /// forward. Returns true if anything changed.
    fn advance(&mut self) -> bool {
        let Some(sink) = &self.sink else { return false };
        let remaining = sink.len();
        let mut moved = false;
        while self.queued.len() > remaining {
            if let Some(finished) = self.queued.pop_front() {
                self.position = finished + 1;
                moved = true;
            }
        }
        if !moved {
            return false;
        }
        if self.position >= self.sentences.len() {
            // End of the book.
            self.position = self.sentences.len().saturating_sub(1);
            self.playing = false;
            self.finished = true;
            sink.pause();
        }
        self.save_position();
        true
    }

    /// Asks the voice thread for the next sentence if the queue has room.
    fn request_more(&mut self) {
        if !self.playing || !self.voice_ready || self.pending.is_some() {
            return;
        }
        if self.next_to_render >= self.sentences.len() || self.next_to_render > self.position + LOOKAHEAD {
            return;
        }
        let index = self.next_to_render;
        let request = RenderRequest {
            generation: self.generation.load(Ordering::SeqCst),
            index,
            text: self.sentences[index].clone(),
            voice: self.voice.clone(),
            speed: self.speed,
        };
        if self.render.send(request).is_ok() {
            self.pending = Some(index);
        }
    }

    fn rendered(&mut self, generation: u64, index: usize, result: Result<Vec<f32>, String>) {
        if generation != self.generation.load(Ordering::SeqCst) {
            return;
        }
        self.pending = None;
        match result {
            Ok(samples) => {
                let after = if self.paragraph_ends.contains(&index) { Break::Paragraph } else { Break::Sentence };
                let samples = pacing::finish_sentence(&samples, SAMPLE_RATE, &self.sentences[index], after, self.speed);
                if let Some(sink) = &self.sink {
                    sink.append(SamplesBuffer::new(1, SAMPLE_RATE, samples));
                    self.queued.push_back(index);
                } else {
                    // No audio device: stop instead of racing through the book.
                    self.playing = false;
                }
                self.next_to_render = index + 1;
            }
            Err(error) => {
                self.error = Some(format!("The voice hit a problem: {error}"));
                self.pause();
            }
        }
    }

    fn save_position(&self) {
        let (Some(book), Some(sentence)) = (&self.book, self.sentences.get(self.position)) else { return };
        if let Ok(mut library) = self.library.lock() {
            library.save_position(&book.id, self.position, sentence);
        }
    }
}
