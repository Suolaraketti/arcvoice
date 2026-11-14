import AVFoundation
import Foundation
import MediaPlayer
import os

/// Plays a book one sentence at a time.
///
/// Kokoro renders a sentence several times faster than it takes to speak it,
/// so the player keeps a few sentences rendered ahead of the one being spoken.
/// Each rendered sentence is handed to an `AVAudioPlayerNode` as its own
/// buffer; when a buffer finishes playing, the position moves forward one
/// sentence and the pump tops the queue back up. Playback starts as soon as
/// the first sentence is ready, which is what makes pressing play feel instant.
@MainActor
final class ReaderPlayer: ObservableObject {

    enum State: Equatable {
        case idle
        case playing
        case paused
        case finished
    }

    // MARK: - Published state

    @Published private(set) var state: State = .idle
    @Published private(set) var book: Book?
    @Published private(set) var sentences: [String] = []
    @Published private(set) var currentIndex = 0
    /// The book's chapters, empty when it has none worth listing.
    @Published private(set) var chapters: [Chapter] = []

    /// Position in `chapters` of the chapter being read.
    var currentChapterIndex: Int? {
        Chapter.index(containing: currentIndex, in: chapters)
    }

    var currentChapter: Chapter? {
        currentChapterIndex.map { chapters[$0] }
    }

    /// Jumps to the start of a chapter. Keeps playing if it was playing.
    func play(chapter: Chapter) {
        seek(to: chapter.start)
    }
    /// True while the sentence that should be playing is still being rendered.
    @Published private(set) var isBuffering = false
    @Published var lastError: String?

    // MARK: - Settings

    var voice: String = SpeechEngine.defaultVoice {
        didSet {
            guard voice != oldValue, book != nil else { return }
            // Throw away audio already rendered in the old voice.
            seek(to: currentIndex)
        }
    }

    /// Reading speed, 0.5 to 2. The voice itself speaks faster or slower,
    /// which sounds natural; stretching recorded audio would sound phasey.
    /// Sentences already rendered at the old speed are rendered again.
    var rate: Float = 1.0 {
        didSet {
            rate = min(2.0, max(0.5, rate))
            guard rate != oldValue else { return }
            if book != nil { seek(to: currentIndex) }
            updateNowPlaying()
        }
    }

    /// Sentences after which a paragraph ends, for a longer pause.
    private(set) var paragraphEnds: Set<Int> = []

    /// Called whenever the reading position changes: book id, sentence index, sentence text.
    var onPositionChanged: (@MainActor (UUID, Int, String) -> Void)?

    // MARK: - Audio graph

    private let engine: SpeechEngine
    private let audioEngine = AVAudioEngine()
    private let node = AVAudioPlayerNode()
    private let format: AVAudioFormat

    // MARK: - Queue bookkeeping

    /// Seconds of rendered audio to keep queued ahead of the listener, like a
    /// video player's buffer. Rendering runs about three times faster than
    /// speech, so the buffer fills quickly, keeps filling while paused, and is
    /// topped up as it plays.
    private let bufferAheadSeconds = 30.0
    /// Seconds of audio queued on the player and not yet heard.
    private var queuedSeconds = 0.0
    /// Below this much buffered audio, sentences are rendered a clause at a
    /// time, so each piece is ready before the one before it finishes
    /// playing. Above it, whole sentences, for the most natural delivery.
    private let lowBufferSeconds = 8.0
    /// When the listener last asked for sound from an empty buffer (play,
    /// skip, voice or speed change), to measure how quickly it came.
    private var waitingSince: ContinuousClock.Instant?
    private let log = Logger(subsystem: "com.kaczmarowski.arcvoice", category: "player")

    /// Bumped on every seek or stop. Anything still in flight from an older
    /// generation (a render, a buffer callback) sees the mismatch and is ignored.
    private var generation = 0
    private var nextToSynthesize = 0
    private var pumpTask: Task<Void, Never>?
    #if os(iOS)
    private var interruptedWhilePlaying = false
    #endif
    private var observers: [NSObjectProtocol] = []

