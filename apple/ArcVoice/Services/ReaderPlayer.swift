import AVFoundation
import Foundation
import MediaPlayer

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

    /// Playback speed. Applied by a time-stretch unit, so it changes instantly
    /// and keeps the pitch of the voice.
    var rate: Float = 1.0 {
        didSet {
            timePitch.rate = min(2.0, max(0.5, rate))
            updateNowPlaying()
        }
    }

    /// Called whenever the reading position changes: book id, sentence index, sentence text.
    var onPositionChanged: (@MainActor (UUID, Int, String) -> Void)?

    // MARK: - Audio graph

    private let engine: SpeechEngine
    private let audioEngine = AVAudioEngine()
    private let node = AVAudioPlayerNode()
    private let timePitch = AVAudioUnitTimePitch()
    private let format: AVAudioFormat

    // MARK: - Queue bookkeeping

    /// How many sentences to keep rendered beyond the one being spoken.
    private let lookahead = 3
    /// A short breath between sentences, in seconds. Headings get a longer one.
    private let sentenceGap = 0.12
    private let headingGap = 0.45

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
        audioEngine.attach(timePitch)
        audioEngine.connect(node, to: timePitch, format: format)
        audioEngine.connect(timePitch, to: audioEngine.mainMixerNode, format: format)

        registerRemoteCommands()
        registerNotifications()
    }

    // MARK: - Opening a book

    func open(book: Book, sentences: [String], startIndex: Int) {
        stopAudio()
        self.book = book
        self.sentences = sentences
        self.currentIndex = min(max(0, startIndex), max(0, sentences.count - 1))
        self.nextToSynthesize = currentIndex
        self.state = .paused
        self.lastError = nil
        updateNowPlaying()
    }

    func close() {
        stopAudio()
        book = nil
        sentences = []
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
            currentIndex = 0
            nextToSynthesize = 0
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
        isBuffering = false
        if state == .finished { state = .paused }
        reportPosition()
        updateNowPlaying()

        if wasPlaying {
            node.play()
            startPump()
        }
    }

    func skip(by delta: Int) {
        seek(to: currentIndex + delta)
    }

    // MARK: - Rendering pump

    private func startPump() {
        guard pumpTask == nil, state == .playing else { return }
        let pumpGeneration = generation
        pumpTask = Task { [weak self] in
            await self?.pump(generation: pumpGeneration)
        }
    }

    /// Renders sentences until the look-ahead window is full, then exits.
    /// It is started again each time a sentence finishes playing.
    private func pump(generation pumpGeneration: Int) async {
        defer {
            if pumpGeneration == generation { pumpTask = nil }
        }
        while !Task.isCancelled, pumpGeneration == generation, state == .playing {
            guard nextToSynthesize < sentences.count,
                  nextToSynthesize - currentIndex <= lookahead else { return }

            let index = nextToSynthesize
            let text = sentences[index]
            let voiceName = voice
            if index == currentIndex { isBuffering = true }

            let samples: [Float]
            do {
                samples = try await engine.synthesize(text, voice: voiceName)
            } catch {
                guard pumpGeneration == generation else { return }
                isBuffering = false
                lastError = "The voice hit a problem: \(error.localizedDescription)"
                pauseAfterFailure()
                return
            }

            // A seek or a pause may have happened while that was rendering.
            guard pumpGeneration == generation, !Task.isCancelled else { return }
            if index == currentIndex { isBuffering = false }
            nextToSynthesize = index + 1
            schedule(samples, index: index, generation: pumpGeneration)
        }
    }

    private func pauseAfterFailure() {
        node.pause()
        audioEngine.pause()
        state = .paused
        updateNowPlaying()
    }

    private func schedule(_ samples: [Float], index: Int, generation scheduledGeneration: Int) {
        let gapSeconds = TextCleaner.isHeading(sentences[index]) ? headingGap : sentenceGap
        let gapFrames = Int(gapSeconds * SpeechEngine.sampleRate)
        let totalFrames = samples.count + gapFrames

        guard let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(totalFrames)),
              let channel = buffer.floatChannelData?[0] else {
            // Could not build a buffer. Treat the sentence as already spoken so
            // the book does not stall on it.
            bufferFinished(index: index, generation: scheduledGeneration)
            return
        }
        buffer.frameLength = AVAudioFrameCount(totalFrames)
        samples.withUnsafeBufferPointer { source in
            if let base = source.baseAddress {
                channel.update(from: base, count: samples.count)
            }
        }
        // The frames after the speech are the pause between sentences.
        for frame in samples.count..<totalFrames {
            channel[frame] = 0
        }

        node.scheduleBuffer(buffer, completionCallbackType: .dataPlayedBack) { [weak self] _ in
            Task { @MainActor [weak self] in
                self?.bufferFinished(index: index, generation: scheduledGeneration)
            }
        }
    }

    private func bufferFinished(index: Int, generation finishedGeneration: Int) {
        // Buffers dropped by a seek also call back. Ignore those.
        guard finishedGeneration == generation else { return }

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
        MPNowPlayingInfoCenter.default().nowPlayingInfo = [
            MPMediaItemPropertyTitle: book.title,
            MPMediaItemPropertyArtist: "\(percent)% read",
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
