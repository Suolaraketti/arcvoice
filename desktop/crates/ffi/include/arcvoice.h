// The ArcVoice engine, as used by the iPhone and Mac apps.
//
// This is the same Rust engine the Windows app uses: text preparation,
// pronunciation, the Kokoro voice model and sentence pacing. One engine means
// a pronunciation fix lands on every platform at once.
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

/// Loads the voice model. `threads` 0 lets the engine choose. Returns NULL on
/// failure and sets `*error` to a message.
ArcVoiceEngine *arcvoice_engine_open(const char *model_path, const char *voices_path, uint32_t threads,
                                     char **error);

void arcvoice_engine_close(ArcVoiceEngine *engine);

/// The English voice names, one per line, sorted.
char *arcvoice_engine_voices(ArcVoiceEngine *engine);

/// Renders one sentence ready to play: the speech with its silent ends
/// trimmed, then the pause that should follow it (longer at a paragraph end
/// or after a heading). `speed` is 0.5 to 2.0 and changes the voice's own
/// pace, so it keeps its natural sound. Returns false and sets `*error` on
/// failure.
bool arcvoice_engine_render(ArcVoiceEngine *engine, const char *text, const char *voice, float speed,
                            bool ends_paragraph, float **samples, size_t *count, char **error);

/// The phonemes a sentence is spoken with, for diagnosing a pronunciation.
char *arcvoice_engine_phonemize(ArcVoiceEngine *engine, const char *text, bool british);

void arcvoice_samples_free(float *samples, size_t count);
void arcvoice_string_free(char *string);

#endif
