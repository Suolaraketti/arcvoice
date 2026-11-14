// The ArcVoice engine, as used by the iPhone and Mac apps.
//
// This is the same Rust engine the Windows app uses: text preparation, the
// voices (Kokoro, the default, and Phonon's optional expressive voices),
// pronunciation and sentence pacing. One engine means a fix lands on every
// platform at once.
//
// An engine is not thread safe: call it from one thread or actor at a time.
// Strings and sample buffers handed out must be freed with the matching
// arcvoice_*_free function.

#ifndef ARCVOICE_H
#define ARCVOICE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef struct ArcVoiceEngine ArcVoiceEngine;

/// Kokoro's output rate: 24 kHz mono.
uint32_t arcvoice_sample_rate(void);

/// Loads the voices. `phonon_dir` is a Phonon checkpoint folder; the two
/// Kokoro paths are its model and voices files. Either engine may be NULL,
/// but not both. `threads` 0 lets the engine choose. Returns NULL on failure
/// and sets `*error` to a message.
ArcVoiceEngine *arcvoice_engine_open(const char *phonon_dir, const char *kokoro_model, const char *kokoro_voices,
                                     uint32_t threads, char **error);

/// Adds Phonon's expressive voices to an open engine, once downloaded.
bool arcvoice_engine_add_phonon(ArcVoiceEngine *engine, const char *phonon_dir, uint32_t threads, char **error);

/// Adds Kokoro's voices to an open engine, once downloaded.
bool arcvoice_engine_add_kokoro(ArcVoiceEngine *engine, const char *kokoro_model, const char *kokoro_voices,
                                uint32_t threads, char **error);

void arcvoice_engine_close(ArcVoiceEngine *engine);

/// The voices, best first, one per line: id, a tab, the label to show
/// ("Harper (US, female)"), a tab, and 1 for an expressive (Phonon) voice or 0.
char *arcvoice_engine_voices(ArcVoiceEngine *engine);

/// What follows a rendered piece of text, which sets the pause after it.
enum {
    ARCVOICE_AFTER_SENTENCE = 0,
    /// The end of a paragraph: a longer pause.
    ARCVOICE_AFTER_PARAGRAPH = 1,
    /// The first part of a sentence, spoken early so playback starts sooner:
    /// only a breath.
    ARCVOICE_AFTER_CLAUSE = 2,
};

/// Renders text ready to play: the speech with its silent ends trimmed, then
/// the pause that should follow it (`after`, one of ARCVOICE_AFTER_*; a
/// heading always gets a long pause). `speed` is 0.5 to 2.0 and changes the
/// voice's own pace, so it keeps its natural sound. Returns false and sets
/// `*error` on failure.
bool arcvoice_engine_render(ArcVoiceEngine *engine, const char *text, const char *voice, float speed,
                            uint32_t after, float **samples, size_t *count, char **error);

/// The files each engine needs, one per line: path relative to the engine's
/// folder, a tab, the URL, a tab, the approximate size in bytes. Phonon's
/// folder is the one given to arcvoice_engine_open; Kokoro's files are the
/// model (kokoro-v1.0.onnx) and voices (voices-v1.0.bin).
char *arcvoice_phonon_downloads(void);
char *arcvoice_kokoro_downloads(void);

/// The phonemes a sentence is spoken with, for diagnosing a pronunciation.
char *arcvoice_engine_phonemize(ArcVoiceEngine *engine, const char *text, bool british);

void arcvoice_samples_free(float *samples, size_t count);
void arcvoice_string_free(char *string);

#endif
