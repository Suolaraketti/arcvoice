import Foundation
#if canImport(UIKit)
import UIKit
#endif

/// The books on this phone. Everything lives under Application Support:
///
///     Library/
///       library.json            list of books
///       <book id>/
///         book.pdf or book.epub the original file
///         sentences.json        extracted text, one string per sentence
///         position.json         where she stopped
@MainActor
final class LibraryStore: ObservableObject {

    @Published private(set) var books: [Book] = []

    private let root: URL
    private let encoder: JSONEncoder
    private let decoder: JSONDecoder

    init() {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        root = support.appendingPathComponent("Library", isDirectory: true)

        encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601

        try? FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        loadIndex()
    }

    // MARK: - Paths

    private var indexURL: URL { root.appendingPathComponent("library.json") }

    private func folder(for id: UUID) -> URL {
        root.appendingPathComponent(id.uuidString, isDirectory: true)
    }

    private func sentencesURL(for id: UUID) -> URL {
        folder(for: id).appendingPathComponent("sentences.json")
    }

    private func paragraphsURL(for id: UUID) -> URL {
        folder(for: id).appendingPathComponent("paragraphs.json")
    }

    private func positionURL(for id: UUID) -> URL {
        folder(for: id).appendingPathComponent("position.json")
    }

    // MARK: - Index

    private func loadIndex() {
        guard let data = try? Data(contentsOf: indexURL),
              let decoded = try? decoder.decode([Book].self, from: data) else {
            books = []
            return
        }
        books = decoded
    }

    private func saveIndex() {
        guard let data = try? encoder.encode(books) else { return }
        try? data.write(to: indexURL, options: .atomic)
    }

    /// The book she touched most recently, which is what "resume" means.
    var mostRecent: Book? {
        books.max { ($0.lastOpenedAt ?? $0.addedAt) < ($1.lastOpenedAt ?? $1.addedAt) }
    }

    var sortedBooks: [Book] {
        books.sorted { ($0.lastOpenedAt ?? $0.addedAt) > ($1.lastOpenedAt ?? $1.addedAt) }
    }

    // MARK: - Import

    /// Copies a PDF or EPUB into the library and extracts its text.
    /// Importing a file that is already in the library returns the existing book.
    func importBook(from source: URL) async throws -> Book {
        let id = UUID()
        let bookFolder = folder(for: id)

        // Files picked from iCloud Drive or another app are security scoped.
        // The copy has to happen while access is held.
        let scoped = source.startAccessingSecurityScopedResource()
        defer { if scoped { source.stopAccessingSecurityScopedResource() } }

        // Apple Books on a Mac keeps EPUBs as unpacked folders.
        if (try? source.resourceValues(forKeys: [.isDirectoryKey]))?.isDirectory == true {
            throw EPUBImportError.unpacked
        }

        // The contents decide the format, not the name: a zip is an EPUB.
        let isEPUB = EPUBTextExtractor.looksLikeEPUB(source)
        let bookURL = bookFolder.appendingPathComponent(isEPUB ? "book.epub" : "book.pdf")

        do {
            try FileManager.default.createDirectory(at: bookFolder, withIntermediateDirectories: true)
            try FileManager.default.copyItem(at: source, to: bookURL)
        } catch {
            try? FileManager.default.removeItem(at: bookFolder)
            throw isEPUB ? EPUBImportError.unreadable as Error : PDFImportError.unreadable
        }

        // A file handed over with "Open in" is a copy in our Inbox. Clear it out.
        if source.path.contains("/Inbox/") {
            try? FileManager.default.removeItem(at: source)
        }

        let extracted: ExtractedBook
        do {
            extracted = try await Task.detached(priority: .userInitiated) {
                isEPUB
                    ? try EPUBTextExtractor.extract(from: bookURL)
                    : try PDFTextExtractor.extract(from: bookURL)
            }.value
        } catch {
            try? FileManager.default.removeItem(at: bookFolder)
            throw error
        }

        if let existing = books.first(where: { $0.bookKey == extracted.bookKey }) {
            try? FileManager.default.removeItem(at: bookFolder)
            return existing
        }

        do {
            let data = try JSONEncoder().encode(extracted.sentences)
            try data.write(to: sentencesURL(for: id), options: .atomic)
            let paragraphs = try JSONEncoder().encode(extracted.paragraphEnds)
            try paragraphs.write(to: paragraphsURL(for: id), options: .atomic)
        } catch {
            try? FileManager.default.removeItem(at: bookFolder)
            throw error
        }

        let fallbackTitle = source.deletingPathExtension().lastPathComponent
            .replacingOccurrences(of: "_", with: " ")
        let book = Book(
            id: id,
            title: extracted.title ?? fallbackTitle,
            bookKey: extracted.bookKey,
            pageCount: extracted.pageCount,
            sentenceCount: extracted.sentences.count,
            addedAt: Date(),
            lastOpenedAt: nil,
            sentenceIndex: 0
        )
        books.append(book)
        saveIndex()
        return book
    }

    // MARK: - Reading

    func sentences(for book: Book) throws -> [String] {
        let data = try Data(contentsOf: sentencesURL(for: book.id))
        return try JSONDecoder().decode([String].self, from: data)
    }

    /// Indices of the sentences that end a paragraph. Books added before
    /// paragraphs were tracked have none, and are paced sentence by sentence.
    func paragraphEnds(for book: Book) -> Set<Int> {
        guard let data = try? Data(contentsOf: paragraphsURL(for: book.id)),
              let ends = try? JSONDecoder().decode([Int].self, from: data)
        else { return [] }
        return Set(ends)
    }

    func position(for book: Book) -> ReadingPosition? {
        guard let data = try? Data(contentsOf: positionURL(for: book.id)) else { return nil }
        return try? decoder.decode(ReadingPosition.self, from: data)
    }

    func markOpened(_ book: Book) {
        guard let index = books.firstIndex(where: { $0.id == book.id }) else { return }
        books[index].lastOpenedAt = Date()
        saveIndex()
    }

    /// Called every time the reader moves to a new sentence. The position file
    /// is tiny, so writing it each time means a crash or a dead battery never
    /// costs more than one sentence.
    func savePosition(bookID: UUID, sentenceIndex: Int, anchor: String) {
        guard let index = books.firstIndex(where: { $0.id == bookID }) else { return }
        let book = books[index]
        let count = max(book.sentenceCount, 1)
        let position = ReadingPosition(
            bookKey: book.bookKey,
            sentenceIndex: sentenceIndex,
            sentenceCount: book.sentenceCount,
            fraction: count > 1 ? Double(sentenceIndex) / Double(count - 1) : 0,
            anchor: String(anchor.prefix(200)),
            updatedAt: Date(),
            device: Self.deviceName
        )
        if let data = try? encoder.encode(position) {
            try? data.write(to: positionURL(for: bookID), options: .atomic)
        }
        if books[index].sentenceIndex != sentenceIndex {
            books[index].sentenceIndex = sentenceIndex
            saveIndex()
        }
    }

    private static var deviceName: String {
        #if canImport(UIKit)
        return UIDevice.current.name
        #else
        return Host.current().localizedName ?? "Mac"
        #endif
    }

    func delete(_ book: Book) {
        try? FileManager.default.removeItem(at: folder(for: book.id))
        books.removeAll { $0.id == book.id }
        saveIndex()
    }
}
