import CoreGraphics
import Foundation
import ImageIO
import PDFKit
import Vision

/// Getting any book file into the library: copying it in (downloading it
/// from iCloud first if needed), telling what it really is from its
/// contents rather than its name, and reading its text.
///
/// Read: EPUB, PDF (scanned PDFs through on-device text recognition), Word
/// (.docx), OpenDocument (.odt), DRM-free Kindle (.mobi, .azw, .azw3, .prc),
/// FictionBook (.fb2), web pages (.html, Safari .webarchive), RTF, Markdown,
/// plain text, and photos or scans of pages. Files that can't be read (an
/// Adobe .acsm ticket, a DRM-locked Kindle book, an old .doc) get a message
/// that says what to do instead.
enum BookFile {

    enum Format: Equatable, Sendable {
        case pdf, epub, docx, odt, fb2, zippedFB2, mobi, rtf, html, webArchive, text, markdown, image

        /// The name the original is kept under in the book's folder.
        var fileExtension: String {
            switch self {
            case .pdf: return "pdf"
            case .epub: return "epub"
            case .docx: return "docx"
            case .odt: return "odt"
            case .fb2: return "fb2"
            case .zippedFB2: return "fb2.zip"
            case .mobi: return "mobi"
            case .rtf: return "rtf"
            case .html: return "html"
            case .webArchive: return "webarchive"
            case .text: return "txt"
            case .markdown: return "md"
            case .image: return "image"
            }
        }
    }

    enum Problem: LocalizedError {
        case unreadable
        case empty
        case adobeTicket
        case kindleLocked
        case kindleUnsupported
        case oldWordDocument
        case pages
        case unknown
        case noText

        var errorDescription: String? {
            switch self {
            case .unreadable:
                return "That file could not be read. If it is in iCloud Drive, check the phone is online and try again."
            case .empty:
                return "That file is empty, so the download probably did not finish. Download the book again."
            case .adobeTicket:
                return "That is an Adobe Digital Editions ticket (.acsm), not the book. It only unlocks a DRM-protected copy, which can't be read aloud. Look for a DRM-free EPUB or PDF download instead."
            case .kindleLocked:
                return "That Kindle book is DRM locked, so its text can't be read. BookFunnel and most ARC sites also offer an EPUB download; choose that one."
            case .kindleUnsupported:
                return "That Kindle file uses a format this version can't read. Download the EPUB or PDF version instead."
            case .oldWordDocument:
                return "That is an older Word document (.doc). Save it as .docx or PDF and add that instead."
            case .pages:
                return "That is a Pages document. In Pages, export it as EPUB or PDF and add that instead."
            case .unknown:
                return "ArcVoice can't read that kind of file. It reads EPUB, PDF, Word, Kindle (MOBI and AZW3), web pages, text and photos of pages."
            case .noText:
                return "No readable text was found in that file."
            }
        }
    }

    // MARK: - Copying in

    /// Copies `source` to `destination`, downloading it first if it is an
    /// iCloud placeholder. Blocks, so call it off the main thread.
    static func copy(_ source: URL, to destination: URL) throws {
        var coordinationError: NSError?
        var copyError: Error?
        NSFileCoordinator().coordinate(readingItemAt: source, options: [.withoutChanges], error: &coordinationError) { url in
            do {
                try? FileManager.default.removeItem(at: destination)
                try FileManager.default.copyItem(at: url, to: destination)
            } catch {
                copyError = error
            }
        }
        if coordinationError != nil || copyError != nil { throw Problem.unreadable }
    }

    // MARK: - What the file is

    /// What the file is, from its contents. The extension only breaks ties
    /// (Markdown is plain text by its bytes).
    static func format(of url: URL, extensionHint: String) throws -> Format {
        guard let handle = try? FileHandle(forReadingFrom: url) else { throw Problem.unreadable }
        let head = (try? handle.read(upToCount: 4096)) ?? Data()
        try? handle.close()
        if head.starts(with: [0x50, 0x4B, 0x03, 0x04]) {
            return try zipFormat(of: url)
        }
        return try format(ofHead: head, extensionHint: extensionHint)
    }

