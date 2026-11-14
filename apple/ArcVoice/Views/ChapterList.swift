import SwiftUI

/// Every chapter of the open book. The one being read is marked, and
/// tapping one jumps to its start.
struct ChapterList: View {
    @EnvironmentObject private var player: ReaderPlayer
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            ScrollViewReader { proxy in
                List {
                    ForEach(Array(player.chapters.enumerated()), id: \.offset) { index, chapter in
                        Button {
                            player.play(chapter: chapter)
                            dismiss()
                        } label: {
                            row(index: index, chapter: chapter)
                        }
                        .buttonStyle(.plain)
                        .id(index)
                    }
                }
                .listStyle(.plain)
                .onAppear {
                    if let current = player.currentChapterIndex {
                        proxy.scrollTo(current, anchor: .center)
                    }
                }
            }
            .navigationTitle("Chapters")
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        #if os(macOS)
        .frame(minWidth: 380, minHeight: 480)
        #endif
    }

    private func row(index: Int, chapter: Chapter) -> some View {
        let isCurrent = index == player.currentChapterIndex
        let end = index + 1 < player.chapters.count ? player.chapters[index + 1].start : player.sentences.count
        let read = isCurrent ? Double(player.currentIndex - chapter.start) / Double(max(1, end - chapter.start)) : nil
        return HStack(spacing: 12) {
            Text("\(index + 1)")
                .font(.footnote.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(minWidth: 24, alignment: .trailing)
            VStack(alignment: .leading, spacing: 3) {
                Text(chapter.title)
                    .font(isCurrent ? .body.weight(.semibold) : .body)
                    .foregroundStyle(index < (player.currentChapterIndex ?? 0) ? .secondary : .primary)
                    .lineLimit(2)
                if let read {
                    ProgressView(value: min(1, max(0, read)))
                        .tint(.accentColor)
                }
            }
            Spacer()
            if isCurrent {
                Image(systemName: "speaker.wave.2.fill")
                    .foregroundStyle(Color.accentColor)
                    .accessibilityLabel("Reading now")
            }
        }
        .padding(.vertical, 4)
        .contentShape(Rectangle())
    }
}