    // MARK: - Setup

    init(engine: SpeechEngine) {
        self.engine = engine
        // 24 kHz mono float is a valid format, so this cannot fail in practice.
        self.format = AVAudioFormat(standardFormatWithSampleRate: SpeechEngine.sampleRate, channels: 1)!

        audioEngine.attach(node)
        audioEngine.connect(node, to: audioEngine.mainMixerNode, format: format)

        registerRemoteCommands()
        registerNotifications()
    }

    // MARK: - Opening a book

    func open(book: Book, sentences: [String], paragraphEnds: Set<Int>, chapters: [Chapter], startIndex: Int) {
        stopAudio()
        self.book = book
        self.sentences = sentences
        self.paragraphEnds = paragraphEnds
        self.chapters = chapters
        self.currentIndex = min(max(0, startIndex), max(0, sentences.count - 1))
        self.nextToSynthesize = currentIndex
        self.state = .paused
        self.lastError = nil
        updateNowPlaying()
        // Start buffering now, so pressing play is instant.
        startPump()
    }

    /// The voice finished loading: start buffering the open book.
    func voiceDidLoad() {
        startPump()
    }

    func close() {
        stopAudio()
        book = nil
        sentences = []
        paragraphEnds = []
        chapters = []
        currentIndex = 0
        state = .idle
        MPNowPlayingInfoCenter.default().nowPlayingInfo = nil
    }

    // MARK: - Transport

    func play() {
        guard book != nil, !sentences.isEmpty else { return }
        if state == .playing { return }
        if state == .finished {
            generation += 1
            pumpTask?.cancel()
            pumpTask = nil
            currentIndex = 0
            nextToSynthesize = 0
            queuedSeconds = 0
        }
        do {
            #if os(iOS)
            // The Mac has no audio session to configure.
            let session = AVAudioSession.sharedInstance()
            try session.setCategory(.playback, mode: .spokenAudio)
            try session.setActive(true)
            #endif
            if !audioEngine.isRunning {
                audioEngine.prepare()
                try audioEngine.start()
            }
        } catch {
            lastError = "Audio could not start: \(error.localizedDescription)"
            return
        }
        node.play()
        state = .playing
        lastError = nil
        isBuffering = queuedSeconds == 0
        if queuedSeconds == 0 { waitingSince = .now }
        log.info("play: \(self.queuedSeconds, format: .fixed(precision: 1))s already buffered")
        updateNowPlaying()
        startPump()
    }

    func pause() {
        guard state == .playing else { return }
        node.pause()
        audioEngine.pause()
        state = .paused
        isBuffering = false
        reportPosition()
        updateNowPlaying()
    }

    func togglePlayPause() {
        if state == .playing { pause() } else { play() }
    }

    /// Jumps to a sentence. Keeps playing if it was playing.
    func seek(to index: Int) {
        guard !sentences.isEmpty else { return }
        let wasPlaying = state == .playing
        let target = min(max(0, index), sentences.count - 1)

        generation += 1
        pumpTask?.cancel()
        pumpTask = nil
        // Stopping the node drops every buffer that was queued on it.
        node.stop()

        currentIndex = target
        nextToSynthesize = target
        queuedSeconds = 0
        isBuffering = wasPlaying
        if state == .finished { state = .paused }
        reportPosition()
        updateNowPlaying()

        if wasPlaying {
            node.play()
            waitingSince = .now
        }
        startPump()
    }

    func skip(by delta: Int) {
        seek(to: currentIndex + delta)
    }

    // MARK: - Rendering pump

    private func startPump() {
        guard pumpTask == nil, book != nil, state == .playing || state == .paused else { return }
        let pumpGeneration = generation
        pumpTask = Task { [weak self] in
            await self?.pump(generation: pumpGeneration)
        }
    }

