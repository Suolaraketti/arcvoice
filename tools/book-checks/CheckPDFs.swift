import Foundation

/// Imports each typeset PDF and compares its chapters with the headings it was
/// typeset with (the `.headings` file next to it).
func words(_ s: String) -> String {
    s.lowercased().components(separatedBy: CharacterSet.alphanumerics.inverted).filter { !$0.isEmpty }.joined(separator: " ")
}
@main struct Score { static func main() {
    let verbose = ProcessInfo.processInfo.environment["VERBOSE"] != nil
    var totalTruth = 0, totalFound = 0, totalDetected = 0
    for path in CommandLine.arguments.dropFirst() {
        let url = URL(fileURLWithPath: path)
        let truth = (try! String(contentsOfFile: path + ".headings", encoding: .utf8)).split(separator: "\n").map { line -> String in
            let parts = line.components(separatedBy: " | ")
            // The distinctive line: the title when there is one, else the only line.
            return words(parts.count >= 2 ? parts[1] : parts[0])
        }
        let t0 = Date()
        let book = try! BookFile.extract(.pdf, from: url)
        let detected = book.chapters.filter { $0.title != "Opening pages" }
        var used = Set<Int>(), found = 0, missed: [String] = []
        for t in truth {
            if let i = detected.indices.first(where: { !used.contains($0) && words(detected[$0].title).contains(t) }) { used.insert(i); found += 1 }
            else { missed.append(t) }
        }
        let extra = detected.indices.filter { !used.contains($0) }.map { detected[$0].title }
        totalTruth += truth.count; totalFound += found; totalDetected += detected.count
        print(String(format: "%-48@ found %3d/%3d  false %3d  %.1fs", url.lastPathComponent as NSString, found, truth.count, extra.count, Date().timeIntervalSince(t0)))
        if verbose || !missed.isEmpty || !extra.isEmpty {
            for m in missed.prefix(verbose ? 99 : 3) { print("      missed:", m.prefix(50)) }
            for e in extra.prefix(verbose ? 99 : 3) { print("      false: ", e.prefix(50)) }
        }
    }
    let recall = Double(totalFound) / Double(max(1, totalTruth))
    let falseAlarms = totalDetected - totalFound
    print(String(format: "PDF chapters found %d of %d (%.1f%%), false alarms %d", totalFound, totalTruth, 100 * recall, falseAlarms))
    // The bar: at least 97% found, at most one false alarm per 50 chapters.
    exit(recall >= 0.97 && falseAlarms * 50 <= totalTruth ? 0 : 1)
}}
