import Foundation

/// Prints each book's chapter list with the sentence each starts at and the
/// first words after its title, for checking a book by eye.
@main struct Chap { static func main() {
for path in CommandLine.arguments.dropFirst() {
    let url = URL(fileURLWithPath: path)
    if url.lastPathComponent == "README.md" { continue }
    guard let format = try? BookFile.format(of: url, extensionHint: url.pathExtension),
          let book = try? BookFile.extract(format, from: url)
    else { print("== \(url.lastPathComponent): could not be read"); continue }
    print("== \(url.lastPathComponent): \(book.chapters.count) chapters, \(book.sentences.count) sentences")
    for c in book.chapters { print("  \(c.start)\t\(c.title.prefix(70))  ->  \(book.sentences[min(c.start + c.title.components(separatedBy: " \u{00B7} ").count, book.sentences.count - 1)].prefix(50))") }
}
}}
