import SwiftUI

@main
struct ArcVoiceApp: App {
    @StateObject private var model = AppModel.shared

    var body: some Scene {
        WindowGroup {
            LibraryView()
                .environmentObject(model)
                .environmentObject(model.library)
                .environmentObject(model.player)
                // A PDF or EPUB sent here from Files, Mail or Safari with "Open in".
                .onOpenURL { url in
                    Task { await model.importBook(from: url) }
                }
                // Start loading the voice right away so it is ready by the
                // time she has picked a book.
                .task { await model.prepareVoice() }
        }
    }
}
