import Foundation
import NaturalLanguage

/// Turns raw per-page PDF text into a clean list of sentences to read aloud.
///
/// ARC PDFs are noisy: every page carries a running header, a page number and
/// often a watermark line ("Advance Reader Copy", "Uncorrected Proof"). Read
/// aloud, that noise interrupts the story every thirty seconds, so most of the
/// work here is finding lines that repeat across pages and dropping them.
enum TextCleaner {

    // MARK: - Entry point

    /// Sentences in reading order, with the ones that end a paragraph marked
    /// so the reader can pause longer there, the way a narrator does.
    struct Prose: Sendable, Equatable {
        var sentences: [String] = []
        /// Indices into `sentences`, ascending.
        var paragraphEnds: [Int] = []
        /// Sentences that are headings (chapter titles, part titles), ascending.
        var headings: [Int] = []
        /// For an EPUB: the first sentence of each chapter file, in order.
        var chapterStarts: [Int] = []
        /// For an EPUB: the sentence each `id` anchor in the markup leads to.
        var anchors: [String: Int] = [:]
        /// For a PDF: the page each sentence starts on.
        var pages: [Int] = []

        mutating func appendBlock(_ block: [String]) {
            guard !block.isEmpty else { return }
            sentences.append(contentsOf: block)
            paragraphEnds.append(sentences.count - 1)
        }

        mutating func appendHeading(_ heading: String) {
            appendBlock([heading])
            headings.append(sentences.count - 1)
        }
    }

    /// - Parameter pages: the raw text of each page, in reading order.
    /// - Returns: sentences in reading order, with page furniture removed.
    static func sentences(fromPages pages: [String]) -> [String] {
        prose(fromPages: pages).sentences
    }

    /// Like `sentences(fromPages:)`, keeping paragraph ends.
    ///
    /// A PDF does not say where paragraphs end, but typeset prose shows it:
    /// the last line of a paragraph ends a sentence and stops well short of
    /// the right margin. Lines are compared with the book's typical full line.
    /// - Parameter headingLines: for each page, the lines set in a heading's
    ///   type, when the PDF says so. Those are the headings, and only those.
    static func prose(fromPages pages: [String], headingLines: [Set<String>]? = nil) -> Prose {
        let pageLines = pages.map { lines(in: $0) }
        let stripped = removeRepeatedLines(pageLines, keeping: headingLines)
        var prose = Prose()
        for block in joinIntoBlocks(stripped, fullLine: typicalLineLength(stripped), headingLines: headingLines) {
            if block.isHeading {
                prose.appendHeading(block.text)
            } else {
                prose.appendBlock(splitSentences(block.text))
            }
            prose.pages += Array(repeating: block.page, count: prose.sentences.count - prose.pages.count)
        }
        return prose
    }

    /// The length of a full line of body text: the median of the longer lines.
    static func typicalLineLength(_ pages: [[String]]) -> Int? {
        let lengths = pages.joined().map(\.count).filter { $0 >= 25 }.sorted()
        guard lengths.count >= 10 else { return nil }
        let median = lengths[lengths.count / 2]
        return median >= 40 ? median : nil
    }

    // MARK: - EPUB chapters

    /// One paragraph of an EPUB chapter, as the book's markup laid it out.
    struct Paragraph: Sendable, Equatable {
        var text: String
        /// True when the markup itself says this is a heading (`<h1>` to `<h6>`).
        var isHeading: Bool
        /// `id` anchors in the markup that lead here ("chapter3.xhtml#ch3"),
        /// which a table of contents may link to.
        var anchors: [String] = []
    }

    /// Paragraphs of each EPUB chapter, in reading order, to sentences.
    ///
    /// An EPUB has no pages, so there are no running headers or page numbers
    /// to find. What it can have is a watermark paragraph stamped into every
    /// chapter ("This copy is licensed to reader@example.com"). Stamps go at
    /// the start or end of a chapter, so the rule only looks there: a short
    /// paragraph among the first or last three of a chapter, whose fingerprint
    /// sits in that zone in half or more of the chapters (and at least four),
    /// is dropped. Looking only at the edges keeps short lines of dialogue
    /// like "No." that recur all through a book. Headings are compared by
    /// their exact text instead, so "Chapter 1" and "Chapter 2" survive while
    /// a repeated "ADVANCE READER COPY" banner does not.
    static func sentences(fromChapters chapters: [[Paragraph]]) -> [String] {
        prose(fromChapters: chapters).sentences
    }

