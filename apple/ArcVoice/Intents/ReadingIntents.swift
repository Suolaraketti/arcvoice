import AppIntents

/// One press starts the book where she left off, the next press pauses it.
/// This is the one to put on the Action Button.
///
/// `AudioPlaybackIntent` tells iOS this action plays audio, so it is allowed
/// to start sound without bringing the app to the front.
struct ToggleReadingIntent: AudioPlaybackIntent {
    static var title: LocalizedStringResource = "Play or Pause Reading"
    static var description = IntentDescription("Resumes your most recent book where you left off, or pauses it if it is playing.")
    static var openAppWhenRun: Bool = false

    @MainActor
    func perform() async throws -> some IntentResult {
        await AppModel.shared.togglePlayback()
        return .result()
    }
}

struct ResumeReadingIntent: AudioPlaybackIntent {
    static var title: LocalizedStringResource = "Resume Reading"
    static var description = IntentDescription("Resumes your most recent book where you left off.")
    static var openAppWhenRun: Bool = false

    @MainActor
    func perform() async throws -> some IntentResult {
        await AppModel.shared.resume()
        return .result()
    }
}

struct PauseReadingIntent: AudioPlaybackIntent {
    static var title: LocalizedStringResource = "Pause Reading"
    static var description = IntentDescription("Pauses the book that is playing.")
    static var openAppWhenRun: Bool = false

    @MainActor
    func perform() async throws -> some IntentResult {
        AppModel.shared.player.pause()
        return .result()
    }
}

/// Jumps back a few sentences, for when her attention drifted.
struct ReplayLastSentencesIntent: AudioPlaybackIntent {
    static var title: LocalizedStringResource = "Go Back a Few Sentences"
    static var description = IntentDescription("Jumps back three sentences in the book that is playing.")
    static var openAppWhenRun: Bool = false

    @MainActor
    func perform() async throws -> some IntentResult {
        AppModel.shared.player.skip(by: -3)
        return .result()
    }
}

/// Makes the actions show up in Shortcuts, Spotlight, Siri and the Action
/// Button picker with no setup.
struct ArcVoiceShortcuts: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
        AppShortcut(
            intent: ToggleReadingIntent(),
            phrases: [
                "Play or pause \(.applicationName)",
                "Read my book with \(.applicationName)",
            ],
            shortTitle: "Play or Pause",
            systemImageName: "playpause.fill"
        )
        AppShortcut(
            intent: ResumeReadingIntent(),
            phrases: ["Resume reading in \(.applicationName)"],
            shortTitle: "Resume Reading",
            systemImageName: "play.fill"
        )
        AppShortcut(
            intent: PauseReadingIntent(),
            phrases: ["Pause reading in \(.applicationName)"],
            shortTitle: "Pause Reading",
            systemImageName: "pause.fill"
        )
        AppShortcut(
            intent: ReplayLastSentencesIntent(),
            phrases: ["Go back in \(.applicationName)"],
            shortTitle: "Go Back",
            systemImageName: "gobackward"
        )
    }
}
