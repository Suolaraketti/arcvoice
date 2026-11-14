# ArcVoice

Reads PDFs out loud with the Kokoro voice, running entirely on your own
device. No account, no cloud, no per-character fees. Built for listening to
advance reader copies, which is why it works so hard at skipping the page
headers, page numbers and watermark lines that ARCs are full of.

| Platform | Where it lives | Built with |
| --- | --- | --- |
| iPhone | `apple/` | Swift, SwiftUI, PDFKit, Kokoro on CoreML |
| Mac | `apple/` (same code, second target) | Swift, SwiftUI, PDFKit, Kokoro on CoreML |
| Windows | `desktop/` | Rust, egui, Kokoro on ONNX Runtime |

## What it does

- Add a PDF and it is ready to read in a few seconds.
- Strips running headers, page numbers, watermark lines and table of contents
  rows before anything is read aloud.
- Starts speaking as soon as the first sentence is rendered and keeps a few
  sentences rendered ahead, so there are no gaps.
- Remembers the exact sentence for every book.
- Speed from 0.8x to 2x and 28 English voices.
- iPhone: reads with the screen locked, lock screen and AirPods controls,
  and Shortcuts actions you can put on the Action Button.
- Mac: media keys, space bar to play and pause, drag a PDF onto the window.
- Windows: space bar and arrow keys, drag a PDF onto the window.

The voice model downloads once on first launch (about 170 MB on Apple
devices, about 350 MB on Windows). After that the app needs no network.

## Build the iPhone and Mac apps

You need a Mac with Xcode 26 or newer (the speech package uses the iOS 26 SDK). The apps themselves run on iOS 18 and macOS 15 or newer.

```sh
brew install xcodegen
cd apple
xcodegen
open ArcVoice.xcodeproj
```

In Xcode, select each target (ArcVoice for iPhone, ArcVoiceMac for Mac), open
Signing & Capabilities, pick your team, and change the bundle identifier from
`com.example.arcvoice` to one of your own. Change `PRODUCT_BUNDLE_IDENTIFIER`
in `apple/project.yml` too so it survives the next `xcodegen`.

The first build is slow because Xcode fetches the speech-swift package. Only
its Kokoro module is compiled into the app.

### Action Button

Settings, Action Button, swipe to Shortcut, Choose a Shortcut, then pick
ArcVoice and "Play or Pause". One press resumes the last book where it
stopped, the next press pauses it.

## Build the Windows app

You need Rust from [rustup.rs](https://rustup.rs).

```sh
cd desktop
cargo build --release -p arcvoice
```

The app is `desktop/target/release/arcvoice.exe`. Every push also builds it on
GitHub; download it from the latest "Windows app" run under Actions.

To check the pipeline without opening a window:

```sh
cargo run --release -p arcvoice-core --example speak -- <models dir> pdf book.pdf
cargo run --release -p arcvoice-core --example speak -- <models dir> text "Hello there."
```

The first prints the cleaned sentences of a PDF, which is the quickest way to
see how well the cleanup handles a particular book. The second writes
`speak.wav`.

## Reading positions and sync

Every app writes a small `position.json` per book in the same format. A book
is identified by the SHA-256 of the PDF file, so the same file is the same
book everywhere. The position stores the sentence index and also the sentence
text, because the Apple and Windows apps use different PDF readers and may
split a book into slightly different sentences. A device that sees a
different sentence count finds its place by searching for that text.

The Windows app can mirror positions into a folder you choose ("Set sync
folder"). Point it at a folder inside iCloud Drive or OneDrive. The Apple apps
do not read that folder yet; that is the next piece of work.

## Known limits

- Scanned PDFs with no text layer are not supported (that needs OCR).
- DRM protected files cannot be opened. Many NetGalley and Edelweiss titles
  are protected or only download through their own apps.
- English only for now.
- On Windows, a word missing from the pronunciation dictionary (an invented
  name, say) is sounded out by a simple rule table. It will be understandable
  but not always right. The Apple apps use a small neural model for this.
- PDF text extraction on Windows uses a pure Rust library. It handles
  ordinary book layouts well but is less forgiving than Apple's PDFKit with
  unusual files.

## How the cleanup works

Both `apple/ArcVoice/Services/TextCleaner.swift` and
`desktop/crates/core/src/text.rs` implement the same rules. Keep them in step.

1. A short line whose shape (lowercased, digits replaced by `#`) appears on
   40% or more of the pages is a watermark or global header.
2. A short line in the first or last two lines of a page, with no sentence
   ending, that sits in that same zone on three or more pages is a running
   header or footer.
3. A bare page number at the top or bottom of a page is dropped.
4. Lines are rejoined into running text across page breaks, mending words
   hyphenated at a line end, and chapter headings are kept as their own beat.

## Credits and licenses

ArcVoice is MIT licensed. See `LICENSE`. It stands on other people's work;
see `THIRD_PARTY.md` for the list and their licenses.
