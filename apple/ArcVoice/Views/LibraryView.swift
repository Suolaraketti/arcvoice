import SwiftUI
import UniformTypeIdentifiers

struct LibraryView: View {
    @EnvironmentObject private var model: AppModel
    @EnvironmentObject private var library: LibraryStore
    @EnvironmentObject private var player: ReaderPlayer

    @State private var showingImporter = false

    var body: some View {
        NavigationStack {
            content
                .navigationTitle("Library")
                .toolbar {
                    ToolbarItem(placement: .primaryAction) {
                        Button {
                            showingImporter = true
                        } label: {
                            Label("Add PDF", systemImage: "plus")
                        }
                    }
                }
                .navigationDestination(item: $model.openBook) { _ in
                    ReaderView()
                }
                .safeAreaInset(edge: .bottom) {
                    VStack(spacing: 8) {
                        VoiceStatusBanner()
                        NowReadingBar()
                    }
                    .padding(.horizontal)
                    .padding(.bottom, 8)
                }
        }
        .fileImporter(
            isPresented: $showingImporter,
            allowedContentTypes: [.pdf],
            allowsMultipleSelection: false
        ) { result in
            switch result {
            case .success(let urls):
                guard let url = urls.first else { return }
                Task { await model.importPDF(from: url) }
            case .failure(let error):
                model.importError = error.localizedDescription
            }
        }
        // Dragging a PDF onto the window adds it. This is the natural way on a Mac.
        .dropDestination(for: URL.self) { urls, _ in
            let pdfs = urls.filter { $0.pathExtension.lowercased() == "pdf" }
            guard !pdfs.isEmpty else { return false }
            Task {
                for url in pdfs { await model.importPDF(from: url) }
            }
            return true
        }
        #if os(macOS)
        .frame(minWidth: 460, minHeight: 560)
        #endif
        .overlay {
            if model.isImporting {
                ImportingOverlay()
            }
        }
        .alert(
            "Couldn't add that book",
            isPresented: Binding(
                get: { model.importError != nil },
                set: { if !$0 { model.importError = nil } }
            )
        ) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(model.importError ?? "")
        }
    }

    @ViewBuilder
    private var content: some View {
        if library.books.isEmpty {
            ContentUnavailableView {
                Label("No books yet", systemImage: "books.vertical")
            } description: {
                Text("Add a PDF from Files or iCloud Drive, or share one to ArcVoice from another app.")
            } actions: {
                Button("Add a PDF") { showingImporter = true }
                    .buttonStyle(.borderedProminent)
            }
        } else {
            List {
                ForEach(library.sortedBooks) { book in
                    Button {
                        model.open(book)
                    } label: {
                        BookRow(book: book, isCurrent: player.book?.id == book.id)
                    }
                    .buttonStyle(.plain)
                    .swipeActions {
                        Button(role: .destructive) {
                            model.delete(book)
                        } label: {
                            Label("Delete", systemImage: "trash")
                        }
                    }
                    // Right click on a Mac, long press on an iPhone.
                    .contextMenu {
                        Button(role: .destructive) {
                            model.delete(book)
                        } label: {
                            Label("Delete", systemImage: "trash")
                        }
                    }
                }
            }
            .listStyle(.plain)
        }
    }
}

private struct BookRow: View {
    let book: Book
    let isCurrent: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(book.title)
                    .font(.headline)
                    .lineLimit(2)
                Spacer()
                if isCurrent {
                    Image(systemName: "speaker.wave.2.fill")
                        .foregroundStyle(.tint)
                        .accessibilityLabel("Current book")
                }
            }
            ProgressView(value: book.progress)
            Text("\(Int((book.progress * 100).rounded()))% read, \(book.pageCount) pages")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .padding(.vertical, 6)
        .contentShape(Rectangle())
    }
}

/// Shows the one-time voice download, and any failure with a retry.
private struct VoiceStatusBanner: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        switch model.voiceState {
        case .loading(let progress, let message):
            VStack(alignment: .leading, spacing: 6) {
                Text(message)
                    .font(.footnote.weight(.medium))
                ProgressView(value: min(1, max(0, progress)))
                Text("The voice downloads once, about 170 MB. After that it works offline.")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            .padding(12)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14))
        case .failed(let reason):
            VStack(alignment: .leading, spacing: 6) {
                Text("The voice couldn't load")
                    .font(.footnote.weight(.medium))
                Text(reason)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(3)
                Button("Try again") {
                    Task { await model.prepareVoice() }
                }
                .font(.footnote)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(12)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14))
        case .notLoaded, .ready:
            EmptyView()
        }
    }
}

/// A small player strip so the book can be paused or reopened from the library.
private struct NowReadingBar: View {
    @EnvironmentObject private var model: AppModel
    @EnvironmentObject private var player: ReaderPlayer

    var body: some View {
        if let book = player.book {
            HStack(spacing: 12) {
                Button {
                    model.open(book)
                } label: {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(book.title)
                            .font(.subheadline.weight(.semibold))
                            .lineLimit(1)
                        Text(player.state == .playing ? "Reading now" : "Paused")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)

                Button {
                    Task { await model.togglePlayback() }
                } label: {
                    Image(systemName: player.state == .playing ? "pause.fill" : "play.fill")
                        .font(.title3)
                        .frame(width: 44, height: 44)
                }
                .accessibilityLabel(player.state == .playing ? "Pause" : "Play")
            }
            .padding(.leading, 14)
            .padding(.trailing, 6)
            .padding(.vertical, 6)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14))
        }
    }
}

private struct ImportingOverlay: View {
    var body: some View {
        ZStack {
            Color.black.opacity(0.25).ignoresSafeArea()
            VStack(spacing: 12) {
                ProgressView()
                Text("Reading the PDF")
                    .font(.subheadline)
            }
            .padding(24)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 16))
        }
    }
}