    /// Like `sentences(fromChapters:)`, keeping paragraph ends.
    static func prose(fromChapters chapters: [[Paragraph]]) -> Prose {
        // A dropped paragraph hands its anchors on to the next one kept.
        let cleaned: [[Paragraph]] = chapters.map { paragraphs in
            var carried: [String] = []
            var kept: [Paragraph] = paragraphs.compactMap { paragraph in
                let text = lines(in: paragraph.text).joined(separator: " ")
                guard hasLettersOrDigits(text), !isContentsEntry(text) else {
                    carried += paragraph.anchors
                    return nil
                }
                defer { carried = [] }
                return Paragraph(
                    text: text, isHeading: paragraph.isHeading || isHeading(text), anchors: carried + paragraph.anchors)
            }
            return kept
        }

        func repeatKey(_ paragraph: Paragraph) -> String {
            paragraph.isHeading ? "h:" + paragraph.text.lowercased() : "p:" + fingerprint(paragraph.text)
        }
        func inStampZone(_ index: Int, _ count: Int) -> Bool {
            index < 3 || index + 3 >= count
        }

        let chapterCount = cleaned.count
        var counts: [String: Int] = [:]
        if chapterCount >= 4 {
            for paragraphs in cleaned {
                var seen = Set<String>()
                for (index, paragraph) in paragraphs.enumerated()
                where inStampZone(index, paragraphs.count) && paragraph.text.count <= 100 {
                    seen.insert(repeatKey(paragraph))
                }
                for key in seen { counts[key, default: 0] += 1 }
            }
        }
        let minimum = max(4, Int((Double(chapterCount) * 0.5).rounded(.up)))

        var prose = Prose()
        for paragraphs in cleaned {
            prose.chapterStarts.append(prose.sentences.count)
            var carried: [String] = []
            for (index, paragraph) in paragraphs.enumerated() {
                if chapterCount >= 4, inStampZone(index, paragraphs.count), paragraph.text.count <= 100,
                   (counts[repeatKey(paragraph)] ?? 0) >= minimum {
                    carried += paragraph.anchors
                    continue
                }
                for anchor in carried + paragraph.anchors where prose.anchors[anchor] == nil {
                    prose.anchors[anchor] = prose.sentences.count
                }
                carried = []
                if paragraph.isHeading {
                    let letters = paragraph.text.filter { $0.isLetter }
                    let allCaps = letters.count >= 3 && letters.allSatisfy { $0.isUppercase }
                    prose.appendHeading(allCaps ? titleCase(paragraph.text) : paragraph.text)
                } else {
                    // Each paragraph is its own block, so a paragraph that ends
                    // without punctuation never runs into the next one.
                    prose.appendBlock(splitSentences(paragraph.text))
                }
            }
        }
        return prose
    }

    /// "CHAPTER ONE" reads more naturally as "Chapter One". Roman numerals
    /// stay in capitals ("PART III" is "Part III", not "Part Iii") so the
    /// voice can still read them as numbers.
    static func titleCase(_ line: String) -> String {
        var out = ""
        var word = ""
        func flush() {
            if isRomanNumeral(word) {
                out += word
            } else if let first = word.first {
                out += String(first).uppercased() + word.dropFirst().lowercased()
            }
            word = ""
        }
        for character in line {
            if character.isLetter || character == "'" || character == "\u{2019}" {
                word.append(character)
            } else {
                flush()
                out.append(character)
            }
        }
        flush()
        return out
    }

    static func isRomanNumeral(_ word: String) -> Bool {
        word.count <= 8
            && !word.isEmpty
            && word.allSatisfy { "IVXLC".contains($0) }
            && !["I", "LIV", "CIVIC", "CIVIL", "VIC", "LIC"].contains(word)
    }

    // MARK: - Lines

