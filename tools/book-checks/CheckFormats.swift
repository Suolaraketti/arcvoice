import Foundation

/// Imports one file of every supported format. Files named `bad-*` must be
/// refused with a message; every other file must import. Exits 1 otherwise.
@main struct CheckFormats { static func main() {
var failed = 0
for path in CommandLine.arguments.dropFirst() {
    let url = URL(fileURLWithPath: path)
    let start = Date()
    do {
        let format = try BookFile.format(of: url, extensionHint: url.pathExtension)
        let book = try BookFile.extract(format, from: url)
        if url.lastPathComponent.hasPrefix("bad-") { failed += 1; print("  SHOULD HAVE BEEN REFUSED:", terminator: " ") }
        print("\(url.lastPathComponent): \(format) | title \(book.title ?? "-") | \(book.sentences.count) sentences, \(book.pageCount) pages, \(book.paragraphEnds.count) paragraphs | \(String(format: "%.1f", Date().timeIntervalSince(start)))s")
        for s in book.sentences.prefix(3) { print("    \(s.prefix(110))") }
    } catch {
        if !url.lastPathComponent.hasPrefix("bad-") { failed += 1; print("  FAILED:", terminator: " ") }
        print("\(url.lastPathComponent): refused: \(error.localizedDescription)")
    }
}
print(failed == 0 ? "Formats: all as expected" : "Formats: \(failed) not as expected")
exit(failed == 0 ? 0 : 1)
}}
