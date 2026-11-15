# ArcVoice

Reads books out loud (EPUB, PDF, Word, Kindle and more) with natural
narrator voices, running entirely on your own device. No account, no cloud, no per-character fees. Built for listening to
advance reader copies, which is why it works so hard at skipping the page
headers, page numbers and watermark lines that ARCs are full of.

| Platform | Where it lives | Built with |
| --- | --- | --- |
| iPhone | `apple/` | Swift, SwiftUI, PDFKit, and the shared voice engine |
| Mac (Apple Silicon) | `apple/` (same code, second target) | Swift, SwiftUI, PDFKit, and the shared voice engine |
| Windows | `desktop/` | Rust, egui, and the shared voice engine |

The voice engine is one piece of Rust code (`desktop/crates/core`) used by
every app: it prepares text for speech, works out pronunciations, runs the
voices (Kokoro, and Phonon as the optional "expressive" voices) and paces the
reading. The iPhone and Mac apps call it through
a small C interface (`desktop/crates/ffi`), so a fix to how a word is said
reaches every device at once.

## What it does

- Add a book and it is ready to read in a few seconds. EPUB is the usual
  download from BookFunnel and most ARC services; open one from Files, Mail
  or a browser with "Open in ArcVoice", or drop it on the window. A file
  still in iCloud Drive is downloaded first.
- iPhone and Mac read EPUB, PDF (scanned PDFs too, through on-device text
  recognition), Word (.docx), OpenDocument (.odt), DRM-free Kindle (.mobi,
  .azw3), FictionBook (.fb2), web pages, RTF, Markdown, plain text and photos
  of pages. What a file is comes from its contents, not its name. Files that
  can't be read (an Adobe .acsm ticket, a DRM-locked Kindle book) say what to
  do instead. Windows reads EPUB and PDF.
- Chapters: the reader shows the chapter being read, and a chapter list
  jumps to any chapter (iPhone and Mac). See "How chapters are found".
- A chapter's decorative first letter, which PDFs often draw as a picture or
  file out of order, is put back ("I throw", not "throw").
- Strips running headers, page numbers, watermark lines and table of contents
  rows before anything is read aloud.
- Starts speaking as soon as the first sentence is rendered and keeps a few
  sentences rendered ahead, so there are no gaps.
- Remembers the exact sentence for every book.
- Speed from 0.8x to 2x. 28 English voices, plus 13 optional expressive
  voices.
- iPhone: reads with the screen locked, lock screen and AirPods controls,
  and Shortcuts actions you can put on the Action Button.
- Mac: media keys, space bar to play and pause, drag a book onto the window.
- Windows: space bar and arrow keys, drag a book onto the window.

The voices download once on first launch (about 350 MB). The expressive
voices are a separate, optional download (about 70 MB). After that the app needs
no network.

## Build the iPhone and Mac apps