    /// Normalizes odd characters and splits a page into trimmed, non-empty lines.
    static func lines(in page: String) -> [String] {
        var text = page
        let replacements: [(String, String)] = [
            ("\r\n", "\n"), ("\r", "\n"), ("\u{2028}", "\n"), ("\u{2029}", "\n"),
            ("\u{00AD}", ""),            // soft hyphen
            ("\u{00A0}", " "),           // non-breaking space
            ("\u{2009}", " "), ("\u{200A}", " "), ("\u{202F}", " "),
            ("\u{200B}", ""),            // zero-width space
            ("\u{2060}", ""), ("\u{FEFF}", ""), // word joiners
            ("\t", " "),
            ("\u{FB00}", "ff"), ("\u{FB01}", "fi"), ("\u{FB02}", "fl"),
            ("\u{FB03}", "ffi"), ("\u{FB04}", "ffl"),
        ]
        for (from, to) in replacements {
            text = text.replacingOccurrences(of: from, with: to)
        }
        return text
            .split(separator: "\n", omittingEmptySubsequences: true)
            .map { collapseSpaces(String($0)) }
            .filter { !$0.isEmpty }
    }

    static func collapseSpaces(_ string: String) -> String {
        string
            .split(separator: " ", omittingEmptySubsequences: true)
            .joined(separator: " ")
    }

    // MARK: - Repeated lines (headers, footers, watermarks)

    /// A line reduced to its shape: lowercased, with every run of digits
    /// replaced by `#`. "Chapter 12" and "Chapter 13" share a fingerprint, and
    /// so do "The Long Road | 41" and "The Long Road | 42".
    static func fingerprint(_ line: String) -> String {
        var out = ""
        var lastWasDigit = false
        for character in line.lowercased() {
            if character.isNumber {
                if !lastWasDigit { out.append("#") }
                lastWasDigit = true
            } else {
                out.append(character)
                lastWasDigit = false
            }
        }
        return out
    }

    static func hasLettersOrDigits(_ line: String) -> Bool {
        line.contains { $0.isLetter || $0.isNumber }
    }

    /// True when a line ends the way prose ends. Running headers almost never do.
    static func endsLikeSentence(_ line: String) -> Bool {
        guard let last = line.last else { return false }
        return ".!?\"\u{201D}\u{2019}'".contains(last)
    }

    private static let pageNumberRegex: NSRegularExpression = {
        // "12", "- 12 -", "Page 12", or a lowercase roman numeral up to xxxix.
        let pattern = "^[\\W_]*(?:(?i:page)\\s+)?(?:\\d{1,4}|(?=[ivx])x{0,3}(?:ix|iv|v?i{0,3}))[\\W_]*$"
        // The pattern is a constant, so a failure here is a programmer error.
        return try! NSRegularExpression(pattern: pattern)
    }()

    static func isPageNumber(_ line: String) -> Bool {
        guard line.count <= 14 else { return false }
        let range = NSRange(line.startIndex..<line.endIndex, in: line)
        return pageNumberRegex.firstMatch(in: line, range: range) != nil
    }

    private static let contentsEntryRegex: NSRegularExpression = {
        // A table of contents row: dot leaders followed by a page number.
        let pattern = "(?:\\.{4,}|(?:\\s\\.){3,})\\s*\\d+\\s*$"
        return try! NSRegularExpression(pattern: pattern)
    }()

    static func isContentsEntry(_ line: String) -> Bool {
        let range = NSRange(line.startIndex..<line.endIndex, in: line)
        return contentsEntryRegex.firstMatch(in: line, range: range) != nil
    }

