import Compression
import Foundation

enum EPUBImportError: LocalizedError {
    case unreadable
    case unpacked
    case notEPUB
    case locked
    case noText

    var errorDescription: String? {
        switch self {
        case .unreadable:
            return "That file could not be opened as an EPUB."
        case .unpacked:
            return "That EPUB has been unpacked into a folder. Add the single .epub file instead."
        case .notEPUB:
            return "That file is not an EPUB, or it is damaged."
        case .locked:
            return "That EPUB is DRM locked, so its text can't be read. Download the DRM-free EPUB or PDF instead if you have one."
        case .noText:
            return "That EPUB has no readable text. It may be all images, or locked in a way this version can't read."
        }
    }
}

/// Reads an EPUB: a zip of XHTML files. `META-INF/container.xml` names the
/// package file, the package's spine lists the chapters in reading order, and
/// each chapter's markup already says where paragraphs and headings are. So
/// unlike a PDF there is nothing to guess about layout: the work is reading
/// the markup leniently, because real books are full of HTML entities and
/// sloppy XML that `XMLParser` would reject.
///
/// The rules mirror `desktop/crates/core/src/epub.rs`. Keep the two in step.
enum EPUBTextExtractor {

    static func extract(from url: URL) throws -> ExtractedBook {
        let data: Data
        do {
            data = try Data(contentsOf: url, options: .mappedIfSafe)
        } catch {
            throw EPUBImportError.unreadable
        }
        guard let archive = ZipReader(data: data) else { throw EPUBImportError.unreadable }
        if isDRMLocked(archive) { throw EPUBImportError.locked }

        guard let container = archive.text("META-INF/container.xml"),
              let packagePath = tags(in: container)
                .first(where: { !$0.closing && $0.name == "rootfile" })?
                .attribute("full-path"),
              let package = archive.text(packagePath)
        else { throw EPUBImportError.notEPUB }

        let base = packagePath.range(of: "/", options: .backwards)
            .map { String(packagePath[..<$0.upperBound]) } ?? ""
        var chapters: [[TextCleaner.Paragraph]] = []
        var chapterPaths: [String] = []
        for href in spineDocuments(in: package) {
            autoreleasepool {
                let path = resolve(base: base, href: href)
                if let markup = archive.text(path) {
                    chapters.append(paragraphs(in: markup, anchorPrefix: path + "#"))
                    chapterPaths.append(path)
                }
            }
        }

        let prose = TextCleaner.prose(fromChapters: chapters)
        let sentences = prose.sentences
        let characters = sentences.reduce(0) { $0 + $1.count }
        guard sentences.count >= 3, characters >= 200 else { throw EPUBImportError.noText }
        let words = sentences.reduce(0) { $0 + $1.split(separator: " ").count }

        return ExtractedBook(
            title: title(in: package),
            // EPUBs have no pages. This is an estimate at 275 words a page.
            pageCount: max(1, (words + 274) / 275),
            sentences: sentences,
            paragraphEnds: prose.paragraphEnds,
            bookKey: try PDFTextExtractor.fileKey(for: url),
            chapters: chaptersFromContents(archive: archive, package: package, base: base, paths: chapterPaths, prose: prose)
                ?? Chapter.list(sentences: sentences, headings: prose.headings)
        )
    }

    // MARK: - DRM

    /// Store-bought EPUBs carry Adobe or Apple DRM: a `rights.xml`, or an
    /// `encryption.xml` that covers the chapters. Many DRM-free books have an
    /// `encryption.xml` too, but only for obfuscated fonts, which is fine.
    private static func isDRMLocked(_ archive: ZipReader) -> Bool {
        if archive.contains("META-INF/rights.xml") || archive.contains("META-INF/sinf.xml") {
            return true
        }
        guard let encryption = archive.text("META-INF/encryption.xml") else { return false }
        return tags(in: encryption)
            .filter { $0.name == "cipherreference" }
            .compactMap { $0.attribute("uri")?.lowercased() }
            .contains { uri in [".xhtml", ".html", ".htm", ".xml"].contains { uri.hasSuffix($0) } }
    }

