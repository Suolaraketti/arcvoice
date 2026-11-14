//! ArcVoice for the desktop: a small window around the engine.
//!
//! Books on the left, the passage being read in the middle, transport
//! controls along the bottom. Drop a PDF or EPUB anywhere on the window to add it.

// Without this, Windows opens a console window behind the app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use arcvoice::engine::{Command, Engine, Snapshot, VoiceStatus};
use arcvoice_core::{Book, Library, PreparedImport};
use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

const SPEEDS: [f32; 8] = [0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ArcVoice")
            .with_inner_size([980.0, 660.0])
            .with_min_inner_size([640.0, 420.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("ArcVoice", options, Box::new(|context| Ok(Box::new(App::new(context)))))
}

struct App {
    library: Arc<Mutex<Library>>,
    engine: Engine,
    /// Finished imports coming back from background threads.
    import_sender: Sender<Result<PreparedImport, String>>,
    import_receiver: Receiver<Result<PreparedImport, String>>,
    imports_running: usize,
    /// Set while the progress slider is being dragged.
    scrub: Option<usize>,
    message: Option<String>,
    voice: String,
    speed: f32,
}

impl App {
    fn new(context: &eframe::CreationContext<'_>) -> Self {
        // If the usual app data folder can't be used, fall back to a folder
        // beside the program so the app still opens.
        let library = Library::open_default()
            .or_else(|_| Library::open(PathBuf::from("ArcVoiceData")))
            .expect("could not create a folder to keep books in");
        let voice = library.settings.voice.clone();
        let speed = library.settings.speed;
        let library = Arc::new(Mutex::new(library));

        let repaint = context.egui_ctx.clone();
        let engine = Engine::start(library.clone(), move || repaint.request_repaint());

        context.egui_ctx.style_mut(|style| {
            style.spacing.item_spacing = egui::vec2(8.0, 8.0);
            style.spacing.button_padding = egui::vec2(10.0, 6.0);
            for (text_style, font) in style.text_styles.iter_mut() {
                font.size = match text_style {
                    egui::TextStyle::Heading => 20.0,
                    egui::TextStyle::Small => 12.0,
                    _ => 15.0,
                };
            }
        });

        let (import_sender, import_receiver) = mpsc::channel();
        Self {
            library,
            engine,
            import_sender,
            import_receiver,
            imports_running: 0,
            scrub: None,
            message: None,
            voice,
            speed,
        }
    }

    /// Reads a book on a background thread so the window stays responsive.
    fn start_import(&mut self, path: PathBuf, context: &egui::Context) {
        self.imports_running += 1;
        let sender = self.import_sender.clone();
        let context = context.clone();
        thread::spawn(move || {
            let result = PreparedImport::read(&path).map_err(|error| format!("{error:#}"));
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    fn finish_imports(&mut self) {
        while let Ok(result) = self.import_receiver.try_recv() {
            self.imports_running = self.imports_running.saturating_sub(1);
            let added = result.and_then(|prepared| {
                self.library.lock().unwrap().add(prepared).map_err(|error| format!("{error:#}"))
            });
            match added {
                Ok(book) => self.open(&book),
                Err(error) => self.message = Some(error),
            }
        }
    }

    fn open(&mut self, book: &Book) {
        let loaded = {
            let mut library = self.library.lock().unwrap();
            library.sentences(book).map(|sentences| {
                let start = library.resume_index(book, &sentences);
                library.mark_opened(&book.id);
                (sentences, start)
            })
        };
        match loaded {
            Ok((sentences, start)) => {
                self.message = None;
                self.engine.send(Command::Open { book: book.clone(), sentences, start });
            }
            Err(error) => self.message = Some(format!("{error:#}")),
        }
    }

    fn pick_books(&mut self, context: &egui::Context) {
        let picked = rfd::FileDialog::new().add_filter("Books", &["pdf", "epub"]).pick_files();
        for path in picked.unwrap_or_default() {
            self.start_import(path, context);
        }
    }

    fn save_settings(&self) {
        let mut library = self.library.lock().unwrap();
        library.settings.voice = self.voice.clone();
        library.settings.speed = self.speed;
        let _ = library.save_settings();
    }
}

impl eframe::App for App {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.finish_imports();

        // Books dropped onto the window.
        let dropped: Vec<PathBuf> =
            context.input(|input| input.raw.dropped_files.iter().filter_map(|file| file.path.clone()).collect());
        for path in dropped {
            if path.extension().is_some_and(|extension| ["pdf", "epub"].iter().any(|kind| extension.eq_ignore_ascii_case(kind))) {
                self.start_import(path, context);
            } else {
                self.message = Some("Only PDF and EPUB files can be added.".to_string());
            }
        }

        let snapshot = self.engine.snapshot();

        // Space plays and pauses, the arrow keys step a sentence.
        let (space, left, right) = context.input(|input| {
            (
                input.key_pressed(egui::Key::Space),
                input.key_pressed(egui::Key::ArrowLeft),
                input.key_pressed(egui::Key::ArrowRight),
            )
        });
        if snapshot.book.is_some() {
            if space {
                self.engine.send(Command::Toggle);
            }
            if left {
                self.engine.send(Command::Skip(-1));
            }
            if right {
                self.engine.send(Command::Skip(1));
            }
        }

        self.library_panel(context, &snapshot);
        self.controls_panel(context, &snapshot);
        self.reading_panel(context, &snapshot);
    }
}

impl App {
    fn library_panel(&mut self, context: &egui::Context, snapshot: &Snapshot) {
        egui::SidePanel::left("library").resizable(true).default_width(260.0).min_width(200.0).show(context, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("Library");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Add book").clicked() {
                        self.pick_books(context);
                    }
                });
            });
            if self.imports_running > 0 {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Reading the book");
                });
            }
            ui.separator();

            let books = self.library.lock().unwrap().sorted_books();
            if books.is_empty() {
                ui.add_space(12.0);
                ui.label("No books yet. Add a PDF or EPUB, or drop one onto this window.");
            }

            let mut to_open: Option<Book> = None;
            let mut to_delete: Option<Book> = None;
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for book in &books {
                    let is_current = snapshot.book.as_ref().is_some_and(|current| current.id == book.id);
                    // The open book's progress is live; the others use what was saved.
                    let progress = if is_current && !snapshot.sentences.is_empty() {
                        snapshot.current_index as f32 / (snapshot.sentences.len().max(2) - 1) as f32
                    } else {
                        book.progress()
                    };
                    let response = ui
                        .group(|ui| {
                            ui.set_width(ui.available_width());
                            let title = egui::RichText::new(&book.title);
                            ui.label(if is_current { title.strong() } else { title });
                            ui.add(egui::ProgressBar::new(progress.clamp(0.0, 1.0)).desired_height(4.0));
                            ui.small(format!("{}% read, {} pages", (progress * 100.0).round(), book.page_count));
                        })
                        .response
                        .interact(egui::Sense::click());
                    if response.clicked() {
                        to_open = Some(book.clone());
                    }
                    response.context_menu(|ui| {
                        if ui.button("Remove from library").clicked() {
                            to_delete = Some(book.clone());
                            ui.close();
                        }
                    });
                }
            });

            if let Some(book) = to_open {
                self.open(&book);
            }
            if let Some(book) = to_delete {
                if snapshot.book.as_ref().is_some_and(|current| current.id == book.id) {
                    self.engine.send(Command::Close);
                }
                if let Err(error) = self.library.lock().unwrap().delete(&book.id) {
                    self.message = Some(format!("{error:#}"));
                }
            }
        });
    }

    fn controls_panel(&mut self, context: &egui::Context, snapshot: &Snapshot) {
        egui::TopBottomPanel::bottom("controls").show(context, |ui| {
            ui.add_space(8.0);
            let count = snapshot.sentences.len();
            let has_book = snapshot.book.is_some() && count > 0;

            // Progress through the book. Dragging jumps when released.
            ui.add_enabled_ui(has_book, |ui| {
                let last = count.saturating_sub(1).max(1);
                let mut value = self.scrub.unwrap_or(snapshot.current_index).min(last);
                ui.spacing_mut().slider_width = ui.available_width();
                let response = ui.add(egui::Slider::new(&mut value, 0..=last).show_value(false));
                if response.dragged() {
                    self.scrub = Some(value);
                }
                if response.drag_stopped() || (response.changed() && !response.dragged()) {
                    self.engine.send(Command::Seek(value));
                    self.scrub = None;
                }
                let shown = self.scrub.unwrap_or(snapshot.current_index);
                ui.horizontal(|ui| {
                    ui.small(format!("{}%", (shown as f32 / last as f32 * 100.0).round()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if has_book {
                            ui.small(format!("Sentence {} of {}", shown + 1, count));
                        }
                    });
                });
            });

            ui.horizontal(|ui| {
                ui.add_enabled_ui(has_book, |ui| {
                    if ui.button("Back").on_hover_text("Previous sentence (left arrow)").clicked() {
                        self.engine.send(Command::Skip(-1));
                    }
                    let label = if snapshot.playing { "Pause" } else { "Play" };
                    let play = egui::Button::new(egui::RichText::new(label).strong()).min_size(egui::vec2(90.0, 0.0));
                    if ui.add(play).on_hover_text("Space").clicked() {
                        self.engine.send(Command::Toggle);
                    }
                    if ui.button("Forward").on_hover_text("Next sentence (right arrow)").clicked() {
                        self.engine.send(Command::Skip(1));
                    }
                });

                ui.separator();

                egui::ComboBox::from_id_salt("speed")
                    .selected_text(format!("{}x", self.speed))
                    .width(70.0)
                    .show_ui(ui, |ui| {
                        for speed in SPEEDS {
                            if ui.selectable_value(&mut self.speed, speed, format!("{speed}x")).clicked() {
                                self.engine.send(Command::SetSpeed(speed));
                                self.save_settings();
                            }
                        }
                    });

                if !snapshot.voices.is_empty() {
                    let selected = snapshot
                        .voices
                        .iter()
                        .find(|voice| voice.id == self.voice)
                        .map_or_else(|| "Choose a voice".to_string(), |voice| voice.label.clone());
                    let mut chosen: Option<String> = None;
                    egui::ComboBox::from_id_salt("voice").selected_text(selected).width(210.0).show_ui(ui, |ui| {
                        for voice in snapshot.voices.iter().filter(|voice| !voice.expressive) {
                            if ui.selectable_label(self.voice == voice.id, &voice.label).clicked() {
                                chosen = Some(voice.id.clone());
                            }
                        }
                        ui.separator();
                        let expressive: Vec<_> = snapshot.voices.iter().filter(|voice| voice.expressive).collect();
                        if expressive.is_empty() {
                            // Picking an expressive voice downloads Phonon (about 70 MB).
                            if ui.selectable_label(false, "Expressive voices (download 70 MB)").clicked() {
                                chosen = Some(arcvoice_core::narrator::PHONON_PREFIX.to_string() + "Harper");
                            }
                        } else {
                            ui.small("Expressive voices");
                            for voice in expressive {
                                if ui.selectable_label(self.voice == voice.id, &voice.label).clicked() {
                                    chosen = Some(voice.id.clone());
                                }
                            }
                        }
                    });
                    if let Some(voice) = chosen {
                        self.voice = voice.clone();
                        self.engine.send(Command::SetVoice(voice));
                        self.save_settings();
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.sync_button(ui);
                });
            });

            self.status_line(ui, snapshot);
            ui.add_space(6.0);
        });
    }

    /// Chooses the folder that positions are mirrored to for other devices.
    fn sync_button(&mut self, ui: &mut egui::Ui) {
        let folder = self.library.lock().unwrap().settings.sync_folder.clone();
        let label = if folder.is_some() { "Sync folder set" } else { "Set sync folder" };
        let hover = match &folder {
            Some(path) => format!("Reading positions are shared through {}. Click to change.", path.display()),
            None => "Pick a folder inside iCloud Drive or OneDrive to share reading positions with other devices.".to_string(),
        };
        if ui.button(label).on_hover_text(hover).clicked() {
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                let mut library = self.library.lock().unwrap();
                library.settings.sync_folder = Some(path);
                let _ = library.save_settings();
            }
        }
    }

    fn status_line(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        let problem = self.message.clone().or_else(|| snapshot.error.clone());
        if let Some(problem) = problem {
            ui.horizontal(|ui| {
                ui.colored_label(ui.visuals().error_fg_color, problem);
                if self.message.is_some() && ui.small_button("Dismiss").clicked() {
                    self.message = None;
                }
            });
            return;
        }
        match &snapshot.voice_status {
            VoiceStatus::Downloading(fraction) => {
                ui.horizontal(|ui| {
                    ui.small("Downloading the voice, one time only");
                    ui.add(egui::ProgressBar::new(*fraction).desired_width(200.0).show_percentage());
                });
            }
            VoiceStatus::Loading => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.small("Getting the voice ready");
                });
            }
            VoiceStatus::Failed(reason) => {
                ui.colored_label(ui.visuals().error_fg_color, reason);
            }
            VoiceStatus::Ready => {
                let text = if snapshot.finished {
                    "End of book"
                } else if snapshot.buffering {
                    "Preparing the next sentence"
                } else {
                    // Keeps the layout from jumping when a status appears.
                    " "
                };
                ui.small(text);
            }
        }
    }

    fn reading_panel(&mut self, context: &egui::Context, snapshot: &Snapshot) {
        egui::CentralPanel::default().show(context, |ui| {
            let Some(book) = &snapshot.book else {
                ui.centered_and_justified(|ui| {
                    ui.label("Pick a book from the library, or drop a PDF or EPUB here.");
                });
                return;
            };
            ui.add_space(6.0);
            ui.heading(&book.title);
            ui.separator();

            // A few sentences either side of the one being read. Clicking any
            // of them jumps there.
            let count = snapshot.sentences.len();
            let current = snapshot.current_index.min(count.saturating_sub(1));
            let first = current.saturating_sub(3);
            let last = (current + 12).min(count);
            let mut jump: Option<usize> = None;

            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 12.0;
                for index in first..last {
                    let sentence = &snapshot.sentences[index];
                    let text = if index == current {
                        egui::RichText::new(sentence).size(19.0).strong()
                    } else {
                        egui::RichText::new(sentence).size(15.0).weak()
                    };
                    let response = ui.add(egui::Label::new(text).wrap().sense(egui::Sense::click()));
                    if response.clicked() {
                        jump = Some(index);
                    }
                    response.on_hover_cursor(egui::CursorIcon::PointingHand);
                }
            });

            if let Some(index) = jump {
                self.engine.send(Command::Seek(index));
            }
        });
    }
}