You need a Mac with Apple Silicon, Xcode 26 or newer, and Rust from
[rustup.rs](https://rustup.rs). The apps run on iOS 18 and macOS 15 or newer;
the Mac app needs Apple Silicon because ONNX Runtime ships for it only.

```sh
brew install xcodegen
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
cd apple
./build-core.sh        # builds the voice engine into Frameworks/
xcodegen
open ArcVoice.xcodeproj
```

Run `./build-core.sh` again after changing anything in `desktop/crates/core`.

The bundle identifier (`com.kaczmarowski.arcvoice`) and team are set in
`apple/project.yml`. To build under another account, change
`PRODUCT_BUNDLE_IDENTIFIER` and `DEVELOPMENT_TEAM` there and run `xcodegen`.


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
cargo run --release -p arcvoice-core --example speak -- <models dir> epub book.epub
cargo run --release -p arcvoice-core --example speak -- <models dir> text "Hello there."
```

The first two print the cleaned sentences of a book, which is the quickest way
to see how well the cleanup handles a particular file. The third writes
`speak.wav`.

The desktop code also builds and runs on a Mac, which is handy for testing.
The prebuilt ONNX Runtime needs a newer macOS target and clang's runtime
library, so build with:

```sh
export MACOSX_DEPLOYMENT_TARGET=14.0
export RUSTFLAGS="-C link-arg=$(clang --print-runtime-dir)/libclang_rt.osx.a -C link-arg=-framework -C link-arg=Accelerate"
cargo test --release -p arcvoice-core
```

## Reading positions and sync

Every app writes a small `position.json` per book in the same format. A book
is identified by the SHA-256 of the book file, so the same file is the same
book everywhere. The position stores the sentence index and also the sentence
text, because the Apple and Windows apps use different PDF readers and
sentence splitters, and may split a book into slightly different sentences. A device that sees a
different sentence count finds its place by searching for that text.

The Windows app can mirror positions into a folder you choose ("Set sync
folder"). Point it at a folder inside iCloud Drive or OneDrive. The Apple apps
do not read that folder yet; that is the next piece of work.

## Known limits

- DRM protected files cannot be opened. Many NetGalley and Edelweiss titles
  are protected or only download through their own apps, and some BookFunnel
  titles can only be read in BookFunnel's own app or cloud reader. ArcVoice
  says so rather than reading garbage.
- EPUB page counts are an estimate (275 words a page); EPUBs have no pages.
- An EPUB that has been unpacked into a folder (as Apple Books does on a Mac)
  can't be added; export or download it as a single `.epub` file.
- English only for now.
- Phonon voices sometimes guess rare or invented names differently from
  how a reader would ("de Bourgh", fantasy names). The default Kokoro
  voices use the pronunciation chain above and are steadier on those.
- Stronger narration models (Qwen3-TTS, Chatterbox, Ming, VoxCPM2) were
  tested in November 2026. Qwen3-TTS 1.7B was clearly more expressive, but
  every one of them renders slower than real time even on a Mac's GPU, and
  iOS allows no GPU work with the screen locked, so none can read live on a
  phone yet.
- Invented names are pronounced by a neural model's best guess. It is
  usually what a reader would say, but an unusual name can come out wrong.
- British voices use American pronunciations for words outside the main
  dictionary (mostly names).
- The Windows app is behind the iPhone and Mac apps on reading books in: it
  reads EPUB and PDF only, has no chapter list, and does not yet use type
  sizes, numbered chapter headings or decorative first letters. The cleanup
  rules below were kept in step until November 2026; the Apple versions now
  go further.
- PDF text extraction on Windows uses a pure Rust library. It handles
  ordinary book layouts well but is less forgiving than Apple's PDFKit with
  unusual files.

## How chapters are found

On iPhone and Mac (`apple/ArcVoice/Models/Chapter.swift`, and the readers in
`Services/`), from the most trustworthy source a book has:

1. **An EPUB's own table of contents** (the EPUB 3 navigation document or the
   EPUB 2 `toc.ncx`). Each entry is mapped to the exact sentence it links to,
   including anchors inside a file that holds several chapters.
2. **A PDF's bookmarks**, which publishing tools write from the real chapter
   list.
3. **Type size.** A PDF without bookmarks still records the size and weight
   of every character, and chapter titles are set larger (or bold). Lines
   clearly larger than the body text are the headings.
4. **Wording**, for files with none of that: "Chapter 6", "PART TWO", a
   number heading a page that is not the page number, short title lines at
   the top of a page after a page that stopped short.

Headings that follow each other make one chapter ("Part II · The Capture ·
Welcome to hell · Ryker"). Printed contents pages are left out of both the
chapter list and the reading.

### Checking it

`tools/book-checks/run.sh` checks all of this against a shelf of real
public-domain books, compiled from the app's own code. It downloads them once
(about 30 MB, not committed), typesets test PDFs like ARCs in seven
chapter-heading styles, and reports:

- EPUBs (25 books, three packagings): every chapter must start on its title.
- PDFs (14 typeset books): at least 97% of chapters found, at most one false
  alarm per 50 chapters. Currently 99%.
- Every format: one file of each must import, and bad files must be refused.

Run it after changing anything in the readers. Books of your own (ARCs) go in
`tools/book-checks/private/`, which is never committed;
`run.sh mine` prints their chapter lists to check by eye.

## How the voice reads

Everything here is in `desktop/crates/core` and shared by every app.

1. **Text for speech** (`speech_text.rs`). Clock times, dates, percentages,
   decades ("the 1990s"), Roman numerals ("Chapter IV", "Henry VIII"), web
   and email addresses, "St." and dashes glued to punctuation are rewritten
   into words. The screen still shows the original text.
2. **Pronunciation.** misaki-rs, a byte-for-byte port of the phonemizer
   Kokoro was trained with, handles ordinary English, including heteronyms
   ("she read it" against "she will read it"), numbers and contractions.
   Words it does not know go to `pronounce.rs`: a short list of book-world
   words, then the CMU Pronouncing Dictionary (135,000 words, real names
   among them), then possessives, plurals, prefixes and compounds built from
   known words ("BookFunnel", "ebook", "Netherfield"), and finally a small
   neural model (`g2p_nn.rs`) for invented names.
3. **The voice** (`tts.rs`, `phonon.rs`, `narrator.rs`). The default voices
   are Kokoro v1.0 on ONNX Runtime, on the CPU (iOS does not allow the GPU in
   the background, and the Neural Engine cannot run Kokoro). In listening
   tests they read fiction the most naturally and accurately. The optional expressive voices are
   Phonon 31m (Gradium, built on Kyutai's Pocket TTS and Mimi codec), run
   with the `ptts` crate on the CPU with 8-bit weights. It reads plain text,
   so step 2 is only used by the Kokoro voices. On some passages its
   narrators move their pitch more than Kokoro, but in listening it was not
   clearly better. Each sentence gets a fixed random seed from its text, so
   going back replays the same take.
4. **Pacing** (`pacing.rs`). Each sentence's own silence is trimmed to a fixed
   margin and a deliberate pause added: about 0.4 s between sentences, 0.75 s
   at the end of a paragraph, longer after a heading, all shorter at higher
   speeds. Each sentence is levelled toward the same loudness so voices and
   takes match. Speed changes the voice's own pace rather than stretching
   audio, up to 1.3x for Phonon; beyond that the rest is made up with a
   pitch-preserving time-stretch (`stretch.rs`, WSOLA).

Measured on 20 sentences of typical fiction with times, numbers, names and
dialogue, transcribed back by Whisper: the previous engine had a 36% word
error rate and went silent on 4 sentences; this one has 7% and none silent,
most of the remainder being how Whisper writes numbers. Phonon Harper,
through the same pipeline, scores 5.6%.

Voices were chosen on a fiction passage with dialogue, scored for
naturalness (UTMOS, out of 5), pitch movement and word error rate:

| Voice | Naturalness | Pitch movement (sd) | Word errors |
| --- | --- | --- | --- |
| Kokoro Heart (default) | 4.52 | 2.85 st | 2.2% |
| Phonon Harper (expressive) | 4.46 | 3.48 st | 4.7% |
| Phonon Maeve | 4.45 | 4.29 st | 3.2% |
| Phonon Hugh | 4.39 | 4.51 st | 4.3% |
| Phonon Spencer | 4.32 | 3.78 st | 1.8% |

At higher speeds (naturalness / word errors): Harper 4.24 / 4.0% at 1.5x and
3.58 / 4.3% at 2x; Kokoro 4.30 / 5.0% and 4.09 / 10.1%. Phonon renders about
25 to 35 times faster than real time on two cores of an M3 and is ready in
under half a second.

## How the cleanup works

Both `apple/ArcVoice/Services/TextCleaner.swift` and
`desktop/crates/core/src/text.rs` implement the same rules. Keep them in step.

For PDFs:

1. A short line whose shape (lowercased, digits replaced by `#`) appears on
   40% or more of the pages is a watermark or global header.
2. A short line in the first or last two lines of a page, with no sentence
   ending, that sits in that same zone on three or more pages is a running
   header or footer.
3. A bare page number at the top or bottom of a page is dropped.
4. Lines are rejoined into running text across page breaks, mending words
   hyphenated at a line end, and chapter headings are kept as their own beat.

Paragraph ends are kept so the voice can pause there: an EPUB marks them, and
in a PDF the last line of a paragraph ends a sentence well short of the
margin.

For EPUBs, read by `apple/ArcVoice/Services/EPUBTextExtractor.swift` and
`desktop/crates/core/src/epub.rs` (also kept in step):

1. Chapters are read in the book's spine order. The table of contents page,
   items marked as outside the reading order, and images are skipped.
2. The markup says where paragraphs and headings are, so nothing is guessed.
   Footnote markers, footnotes and page break markers are left out, and HTML
   entities are decoded.
3. A short paragraph in the first or last three of a chapter, whose shape
   sits in that zone in half or more of the chapters, is a stamped watermark
   ("This copy is licensed to ...") and is dropped. Headings are matched on
   their exact text, so "Chapter 1" and "Chapter 2" are both kept.
4. Files with Adobe or Apple DRM are refused with a clear message.

## Credits and licenses

ArcVoice is MIT licensed. See `LICENSE`. It stands on other people's work;
see `THIRD_PARTY.md` for the list and their licenses.