    /// The format of everything that is not a zip, from its first bytes.
    static func format(ofHead head: Data, extensionHint: String) throws -> Format {
        if head.isEmpty { throw Problem.empty }
        let bytes = [UInt8](head.prefix(4096))
        func at(_ offset: Int, _ ascii: String) -> Bool {
            let wanted = Array(ascii.utf8)
            return bytes.count >= offset + wanted.count && Array(bytes[offset..<(offset + wanted.count)]) == wanted
        }
        // A PDF may have a few stray bytes before its header.
        if head.prefix(1024).range(of: Data("%PDF-".utf8)) != nil { return .pdf }
        if at(60, "BOOKMOBI") || at(60, "TEXtREAd") { return .mobi }
        if at(0, "TPZ") { throw Problem.kindleUnsupported }
        if bytes.starts(with: [0xD0, 0xCF, 0x11, 0xE0]) { throw Problem.oldWordDocument }
        if at(0, "{\\rtf") { return .rtf }
        if bytes.starts(with: [0xFF, 0xD8, 0xFF]) || bytes.starts(with: [0x89, 0x50, 0x4E, 0x47])
            || at(0, "GIF8") || bytes.starts(with: [0x49, 0x49, 0x2A, 0x00]) || bytes.starts(with: [0x4D, 0x4D, 0x00, 0x2A])
            || (at(4, "ftyp") && ["heic", "heix", "mif1", "msf1", "avif"].contains { at(8, $0) }) {
            return .image
        }
        if at(0, "bplist00") { return .webArchive }

        let text = decodeText(head).lowercased()
        if text.contains("fulfillmenttoken") || text.contains("ns.adobe.com/adept") { throw Problem.adobeTicket }
        if text.contains("<fictionbook") { return .fb2 }
        if text.contains("<html") || text.contains("<!doctype html") || text.contains("<body") { return .html }
        let isUTF16 = bytes.starts(with: [0xFF, 0xFE]) || bytes.starts(with: [0xFE, 0xFF])
        if isUTF16 || !bytes.contains(0) {
            return ["md", "markdown", "mdown", "mkd"].contains(extensionHint.lowercased()) ? .markdown : .text
        }
        throw Problem.unknown
    }

    private static func zipFormat(of url: URL) throws -> Format {
        guard let data = try? Data(contentsOf: url, options: .mappedIfSafe), let archive = ZipReader(data: data) else {
            // Damaged: let the EPUB reader say so.
            return .epub
        }
        if archive.contains("word/document.xml") { return .docx }
        if archive.contains("Index/Document.iwa") { throw Problem.pages }
        if archive.contains("content.xml") {
            let kind = archive.text("mimetype") ?? ""
            if kind.contains("opendocument.text") { return .odt }
            if kind.contains("opendocument") { throw Problem.unknown }
        }
        if archive.names.contains(where: { $0.lowercased().hasSuffix(".fb2") }) { return .zippedFB2 }
        return .epub
    }

    // MARK: - Reading

    /// Reads a book's text. Blocks, so call it off the main thread.
    static func extract(_ format: Format, from url: URL) throws -> ExtractedBook {
        switch format {
        case .epub:
            return try EPUBTextExtractor.extract(from: url)
        case .pdf:
            do {
                return try PDFTextExtractor.extract(from: url)
            } catch PDFImportError.noText {
                // Scanned pages: read them with text recognition.
                return try scannedPDF(url)
            }
        case .image:
            return try images(url)
        case .html:
            let markup = decodeText(try data(url))
            return try book(chapters: [EPUBTextExtractor.paragraphs(in: markup)], title: htmlTitle(markup), url: url)
        case .webArchive:
            let markup = try webArchiveMarkup(try data(url))
            return try book(chapters: [EPUBTextExtractor.paragraphs(in: markup)], title: htmlTitle(markup), url: url)
        case .text:
            return try book(chapters: [plainParagraphs(withoutGutenbergNotice(decodeText(try data(url))))], title: nil, url: url)
        case .markdown:
            return try book(chapters: [markdownParagraphs(decodeText(try data(url)))], title: nil, url: url)
        case .rtf:
            return try book(chapters: [plainParagraphs(try rtfText(url), linesAreParagraphs: true)], title: nil, url: url)
        case .docx:
            return try wordDocument(url)
        case .odt:
            return try openDocument(url)
        case .fb2:
            return try fictionBook(decodeText(try data(url)), url: url)
        case .zippedFB2:
            guard let archive = ZipReader(data: try data(url)),
                  let name = archive.names.first(where: { $0.lowercased().hasSuffix(".fb2") }),
                  let bytes = archive.data(name)
            else { throw Problem.unreadable }
            return try fictionBook(decodeText(Data(bytes)), url: url)
        case .mobi:
            let (markup, title) = try mobiMarkup(try data(url))
            return try book(chapters: [EPUBTextExtractor.paragraphs(in: markup)], title: title, url: url)
        }
    }

