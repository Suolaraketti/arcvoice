import Foundation
import CoreText
import CoreGraphics
import AppKit

/// Typesets an EPUB as a novel PDF in one of several chapter-heading styles,
/// with running headers and page numbers and no bookmarks, and writes the
/// heading each chapter got to a sidecar file, one per line.
let numberWords = ["Zero","One","Two","Three","Four","Five","Six","Seven","Eight","Nine","Ten","Eleven","Twelve","Thirteen","Fourteen","Fifteen","Sixteen","Seventeen","Eighteen","Nineteen","Twenty"]
func spelled(_ n: Int) -> String {
    if n < numberWords.count { return numberWords[n] }
    let tens = ["","","Twenty","Thirty","Forty","Fifty","Sixty","Seventy","Eighty","Ninety"]
    if n < 100 { return tens[n / 10] + (n % 10 == 0 ? "" : "-" + numberWords[n % 10]) }
    return String(n)
}
func roman(_ n: Int) -> String {
    let table: [(Int, String)] = [(100,"C"),(90,"XC"),(50,"L"),(40,"XL"),(10,"X"),(9,"IX"),(5,"V"),(4,"IV"),(1,"I")]
    var n = n, out = ""; for (v, s) in table { while n >= v { out += s; n -= v } }; return out
}
/// The part of a TOC title after "Chapter N." ("Chapter IV. The Sea-Cook" -> "The Sea-Cook").
func bareTitle(_ title: String) -> String? {
    let pattern = "^(chapter|letter|stave|book|part)?\\s*[0-9ivxlcdm]+\\b\\s*[.:—-]?\\s*"
    let stripped = title.replacingOccurrences(of: pattern, with: "", options: [.regularExpression, .caseInsensitive])
    let t = stripped.trimmingCharacters(in: .whitespaces)
    return t.isEmpty || t.count > 50 ? nil : t
}

@main struct Gen { static func main() throws {
    let args = CommandLine.arguments
    let (epub, style, out) = (args[1], args[2], args[3])
    let book = try BookFile.extract(.epub, from: URL(fileURLWithPath: epub))
    let title = book.title ?? "Untitled"
    let chapters = book.chapters
    // Paragraphs of each section.
    var ends = Set(book.paragraphEnds)
    func paragraphs(_ range: Range<Int>) -> [String] {
        var out: [String] = [], cur: [String] = []
        for i in range { cur.append(book.sentences[i]); if ends.contains(i) { out.append(cur.joined(separator: " ")); cur = [] } }
        if !cur.isEmpty { out.append(cur.joined(separator: " ")) }
        return out
    }
    _ = ends.insert(-1)
    let pageW: CGFloat = 396, pageH: CGFloat = 612, margin: CGFloat = 54
    var mediaBox = CGRect(x: 0, y: 0, width: pageW, height: pageH)
    let ctx = CGContext(URL(fileURLWithPath: out) as CFURL, mediaBox: &mediaBox, nil)!
    let body = NSFont(name: "Georgia", size: 10.5)!, small = NSFont(name: "Georgia", size: 8)!
    let para = NSMutableParagraphStyle(); para.alignment = .justified; para.firstLineHeadIndent = 16; para.lineSpacing = 2
    let center = NSMutableParagraphStyle(); center.alignment = .center; center.paragraphSpacing = 6
    var pageNumber = 0
    var headings: [String] = []
    func header() {
        pageNumber += 1
        let text = pageNumber % 2 == 0 ? "A. N. Author" : title.uppercased()
        for (t, y) in [(text, pageH - 36), (String(pageNumber), CGFloat(28))] {
            let line = CTLineCreateWithAttributedString(NSAttributedString(string: t, attributes: [.font: small]))
            let w = CTLineGetTypographicBounds(line, nil, nil, nil)
            ctx.textPosition = CGPoint(x: (pageW - w) / 2, y: y); CTLineDraw(line, ctx)
        }
    }
    var chapterNumber = 0
    var licence = false
    for (index, chapter) in chapters.enumerated() {
        let end = index + 1 < chapters.count ? chapters[index + 1].start : book.sentences.count
        var paras = paragraphs(chapter.start..<end)
        // Project Gutenberg's licence is not part of the book.
        if licence { break }
        if let cut = paras.firstIndex(where: { $0.contains("*** END OF") }) { paras = Array(paras[..<cut]); licence = true }
        if paras.isEmpty { continue }
        let isFront = chapter.title == "Opening pages"
        // Drop the EPUB's own heading lines; the style sets the heading.
        let headingCount = isFront ? 0 : chapter.title.components(separatedBy: " \u{00B7} ").count
        paras.removeFirst(min(headingCount, max(0, paras.count - 1)))
        var lines: [String] = []
        if !isFront {
            chapterNumber += 1
            let t = bareTitle(chapter.title.components(separatedBy: " \u{00B7} ").last ?? "")
            switch style {
            case "chapter-title": lines = ["Chapter \(chapterNumber)"] + (t.map { [$0] } ?? [])
            case "number-title-pov": lines = [String(chapterNumber)] + (t.map { [$0] } ?? ["Untitled"]) + [chapterNumber % 2 == 0 ? "Ryker" : "Rhea"]
            case "caps-words": lines = ["CHAPTER \(spelled(chapterNumber).uppercased())"]
            case "roman": lines = [roman(chapterNumber)]
            case "title-only": lines = [t ?? "Part \(spelled(chapterNumber))"]
            case "number-only": lines = [String(chapterNumber)]
            case "number-pov": lines = [String(chapterNumber), chapterNumber % 3 == 0 ? "Ryker" : "Rhea"]
            default: fatalError("style")
            }
            // The scorer looks for the distinctive line: for a bare number, "Chapter N".
            headings.append(style.hasPrefix("number-") && style != "number-title-pov" ? "Chapter \(chapterNumber)" : lines.joined(separator: " | "))
        }
        let text = NSMutableAttributedString()
        if !lines.isEmpty {
            text.append(NSAttributedString(string: "\n\n\n" + lines.joined(separator: "\n") + "\n\n", attributes: [.font: NSFont(name: "Georgia", size: 16)!, .paragraphStyle: center]))
        }
        text.append(NSAttributedString(string: paras.joined(separator: "\n"), attributes: [.font: body, .paragraphStyle: para]))
        let setter = CTFramesetterCreateWithAttributedString(text)
        var position = 0
        while position < text.length {
            ctx.beginPDFPage(nil); header()
            let path = CGPath(rect: CGRect(x: margin, y: margin, width: pageW - 2 * margin, height: pageH - 2 * margin - 6), transform: nil)
            let frame = CTFramesetterCreateFrame(setter, CFRange(location: position, length: 0), path, nil)
            CTFrameDraw(frame, ctx)
            let visible = CTFrameGetVisibleStringRange(frame)
            ctx.endPDFPage()
            if visible.length == 0 { break }
            position += visible.length
        }
    }
    ctx.closePDF()
    try headings.joined(separator: "\n").write(toFile: out + ".headings", atomically: true, encoding: .utf8)
}}