    // MARK: - Table of contents

    /// Chapters from the book's own table of contents: the EPUB 3 navigation
    /// document, or the EPUB 2 `toc.ncx`. Each entry links to a chapter file,
    /// often to an anchor inside it when several chapters share a file, and
    /// is mapped to the sentence it starts at. Nil when the book has no
    /// usable table of contents, so headings are used instead.
    static func chaptersFromContents(
        archive: ZipReader, package: String, base: String, paths: [String], prose: TextCleaner.Prose
    ) -> [Chapter]? {
        var navHref: String?
        var ncxHref: String?
        var ncxID: String?
        var items: [String: String] = [:]
        for tag in tags(in: package) where !tag.closing {
            if tag.name == "item", let href = tag.attribute("href") {
                if let id = tag.attribute("id") { items[id] = href }
                if (tag.attribute("properties") ?? "").split(separator: " ").contains("nav") { navHref = href }
                if tag.attribute("media-type") == "application/x-dtbncx+xml" { ncxHref = href }
            }
            if tag.name == "spine" { ncxID = tag.attribute("toc") }
        }
        if ncxHref == nil, let ncxID { ncxHref = items[ncxID] }

        var links: [(title: String, href: String, from: String)] = []
        if let navHref {
            let path = resolve(base: base, href: navHref)
            if let nav = archive.text(path) { links = navLinks(in: nav).map { ($0.title, $0.href, path) } }
        }
        if links.count < 2, let ncxHref {
            let path = resolve(base: base, href: ncxHref)
            if let ncx = archive.text(path) { links = ncxLinks(in: ncx).map { ($0.title, $0.href, path) } }
        }
        guard links.count >= 2 else { return nil }

        let fileStarts = Dictionary(zip(paths, prose.chapterStarts), uniquingKeysWith: { first, _ in first })
        var entries: [(title: String, start: Int)] = []
        for link in links {
            let folder = link.from.range(of: "/", options: .backwards).map { String(link.from[..<$0.upperBound]) } ?? ""
            let path = resolve(base: folder, href: link.href)
            let fragment = link.href.split(separator: "#", maxSplits: 1).dropFirst().first.map(String.init)
            let start = fragment.flatMap { prose.anchors[path + "#" + ($0.removingPercentEncoding ?? $0)] }
                ?? fileStarts[path]
            guard let start, start < prose.sentences.count else { continue }
            entries.append((link.title, start))
        }
        let chapters = Chapter.fromContents(entries, sentenceCount: prose.sentences.count)
        return chapters.count >= 2 ? chapters : nil
    }

    /// The links of an EPUB 3 navigation document's table of contents
    /// (`<nav epub:type="toc">`), in order, nested entries included.
    static func navLinks(in nav: String) -> [(title: String, href: String)] {
        var links: [(title: String, href: String)] = []
        var navDepth = 0
        var inContents = false
        var href: String?
        var label = ""
        for piece in pieces(in: nav) {
            switch piece {
            case .tag(let tag):
                if tag.name == "nav", !tag.selfClosing {
                    if tag.closing {
                        navDepth -= 1
                        if navDepth == 0 { inContents = false }
                    } else {
                        navDepth += 1
                        let kind = "\(tag.attribute("epub:type") ?? "") \(tag.attribute("role") ?? "")".lowercased()
                        if navDepth == 1 { inContents = kind.contains("toc") }
                    }
                }
                guard inContents, tag.name == "a" else { continue }
                if tag.closing, let target = href {
                    links.append((TextCleaner.collapseSpaces(label), target))
                    href = nil
                } else if !tag.closing {
                    href = tag.attribute("href")
                    label = ""
                }
            case .text(let text):
                if href != nil { label += decodeEntities(text) + " " }
            }
        }
        return links.filter { !$0.title.isEmpty }
    }