    private static func data(_ url: URL) throws -> Data {
        do {
            return try Data(contentsOf: url, options: .mappedIfSafe)
        } catch {
            throw Problem.unreadable
        }
    }

    /// The common last step: paragraphs to sentences, with the same cleanup
    /// an EPUB gets.
    static func book(
        chapters: [[TextCleaner.Paragraph]], title: String?, url: URL, minimumSentences: Int = 3
    ) throws -> ExtractedBook {
        let prose = TextCleaner.prose(fromChapters: chapters)
        return try book(prose: prose, pageCount: nil, title: title, url: url, minimumSentences: minimumSentences)
    }

    private static func book(
        prose: TextCleaner.Prose, pageCount: Int?, title: String?, url: URL, minimumSentences: Int = 3
    ) throws -> ExtractedBook {
        let sentences = prose.sentences
        let characters = sentences.reduce(0) { $0 + $1.count }
        guard sentences.count >= minimumSentences, characters >= minimumSentences * 20 else { throw Problem.noText }
        let words = sentences.reduce(0) { $0 + $1.split(separator: " ").count }
        let cleanTitle = title?.trimmingCharacters(in: .whitespacesAndNewlines)
        return ExtractedBook(
            title: (cleanTitle?.count ?? 0) >= 2 ? cleanTitle : nil,
            // Without pages, an estimate at 275 words a page.
            pageCount: pageCount ?? max(1, (words + 274) / 275),
            sentences: sentences,
            paragraphEnds: prose.paragraphEnds,
            bookKey: try PDFTextExtractor.fileKey(for: url)
        )
    }

    // MARK: - Text encodings

    /// Text in whatever encoding it came in: UTF-8 or UTF-16 (with a byte
    /// order mark), else Windows-1252, which never fails.
    static func decodeText(_ data: Data) -> String {
        let bytes = [UInt8](data.prefix(4))
        if bytes.starts(with: [0xFF, 0xFE]) || bytes.starts(with: [0xFE, 0xFF]),
           let text = String(data: data, encoding: .utf16) {
            return text
        }
        if let text = String(data: data, encoding: .utf8) {
            return text.hasPrefix("\u{FEFF}") ? String(text.dropFirst()) : text
        }
        // A cut-off multi-byte character at the end of a sample is still UTF-8.
        if data.count > 8, let text = String(data: data.dropLast(3), encoding: .utf8) {
            return text
        }
        return String(data: data, encoding: .windowsCP1252) ?? String(decoding: data, as: UTF8.self)
    }

    // MARK: - Plain text and Markdown

    /// Paragraphs of plain text. Most text separates paragraphs with a blank
    /// line and may wrap lines inside them (Project Gutenberg does); text
    /// with almost no blank lines has one paragraph per line.
    static func plainParagraphs(_ text: String, linesAreParagraphs: Bool = false) -> [TextCleaner.Paragraph] {
        let lines = text.replacingOccurrences(of: "\r\n", with: "\n").replacingOccurrences(of: "\r", with: "\n")
            .components(separatedBy: "\n")
        let blank = lines.filter { $0.trimmingCharacters(in: .whitespaces).isEmpty }.count
        let filled = lines.count - blank
        let byLine = linesAreParagraphs || blank * 20 < filled
        var out: [TextCleaner.Paragraph] = []
        var current: [String] = []
        func flush() {
            let joined = TextCleaner.collapseSpaces(current.joined(separator: " "))
            if !joined.isEmpty { out.append(TextCleaner.Paragraph(text: joined, isHeading: false)) }
            current = []
        }
        for line in lines {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.isEmpty {
                flush()
            } else {
                current.append(trimmed)
                if byLine { flush() }
            }
        }
        flush()
        return out
    }

