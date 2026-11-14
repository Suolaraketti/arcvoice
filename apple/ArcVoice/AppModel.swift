import Foundation
import SwiftUI

/// Ties the library, the voice and the player together. One shared instance,
/// because the Action Button shortcut has to reach the same player the screen
/// is showing, including when iOS starts the app in the background to run it.
@MainActor
final class AppModel: ObservableObject {

    static let shared = AppModel()

    enum VoiceState: Equatable {
        case notLoaded
        case loading(progress: Double, message: String)
        case ready
        case failed(String)
    }

    let library: LibraryStore
    let player: ReaderPlayer
    private let engine: SpeechEngine

    @Published private(set) var voiceState: VoiceState = .notLoaded
    @Published private(set) var voices: [SpeechEngine.Voice] = []
    /// Progress of the optional classic (Kokoro) voices download, 0 to 1,
    /// while it runs.
    @Published private(set) var classicDownloadProgress: Double?
    @Published var classicDownloadError: String?
    @Published private(set) var isImporting = false
    @Published var importError: String?
    /// The book whose reader screen is showing. Setting it navigates.
    @Published var openBook: Book?

    @Published var selectedVoice: String {
        didSet {
            UserDefaults.standard.set(selectedVoice, forKey: Keys.voice)
            player.voice = selectedVoice
        }
    }

    @Published var rate: Double {
        didSet {
            UserDefaults.standard.set(rate, forKey: Keys.rate)
            player.rate = Float(rate)
        }
    }

    static let rateChoices: [Double] = [0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0]

    private enum Keys {
        static let voice = "selectedVoice"
        static let rate = "playbackRate"
    }

    private var loadTask: Task<Void, Never>?

    private init() {
        let engine = SpeechEngine()
        let library = LibraryStore()
        let player = ReaderPlayer(engine: engine)
        self.engine = engine
        self.library = library
        self.player = player

        let savedVoice = UserDefaults.standard.string(forKey: Keys.voice) ?? SpeechEngine.defaultVoice
        let savedRate = UserDefaults.standard.double(forKey: Keys.rate)
        self.selectedVoice = savedVoice
        self.rate = savedRate > 0 ? savedRate : 1.0

        // didSet does not run for assignments inside init, so push the saved
        // settings into the player by hand.
        player.voice = savedVoice
        player.rate = Float(savedRate > 0 ? savedRate : 1.0)
        player.onPositionChanged = { [weak library] bookID, index, anchor in
            library?.savePosition(bookID: bookID, sentenceIndex: index, anchor: anchor)
        }
    }

    // MARK: - Voice

    /// Downloads (first launch only) and loads the voices. Safe to call from
    /// several places at once; they all wait on the same load.
    func prepareVoice() async {
        if voiceState == .ready { return }
        if let loadTask {
            await loadTask.value
            return
        }
        let task = Task { [weak self] in
            guard let self else { return }
            await self.runLoad()
        }
        loadTask = task
        await task.value
        loadTask = nil
    }

    private func runLoad() async {
        voiceState = .loading(progress: 0, message: "Getting the voice ready")
        do {
            voices = try await engine.load { fraction, message in
                Task { @MainActor in
                    AppModel.shared.updateLoadProgress(fraction, message)
                }
            }
            // A classic voice chosen before, on a device without the classic
            // voices (or a voice that no longer exists), becomes the default.
            if !voices.contains(where: { $0.id == selectedVoice }) {
                selectedVoice = SpeechEngine.defaultVoice
            }
            voiceState = .ready
            player.voiceDidLoad()
        } catch {
            voiceState = .failed(error.localizedDescription)
        }
    }

    private func updateLoadProgress(_ fraction: Double, _ message: String) {
        // Late progress callbacks must not overwrite "ready".
        guard case .loading = voiceState else { return }
        voiceState = .loading(progress: fraction, message: message)
    }

    var hasClassicVoices: Bool {
        voices.contains { $0.classic }
    }

    /// Downloads Kokoro's classic voices (about 350 MB) and adds them to the
    /// voice picker.
    func downloadClassicVoices() async {
        guard voiceState == .ready, classicDownloadProgress == nil, !hasClassicVoices else { return }
        classicDownloadProgress = 0
        classicDownloadError = nil
        do {
            voices = try await engine.addClassicVoices { fraction, _ in
                Task { @MainActor in
                    if AppModel.shared.classicDownloadProgress != nil {
                        AppModel.shared.classicDownloadProgress = fraction
                    }
                }
            }
        } catch {
            classicDownloadError = error.localizedDescription
        }
        classicDownloadProgress = nil
    }

    // MARK: - Books

    func importBook(from url: URL) async {
        isImporting = true
        defer { isImporting = false }
        do {
            let book = try await library.importBook(from: url)
            open(book)
        } catch {
            importError = error.localizedDescription
        }
    }

    /// Loads a book into the player at its saved position and shows its screen.
    func open(_ book: Book) {
        if loadIntoPlayer(book) {
            openBook = book
        }
    }

    @discardableResult
    private func loadIntoPlayer(_ book: Book) -> Bool {
        if player.book?.id == book.id { return true }
        do {
            let sentences = try library.sentences(for: book)
            let saved = library.position(for: book)?.sentenceIndex ?? book.sentenceIndex
            player.open(
                book: book, sentences: sentences, paragraphEnds: library.paragraphEnds(for: book), startIndex: saved)
            library.markOpened(book)
            return true
        } catch {
            importError = "That book's text could not be loaded. Try importing the book again."
            return false
        }
    }

    func delete(_ book: Book) {
        if player.book?.id == book.id { player.close() }
        if openBook?.id == book.id { openBook = nil }
        library.delete(book)
    }

    // MARK: - Playback

    /// Starts reading, loading the voice first if it is not ready yet.
    func play() async {
        await prepareVoice()
        guard voiceState == .ready else { return }
        player.play()
    }

    func togglePlayback() async {
        if player.state == .playing {
            player.pause()
        } else {
            await resume()
        }
    }

    /// Picks up the most recent book where she left off. This is what the
    /// Action Button runs, so it works with no book open and no screen showing.
    func resume() async {
        if player.book == nil {
            guard let recent = library.mostRecent, loadIntoPlayer(recent) else { return }
        }
        await play()
    }
}