    /// Drops lines that are page furniture rather than story.
    ///
    /// Three rules, from most to least certain:
    /// 1. A short line whose fingerprint shows up on 40% or more of the pages
    ///    is a watermark or a global header, wherever it sits on the page.
    /// 2. A short line in the first or last two lines of a page, that does not
    ///    end like a sentence, and whose fingerprint sits in that same edge
    ///    zone on three or more pages, is a running header or footer. This is
    ///    what catches chapter titles repeated at the top of each page.
    /// 3. A bare page number in the edge zone.
    static func removeRepeatedLines(_ pageLines: [[String]], keeping headingLines: [Set<String>]? = nil) -> [[String]] {
        let pageCount = pageLines.count
        var anywhere: [String: Int] = [:]
        var edge: [String: Int] = [:]

        if pageCount >= 4 {
            for lines in pageLines {
                var seen = Set<String>()
                var seenEdge = Set<String>()
                let count = lines.count
                for (index, line) in lines.enumerated() where line.count <= 100 {
                    let key = fingerprint(line)
                    seen.insert(key)
                    if index < 2 || index >= count - 2 {
                        seenEdge.insert(key)
                    }
                }
                for key in seen { anywhere[key, default: 0] += 1 }
                for key in seenEdge { edge[key, default: 0] += 1 }
            }
        }

        let anywhereMinimum = max(4, Int((Double(pageCount) * 0.4).rounded(.up)))
        let edgeMinimum = 3

        // "Chapter 6" and "Chapter 7" share a fingerprint and both sit at the
        // top of a page, so they look like a running header. But a header
        // repeats the same words page after page, while each chapter heading
        // is different. A line that opens with a chapter word is kept the
        // first time its exact words appear; only the repeats go (a book that
        // prints the chapter name atop every page keeps it on the first).
        var headingsSeen = Set<String>()

        // Page numbers rise with the page; find the offset between the two
        // (front matter shifts it), so a number that does not fit is a
        // chapter number ("23" heading a chapter), not a page number.
        var offsets: [Int: Int] = [:]
        for (pageIndex, lines) in pageLines.enumerated() {
            let edges = lines.prefix(2) + lines.suffix(2)
            for line in Set(edges) {
                if let number = Int(line.trimmingCharacters(in: .whitespaces)) {
                    offsets[number - pageIndex, default: 0] += 1
                }
            }
        }
        let pageOffset = offsets.filter { $0.value >= 3 }.max { $0.value < $1.value }?.key
        let pageLengths = pageLines.map(\.count).filter { $0 > 0 }.sorted()
        let typicalPage = pageLengths.isEmpty ? 0 : pageLengths[pageLengths.count / 2]

        var pageIndex = -1
        return pageLines.map { lines in
            pageIndex += 1
            // A chapter opener: the previous page stopped short.
            let opener = pageIndex > 0 && typicalPage >= 10
                && pageLines[pageIndex - 1].count * 100 < typicalPage * 90
            let count = lines.count
            var kept: [String] = []
            kept.reserveCapacity(count)
            // A printed table of contents: a "Contents" title, or a page that
            // is mostly "Chapter ..." lines. Its entries are neither chapters
            // nor anything to read aloud.
            let isContentsPage = lines.contains { ["contents", "table of contents"].contains($0.lowercased()) }
                || lines.filter(startsWithHeadingWord).count >= 5
            for (index, line) in lines.enumerated() {
                // Ornaments like "* * *" have nothing to say out loud.
                guard hasLettersOrDigits(line), !isContentsEntry(line) else { continue }
                if isContentsPage, startsWithHeadingWord(line) { continue }
                // Set in a heading's type: a heading, however often its
                // words recur (a narrator's name heads many chapters).
                if let headingLines, pageIndex < headingLines.count, headingLines[pageIndex].contains(line) {
                    kept.append(line)
                    continue
                }
                let isEdge = index < 2 || index >= count - 2
                // At the top of a chapter opener, a number that is not the
                // page's own is the chapter's: "23", "XXIII". Checked first,
                // since bare numbers look like the page numbers on every page.
                let trimmed = line.trimmingCharacters(in: .whitespaces)
                if opener, index < 3, isPageNumber(line),
                   Int(trimmed).map({ pageOffset == nil || $0 - pageIndex != pageOffset })
                    ?? (isRomanNumeral(trimmed) || trimmed == "I") {
                    kept.append(line)
                    continue
                }
                if startsWithHeadingWord(line), line.count <= 60, !endsLikeSentence(line) {
                    let exact = line.lowercased()
                    if !headingsSeen.contains(exact) {
                        headingsSeen.insert(exact)
                        kept.append(line)
                        continue
                    }
                }
                if pageCount >= 4, line.count <= 100 {
                    let key = fingerprint(line)
                    if (anywhere[key] ?? 0) >= anywhereMinimum { continue }
                    if isEdge, line.count <= 70, !endsLikeSentence(line),
                       (edge[key] ?? 0) >= edgeMinimum {
                        continue
                    }
                }
                if isEdge, isPageNumber(line) { continue }
                kept.append(line)
            }
            return kept
        }
    }

