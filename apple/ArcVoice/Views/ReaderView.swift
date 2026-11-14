import SwiftUI

struct ReaderView: View {
    @EnvironmentObject private var model: AppModel
    @EnvironmentObject private var player: ReaderPlayer

    /// Non-nil only while the progress slider is being dragged.
    @State private var scrubValue: Double?

    var body: some View {
        VStack(spacing: 0) {
            passage
            Divider()
            controls
                .padding(.horizontal)
                .padding(.top, 12)
                .padding(.bottom, 8)
        }
        .navigationTitle(player.book?.title ?? "")
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        #endif
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                settingsMenu
            }
        }
    }

    // MARK: - Text

    /// A few sentences either side of the one being read. Tapping any of them
    /// jumps there.
    private var visibleRange: Range<Int> {
        let count = player.sentences.count
        guard count > 0 else { return 0..<0 }
        let lower = max(0, player.currentIndex - 2)
        let upper = min(count, player.currentIndex + 7)
        return lower..<upper
    }

    private var passage: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 14) {
                    ForEach(Array(visibleRange), id: \.self) { index in
                        Text(player.sentences[index])
                            .font(index == player.currentIndex ? .title3.weight(.medium) : .body)
                            .foregroundStyle(index == player.currentIndex ? .primary : .secondary)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .contentShape(Rectangle())
                            .onTapGesture { player.seek(to: index) }
                            .id(index)
                    }
                }
                .padding()
            }
            .onChange(of: player.currentIndex) { _, newValue in
                withAnimation(.easeInOut(duration: 0.25)) {
                    proxy.scrollTo(newValue, anchor: .center)
                }
            }
        }
    }

    // MARK: - Controls

    private var controls: some View {
        VStack(spacing: 10) {
            progress
            statusLine
            HStack(spacing: 36) {
                Button {
                    player.skip(by: -1)
                } label: {
                    Image(systemName: "backward.end.fill")
                        .font(.title2)
                        .frame(width: 52, height: 52)
                }
                .accessibilityLabel("Previous sentence")

                Button {
                    Task { await model.togglePlayback() }
                } label: {
                    Image(systemName: player.state == .playing ? "pause.circle.fill" : "play.circle.fill")
                        .font(.system(size: 68))
                }
                .accessibilityLabel(player.state == .playing ? "Pause" : "Play")
                // Space bar on a Mac or an iPad keyboard.
                .keyboardShortcut(.space, modifiers: [])

                Button {
                    player.skip(by: 1)
                } label: {
                    Image(systemName: "forward.end.fill")
                        .font(.title2)
                        .frame(width: 52, height: 52)
                }
                .accessibilityLabel("Next sentence")
            }
        }
    }

    private var progress: some View {
        let last = Double(max(player.sentences.count - 1, 1))
        let shown = scrubValue ?? Double(player.currentIndex)
        return VStack(spacing: 2) {
            Slider(
                value: Binding(
                    get: { min(last, max(0, scrubValue ?? Double(player.currentIndex))) },
                    set: { scrubValue = $0 }
                ),
                in: 0...last,
                onEditingChanged: { editing in
                    if !editing, let target = scrubValue {
                        player.seek(to: Int(target.rounded()))
                        scrubValue = nil
                    }
                }
            )
            .accessibilityLabel("Position in book")
            HStack {
                Text("\(Int((shown / last * 100).rounded()))%")
                Spacer()
                Text("Sentence \(Int(shown.rounded()) + 1) of \(player.sentences.count)")
            }
            .font(.caption)
            .foregroundStyle(.secondary)
            .monospacedDigit()
        }
    }

    @ViewBuilder
    private var statusLine: some View {
        Group {
            if let error = player.lastError {
                Text(error).foregroundStyle(.red)
            } else {
                switch model.voiceState {
                case .loading(let progress, let message):
                    Text("\(message) \(Int((min(1, max(0, progress)) * 100).rounded()))%")
                case .failed:
                    Button("The voice couldn't load. Tap to try again.") {
                        Task { await model.prepareVoice() }
                    }
                case .notLoaded, .ready:
                    if player.isBuffering {
                        Text("Preparing the next sentence")
                    } else if player.state == .finished {
                        Text("End of book")
                    } else {
                        // Keeps the layout from jumping when a status appears.
                        Text(" ")
                    }
                }
            }
        }
        .font(.caption)
        .foregroundStyle(.secondary)
        .lineLimit(2)
        .multilineTextAlignment(.center)
    }

    // MARK: - Voice and speed

    private var settingsMenu: some View {
        Menu {
            Picker("Speed", selection: $model.rate) {
                ForEach(AppModel.rateChoices, id: \.self) { choice in
                    Text(Self.speedLabel(choice)).tag(choice)
                }
            }
            .pickerStyle(.menu)

            if !model.voices.isEmpty {
                Picker("Voice", selection: $model.selectedVoice) {
                    Section("Voices") {
                        ForEach(model.voices.filter { !$0.classic }, id: \.id) { voice in
                            Text(voice.label).tag(voice.id)
                        }
                    }
                    if model.hasClassicVoices {
                        Section("Classic voices") {
                            ForEach(model.voices.filter(\.classic), id: \.id) { voice in
                                Text(voice.label).tag(voice.id)
                            }
                        }
                    }
                }
                .pickerStyle(.menu)

                if let progress = model.classicDownloadProgress {
                    Text("Downloading classic voices, \(Int((progress * 100).rounded()))%")
                } else if !model.hasClassicVoices {
                    Button {
                        Task { await model.downloadClassicVoices() }
                    } label: {
                        Label(
                            model.classicDownloadError == nil
                                ? "Get classic voices (350 MB)" : "Retry classic voices download",
                            systemImage: "arrow.down.circle")
                    }
                }
            }
        } label: {
            Label("Voice and speed", systemImage: "slider.horizontal.3")
        }
    }

    private static func speedLabel(_ value: Double) -> String {
        let text = value == value.rounded() ? String(format: "%.0f", value) : String(format: "%g", value)
        return "\(text)x"
    }
}
