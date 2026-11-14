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

    /// - Parameter pages: the raw text of each page, in reading order.
    /// - Returns: sentences in reading order, with page furniture removed.
    static func sentences(fromPages pages: [String]) -> [String] {
        let pageLines = pages.map { lines(in: $0) }
        let stripped = removeRepeatedLines(pageLines)
        let blocks = joinIntoBlocks(stripped)
        var result: [String] = []
        for block in blocks {
            result.append(contentsOf: splitSentences(block))
        }
        return result
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
    static func removeRepeatedLines(_ pageLines: [[String]]) -> [[String]] {
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

        return pageLines.map { lines in
            let count = lines.count
            var kept: [String] = []
            kept.reserveCapacity(count)
            for (index, line) in lines.enumerated() {
                // Ornaments like "* * *" have nothing to say out loud.
                guard hasLettersOrDigits(line), !isContentsEntry(line) else { continue }
                let isEdge = index < 2 || index >= count - 2
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
    static func joinIntoBlocks(_ pages: [[String]]) -> [String] {
        var blocks: [String] = []
        var current = ""

        func flush() {
            let trimmed = current.trimmingCharacters(in: .whitespaces)
            if !trimmed.isEmpty { blocks.append(trimmed) }
            current = ""
        }

        for lines in pages {
            for line in lines {
                if isHeading(line) {
                    flush()
                    // "CHAPTER ONE" reads more naturally as "Chapter One".
                    let letters = line.filter { $0.isLetter }
                    let allCaps = !letters.isEmpty && letters.allSatisfy { $0.isUppercase }
                    blocks.append(allCaps ? line.capitalized : line)
                    continue
                }
                if current.isEmpty {
                    current = line
                    continue
                }
                // A word broken across lines: "some-" + "thing" -> "something".
                if let last = current.last, last == "-" || last == "\u{2010}",
                   let first = line.first, first.isLowercase,
                   let before = current.dropLast().last, before.isLetter {
                    current.removeLast()
                    current += line
                } else {
                    current += " " + line
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
