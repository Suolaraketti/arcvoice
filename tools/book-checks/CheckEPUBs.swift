import Foundation

/// Imports each EPUB and checks its chapter list: every chapter must start on
/// its own title. Front-matter entries with no printed title (a dedication,
/// an epigraph) are allowed, and so is one in a hundred (an untitled
/// preface). Exits 1 beyond that.
func words(_ s: String) -> [String] {
    s.lowercased().components(separatedBy: CharacterSet.alphanumerics.inverted).filter { !$0.isEmpty }
}

let untitledParts: Set<String> = ["titlepage", "dedication", "epigraph", "imprint", "colophon", "uncopyright", "half title"]

@main struct CheckEPUBs {
    static func main() {
        var chapters = 0, off = 0
        for path in CommandLine.arguments.dropFirst() {
            let url = URL(fileURLWithPath: path)
            do {
                let book = try BookFile.extract(try BookFile.format(of: url, extensionHint: url.pathExtension), from: url)
                var misses: [String] = []
                for chapter in book.chapters where chapter.title != "Opening pages" {
                    chapters += 1
                    if untitledParts.contains(chapter.title.lowercased()) { continue }
                    let nearby = words(book.sentences[chapter.start..<min(book.sentences.count, chapter.start + 3)]
                        .joined(separator: " ")).joined(separator: " ")
                    let starts = chapter.title.components(separatedBy: " \u{00B7} ").contains { part in
                        let w = words(part).prefix(3)
                        return !w.isEmpty && nearby.contains(w.joined(separator: " "))
                    }
                    if !starts { misses.append("\(chapter.title.prefix(40)) -> \(book.sentences[chapter.start].prefix(50))") }
                }
                off += misses.count
                print(String(format: "  %-44@ %3d chapters %@", url.lastPathComponent as NSString, book.chapters.count,
                             misses.isEmpty ? "" : "\(misses.count) OFF"))
                for miss in misses.prefix(3) { print("      ", miss) }
            } catch {
                off += 1
                print("  \(url.lastPathComponent): ERROR \(error.localizedDescription)")
            }
        }
        print("EPUB chapters: \(chapters), starting off their title: \(off)")
        exit(off * 100 <= chapters ? 0 : 1)
    }
}