    /// Renders until the buffer holds `bufferAheadSeconds` of audio, then
    /// exits. It is started again each time a buffer finishes playing.
    ///
    /// Like a video player, it adapts to how much is buffered. Starting from
    /// an empty buffer (a new book, a skip, a voice or speed change) or
    /// running low, sentences are rendered in clause-sized pieces with a short
    /// first piece, so speech starts within a second and each piece arrives
    /// before the last one finishes. With a few seconds in hand it renders
    /// whole sentences again.
    private func pump(generation pumpGeneration: Int) async {
        defer {
            if pumpGeneration == generation { pumpTask = nil }
        }
        while !Task.isCancelled, pumpGeneration == generation, state == .playing || state == .paused {
            guard nextToSynthesize < sentences.count,
                  queuedSeconds < bufferAheadSeconds || nextToSynthesize <= currentIndex + 1 else { return }

            let index = nextToSynthesize
            let text = sentences[index]
            let voiceName = voice
            let speed = rate
            let after: SpeechEngine.After = paragraphEnds.contains(index) ? .paragraph : .sentence
            var pieces: [(text: String, after: SpeechEngine.After)] = [(text, after)]
            if queuedSeconds < lowBufferSeconds {
                let parts = Self.clausePieces(text, shortFirst: queuedSeconds == 0)
                pieces = parts.enumerated().map { number, part in
                    (part, number == parts.count - 1 ? after : .clause)
                }
            }
            if index == currentIndex, state == .playing { isBuffering = true }

            for (number, piece) in pieces.enumerated() {
                let samples: [Float]
                let started = ContinuousClock.now
                do {
                    samples = try await engine.synthesize(piece.text, voice: voiceName, speed: speed, after: piece.after)
                } catch SpeechEngineError.notLoaded {
                    // Buffering starts again once the voice has loaded.
                    return
                } catch {
                    // One piece the voice cannot read must not stop the book.
                    // Log it and pass over it with a beat of silence.
                    guard pumpGeneration == generation else { return }
                    log.error("skipped sentence \(index): \(error.localizedDescription)")
                    samples = [Float](repeating: 0, count: Int(SpeechEngine.sampleRate * 0.3))
                }

                // A seek, or a voice or speed change, may have happened meanwhile.
                guard pumpGeneration == generation, !Task.isCancelled else { return }
                let took = started.duration(to: .now)
                let audio = Double(samples.count) / SpeechEngine.sampleRate
                log.info(
                    "rendered \(index)\(pieces.count > 1 ? " piece \(number + 1)/\(pieces.count)" : ""): \(audio, format: .fixed(precision: 2))s of audio in \(took.description), buffer \(self.queuedSeconds + audio, format: .fixed(precision: 1))s"
                )
                if let waited = waitingSince, state == .playing {
                    log.info("sound started \(waited.duration(to: .now).description) after asking")
                    waitingSince = nil
                }
                if index == currentIndex { isBuffering = false }
                schedule(
                    samples, index: index, endsSentence: number == pieces.count - 1, generation: pumpGeneration)
            }
            nextToSynthesize = index + 1
        }
    }

    /// A sentence cut into pieces for rendering while the buffer is low.
    /// Cuts prefer clause breaks (comma, semicolon, colon, dash) and fall
    /// back to the space between two words. With `shortFirst` (nothing
    /// buffered at all) the pieces grow: about 20 to 40 characters, then up
    /// to 65, 110 and 140, so the first sound comes fast and each piece lasts
    /// long enough to cover rendering the next (measured: a piece renders in
    /// well under half the time it takes to speak, plus a fixed moment).
    /// Otherwise pieces are up to 140 characters.
    static func clausePieces(_ text: String, shortFirst: Bool) -> [String] {
        let sizes = shortFirst ? [(20, 40), (30, 65), (40, 110)] : []
        var pieces: [String] = []
        var rest = Array(text.trimmingCharacters(in: .whitespaces))
        while !rest.isEmpty {
            let (minimum, limit) = pieces.count < sizes.count ? sizes[pieces.count] : (40, 140)
            // Not worth cutting: what is left is barely over the limit.
            if rest.count <= limit + 20 {
                pieces.append(String(rest))
                break
            }
            let upper = min(limit, rest.count - 12)
            var cut: Int?
            if minimum < upper {
                for index in minimum..<upper where isClauseBreak(rest, at: index) {
                    cut = index + 1
                }
                if cut == nil {
                    let target = limit * 4 / 5
                    cut = (minimum..<upper).filter { rest[$0] == " " }
                        .min(by: { abs($0 - target) < abs($1 - target) })
                }
            }
            guard let cut else {
                pieces.append(String(rest))
                break
            }
            pieces.append(String(rest[..<cut]).trimmingCharacters(in: .whitespaces))
            rest = Array(String(rest[cut...]).trimmingCharacters(in: .whitespaces))
        }
        return pieces.filter { !$0.isEmpty }
    }

