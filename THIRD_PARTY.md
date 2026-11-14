# Third-party work

ArcVoice does not include any of these in its source tree, apart from the
data files noted below. They are fetched at build time or downloaded on first
launch.

| Component | Used by | License |
| --- | --- | --- |
| [Kokoro-82M](https://huggingface.co/hexgrad/Kokoro-82M) voice model and voices | all | Apache-2.0 |
| [kokoro-onnx](https://github.com/thewh1teagle/kokoro-onnx) (ONNX export of the model, hosted model files) | all | MIT |
| [ONNX Runtime](https://github.com/microsoft/onnxruntime) | all | MIT |
| [ort](https://github.com/pykeio/ort) | all | MIT or Apache-2.0 |
| [misaki-rs](https://github.com/MicheleYin/misaki-rs) (phonemizer, built without espeak) | all | MIT |
| [CMU Pronouncing Dictionary](https://github.com/cmusphinx/cmudict) | all | BSD-2-Clause style, see below |
| [g2p_en](https://github.com/Kyubyong/g2p) (neural letter-to-sound model weights) | all | Apache-2.0 |
| [pdf-extract](https://github.com/jrmuizel/pdf-extract) and [lopdf](https://github.com/J-F-Liu/lopdf) | Windows | MIT |
| [egui and eframe](https://github.com/emilk/egui) | Windows | MIT or Apache-2.0 |
| [rodio](https://github.com/RustAudio/rodio) | Windows | MIT or Apache-2.0 |
| [rfd](https://github.com/PolyMeilex/rfd) | Windows | MIT |
| [ureq](https://github.com/algesten/ureq) | Windows | MIT or Apache-2.0 |

Data files in `desktop/crates/core/assets/`:

- `kokoro_vocab.json` is the phoneme-to-id table from Kokoro's published
  model configuration (Apache-2.0).
- `cmudict.dict` is the CMU Pronouncing Dictionary, copyright Carnegie Mellon
  University; its license is in `cmudict.LICENSE` next to it.
- `g2p_en.npz` holds the trained weights of g2p_en by Kyubyong Park and
  Jongseok Kim, released under the Apache License 2.0
  (https://www.apache.org/licenses/LICENSE-2.0). They are unchanged.

espeak-ng, the usual fallback phonemizer for Kokoro, is GPL-3.0 and is
deliberately not used anywhere in this project.
