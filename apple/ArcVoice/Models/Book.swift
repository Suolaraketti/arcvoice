import Foundation

/// One imported PDF or EPUB in the library.
struct Book: Identifiable, Codable, Hashable, Sendable {
    let id: UUID
    var title: String
    /// SHA-256 of the original file's bytes. The same file on another device
    /// produces the same key, which is what a future sync matches on.
    var bookKey: String
    var pageCount: Int
    var sentenceCount: Int
    var addedAt: Date
    var lastOpenedAt: Date?
    /// Mirror of the saved position, kept here so the library list can show
    /// progress without opening every position file.
    var sentenceIndex: Int

    var progress: Double {
        guard sentenceCount > 1 else { return 0 }
        return min(1, max(0, Double(sentenceIndex) / Double(sentenceCount - 1)))
    }
}

/// Where the reader is in a book. Written as `position.json` next to the book.
///
/// The format is deliberately device neutral so the Windows app can read and
/// write the same file later. `sentenceIndex` is exact when both devices split
/// the text the same way; `anchor` is the sentence text itself, so a device
/// whose extractor split things slightly differently can still find the spot
/// by searching for it near `fraction`.
struct ReadingPosition: Codable, Sendable {
    var version: Int = 1
    var bookKey: String
    var sentenceIndex: Int
    var sentenceCount: Int
    var fraction: Double
    var anchor: String
    var updatedAt: Date
    var device: String
}