    private static func isClauseBreak(_ characters: [Character], at index: Int) -> Bool {
        let mark = characters[index]
        if mark == "\u{2014}" || mark == "\u{2013}" { return true }
        return [",", ";", ":"].contains(mark) && index + 1 < characters.count && characters[index + 1] == " "
    }

    /// Queues rendered audio for sentence `index`: all of it, or its opening
    /// clause when `endsSentence` is false. The engine has already trimmed it
    /// and added the pause that follows it.
    private func schedule(_ samples: [Float], index: Int, endsSentence: Bool, generation scheduledGeneration: Int) {
        let totalFrames = max(1, samples.count)
        let seconds = Double(totalFrames) / SpeechEngine.sampleRate

        guard let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(totalFrames)),
              let channel = buffer.floatChannelData?[0] else {
            // Could not build a buffer. Treat the sentence as already spoken so
            // the book does not stall on it.
            if endsSentence { bufferFinished(index: index, seconds: 0, endsSentence: true, generation: scheduledGeneration) }
            return
        }
        queuedSeconds += seconds
        buffer.frameLength = AVAudioFrameCount(totalFrames)
        channel[0] = 0
        samples.withUnsafeBufferPointer { source in
            if let base = source.baseAddress {
                channel.update(from: base, count: samples.count)
            }
        }

