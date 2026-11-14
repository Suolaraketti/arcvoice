# Third-party work

ArcVoice does not include any of these in its source tree, apart from the
small vocabulary table noted below. They are fetched at build time or
downloaded on first launch.

| Component | Used by | License |
| --- | --- | --- |
| [Kokoro-82M](https://huggingface.co/hexgrad/Kokoro-82M) voice model and voices | all | Apache-2.0 |
| [speech-swift](https://github.com/soniqo/speech-swift) (Kokoro on CoreML, phonemizer) | iPhone, Mac | Apache-2.0 |
| [kokoro-onnx](https://github.com/thewh1teagle/kokoro-onnx) (ONNX export of the model, hosted model files) | Windows | MIT |
| [ONNX Runtime](https://github.com/microsoft/onnxruntime) | Windows | MIT |
| [ort](https://github.com/pykeio/ort) | Windows | MIT or Apache-2.0 |
| [misaki-rs](https://github.com/MicheleYin/misaki-rs) (phonemizer, built without espeak) | Windows | MIT |
| [pdf-extract](https://github.com/jrmuizel/pdf-extract) and [lopdf](https://github.com/J-F-Liu/lopdf) | Windows | MIT |
| [egui and eframe](https://github.com/emilk/egui) | Windows | MIT or Apache-2.0 |
| [rodio](https://github.com/RustAudio/rodio) | Windows | MIT or Apache-2.0 |
| [rfd](https://github.com/PolyMeilex/rfd) | Windows | MIT |
| [ureq](https://github.com/algesten/ureq) | Windows | MIT or Apache-2.0 |

`desktop/crates/core/assets/kokoro_vocab.json` is the phoneme-to-id table from
Kokoro's published model configuration (Apache-2.0).

espeak-ng, the usual fallback phonemizer for Kokoro, is GPL-3.0 and is
deliberately not used anywhere in this project.