    /// Project Gutenberg texts open and close with a licence notice.
    static func withoutGutenbergNotice(_ text: String) -> String {
        var body = Substring(text)
        if let start = body.range(of: "*** START OF"),
           let lineEnd = body[start.upperBound...].firstIndex(of: "\n") {
            body = body[body.index(after: lineEnd)...]
        }
        if let end = body.range(of: "*** END OF") {
            body = body[..<end.lowerBound]
        }
        return String(body)
    }

    static func markdownParagraphs(_ text: String) -> [TextCleaner.Paragraph] {
        func replace(_ pattern: String, _ template: String, in string: String) -> String {
            string.replacingOccurrences(of: pattern, with: template, options: .regularExpression)
        }
        var out: [TextCleaner.Paragraph] = []
        var current: [String] = []
        var inCode = false
        func flush() {
            var joined = current.joined(separator: " ")
            joined = replace("!\\[[^\\]]*\\]\\([^)]*\\)", "", in: joined)
            joined = replace("\\[([^\\]]*)\\]\\([^)]*\\)", "$1", in: joined)
            joined = replace("[*_`]{1,3}", "", in: joined)
            joined = TextCleaner.collapseSpaces(joined)
            if !joined.isEmpty { out.append(TextCleaner.Paragraph(text: joined, isHeading: false)) }
            current = []
        }
        for raw in text.replacingOccurrences(of: "\r\n", with: "\n").components(separatedBy: "\n") {
            let line = raw.trimmingCharacters(in: .whitespaces)
            if line.hasPrefix("```") || line.hasPrefix("~~~") {
                flush()
                inCode.toggle()
                continue
            }
            if inCode { continue }
            if line.isEmpty || line.range(of: "^([-*_]\\s*){3,}$", options: .regularExpression) != nil {
                flush()
            } else if line.hasPrefix("#") {
                flush()
                let heading = TextCleaner.collapseSpaces(line.drop { $0 == "#" }.trimmingCharacters(in: .whitespaces))
                if !heading.isEmpty { out.append(TextCleaner.Paragraph(text: heading, isHeading: true)) }
            } else {
                current.append(replace("^(>\\s*)+|^([-*+]|\\d+[.)])\\s+", "", in: line))
            }
        }
        flush()
        return out
    }

    // MARK: - RTF and web pages

    private static func rtfText(_ url: URL) throws -> String {
        let options: [NSAttributedString.DocumentReadingOptionKey: Any] = [.documentType: NSAttributedString.DocumentType.rtf]
        guard let text = try? NSAttributedString(url: url, options: options, documentAttributes: nil) else {
            throw Problem.unreadable
        }
        return text.string
    }

    static func htmlTitle(_ markup: String) -> String? {
        guard let open = markup.range(of: "<title[^>]*>", options: [.regularExpression, .caseInsensitive]),
              let close = markup.range(of: "</title>", options: .caseInsensitive, range: open.upperBound..<markup.endIndex)
        else { return nil }
        return TextCleaner.collapseSpaces(EPUBTextExtractor.decodeEntities(String(markup[open.upperBound..<close.lowerBound])))
    }

