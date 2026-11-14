import ArcVoiceCore
import Foundation

enum SpeechEngineError: LocalizedError {
    case notLoaded
    case download(String)
    case engine(String)

    var errorDescription: String? {
        switch self {
        case .notLoaded: return "The voice is not loaded yet."
        case .download(let detail): return "The voice could not be downloaded. \(detail)"
        case .engine(let detail): return detail
        }
    }
}

/// The voices: the same Rust engine the Windows app uses, running on this
/// device. It prepares each sentence for speech (times, numbers, names),
/// renders it, and adds the pause that should follow it. One engine
/// everywhere means a fix reaches every platform at once.
///
/// Two families of voices live in it. Kokoro's are the default: in listening
/// tests they read fiction the most naturally and accurately (about 350 MB).
/// Phonon's "expressive" voices are an optional extra (about 70 MB), fetched
/// only when asked for. Devices that already have Phonon from an earlier
/// version keep it.
///
/// An actor because the engine is not thread safe and each render blocks for
/// a moment; this keeps every call in a single queue, off the main thread.
actor SpeechEngine {

    /// The engine always produces 24 kHz mono audio.
    static let sampleRate: Double = 24_000
    static let defaultVoice = "af_heart"

    struct Voice: Hashable, Sendable {
        let id: String
        /// "Harper (US, female)".
        let label: String
        /// One of Phonon's optional expressive voices.
        let expressive: Bool
    }

    private var engine: OpaquePointer?
    private var voices: [Voice] = []

    deinit {
        if let engine { arcvoice_engine_close(engine) }
    }

    /// Two performance cores on an iPhone; more threads only add heat. Phonon
    /// renders well over ten times faster than it speaks on two.
    private static var threads: UInt32 {
        #if os(iOS)
        return 2
        #else
        return UInt32(min(4, max(1, ProcessInfo.processInfo.activeProcessorCount / 2)))
        #endif
    }

    /// The voice files sit in Application Support, not Caches, so iOS never
    /// deletes them to free space and forces a download in the middle of a book.
    private static func directory(_ name: String) throws -> URL {
        let support = try FileManager.default.url(
            for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
        var directory = support.appendingPathComponent(name, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        // They can always be downloaded again, so keep them out of iCloud backups.
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? directory.setResourceValues(values)
        // The earlier CoreML voice is no longer used. Give its space back.
        try? FileManager.default.removeItem(at: support.appendingPathComponent("KokoroModel", isDirectory: true))
        return directory
    }

    private static func phononDirectory() throws -> URL { try directory("Phonon") }
    private static func kokoroDirectory() throws -> URL { try directory("KokoroONNX") }

    /// Loads the voices, downloading Kokoro the first time. Returns them,
    /// best first.
    func load(progress: @escaping @Sendable (Double, String) -> Void) async throws -> [Voice] {
        if engine != nil { return voices }

        let kokoro = try Self.kokoroDirectory()
        try await Self.download(Self.files(arcvoice_kokoro_downloads()), into: kokoro, progress: progress)
        progress(1, "Loading the voice")

        let phonon = try Self.phononDirectory()
        let phononReady = Self.isDownloaded(Self.files(arcvoice_phonon_downloads()), in: phonon)
        var error: UnsafeMutablePointer<CChar>?
        let opened = arcvoice_engine_open(
            phononReady ? phonon.path : nil, kokoro.appendingPathComponent("kokoro-v1.0.onnx").path,
            kokoro.appendingPathComponent("voices-v1.0.bin").path, Self.threads, &error)
        guard let opened else {
            throw SpeechEngineError.engine(Self.take(error) ?? "The voice could not be loaded.")
        }
        engine = opened
        voices = readVoices()
        // The first render is slower than the rest. Take that hit now.
        _ = try? synthesize("Ready.", voice: Self.defaultVoice, speed: 1, after: .sentence)
        return voices
    }

    /// True once Phonon's expressive voices are on this device.
    func hasExpressiveVoices() -> Bool {
        voices.contains { $0.expressive }
    }

    /// Downloads Phonon's expressive voices (about 70 MB) and adds them.
    func addExpressiveVoices(progress: @escaping @Sendable (Double, String) -> Void) async throws -> [Voice] {
        guard let engine else { throw SpeechEngineError.notLoaded }
        if hasExpressiveVoices() { return voices }
        let phonon = try Self.phononDirectory()
        try await Self.download(Self.files(arcvoice_phonon_downloads()), into: phonon, progress: progress)
        progress(1, "Loading the expressive voices")
        var error: UnsafeMutablePointer<CChar>?
        guard arcvoice_engine_add_phonon(engine, phonon.path, Self.threads, &error) else {
            throw SpeechEngineError.engine(Self.take(error) ?? "The expressive voices could not be loaded.")
        }
        voices = readVoices()
        return voices
    }

    private func readVoices() -> [Voice] {
        guard let engine else { return [] }
        return (Self.take(arcvoice_engine_voices(engine)) ?? "")
            .split(separator: "\n")
            .compactMap { line in
                let fields = line.split(separator: "\t", omittingEmptySubsequences: false)
                guard fields.count == 3 else { return nil }
                return Voice(id: String(fields[0]), label: String(fields[1]), expressive: fields[2] == "1")
            }
    }

    /// What follows a rendered piece, which sets the pause after it.
    enum After {
        case sentence
        case paragraph
        /// The first part of a sentence, played early so reading starts sooner.
        case clause

        var code: UInt32 {
            switch self {
            case .sentence: return UInt32(ARCVOICE_AFTER_SENTENCE)
            case .paragraph: return UInt32(ARCVOICE_AFTER_PARAGRAPH)
            case .clause: return UInt32(ARCVOICE_AFTER_CLAUSE)
            }
        }
    }

    /// Renders text at 24 kHz, ready to play: the speech, then the pause that
    /// belongs after it. An unknown voice falls back to the default.
    func synthesize(_ text: String, voice: String, speed: Float, after: After) throws -> [Float] {
        guard let engine else { throw SpeechEngineError.notLoaded }
        let chosen = voices.contains { $0.id == voice } ? voice : Self.defaultVoice
        var samples: UnsafeMutablePointer<Float>?
        var count = 0
        var error: UnsafeMutablePointer<CChar>?
        guard arcvoice_engine_render(engine, text, chosen, speed, after.code, &samples, &count, &error),
              let samples
        else {
            throw SpeechEngineError.engine(Self.take(error) ?? "The voice could not read that sentence.")
        }
        defer { arcvoice_samples_free(samples, count) }
        return Array(UnsafeBufferPointer(start: samples, count: count))
    }

    /// The phonemes a sentence is spoken with by the Kokoro voices, for
    /// looking into a word they say oddly.
    func phonemes(for text: String) -> String? {
        guard let engine else { return nil }
        return Self.take(arcvoice_engine_phonemize(engine, text, false))
    }

    /// Copies a string from the engine and frees the original.
    private static func take(_ pointer: UnsafeMutablePointer<CChar>?) -> String? {
        guard let pointer else { return nil }
        defer { arcvoice_string_free(pointer) }
        return String(cString: pointer)
    }

    // MARK: - Download

    /// A file the engine needs: path inside its folder, source, approximate size.
    private struct RemoteFile {
        let path: String
        let url: URL
        let bytes: Int64
    }

    /// The engine's own list of files, so this app always downloads exactly
    /// what the engine expects.
    private static func files(_ list: UnsafeMutablePointer<CChar>?) -> [RemoteFile] {
        (take(list) ?? "").split(separator: "\n").compactMap { line in
            let fields = line.split(separator: "\t")
            guard fields.count == 3, let url = URL(string: String(fields[1])), let bytes = Int64(fields[2])
            else { return nil }
            return RemoteFile(path: String(fields[0]), url: url, bytes: bytes)
        }
    }

    private static func isComplete(_ url: URL, expected: Int64) -> Bool {
        let size = (try? url.resourceValues(forKeys: [.fileSizeKey]))?.fileSize ?? 0
        return Int64(size) >= expected * 9 / 10
    }

    private static func isDownloaded(_ files: [RemoteFile], in directory: URL) -> Bool {
        !files.isEmpty && files.allSatisfy { isComplete(directory.appendingPathComponent($0.path), expected: $0.bytes) }
    }

    private static func download(
        _ files: [RemoteFile], into directory: URL, progress: @escaping @Sendable (Double, String) -> Void
    ) async throws {
        let total = max(1, files.reduce(0) { $0 + $1.bytes })
        var done: Int64 = 0
        for file in files {
            let target = directory.appendingPathComponent(file.path)
            if !isComplete(target, expected: file.bytes) {
                try FileManager.default.createDirectory(
                    at: target.deletingLastPathComponent(), withIntermediateDirectories: true)
                let before = done
                let downloader = FileDownloader(destination: target) { written in
                    progress(Double(before + written) / Double(total), "Downloading the voice")
                }
                try await downloader.download(file.url)
                guard isComplete(target, expected: file.bytes) else {
                    try? FileManager.default.removeItem(at: target)
                    throw SpeechEngineError.download("The file was incomplete. Check the connection and try again.")
                }
            }
            done += file.bytes
            progress(Double(done) / Double(total), "Downloading the voice")
        }
    }
}

/// Downloads one file to a destination, reporting bytes as they arrive.
private final class FileDownloader: NSObject, URLSessionDownloadDelegate, @unchecked Sendable {
    private let destination: URL
    private let onBytes: @Sendable (Int64) -> Void
    private var continuation: CheckedContinuation<Void, Error>?
    private var lastReported: Int64 = 0

    init(destination: URL, onBytes: @escaping @Sendable (Int64) -> Void) {
        self.destination = destination
        self.onBytes = onBytes
    }

    func download(_ url: URL) async throws {
        let session = URLSession(configuration: .default, delegate: self, delegateQueue: nil)
        defer { session.finishTasksAndInvalidate() }
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            self.continuation = continuation
            session.downloadTask(with: url).resume()
        }
    }

    func urlSession(
        _ session: URLSession, downloadTask: URLSessionDownloadTask, didWriteData bytesWritten: Int64,
        totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64
    ) {
        // Every chunk would flood the main thread with progress updates.
        if totalBytesWritten - lastReported >= 1_000_000 {
            lastReported = totalBytesWritten
            onBytes(totalBytesWritten)
        }
    }

    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {
        // The temporary file disappears when this returns, so move it now.
        let status = (downloadTask.response as? HTTPURLResponse)?.statusCode ?? 200
        guard (200..<300).contains(status) else {
            finish(SpeechEngineError.download("The server answered \(status)."))
            return
        }
        do {
            try? FileManager.default.removeItem(at: destination)
            try FileManager.default.moveItem(at: location, to: destination)
            finish(nil)
        } catch {
            finish(error)
        }
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        if let error { finish(SpeechEngineError.download(error.localizedDescription)) }
    }

    private func finish(_ error: Error?) {
        guard let continuation else { return }
        self.continuation = nil
        if let error { continuation.resume(throwing: error) } else { continuation.resume() }
    }
}