    /// The entries of an EPUB 2 `toc.ncx`, in order, nested ones included.
    static func ncxLinks(in ncx: String) -> [(title: String, href: String)] {
        var links: [(title: String, href: String)] = []
        var label = ""
        var inText = false
        for piece in pieces(in: ncx) {
            switch piece {
            case .tag(let tag):
                switch tag.name {
                case "navpoint" where !tag.closing:
                    label = ""
                case "text":
                    inText = !tag.closing && !tag.selfClosing
                case "content" where !tag.closing:
                    if let src = tag.attribute("src") {
                        links.append((TextCleaner.collapseSpaces(label), src))
                        label = ""
                    }
                default:
                    break
                }
            case .text(let text):
                if inText { label += decodeEntities(text) + " " }
            }
        }
        return links.filter { !$0.title.isEmpty }
    }

    // MARK: - Paths

    /// Joins a path from the package file to the folder the package sits in,
    /// undoing `%20` escapes and `..` steps.
    static func resolve(base: String, href: String) -> String {
        let withoutFragment = href.split(separator: "#", omittingEmptySubsequences: false).first.map(String.init) ?? ""
        let decoded = withoutFragment.removingPercentEncoding ?? withoutFragment
        var parts: [Substring] = []
        for part in (base + decoded).split(separator: "/", omittingEmptySubsequences: true) {
            switch part {
            case ".": continue
            case "..": _ = parts.popLast()
            default: parts.append(part)
            }
        }
        return parts.joined(separator: "/")
    }

    // MARK: - The package: title and reading order

    /// Hrefs of the chapters to read, in order. Skips the table of contents
    /// page, anything marked as outside the main reading order, and spine
    /// entries that are images rather than text.
    static func spineDocuments(in package: String) -> [String] {
        var manifest: [String: (href: String, mediaType: String, properties: String)] = [:]
        var spine: [(idref: String, linear: Bool)] = []
        for tag in tags(in: package) where !tag.closing {
            switch tag.name {
            case "item":
                if let id = tag.attribute("id"), let href = tag.attribute("href") {
                    manifest[id] = (href, tag.attribute("media-type") ?? "", tag.attribute("properties") ?? "")
                }
            case "itemref":
                if let idref = tag.attribute("idref") {
                    spine.append((idref, tag.attribute("linear") != "no"))
                }
            default:
                break
            }
        }
        return spine
            .filter(\.linear)
            .compactMap { manifest[$0.idref] }
            .filter { item in
                let isText = item.mediaType.isEmpty || item.mediaType.contains("html")
                let isNav = item.properties.split(separator: " ").contains("nav")
                return isText && !isNav
            }
            .map(\.href)
    }

    private static let titleRegex: NSRegularExpression = {
        let pattern = "<(?:[a-z0-9]+:)?title\\b[^>]*>(.*?)</(?:[a-z0-9]+:)?title\\s*>"
        // The pattern is a constant, so a failure here is a programmer error.
        return try! NSRegularExpression(pattern: pattern, options: [.caseInsensitive, .dotMatchesLineSeparators])
    }()

    static func title(in package: String) -> String? {
        let range = NSRange(package.startIndex..<package.endIndex, in: package)
        guard let match = titleRegex.firstMatch(in: package, range: range),
              let captured = Range(match.range(at: 1), in: package)
        else { return nil }
        let title = TextCleaner.collapseSpaces(
            decodeEntities(String(package[captured])).replacingOccurrences(of: "\n", with: " ")
        )
        return title.count >= 2 ? title : nil
    }

    // MARK: - Chapter markup to paragraphs

    /// Elements whose contents are never read aloud.
    private static let skipped: Set<String> = [
        "head", "script", "style", "title", "svg", "math", "rt", "rp", "template", "object",
    ]

