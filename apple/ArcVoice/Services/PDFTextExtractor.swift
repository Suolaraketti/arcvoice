import CryptoKit
import Foundation
import PDFKit
#if canImport(UIKit)
import UIKit
typealias PlatformFont = UIFont
#else
import AppKit
typealias PlatformFont = NSFont
#endif

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
        var styledLines: [[StyledLine]] = []
        pages.reserveCapacity(document.pageCount)
        for index in 0..<document.pageCount {
            autoreleasepool {
                let page = document.page(at: index)
                let (text, lines) = page.map(styled) ?? ("", [])
                pages.append(text)
                styledLines.append(lines)
            }
        }

        // A chapter's decorative first letter is often a picture, or text
        // filed out of order. Put it back, and clear any big lone letters
        // left over so they are neither read out nor taken for headings.
        pages = BookFile.restoringDropCaps(pages)
        for index in pages.indices {
            let bigLetters = Set(styledLines[index].filter { BookFile.isDropCapLetter($0.text) }.map(\.text))
            guard !bigLetters.isEmpty else { continue }
            pages[index] = pages[index].components(separatedBy: "\n")
                .filter { !bigLetters.contains($0.trimmingCharacters(in: .whitespaces)) || isChapterNumeral($0) }
                .joined(separator: "\n")
            styledLines[index].removeAll { bigLetters.contains($0.text) && !isChapterNumeral($0.text) }
        }
        let prose = TextCleaner.prose(fromPages: pages, headingLines: headingLines(styledLines))
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
            chapters: chaptersFromBookmarks(document, prose: prose)
                ?? Chapter.list(sentences: sentences, headings: prose.headings, pages: prose.pages)
        )
    }

    // MARK: - Type sizes

    /// A line of a page with the size and weight of most of its type.
    struct StyledLine {
        var text: String
        var size: CGFloat
        var bold: Bool
    }

    /// A page's text, and each line with its type size.
    private static func styled(_ page: PDFPage) -> (String, [StyledLine]) {
        guard let attributed = page.attributedString else { return (page.string ?? "", []) }
        let text = attributed.string as NSString
        var lines: [StyledLine] = []
        var location = 0
        while location < text.length {
            let range = text.lineRange(for: NSRange(location: location, length: 0))
            var sizes: [CGFloat: Int] = [:]
            var boldCount = 0
            attributed.enumerateAttribute(.font, in: range) { value, run, _ in
                guard let font = value as? PlatformFont else { return }
                sizes[(font.pointSize * 2).rounded() / 2, default: 0] += run.length
                let name = font.fontName.lowercased()
                if ["bold", "black", "heavy", "semibold", "demi"].contains(where: name.contains) { boldCount += run.length }
            }
            let line = text.substring(with: range)
            if let size = sizes.max(by: { $0.value < $1.value })?.key,
               let normalized = TextCleaner.lines(in: line).first {
                lines.append(StyledLine(text: normalized, size: size, bold: boldCount * 2 > range.length))
            }
            location = NSMaxRange(range)
        }
        return (attributed.string, lines)
    }

    /// The lines on each page set in a heading's type: clearly larger than
    /// the body text (most of the book's characters), or bold where the
    /// body is not and short with no closing punctuation. Chapter titles are
    /// set this way in nearly every typeset book, which makes type size far
    /// more reliable than guessing from the words. Nil when the PDF shows no
    /// such lines (a plain or scanned file), so the words are used instead.
    static func headingLines(_ pages: [[StyledLine]]) -> [Set<String>]? {
        var weights: [CGFloat: Int] = [:]
        var boldWeight = 0
        var total = 0
        for line in pages.joined() {
            weights[line.size, default: 0] += line.text.count
            if line.bold { boldWeight += line.text.count }
            total += line.text.count
        }
        guard let body = weights.max(by: { $0.value < $1.value })?.key, body > 0 else { return nil }
        let bodyIsBold = boldWeight * 2 > total
        let sets = pages.map { lines in
            Set(lines.filter { line in
                guard TextCleaner.hasLettersOrDigits(line.text), line.text.count <= 80 else { return false }
                if line.size >= body * 1.2 { return true }
                return line.bold && !bodyIsBold && line.size >= body * 0.95 && line.text.count <= 60
                    && !TextCleaner.endsLikeSentence(line.text)
            }.map(\.text))
        }
        // A few large lines (a title page) are not a pattern of chapter titles.
        return sets.reduce(0) { $0 + $1.count } >= 3 ? sets : nil
    }

    /// "I" or "V" alone can number a chapter; other lone letters cannot.
    private static func isChapterNumeral(_ line: String) -> Bool {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        return trimmed == "I" || TextCleaner.isRomanNumeral(trimmed)
    }

    /// Chapters from the PDF's bookmarks (its outline), which publishing
    /// tools usually write from the book's real chapter list. Each bookmark
    /// starts at the first sentence on its page. Nil when there are none.
    static func chaptersFromBookmarks(_ document: PDFDocument, prose: TextCleaner.Prose) -> [Chapter]? {
        guard let root = document.outlineRoot, root.numberOfChildren > 0, !prose.pages.isEmpty else { return nil }
        var entries: [(title: String, start: Int)] = []
        func walk(_ outline: PDFOutline) {
            for index in 0..<outline.numberOfChildren {
                guard let item = outline.child(at: index) else { continue }
                if let label = item.label, let page = item.destination?.page {
                    let pageIndex = document.index(for: page)
                    if let start = prose.pages.firstIndex(where: { $0 >= pageIndex }) {
                        entries.append((label, start))
                    }
                }
                walk(item)
            }
        }
        walk(root)
        let chapters = Chapter.fromContents(entries, sentenceCount: prose.sentences.count)
        return chapters.count >= 2 ? chapters : nil
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
