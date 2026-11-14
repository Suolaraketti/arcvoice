import CryptoKit
import Foundation
import PDFKit

enum PDFImportError: LocalizedError {
    case unreadable
    case locked
    case noText

    var errorDescription: String? {
        switch self {
        case .unreadable:
            return "That file could not be opened as a PDF."
        case .locked:
            return "That PDF is password protected or DRM locked, so its text can't be read."
        case .noText:
            return "That PDF has no readable text, even after scanning its pages for words."
        }
    }
}

struct ExtractedBook: Sendable {
    var title: String?
    var pageCount: Int
    var sentences: [String]
    /// Indices of the sentences that end a paragraph.
    var paragraphEnds: [Int]
    var bookKey: String
    var chapters: [Chapter] = []
}

/// Reads a PDF with Apple's built-in PDFKit. Safe to call off the main thread;
/// the document is created and released inside the call.
enum PDFTextExtractor {

    static func extract(from url: URL) throws -> ExtractedBook {
        guard let document = PDFDocument(url: url) else {
            throw PDFImportError.unreadable
        }
        if document.isLocked, !document.unlock(withPassword: "") {
            throw PDFImportError.locked
        }

        var pages: [String] = []
        pages.reserveCapacity(document.pageCount)
        for index in 0..<document.pageCount {
            autoreleasepool {
                pages.append(document.page(at: index)?.string ?? "")
            }
        }

        // A chapter's decorative first letter is often a picture, not text.
        pages = BookFile.restoringDropCaps(pages)
        let prose = TextCleaner.prose(fromPages: pages)
        let sentences = prose.sentences
        let characters = sentences.reduce(0) { $0 + $1.count }
        guard sentences.count >= 3, characters >= 200 else {
            throw PDFImportError.noText
        }

        return ExtractedBook(
            title: cleanTitle(document.documentAttributes?[PDFDocumentAttribute.titleAttribute] as? String),
            pageCount: document.pageCount,
            sentences: sentences,
            paragraphEnds: prose.paragraphEnds,
            bookKey: try fileKey(for: url),
            chapters: Chapter.list(sentences: sentences, headings: prose.headings)
        )
    }

    /// SHA-256 of the file's bytes, as lowercase hex.
    static func fileKey(for url: URL) throws -> String {
        let data = try Data(contentsOf: url, options: .mappedIfSafe)
        return SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }

    /// PDF metadata titles are often junk left behind by the layout tool.
    private static func cleanTitle(_ raw: String?) -> String? {
        guard let raw else { return nil }
        let title = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard title.count >= 2 else { return nil }
        let lowered = title.lowercased()
        let junkMarkers = ["untitled", "microsoft word", ".indd", ".doc", ".pdf", ".qxd"]
        if junkMarkers.contains(where: { lowered.contains($0) }) { return nil }
        return title
    }
}