    /// Elements that start a new paragraph when they open or close.
    private static let blocks: Set<String> = [
        "p", "div", "h1", "h2", "h3", "h4", "h5", "h6", "li", "ul", "ol", "dl", "dt", "dd", "blockquote",
        "section", "article", "header", "footer", "aside", "nav", "main", "figure", "figcaption", "table",
        "tr", "td", "th", "caption", "pre", "hr", "br", "body", "hgroup", "address", "center",
    ]

    private static let headings: Set<String> = ["h1", "h2", "h3", "h4", "h5", "h6"]

    private static let apparatusKinds: Set<String> = [
        "noteref", "footnote", "endnote", "rearnote", "pagebreak",
        "doc-noteref", "doc-footnote", "doc-endnote", "doc-pagebreak",
    ]

    /// Footnote markers, footnotes and page break markers are part of the
    /// book's apparatus, not its story. Read aloud they are a stray "12".
    private static func isApparatus(_ tag: Tag) -> Bool {
        let kinds = "\(tag.attribute("epub:type") ?? "") \(tag.attribute("role") ?? "")".lowercased()
        return kinds.split(separator: " ").contains { apparatusKinds.contains(String($0)) }
    }

    /// Splits one chapter's XHTML into paragraphs, marking real headings.
    static func paragraphs(in markup: String, anchorPrefix: String = "") -> [TextCleaner.Paragraph] {
        var out: [TextCleaner.Paragraph] = []
        var current = ""
        var currentIsHeading = false
        // Anchors met since the last paragraph: they lead to the next one.
        var anchors: [String] = []
        var headingDepth = 0
        // While inside an element that is not read, its name and nesting depth.
        var skipping: (name: String, depth: Int)?

        func flush() {
            let text = TextCleaner.collapseSpaces(current.replacingOccurrences(of: "\n", with: " ")
                .replacingOccurrences(of: "\r", with: " ").replacingOccurrences(of: "\t", with: " ")
                .replacingOccurrences(of: "\u{00A0}", with: " "))
            if !text.isEmpty {
                out.append(TextCleaner.Paragraph(text: text, isHeading: currentIsHeading, anchors: anchors))
                anchors = []
            }
            current = ""
            currentIsHeading = false
        }

        for piece in pieces(in: markup) {
            switch piece {
            case .text(let raw):
                guard skipping == nil, !raw.isEmpty else { continue }
                if headingDepth > 0, raw.contains(where: { !$0.isWhitespace }) {
                    currentIsHeading = true
                }
                current += decodeEntities(raw)
            case .tag(let tag):
                if let active = skipping {
                    if tag.name == active.name {
                        if tag.closing {
                            skipping = active.depth > 1 ? (active.name, active.depth - 1) : nil
                        } else if !tag.selfClosing {
                            skipping = (active.name, active.depth + 1)
                        }
                    }
                    continue
                }
                if !tag.closing, !tag.selfClosing, skipped.contains(tag.name) || isApparatus(tag) {
                    skipping = (tag.name, 1)
                    continue
                }
                if blocks.contains(tag.name) {
                    flush()
                }
                if !tag.closing, tag.attributes.contains("id=") || tag.attributes.contains("name="),
                   let id = tag.attribute("id") ?? tag.attribute("name") {
                    anchors.append(anchorPrefix + id)
                }
                if headings.contains(tag.name), !tag.selfClosing {
                    headingDepth = tag.closing ? max(0, headingDepth - 1) : headingDepth + 1
                }
            }
        }
        flush()
        return out
    }

    // MARK: - A lenient markup scanner

    struct Tag {
        /// Lowercased local name: `<xhtml:p>` and `<P>` are both `p`.
        var name: String
        var attributes: String
        var closing: Bool
        var selfClosing: Bool

        private static let attributeRegex: NSRegularExpression = {
            let pattern = "([^\\s=/]+)\\s*=\\s*(?:\"([^\"]*)\"|'([^']*)'|([^\\s\"'>]+))"
            return try! NSRegularExpression(pattern: pattern)
        }()