    /// The page inside a Safari web archive.
    private static func webArchiveMarkup(_ data: Data) throws -> String {
        guard let archive = try? PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any],
              let main = archive["WebMainResource"] as? [String: Any],
              let page = main["WebResourceData"] as? Data
        else { throw Problem.unknown }
        return decodeText(page)
    }

    // MARK: - Word and OpenDocument

    /// A .docx: `word/document.xml`, where `<w:p>` is a paragraph, `<w:t>`
    /// holds its text, and a "Heading" or "Title" style marks a heading.
    private static func wordDocument(_ url: URL) throws -> ExtractedBook {
        guard let archive = ZipReader(data: try data(url)), let xml = archive.text("word/document.xml") else {
            throw Problem.unreadable
        }
        var out: [TextCleaner.Paragraph] = []
        var current = ""
        var isHeading = false
        var inText = false
        for piece in EPUBTextExtractor.pieces(in: xml) {
            switch piece {
            case .tag(let tag):
                switch tag.name {
                case "p":
                    if tag.closing || tag.selfClosing {
                        let text = TextCleaner.collapseSpaces(current)
                        if !text.isEmpty { out.append(TextCleaner.Paragraph(text: text, isHeading: isHeading)) }
                    }
                    current = ""
                    if !tag.closing { isHeading = false }
                case "t":
                    inText = !tag.closing && !tag.selfClosing
                case "tab", "br", "cr":
                    if !tag.closing { current += " " }
                case "pstyle":
                    let style = (tag.attribute("w:val") ?? "").lowercased()
                    if style.hasPrefix("heading") || style == "title" { isHeading = true }
                default:
                    break
                }
            case .text(let text):
                if inText { current += EPUBTextExtractor.decodeEntities(text) }
            }
        }
        let title = archive.text("docProps/core.xml").flatMap { element("title", in: $0) }
        return try book(chapters: [out], title: title, url: url)
    }

    /// An .odt: `content.xml`, where `<text:p>` is a paragraph and
    /// `<text:h>` a heading. Footnotes and comments are left out.
    private static func openDocument(_ url: URL) throws -> ExtractedBook {
        guard let archive = ZipReader(data: try data(url)), let xml = archive.text("content.xml") else {
            throw Problem.unreadable
        }
        var out: [TextCleaner.Paragraph] = []
        var current = ""
        var isHeading = false
        var depth = 0
        var skipping = 0
        for piece in EPUBTextExtractor.pieces(in: xml) {
            switch piece {
            case .tag(let tag):
                if ["note", "annotation"].contains(tag.name), !tag.selfClosing {
                    skipping += tag.closing ? -1 : 1
                    continue
                }
                guard skipping == 0 else { continue }
                switch tag.name {
                case "p", "h":
                    if tag.selfClosing { continue }
                    if tag.closing {
                        depth = max(0, depth - 1)
                        if depth == 0 {
                            let text = TextCleaner.collapseSpaces(current)
                            if !text.isEmpty { out.append(TextCleaner.Paragraph(text: text, isHeading: isHeading)) }
                            current = ""
                        }
                    } else {
                        if depth == 0 { isHeading = tag.name == "h" }
                        depth += 1
                    }
                case "s", "tab", "line-break":
                    if !tag.closing { current += " " }
                default:
                    break
                }
            case .text(let text):
                if depth > 0, skipping == 0 { current += EPUBTextExtractor.decodeEntities(text) }
            }
        }
        let title = archive.text("meta.xml").flatMap { element("title", in: $0) }
        return try book(chapters: [out], title: title, url: url)
    }

    /// The text of the first element named `name` (any namespace prefix).
    static func element(_ name: String, in xml: String) -> String? {
        let pattern = "<(?:[\\w-]+:)?\(name)(?:\\s[^>]*)?>([^<]*)</(?:[\\w-]+:)?\(name)>"
        guard let regex = try? NSRegularExpression(pattern: pattern, options: .caseInsensitive),
              let match = regex.firstMatch(in: xml, range: NSRange(xml.startIndex..., in: xml)),
              let range = Range(match.range(at: 1), in: xml)
        else { return nil }
        let text = TextCleaner.collapseSpaces(EPUBTextExtractor.decodeEntities(String(xml[range])))
        return text.isEmpty ? nil : text
    }

    // MARK: - FictionBook

    /// An .fb2: XML with the story in its first `<body>`, `<title>` for
    /// chapter headings and `<v>` for lines of verse. Later bodies hold
    /// footnotes, and `<binary>` holds pictures.
    private static func fictionBook(_ xml: String, url: URL) throws -> ExtractedBook {
        var markup = xml
        if let start = markup.range(of: "<body", options: .caseInsensitive) {
            let end = markup.range(of: "</body>", options: .caseInsensitive, range: start.upperBound..<markup.endIndex)
            markup = String(markup[start.lowerBound..<(end?.upperBound ?? markup.endIndex)])
        }
        for (from, to) in [
            ("<title>", "<h2>"), ("</title>", "</h2>"), ("<subtitle>", "<h3>"), ("</subtitle>", "</h3>"),
            ("<v>", "<p>"), ("</v>", "</p>"),
        ] {
            markup = markup.replacingOccurrences(of: from, with: to, options: .caseInsensitive)
        }
        let title = element("book-title", in: xml)
        return try book(chapters: [EPUBTextExtractor.paragraphs(in: markup)], title: title, url: url)
    }

    // MARK: - Kindle

    /// The HTML inside a DRM-free MOBI, AZW or AZW3 file, and its title.
    ///
    /// These are Palm databases: record 0 holds the headers, the next records
    /// hold the book's HTML, usually compressed with PalmDOC's simple LZ77.
    /// Each text record may end in trailing entries the headers describe,
    /// which are not text and are cut off before decompressing.
    static func mobiMarkup(_ data: Data) throws -> (markup: String, title: String?) {
        let bytes = [UInt8](data)
        let count = bytes.count
        func u16(_ offset: Int) -> Int {
            offset + 2 <= count ? Int(bytes[offset]) << 8 | Int(bytes[offset + 1]) : 0
        }
        func u32(_ offset: Int) -> Int { u16(offset) << 16 | u16(offset + 2) }

        let recordCount = u16(76)
        guard recordCount > 1, 78 + recordCount * 8 <= count else { throw Problem.unreadable }
        let offsets = (0..<recordCount).map { u32(78 + $0 * 8) }
        func record(_ index: Int) -> ArraySlice<UInt8> {
            let start = offsets[index]
            let end = index + 1 < recordCount ? offsets[index + 1] : count
            guard start <= end, end <= count else { return [] }
            return bytes[start..<end]
        }

        let header = offsets[0]
        let compression = u16(header)
        let textRecords = u16(header + 8)
        if u16(header + 12) != 0 { throw Problem.kindleLocked }
        guard compression == 1 || compression == 2 else { throw Problem.kindleUnsupported }

        var extraFlags = 0
        var utf8 = false
        var title: String?
        if header + 20 <= count, Array(bytes[(header + 16)..<(header + 20)]) == Array("MOBI".utf8) {
            let headerLength = u32(header + 20)
            utf8 = u32(header + 28) == 65001
            if headerLength >= 0xE4 { extraFlags = u16(header + 0xF2) }
            let nameStart = header + u32(header + 0x54)
            let nameLength = u32(header + 0x58)
            if nameLength > 0, nameStart + nameLength <= count {
                let name = Data(bytes[nameStart..<(nameStart + nameLength)])
                title = utf8 ? String(decoding: name, as: UTF8.self) : String(data: name, encoding: .windowsCP1252)
            }
        }

        var text: [UInt8] = []
        for index in 1..<min(textRecords + 1, recordCount) {
            var chunk = Array(record(index))
            chunk.removeLast(min(chunk.count, trailingSize(chunk, flags: extraFlags)))
            text += compression == 2 ? palmDOCDecompress(chunk) : chunk
        }
        let markup = utf8
            ? String(decoding: text, as: UTF8.self)
            : (String(data: Data(text), encoding: .windowsCP1252) ?? String(decoding: text, as: UTF8.self))
        return (markup, title)
    }

    /// How many bytes at the end of a text record are trailing entries.
    static func trailingSize(_ record: [UInt8], flags: Int) -> Int {
        func entrySize(_ end: Int) -> Int {
            // A variable-width size, read backwards; its last byte has the top bit set.
            var result = 0
            var shift = 0
            var position = end
            while position > 0 {
                let byte = Int(record[position - 1])
                result |= (byte & 0x7F) << shift
                shift += 7
                position -= 1
                if byte & 0x80 != 0 || shift >= 28 { break }
            }
            return result
        }
        var size = 0
        var bits = flags >> 1
        while bits != 0 {
            if bits & 1 != 0 { size += entrySize(record.count - size) }
            bits >>= 1
        }
        if flags & 1 != 0, record.count > size {
            size += Int(record[record.count - size - 1] & 0x3) + 1
        }
        return size
    }

    /// PalmDOC's LZ77: literal bytes, runs of literals, back references, and
    /// a space folded into the next character.
    static func palmDOCDecompress(_ input: [UInt8]) -> [UInt8] {
        var out: [UInt8] = []
        out.reserveCapacity(input.count * 2)
        var index = 0
        while index < input.count {
            let byte = input[index]
            index += 1
            switch byte {
            case 0x01...0x08:
                let end = min(input.count, index + Int(byte))
                out += input[index..<end]
                index = end
            case 0x80...0xBF:
                guard index < input.count else { return out }
                let pair = Int(byte) << 8 | Int(input[index])
                index += 1
                let distance = (pair >> 3) & 0x7FF
                let length = (pair & 0x7) + 3
                guard distance > 0, distance <= out.count else { continue }
                for _ in 0..<length { out.append(out[out.count - distance]) }
            case 0xC0...0xFF:
                out.append(0x20)
                out.append(byte ^ 0x80)
            default:
                out.append(byte)
            }
        }
        return out
    }

    // MARK: - Text recognition

    /// A PDF of scanned pages, read page by page with on-device text
    /// recognition. A 300-page book takes a few minutes on an iPhone.
    private static func scannedPDF(_ url: URL) throws -> ExtractedBook {
        guard let document = PDFDocument(url: url) else { throw PDFImportError.unreadable }
        var pages: [String] = []
        for index in 0..<document.pageCount {
            autoreleasepool {
                if let page = document.page(at: index), let image = render(page) {
                    pages.append((try? recognize(image, orientation: .up)) ?? "")
                } else {
                    pages.append("")
                }
            }
        }
        let prose = TextCleaner.prose(fromPages: pages)
        guard !prose.sentences.isEmpty else { throw PDFImportError.noText }
        return try book(prose: prose, pageCount: document.pageCount, title: nil, url: url)
    }

    /// A photo or scan of pages: every frame of the image, read with text
    /// recognition.
    private static func images(_ url: URL) throws -> ExtractedBook {
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil) else { throw Problem.unreadable }
        var pages: [String] = []
        for index in 0..<max(1, CGImageSourceGetCount(source)) {
            try autoreleasepool {
                guard let image = CGImageSourceCreateImageAtIndex(source, index, nil) else { return }
                let properties = CGImageSourceCopyPropertiesAtIndex(source, index, nil) as? [CFString: Any]
                let raw = (properties?[kCGImagePropertyOrientation] as? UInt32) ?? 1
                pages.append(try recognize(image, orientation: CGImagePropertyOrientation(rawValue: raw) ?? .up))
            }
        }
        let prose = TextCleaner.prose(fromPages: pages)
        return try book(prose: prose, pageCount: pages.count, title: nil, url: url, minimumSentences: 1)
    }

    // MARK: - Drop caps

    /// Puts back each chapter's first letter where the PDF drew it as a
    /// picture.
    ///
    /// Such a chapter page starts with an object placeholder (U+FFFC), then
    /// short heading lines (number, title, narrator), then an opening that
    /// starts mid-word: "throw my practice sword" for "I throw my practice
    /// sword", "hea." for "Rhea.". Text recognition reads decorative capitals
    /// poorly, so the letter is chosen from the book's own words instead: the
    /// one that makes a word the book uses most ("Rhea", "My", "General"),
    /// or "I" or "A" as a word of its own when the book says "I throw".
    /// Nothing is guessed when no letter makes a word the book uses.
    static func restoringDropCaps(_ pages: [String]) -> [String] {
        let openings: [(page: Int, line: Int)] = pages.indices.compactMap { index in
            let lines = pages[index].components(separatedBy: "\n")
            guard lines.first?.trimmingCharacters(in: .whitespaces) == "\u{FFFC}",
                  // Skipping the rest of a heading broken with a hyphen ("double-", "checks").
                  let line = lines.prefix(8).indices.first(where: { index in
                      (lines[index].trimmingCharacters(in: .whitespaces).first?.isLowercase ?? false)
                          && !(index > 0 && lines[index - 1].trimmingCharacters(in: .whitespaces).hasSuffix("-"))
                  }),
                  lines[1..<line].allSatisfy({ heading in
                      let trimmed = heading.trimmingCharacters(in: .whitespaces)
                      return trimmed.count < 40 && !TextCleaner.endsLikeSentence(trimmed)
                  })
            else { return nil }
            return (index, line)
        }
        guard !openings.isEmpty else { return pages }

        let whole = pages.joined(separator: "\n")
        var counts: [String: Int] = [:]
        whole.enumerateSubstrings(in: whole.startIndex..., options: .byWords) { word, _, _, _ in
            if let word { counts[word, default: 0] += 1 }
        }
        func uses(_ phrase: String) -> Int {
            whole.components(separatedBy: phrase).count - 1
        }

        var restored = pages
        for opening in openings {
            var lines = restored[opening.page].components(separatedBy: "\n")
            let line = lines[opening.line].trimmingCharacters(in: .whitespaces)
            if let letter = dropCapLetter(for: line, counts: counts, uses: uses) {
                lines[opening.line] = letter
                restored[opening.page] = lines.joined(separator: "\n")
            }
        }
        return restored
    }

    /// The best start for an opening that lost its first letter: a capital
    /// joined to it, or "I " or "A " before it. Scored by how often the book
    /// uses the result, a word that starts a sentence counting most.
    static func dropCapLetter(for line: String, counts: [String: Int], uses: (String) -> Int) -> String? {
        let fragment = String(line.prefix { $0.isLetter || $0 == "'" || $0 == "\u{2019}" })
        guard !fragment.isEmpty else { return nil }
        var best: (text: String, score: Int)?
        func consider(_ text: String, _ score: Int) {
            if score > 0, score > (best?.score ?? 0) { best = (text, score) }
        }
        for letter in "ABCDEFGHIJKLMNOPQRSTUVWXYZ" {
            let word = String(letter) + fragment
            consider(String(letter) + line, (counts[word] ?? 0) * 3 + (counts[word.lowercased()] ?? 0))
        }
        // "I throw": a word of its own. Counted as a phrase, three words long
        // where the line allows, so "I throw" beats "Ithrow" but "It was"
        // beats "I t was".
        let words = line.split(separator: " ").prefix(2).joined(separator: " ")
        for pronoun in ["I", "A"] where fragment.count > 1 {
            consider(pronoun + " " + line, uses(pronoun + " " + words) * 3)
        }
        return best?.text
    }

    /// A PDF page drawn at about 150 dpi, enough for text recognition.
    private static func render(_ page: PDFPage) -> CGImage? {
        let box = page.bounds(for: .mediaBox)
        guard box.width > 0, box.height > 0 else { return nil }
        let scale = min(2, 2400 / max(box.width, box.height))
        let width = Int(box.width * scale)
        let height = Int(box.height * scale)
        guard let context = CGContext(
            data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
            space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)
        else { return nil }
        context.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
        context.fill(CGRect(x: 0, y: 0, width: width, height: height))
        context.scaleBy(x: scale, y: scale)
        context.translateBy(x: -box.minX, y: -box.minY)
        page.draw(with: .mediaBox, to: context)
        return context.makeImage()
    }

    /// The lines of text in an image, in reading order.
    private static func recognize(_ image: CGImage, orientation: CGImagePropertyOrientation) throws -> String {
        let request = VNRecognizeTextRequest()
        request.recognitionLevel = .accurate
        request.usesLanguageCorrection = true
        request.recognitionLanguages = ["en-US"]
        try VNImageRequestHandler(cgImage: image, orientation: orientation, options: [:]).perform([request])
        return (request.results ?? []).compactMap { $0.topCandidates(1).first?.string }.joined(separator: "\n")
    }
}
