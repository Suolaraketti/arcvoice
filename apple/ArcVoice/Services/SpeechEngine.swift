import CoreML
import Foundation
import KokoroTTS

enum SpeechEngineError: LocalizedError {
    case notLoaded

    var errorDescription: String? {
        switch self {
        case .notLoaded: return "The voice is not loaded yet."
        }
    }
}

/// Owns the Kokoro model. An actor because the model is not thread safe and
/// each synthesis call blocks for a moment; this keeps every call in a single
/// queue, off the main thread.
actor SpeechEngine {

    /// Kokoro always produces 24 kHz mono audio.
    static let sampleRate: Double = 24_000
    static let defaultVoice = "af_heart"

    private var model: KokoroTTSModel?

    /// The model sits in Application Support, not Caches, so iOS never deletes
    /// it to free space and forces a 170 MB download in the middle of a book.
    private static func modelDirectory() throws -> URL {
        let support = try FileManager.default.url(
            for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
        var directory = support.appendingPathComponent("KokoroModel", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        // It can always be downloaded again, so keep it out of iCloud backups.
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? directory.setResourceValues(values)
        return directory
    }

    /// Loads the model, downloading it the first time. Returns the voice names.
    ///
    /// `.cpuAndNeuralEngine` is deliberate. iOS does not let an app use the GPU
    /// while it is in the background, and this app spends most of its life
    /// reading with the screen locked. If speech ever sounds garbled on a
    /// particular phone, `.cpuOnly` is the safe fallback.
    func load(progress: @escaping @Sendable (Double, String) -> Void) async throws -> [String] {
        if let model { return model.availableVoices }

        let directory = try Self.modelDirectory()
        let loaded: KokoroTTSModel
        do {
            // Try what is already on disk first, so opening the app on a plane
            // does not sit waiting on a network check.
            loaded = try await KokoroTTSModel.fromPretrained(
                cacheDir: directory,
                offlineMode: true,
                computeUnits: .cpuAndNeuralEngine,
                progressHandler: progress)
        } catch {
            loaded = try await KokoroTTSModel.fromPretrained(
                cacheDir: directory,
                offlineMode: false,
                computeUnits: .cpuAndNeuralEngine,
                progressHandler: progress)
        }
        model = loaded
        return loaded.availableVoices
    }

    /// Renders one sentence to audio samples at 24 kHz.
    func synthesize(_ text: String, voice: String) throws -> [Float] {
        guard let model else { throw SpeechEngineError.notLoaded }
        let chosen = model.availableVoices.contains(voice) ? voice : Self.defaultVoice
        return try model.synthesize(text: text, voice: chosen, language: "en", speed: 1.0)
    }
}