        /// The value of an attribute, matched on its full name, entities decoded.
        func attribute(_ wanted: String) -> String? {
            let range = NSRange(attributes.startIndex..<attributes.endIndex, in: attributes)
            for match in Tag.attributeRegex.matches(in: attributes, range: range) {
                guard let nameRange = Range(match.range(at: 1), in: attributes),
                      attributes[nameRange].lowercased() == wanted.lowercased()
                else { continue }
                for group in 2...4 {
                    if let valueRange = Range(match.range(at: group), in: attributes) {
                        return EPUBTextExtractor.decodeEntities(String(attributes[valueRange]))
                    }
                }
            }
            return nil
        }
    }

    enum Piece {
        case text(String)
        case tag(Tag)
    }

    static func tags(in markup: String) -> [Tag] {
        pieces(in: markup).compactMap { piece in
            if case .tag(let tag) = piece { return tag }
            return nil
        }
    }

    /// Splits markup into text runs and tags. Comments, processing
    /// instructions and doctypes are dropped; CDATA is passed through as text.
    /// Works on UTF-8 bytes, since every character that matters is ASCII.
    static func pieces(in markup: String) -> [Piece] {
        let bytes = Array(markup.utf8)
        let count = bytes.count
        var out: [Piece] = []
        var index = 0

        func text(_ start: Int, _ end: Int) -> String {
            String(decoding: bytes[start..<end], as: UTF8.self)
        }
        func hasPrefix(_ prefix: String, at position: Int) -> Bool {
            let needle = Array(prefix.utf8)
            guard position + needle.count <= count else { return false }
            return Array(bytes[position..<position + needle.count]) == needle
        }
        func find(_ needle: String, from start: Int) -> Int? {
            let needle = Array(needle.utf8)
            guard !needle.isEmpty, start + needle.count <= count else { return nil }
            var position = start
            while position + needle.count <= count {
                if bytes[position] == needle[0], Array(bytes[position..<position + needle.count]) == needle {
                    return position
                }
                position += 1
            }
            return nil
        }

        let lessThan = UInt8(ascii: "<"), greaterThan = UInt8(ascii: ">")
        let doubleQuote = UInt8(ascii: "\""), singleQuote = UInt8(ascii: "'")

        while index < count {
            guard let open = bytes[index...].firstIndex(of: lessThan) else {
                out.append(.text(text(index, count)))
                break
            }
            if open > index {
                out.append(.text(text(index, open)))
                index = open
                continue
            }
            if hasPrefix("<!--", at: index) {
                index = find("-->", from: index + 4).map { $0 + 3 } ?? count
                continue
            }
            if hasPrefix("<![CDATA[", at: index) {
                let start = index + 9
                let end = find("]]>", from: start) ?? count
                out.append(.text(text(start, end)))
                index = min(count, end + 3)
                continue
            }
            if hasPrefix("<!", at: index) || hasPrefix("<?", at: index) {
                index = find(">", from: index).map { $0 + 1 } ?? count
                continue
            }
            // Find the closing '>' that is not inside a quoted attribute value.
            var position = index + 1
            var quote: UInt8?
            var end: Int?
            while position < count {
                let byte = bytes[position]
                if let open = quote {
                    if byte == open { quote = nil }
                } else if byte == doubleQuote || byte == singleQuote {
                    quote = byte
                } else if byte == greaterThan {
                    end = position
                    break
                }
                position += 1
            }
            guard let end else {
                // A stray '<' with no tag after it is text.
                out.append(.text("<"))
                index += 1
                continue
            }
            var inner = Substring(text(index + 1, end).trimmingCharacters(in: .whitespacesAndNewlines))
            index = end + 1

            let closing = inner.hasPrefix("/")
            while inner.hasPrefix("/") { inner = inner.dropFirst() }
            let selfClosing = inner.hasSuffix("/")
            while inner.hasSuffix("/") { inner = inner.dropLast() }
            let nameEnd = inner.firstIndex(where: { $0.isWhitespace }) ?? inner.endIndex
            let qualified = inner[..<nameEnd]
            let local = qualified.split(separator: ":", omittingEmptySubsequences: false).last.map(String.init) ?? ""
            guard let first = local.first, first.isASCII, first.isLetter else {
                // "a < b" in sloppy markup: not a tag.
                continue
            }
            out.append(.tag(Tag(
                name: local.lowercased(),
                attributes: String(inner[nameEnd...]),
                closing: closing,
                selfClosing: selfClosing
            )))
        }
        return out
    }