    // MARK: - Joining lines back into running text

    private static let headingWords: Set<String> = [
        "chapter", "part", "book", "prologue", "epilogue", "interlude",
        "acknowledgments", "acknowledgements", "contents", "dedication",
        "introduction", "foreword", "preface", "afterword",
    ]

    /// True for "Chapter 6", "PART TWO", "Prologue": a line that opens with
    /// a word that names a part of a book.
    static func startsWithHeadingWord(_ line: String) -> Bool {
        guard let first = line.split(separator: " ").first else { return false }
        return headingWords.contains(first.lowercased().trimmingCharacters(in: .punctuationCharacters))
    }

    /// Chapter headings have no closing punctuation, so left alone they run
    /// straight into the first sentence of the chapter. Spotting them lets the
    /// reader give them their own beat.
    static func isHeading(_ line: String) -> Bool {
        guard line.count <= 60, !endsLikeSentence(line) else { return false }
        let words = line.split(separator: " ")
        guard let first = words.first, words.count <= 8 else { return false }
        let firstWord = first.lowercased().trimmingCharacters(in: .punctuationCharacters)
        if headingWords.contains(firstWord) { return true }
        let letters = line.filter { $0.isLetter }
        return letters.count >= 3 && letters.allSatisfy { $0.isUppercase }
    }

    /// Merges the lines of every page into blocks of running text. A heading
    /// becomes a block of its own; everything between headings is one block,
    /// carried across page breaks so a sentence split by a page turn is whole.
    struct Block: Equatable {
        var text: String
        var isHeading: Bool
        /// The page the block starts on.
        var page: Int = 0
    }

    static func joinIntoBlocks(_ pages: [[String]], fullLine: Int? = nil, headingLines: [Set<String>]? = nil) -> [Block] {
        var blocks: [Block] = []
        var current = ""
        func endsParagraph(_ line: String) -> Bool {
            guard let fullLine else { return false }
            return endsLikeSentence(line) && line.count * 100 < fullLine * 78
        }

        var currentPage = 0
        func flush() {
            let trimmed = current.trimmingCharacters(in: .whitespaces)
            if !trimmed.isEmpty { blocks.append(Block(text: trimmed, isHeading: false, page: currentPage)) }
            current = ""
        }

        // A chapter ends partway down a page and the next one starts on a
        // fresh page, so a page after a short one is a chapter opener.
        let pageLengths = pages.map(\.count).filter { $0 > 0 }.sorted()
        let typicalPage = pageLengths.isEmpty ? 0 : pageLengths[pageLengths.count / 2]

        /// A chapter page opens with title lines ("Night stories", then the
        /// narrator, "Rhea") above the first paragraph. Only at the top of a
        /// page, after a paragraph that has ended, starting with a capital.
        /// Short lines that do not end a sentence anywhere; on a chapter
        /// opener also longer titles, and titles that end with a full stop
        /// ("Loomings.").
        func isPageTopHeading(_ line: String, position: Int, opener: Bool) -> Bool {
            guard let fullLine, position < (opener ? 6 : 5), current.isEmpty, let first = line.first,
                  first.isUppercase || first.isNumber, hasLettersOrDigits(line)
            else { return false }
            if line.count * 100 < fullLine * 50, !endsLikeSentence(line) { return true }
            guard opener, line.count * 100 < fullLine * 80 else { return false }
            let words = line.split(separator: " ").count
            return !endsLikeSentence(line) || (line.hasSuffix(".") && words <= 8 && !line.hasSuffix("..."))
        }

        for (pageIndex, lines) in pages.enumerated() {
            var bodyStarted = false
            let opener = pageIndex > 0 && typicalPage >= 10 && pages[pageIndex - 1].count * 100 < typicalPage * 90
            for (position, line) in lines.enumerated() {
                let isHeadingLine: Bool
                if let headingLines {
                    // The type says which lines are headings.
                    isHeadingLine = pageIndex < headingLines.count && headingLines[pageIndex].contains(line)
                    // A title set over two lines: "Mistakes and double-" "checks".
                    if isHeadingLine, current.isEmpty, var last = blocks.last, last.isHeading, last.page == pageIndex,
                       last.text.hasSuffix("-") || (line.first?.isLowercase ?? false) {
                        last.text += last.text.hasSuffix("-") ? line : " " + line
                        blocks[blocks.count - 1] = last
                        continue
                    }
                } else {
                    isHeadingLine = isHeading(line) || (!bodyStarted && isPageTopHeading(line, position: position, opener: opener))
                }
                if isHeadingLine {
                    flush()
                    // "CHAPTER ONE" reads more naturally as "Chapter One".
                    let letters = line.filter { $0.isLetter }
                    let allCaps = !letters.isEmpty && letters.allSatisfy { $0.isUppercase }
                    blocks.append(Block(text: allCaps ? titleCase(line) : line, isHeading: true, page: pageIndex))
                    continue
                }
                // The rest of a heading broken over two lines: "Mistakes and
                // double-" then "checks", or "And then there was" then "one".
                if !bodyStarted, current.isEmpty, var last = blocks.last, last.isHeading,
                   line.count < 25, !endsLikeSentence(line),
                   last.text.hasSuffix("-") || (line.first?.isLowercase ?? false) {
                    last.text += last.text.hasSuffix("-") ? line : " " + line
                    blocks[blocks.count - 1] = last
                    continue
                }
                bodyStarted = true
                if current.isEmpty {
                    current = line
                    currentPage = pageIndex
                } else if let last = current.last, last == "-" || last == "\u{2010}",
                          let first = line.first, first.isLowercase,
                          let before = current.dropLast().last, before.isLetter {
                    // A word broken across lines: "some-" + "thing" -> "something".
                    current.removeLast()
                    current += line
                } else {
                    current += " " + line
                }
                if endsParagraph(line) {
                    flush()
                }
            }
        }
        flush()
        return blocks
    }

