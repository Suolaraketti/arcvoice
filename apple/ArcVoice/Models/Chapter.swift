import Foundation

/// A chapter (or part) of a book: its title and the sentence it starts at.
/// Saved as `chapters.json` next to the book's sentences.
struct Chapter: Codable, Hashable, Sendable {
    var title: String
    /// Index of the chapter's first sentence: its heading.
    var start: Int

    /// Chapters from the headings found when the book was read in. Headings
    /// that follow each other belong to one chapter ("Part II", "The
    /// Capture", "Welcome to hell", "Ryker" is one entry). Text before the
    /// first heading (title page, copyright) is "Opening pages". A book with
    /// fewer than two headings, or so many that they can't be chapters, has
    /// no chapter list.
    /// - Parameter pages: for a PDF, each sentence's page, so headings on
    ///   different pages ("Playlist" and then "Prologue") stay apart.
    static func list(sentences: [String], headings: [Int], pages: [Int]? = nil) -> [Chapter] {
        var chapters: [Chapter] = []
        var previous: Int?
        for index in headings where sentences.indices.contains(index) {
            let text = displayTitle(sentences[index].trimmingCharacters(in: .whitespaces))
            let samePage = pages.map { pages in
                previous.map { pages.indices.contains($0) && pages.indices.contains(index) && pages[$0] == pages[index] } ?? false
            } ?? true
            if let previous, index == previous + 1, samePage, var last = chapters.popLast() {
                if last.title.count < 80 { last.title += " \u{00B7} " + text }
                chapters.append(last)
            } else {
                chapters.append(Chapter(title: text, start: index))
            }
            previous = index
        }
        guard chapters.count >= 2, chapters.count * 8 <= sentences.count else { return [] }
        // A chapter headed only by its number reads "Chapter 23".
        // ("Chapter 23 · Rhea" when a narrator's name follows.)
        for index in chapters.indices {
            let first = chapters[index].title.components(separatedBy: " \u{00B7} ")[0]
            if Int(first) != nil || ((TextCleaner.isRomanNumeral(first) || first == "I") && first.count <= 7) {
                chapters[index].title = "Chapter " + chapters[index].title
            }
        }
        if let first = chapters.first, first.start > 0 {
            chapters.insert(Chapter(title: "Opening pages", start: 0), at: 0)
        }
        return chapters
    }

    /// Chapters from a book's own table of contents (or a PDF's bookmarks):
    /// titles with the sentence each starts at, in the book's order.
    /// Entries that land on the same sentence are one chapter ("Part One ·
    /// Chapter 1"); an entry that would go backwards is left out.
    static func fromContents(_ entries: [(title: String, start: Int)], sentenceCount: Int) -> [Chapter] {
        var chapters: [Chapter] = []
        for entry in entries {
            let title = displayTitle(TextCleaner.collapseSpaces(entry.title))
            guard !title.isEmpty, entry.start < sentenceCount else { continue }
            if var last = chapters.last, entry.start <= last.start {
                if entry.start == last.start, !last.title.contains(title), last.title.count < 80 {
                    last.title += " \u{00B7} " + title
                    chapters[chapters.count - 1] = last
                }
                continue
            }
            chapters.append(Chapter(title: title, start: entry.start))
        }
        if let first = chapters.first, first.start > 0 {
            chapters.insert(Chapter(title: "Opening pages", start: 0), at: 0)
        }
        return chapters
    }

    /// For books read in before chapters were saved: a heading is a
    /// one-sentence paragraph that looks like one ("Chapter 3", "PROLOGUE").
    static func guessed(sentences: [String], paragraphEnds: Set<Int>) -> [Chapter] {
        let headings = sentences.indices.filter { index in
            paragraphEnds.contains(index) && (index == 0 || paragraphEnds.contains(index - 1))
                && TextCleaner.isHeading(sentences[index])
        }
        return list(sentences: sentences, headings: headings)
    }

    /// Headings set in small capitals come out of some PDFs as "THE
    /// cAPTURE" or "PART Iv". Words that are mostly capitals read better as
    /// "The Capture" and "Part IV"; Roman numerals stay in capitals.
    static func displayTitle(_ title: String) -> String {
        let numbered: Set<String> = ["part", "book", "chapter", "volume", "act"]
        var previous = ""
        return title.split(separator: " ", omittingEmptySubsequences: false).map { word -> String in
            defer { previous = word.lowercased() }
            let letters = word.filter(\.isLetter)
            let capitals = letters.filter(\.isUppercase).count
            // Initials ("N.S.") and short words are left alone.
            guard letters.count >= 2, !word.contains(".") else { return String(word) }
            // "Part Iv" is Part IV.
            if numbered.contains(previous), TextCleaner.isRomanNumeral(String(letters).uppercased()) {
                return word.uppercased()
            }
            // Mostly capitals ("TREATy"), or a capital after a small letter ("cAsTlE").
            let mixed = word.allSatisfy(\.isLetter) && (letters.first?.isLowercase ?? false) && capitals > 0
            guard mixed || (capitals >= letters.count - 1 && letters.count > 2) else { return String(word) }
            if TextCleaner.isRomanNumeral(String(letters).uppercased()) { return word.uppercased() }
            return word.prefix(1).uppercased() + word.dropFirst().lowercased()
        }.joined(separator: " ")
    }

    /// The chapter that contains sentence `index`, as a position in `chapters`.
    static func index(containing index: Int, in chapters: [Chapter]) -> Int? {
        chapters.lastIndex { $0.start <= index }
    }
}