    // MARK: - Entities

    private static let namedEntities: [String: String] = [
        "amp": "&", "lt": "<", "gt": ">", "quot": "\"", "apos": "'",
        "nbsp": "\u{00A0}", "ensp": " ", "emsp": " ", "thinsp": " ",
        "shy": "", "zwsp": "", "zwnj": "", "zwj": "",
        "mdash": "\u{2014}", "ndash": "\u{2013}", "hellip": "\u{2026}",
        "lsquo": "\u{2018}", "rsquo": "\u{2019}", "ldquo": "\u{201C}", "rdquo": "\u{201D}",
        "sbquo": "\u{201A}", "bdquo": "\u{201E}", "laquo": "\u{00AB}", "raquo": "\u{00BB}",
        "copy": "\u{00A9}", "reg": "\u{00AE}", "trade": "\u{2122}", "deg": "\u{00B0}",
        "middot": "\u{00B7}", "bull": "\u{2022}", "times": "\u{00D7}",
        "eacute": "é", "egrave": "è", "ecirc": "ê", "euml": "ë",
        "aacute": "á", "agrave": "à", "acirc": "â", "auml": "ä", "aring": "å", "aelig": "æ",
        "ccedil": "ç", "iacute": "í", "icirc": "î", "iuml": "ï", "ntilde": "ñ",
        "oacute": "ó", "ocirc": "ô", "ouml": "ö", "oslash": "ø",
        "uacute": "ú", "ucirc": "û", "uuml": "ü", "szlig": "ß",
        "Eacute": "É", "Aacute": "Á", "Ccedil": "Ç", "Ntilde": "Ñ", "Ouml": "Ö", "Uuml": "Ü", "Auml": "Ä",
    ]

    /// Decodes `&amp;`, `&#8217;`, `&#x2014;` and the named entities books
    /// use. An unknown entity is dropped rather than read out.
    static func decodeEntities(_ text: String) -> String {
        guard text.contains("&") else { return text }
        var out = ""
        var rest = Substring(text)
        while let ampersand = rest.firstIndex(of: "&") {
            out += rest[..<ampersand]
            let after = rest[rest.index(after: ampersand)...]
            guard let semicolon = after.prefix(11).firstIndex(of: ";") else {
                out += "&"
                rest = after
                continue
            }
            let entity = String(after[..<semicolon])
            var decoded: String?
            if entity.hasPrefix("#x") || entity.hasPrefix("#X") {
                decoded = UInt32(entity.dropFirst(2), radix: 16).flatMap(Unicode.Scalar.init).map { String($0) }
            } else if entity.hasPrefix("#") {
                decoded = UInt32(entity.dropFirst()).flatMap(Unicode.Scalar.init).map { String($0) }
            } else {
                decoded = namedEntities[entity]
            }
            if let decoded {
                out += decoded
            } else if !entity.allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber) }) {
                // Not an entity at all, just an ampersand in the text.
                out += "&"
                rest = after
                continue
            }
            rest = after[after.index(after: semicolon)...]
        }
        out += rest
        return out
    }
}

// MARK: - The zip

/// Just enough of a zip reader for EPUBs: the central directory, and entries
/// that are stored or deflated. iOS has no public zip API, and this avoids a
/// package dependency for a few dozen lines.
struct ZipReader {
    private struct Entry {
        var method: UInt16
        var compressedSize: Int
        var uncompressedSize: Int
        var localHeaderOffset: Int
    }

