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
    @Published private(set) var voices: [String] = []
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

    /// Downloads (first launch only) and loads the Kokoro model. Safe to call
    /// from several places at once; they all wait on the same load.
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
            let names = try await engine.load { fraction, message in
                Task { @MainActor in
                    AppModel.shared.updateLoadProgress(fraction, message)
                }
            }
            voices = names.filter(Self.isEnglishVoice).sorted()
            if !names.contains(selectedVoice) {
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

    /// Kokoro voice names start with a language letter and a gender letter:
    /// `af_` American female, `am_` American male, `bf_` and `bm_` British.
    static func isEnglishVoice(_ name: String) -> Bool {
        name.hasPrefix("af_") || name.hasPrefix("am_") || name.hasPrefix("bf_") || name.hasPrefix("bm_")
    }

    static func displayName(forVoice name: String) -> String {
        let parts = name.split(separator: "_", maxSplits: 1)
        guard parts.count == 2, parts[0].count == 2 else { return name }
        let accent: String
        switch parts[0].first {
        case "a": accent = "US"
        case "b": accent = "UK"
        default: accent = String(parts[0])
        }
        let gender = parts[0].last == "f" ? "female" : "male"
        return "\(parts[1].capitalized) (\(accent), \(gender))"
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