    // MARK: - Sentences

    /// Verbs that mark "she said" style tags after a line of dialogue.
    private static let speechVerbs: Set<String> = [
        "said", "asked", "whispered", "called", "shouted", "replied", "cried", "murmured", "muttered",
        "yelled", "answered", "snapped", "laughed", "sighed", "breathed", "hissed", "growled",
        "demanded", "added", "continued", "began", "exclaimed", "screamed", "insisted", "wondered",
        "says", "asks", "whispers", "calls", "replies",
    ]

    /// True when the text starts with a dialogue tag such as "Mara whispered"
    /// or "said the ferryman".
    static func startsWithSpeechTag(_ text: String) -> Bool {
        text.split(separator: " ").prefix(2).contains { word in
            let letters = String(word.filter { $0.isLetter }).lowercased()
            return speechVerbs.contains(letters)
        }
    }

    /// True for a sentence that is a quoted exclamation or question, like `"Wait!"`.
    static func endsWithQuotedExclamation(_ sentence: String) -> Bool {
        let characters = Array(sentence)
        guard characters.count >= 2, let last = characters.last, "\"\u{201D}\u{2019}'".contains(last) else {
            return false
        }
        let before = characters[characters.count - 2]
        return before == "!" || before == "?"
    }

    static func splitSentences(_ block: String) -> [String] {
        let tokenizer = NLTokenizer(unit: .sentence)
        tokenizer.string = block
        var sentences: [String] = []
        tokenizer.enumerateTokens(in: block.startIndex..<block.endIndex) { range, _ in
            let sentence = collapseSpaces(String(block[range]))
            guard hasLettersOrDigits(sentence) else { return true }
            // The tokenizer splits "Stop!" she said. after the closing quote.
            // A piece that starts lowercase belongs to the sentence before it.
            if let first = sentence.first, first.isLowercase, !sentences.isEmpty {
                sentences[sentences.count - 1] += " " + sentence
            } else if let previous = sentences.last, endsWithQuotedExclamation(previous),
                      startsWithSpeechTag(sentence) {
                // "Wait!" Mara whispered. is one sentence, even with the capital.
                sentences[sentences.count - 1] += " " + sentence
            } else {
                sentences.append(sentence)
            }
            return true
        }
        return sentences
    }
}