    private let bytes: [UInt8]
    private let entries: [String: Entry]

    init?(data: Data) {
        let bytes = [UInt8](data)
        self.bytes = bytes
        let count = bytes.count
        guard count >= 22 else { return nil }

        func u16(_ offset: Int) -> Int { Int(bytes[offset]) | Int(bytes[offset + 1]) << 8 }
        func u32(_ offset: Int) -> Int { u16(offset) | u16(offset + 2) << 16 }

        // The end of central directory record sits in the last 64 KB.
        var end: Int?
        var position = count - 22
        let lowest = max(0, count - 22 - 65_535)
        while position >= lowest {
            if u32(position) == 0x0605_4B50 { end = position; break }
            position -= 1
        }
        guard let end else { return nil }

        let entryCount = u16(end + 10)
        var offset = u32(end + 16)
        var entries: [String: Entry] = [:]
        for _ in 0..<entryCount {
            guard offset + 46 <= count, u32(offset) == 0x0201_4B50 else { return nil }
            let nameLength = u16(offset + 28)
            let extraLength = u16(offset + 30)
            let commentLength = u16(offset + 32)
            guard offset + 46 + nameLength <= count else { return nil }
            let name = String(decoding: bytes[(offset + 46)..<(offset + 46 + nameLength)], as: UTF8.self)
            entries[name] = Entry(
                method: UInt16(u16(offset + 10)),
                compressedSize: u32(offset + 20),
                uncompressedSize: u32(offset + 24),
                localHeaderOffset: u32(offset + 42)
            )
            offset += 46 + nameLength + extraLength + commentLength
        }
        self.entries = entries
    }

    /// Every file name in the archive.
    var names: [String] { Array(entries.keys) }

    func contains(_ name: String) -> Bool {
        entries[name] != nil
    }

    func data(_ name: String) -> [UInt8]? {
        guard let entry = entries[name] else { return nil }
        func u16(_ offset: Int) -> Int { Int(bytes[offset]) | Int(bytes[offset + 1]) << 8 }
        let header = entry.localHeaderOffset
        guard header + 30 <= bytes.count else { return nil }
        let start = header + 30 + u16(header + 26) + u16(header + 28)
        guard start + entry.compressedSize <= bytes.count else { return nil }
        let compressed = bytes[start..<(start + entry.compressedSize)]

        switch entry.method {
        case 0:
            return Array(compressed)
        case 8:
            guard entry.uncompressedSize > 0 else { return [] }
            var output = [UInt8](repeating: 0, count: entry.uncompressedSize)
            let written = compressed.withUnsafeBufferPointer { source in
                output.withUnsafeMutableBufferPointer { destination in
                    // COMPRESSION_ZLIB is raw DEFLATE, which is what zip uses.
                    compression_decode_buffer(
                        destination.baseAddress!, entry.uncompressedSize,
                        source.baseAddress!, entry.compressedSize,
                        nil, COMPRESSION_ZLIB
                    )
                }
            }
            return written == entry.uncompressedSize ? output : nil
        default:
            return nil
        }
    }

    /// An entry as text. Book files are UTF-8 almost always; UTF-16 shows up
    /// now and then and is marked with a byte order mark.
    func text(_ name: String) -> String? {
        guard let bytes = data(name) else { return nil }
        if bytes.starts(with: [0xFF, 0xFE]) {
            return String(data: Data(bytes.dropFirst(2)), encoding: .utf16LittleEndian)
        }
        if bytes.starts(with: [0xFE, 0xFF]) {
            return String(data: Data(bytes.dropFirst(2)), encoding: .utf16BigEndian)
        }
        if bytes.starts(with: [0xEF, 0xBB, 0xBF]) {
            return String(decoding: bytes.dropFirst(3), as: UTF8.self)
        }
        return String(decoding: bytes, as: UTF8.self)
    }
}