        node.scheduleBuffer(buffer, completionCallbackType: .dataPlayedBack) { [weak self] _ in
            Task { @MainActor [weak self] in
                self?.bufferFinished(
                    index: index, seconds: seconds, endsSentence: endsSentence, generation: scheduledGeneration)
            }
        }
    }

    private func bufferFinished(index: Int, seconds: Double, endsSentence: Bool, generation finishedGeneration: Int) {
        // Buffers dropped by a seek also call back. Ignore those.
        guard finishedGeneration == generation else { return }
        queuedSeconds = max(0, queuedSeconds - seconds)
        guard endsSentence else {
            startPump()
            return
        }

        if index + 1 >= sentences.count {
            finishBook()
            return
        }
        currentIndex = index + 1
        reportPosition()
        updateNowPlaying()
        startPump()
    }

    private func finishBook() {
        generation += 1
        pumpTask?.cancel()
        pumpTask = nil
        node.stop()
        audioEngine.pause()
        queuedSeconds = 0
        currentIndex = max(0, sentences.count - 1)
        state = .finished
        isBuffering = false
        reportPosition()
        updateNowPlaying()
    }

    private func stopAudio() {
        generation += 1
        pumpTask?.cancel()
        pumpTask = nil
        node.stop()
        if audioEngine.isRunning { audioEngine.pause() }
        queuedSeconds = 0
        isBuffering = false
    }

    private func reportPosition() {
        guard let book, sentences.indices.contains(currentIndex) else { return }
        onPositionChanged?(book.id, currentIndex, sentences[currentIndex])
    }

    // MARK: - Lock screen and headphone controls

    private func registerRemoteCommands() {
        let center = MPRemoteCommandCenter.shared()

        center.playCommand.addTarget { [weak self] _ in
            Task { @MainActor [weak self] in self?.play() }
            return .success
        }
        center.pauseCommand.addTarget { [weak self] _ in
            Task { @MainActor [weak self] in self?.pause() }
            return .success
        }
        center.togglePlayPauseCommand.addTarget { [weak self] _ in
            Task { @MainActor [weak self] in self?.togglePlayPause() }
            return .success
        }
        // "Next" and "previous" move one sentence. Previous is the useful one:
        // a double or triple tap on AirPods replays what she just missed.
        center.nextTrackCommand.addTarget { [weak self] _ in
            Task { @MainActor [weak self] in self?.skip(by: 1) }
            return .success
        }
        center.previousTrackCommand.addTarget { [weak self] _ in
            Task { @MainActor [weak self] in self?.skip(by: -1) }
            return .success
        }
    }

    private func updateNowPlaying() {
        guard let book else {
            MPNowPlayingInfoCenter.default().nowPlayingInfo = nil
            return
        }
        let percent = sentences.count > 1
            ? Int((Double(currentIndex) / Double(sentences.count - 1) * 100).rounded())
            : 0
        // The lock screen shows the chapter under the book's title.
        let detail = currentChapter.map { "\($0.title) \u{00B7} \(percent)%" } ?? "\(percent)% read"
        MPNowPlayingInfoCenter.default().nowPlayingInfo = [
            MPMediaItemPropertyTitle: book.title,
            MPMediaItemPropertyArtist: detail,
            MPNowPlayingInfoPropertyPlaybackRate: state == .playing ? Double(rate) : 0.0,
        ]
        #if os(macOS)
        // On the Mac the system only routes the keyboard's media keys to an
        // app that says whether it is playing.
        MPNowPlayingInfoCenter.default().playbackState = state == .playing ? .playing : .paused
        #endif
    }

    // MARK: - Interruptions and route changes

    private func registerNotifications() {
        let center = NotificationCenter.default

        #if os(iOS)
        observers.append(center.addObserver(
            forName: AVAudioSession.interruptionNotification, object: nil, queue: .main
        ) { [weak self] notification in
            let info = notification.userInfo
            let typeValue = info?[AVAudioSessionInterruptionTypeKey] as? UInt
            let optionsValue = info?[AVAudioSessionInterruptionOptionKey] as? UInt ?? 0
            Task { @MainActor [weak self] in
                self?.handleInterruption(typeValue: typeValue, optionsValue: optionsValue)
            }
        })

        observers.append(center.addObserver(
            forName: AVAudioSession.routeChangeNotification, object: nil, queue: .main
        ) { [weak self] notification in
            let reasonValue = notification.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt
            Task { @MainActor [weak self] in
                self?.handleRouteChange(reasonValue: reasonValue)
            }
        })
        #endif

        observers.append(center.addObserver(
            forName: .AVAudioEngineConfigurationChange, object: audioEngine, queue: .main
        ) { [weak self] _ in
            Task { @MainActor [weak self] in
                self?.handleEngineReset()
            }
        })
    }

    #if os(iOS)
    /// A phone call, Siri or an alarm took the audio away.
    private func handleInterruption(typeValue: UInt?, optionsValue: UInt) {
        guard let typeValue, let type = AVAudioSession.InterruptionType(rawValue: typeValue) else { return }
        switch type {
        case .began:
            interruptedWhilePlaying = state == .playing
            if interruptedWhilePlaying { pause() }
        case .ended:
            let options = AVAudioSession.InterruptionOptions(rawValue: optionsValue)
            if interruptedWhilePlaying, options.contains(.shouldResume) { play() }
            interruptedWhilePlaying = false
        @unknown default:
            break
        }
    }

    /// Headphones unplugged or AirPods taken out: stop instead of blasting the
    /// book out of the phone speaker.
    private func handleRouteChange(reasonValue: UInt?) {
        guard let reasonValue,
              let reason = AVAudioSession.RouteChangeReason(rawValue: reasonValue) else { return }
        if reason == .oldDeviceUnavailable { pause() }
    }
    #endif

    /// The audio hardware changed underneath the engine, which stops it.
    /// Restart from the current sentence if she was listening.
    private func handleEngineReset() {
        guard state == .playing else { return }
        state = .paused
        seek(to: currentIndex)
        play()
    }
}
