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

/// The voice: the same Rust engine the Windows app uses, running on this
/// device. It prepares each sentence for speech (times, numbers, names),
/// works out pronunciations, renders it with the Kokoro model and adds the
/// pause that should follow it. One engine everywhere means a pronunciation
/// fix reaches every platform at once.
///
/// An actor because the engine is not thread safe and each render blocks for
/// a moment; this keeps every call in a single queue, off the main thread.
actor SpeechEngine {

    /// Kokoro always produces 24 kHz mono audio.
    static let sampleRate: Double = 24_000
    static let defaultVoice = "af_heart"

    /// The kokoro-onnx project's release of the Kokoro v1.0 model and voices,
    /// the files the Windows app uses too. The full precision model is used
    /// on purpose: it is the voice exactly as trained and as Windows plays it,
    /// and it still renders about three times faster than it speaks on two
    /// iPhone cores. (The 8-bit version is twice as fast and scored the same
    /// on quality checks, if battery ever matters more.)
    private static let release = URL(
        string: "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0")!
    private static let files: [(name: String, bytes: Int64)] = [
        ("voices-v1.0.bin", 28_214_398),
        ("kokoro-v1.0.onnx", 325_532_387),
    ]

    private var engine: OpaquePointer?
    private var voices: [String] = []

    deinit {
        if let engine { arcvoice_engine_close(engine) }
    }

    /// The model sits in Application Support, not Caches, so iOS never deletes
    /// it to free space and forces a download in the middle of a book.
    private static func modelDirectory() throws -> URL {
        let support = try FileManager.default.url(
            for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
        var directory = support.appendingPathComponent("KokoroONNX", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        // It can always be downloaded again, so keep it out of iCloud backups.
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? directory.setResourceValues(values)
        // The earlier CoreML voice is no longer used. Give its space back.
        try? FileManager.default.removeItem(at: support.appendingPathComponent("KokoroModel", isDirectory: true))
        return directory
    }

    /// Loads the voice, downloading it the first time. Returns the voice names.
    func load(progress: @escaping @Sendable (Double, String) -> Void) async throws -> [String] {
        if engine != nil { return voices }

        let directory = try Self.modelDirectory()
        try await Self.downloadMissing(into: directory, progress: progress)
        progress(1, "Loading the voice")

        let model = directory.appendingPathComponent(Self.files[1].name).path
        let voicesFile = directory.appendingPathComponent(Self.files[0].name).path
        // An iPhone has two performance cores; more threads only add heat.
        #if os(iOS)
        let threads: UInt32 = 2
        #else
        let threads = UInt32(min(4, max(1, ProcessInfo.processInfo.activeProcessorCount / 2)))
        #endif
        var error: UnsafeMutablePointer<CChar>?
        guard let opened = arcvoice_engine_open(model, voicesFile, threads, &error) else {
            throw SpeechEngineError.engine(Self.take(error) ?? "The voice could not be loaded.")
        }
        engine = opened
        voices = (Self.take(arcvoice_engine_voices(opened)) ?? "")
            .split(separator: "\n").map(String.init)
        // The first render is slower than the rest. Take that hit now.
        _ = try? synthesize("Ready.", voice: Self.defaultVoice, speed: 1, endsParagraph: false)
        return voices
    }

    /// Renders one sentence at 24 kHz, ready to play: the speech, then the
    /// pause that belongs after it.
    func synthesize(_ text: String, voice: String, speed: Float, endsParagraph: Bool) throws -> [Float] {
        guard let engine else { throw SpeechEngineError.notLoaded }
        let chosen = voices.contains(voice) ? voice : Self.defaultVoice
        var samples: UnsafeMutablePointer<Float>?
        var count = 0
        var error: UnsafeMutablePointer<CChar>?
        guard arcvoice_engine_render(engine, text, chosen, speed, endsParagraph, &samples, &count, &error),
              let samples
        else {
            throw SpeechEngineError.engine(Self.take(error) ?? "The voice could not read that sentence.")
        }
        defer { arcvoice_samples_free(samples, count) }
        return Array(UnsafeBufferPointer(start: samples, count: count))
    }

    /// The phonemes a sentence is spoken with, for looking into a word the
    /// voice says oddly.
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

    private static func isComplete(_ url: URL, expected: Int64) -> Bool {
        let size = (try? url.resourceValues(forKeys: [.fileSizeKey]))?.fileSize ?? 0
        return Int64(size) >= expected * 9 / 10
    }

    private static func downloadMissing(
        into directory: URL, progress: @escaping @Sendable (Double, String) -> Void
    ) async throws {
        let total = files.reduce(0) { $0 + $1.bytes }
        var done: Int64 = 0
        for file in files {
            let target = directory.appendingPathComponent(file.name)
            if !isComplete(target, expected: file.bytes) {
                let before = done
                let downloader = FileDownloader(destination: target) { written in
                    progress(Double(before + written) / Double(total), "Downloading the voice")
                }
                try await downloader.download(release.appendingPathComponent(file.name))
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
