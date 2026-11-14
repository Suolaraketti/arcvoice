//! C interface to the ArcVoice engine for the iPhone and Mac apps. See
//! `include/arcvoice.h` for the contract.

use arcvoice_core::pacing::{self, Break};
use arcvoice_core::{Kokoro, LoadOptions, Narrator, Phonon, SAMPLE_RATE};
use std::ffi::{c_char, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

pub struct ArcVoiceEngine {
    narrator: Narrator,
}

fn load_kokoro(model: &str, voices: &str, threads: u32) -> Result<Kokoro, String> {
    let options = LoadOptions { threads: (threads > 0).then_some(threads as usize), ..LoadOptions::default() };
    Kokoro::load_with(Path::new(model), Path::new(voices), &options).map_err(|e| format!("{e:#}"))
}

fn string_out(text: &str) -> *mut c_char {
    CString::new(text.replace('\0', " ")).map_or(std::ptr::null_mut(), CString::into_raw)
}

fn set_error(error: *mut *mut c_char, message: &str) {
    if !error.is_null() {
        // SAFETY: the caller passed a writable pointer for the error.
        unsafe { *error = string_out(message) };
    }
}

/// # Safety
/// `pointer` is null or a valid NUL-terminated string.
unsafe fn borrowed<'a>(pointer: *const c_char) -> Option<&'a str> {
    if pointer.is_null() {
        None
    } else {
        unsafe { CStr::from_ptr(pointer) }.to_str().ok()
    }
}

/// Runs `body`, turning a panic into an error message instead of letting it
/// cross into Swift.
fn guarded<T>(error: *mut *mut c_char, fallback: T, body: impl FnOnce() -> Result<T, String>) -> T {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(value)) => value,
        Ok(Err(message)) => {
            set_error(error, &message);
            fallback
        }
        Err(_) => {
            set_error(error, "the voice engine hit an internal error");
            fallback
        }
    }
}

#[no_mangle]
pub extern "C" fn arcvoice_sample_rate() -> u32 {
    SAMPLE_RATE
}

/// # Safety
/// The paths are valid strings; `error` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn arcvoice_engine_open(
    phonon_dir: *const c_char,
    kokoro_model: *const c_char,
    kokoro_voices: *const c_char,
    threads: u32,
    error: *mut *mut c_char,
) -> *mut ArcVoiceEngine {
    guarded(error, std::ptr::null_mut(), || {
        let threads_option = (threads > 0).then_some(threads as usize);
        let phonon = match unsafe { borrowed(phonon_dir) } {
            Some(dir) => Some(Phonon::load(Path::new(dir), threads_option).map_err(|e| format!("{e:#}"))?),
            None => None,
        };
        let kokoro = match (unsafe { borrowed(kokoro_model) }, unsafe { borrowed(kokoro_voices) }) {
            (Some(model), Some(voices)) => Some(load_kokoro(model, voices, threads)?),
            _ => None,
        };
        if phonon.is_none() && kokoro.is_none() {
            return Err("no voice files were given".to_string());
        }
        if kokoro.is_some() {
            // The pronunciation dictionary and name model load in the background.
            std::thread::spawn(arcvoice_core::pronounce::warm_up);
        }
        Ok(Box::into_raw(Box::new(ArcVoiceEngine { narrator: Narrator::new(phonon, kokoro) })))
    })
}

/// # Safety
/// `engine` is a live engine, the paths are valid strings, and `error` is
/// null or writable.
#[no_mangle]
pub unsafe extern "C" fn arcvoice_engine_add_kokoro(
    engine: *mut ArcVoiceEngine,
    kokoro_model: *const c_char,
    kokoro_voices: *const c_char,
    threads: u32,
    error: *mut *mut c_char,
) -> bool {
    guarded(error, false, || {
        let engine = unsafe { engine.as_mut() }.ok_or("the voice is not loaded")?;
        let model = unsafe { borrowed(kokoro_model) }.ok_or("the model path is missing")?;
        let voices = unsafe { borrowed(kokoro_voices) }.ok_or("the voices path is missing")?;
        engine.narrator.set_kokoro(load_kokoro(model, voices, threads)?);
        std::thread::spawn(arcvoice_core::pronounce::warm_up);
        Ok(true)
    })
}

/// # Safety
/// `engine` came from `arcvoice_engine_open` and is not used again.
#[no_mangle]
pub unsafe extern "C" fn arcvoice_engine_close(engine: *mut ArcVoiceEngine) {
    if !engine.is_null() {
        drop(unsafe { Box::from_raw(engine) });
    }
}

/// # Safety
/// `engine` is a live engine.
#[no_mangle]
pub unsafe extern "C" fn arcvoice_engine_voices(engine: *mut ArcVoiceEngine) -> *mut c_char {
    let Some(engine) = (unsafe { engine.as_ref() }) else { return std::ptr::null_mut() };
    let lines: Vec<String> = engine
        .narrator
        .voices()
        .iter()
        .map(|voice| format!("{}\t{}\t{}", voice.id, voice.label, u8::from(voice.classic)))
        .collect();
    string_out(&lines.join("\n"))
}

/// # Safety
/// `engine` is a live engine, the strings are valid, and the out pointers
/// are writable.
#[no_mangle]
pub unsafe extern "C" fn arcvoice_engine_render(
    engine: *mut ArcVoiceEngine,
    text: *const c_char,
    voice: *const c_char,
    speed: f32,
    after: u32,
    samples: *mut *mut f32,
    count: *mut usize,
    error: *mut *mut c_char,
) -> bool {
    guarded(error, false, || {
        let engine = unsafe { engine.as_mut() }.ok_or("the voice is not loaded")?;
        let text = unsafe { borrowed(text) }.ok_or("the sentence is missing")?;
        let voice = unsafe { borrowed(voice) }.unwrap_or(arcvoice_core::narrator::DEFAULT_VOICE);
        if samples.is_null() || count.is_null() {
            return Err("no place to put the audio".to_string());
        }
        let speech = engine.narrator.synthesize(text, voice, speed).map_err(|e| format!("{e:#}"))?;
        let after = match after {
            1 => Break::Paragraph,
            2 => Break::Clause,
            _ => Break::Sentence,
        };
        let finished = pacing::finish_sentence(&speech, SAMPLE_RATE, text, after, speed).into_boxed_slice();
        unsafe {
            *count = finished.len();
            *samples = Box::into_raw(finished).cast::<f32>();
        }
        Ok(true)
    })
}

fn downloads_out(downloads: Vec<arcvoice_core::models::Download>) -> *mut c_char {
    let lines: Vec<String> = downloads.iter().map(|d| format!("{}\t{}\t{}", d.path, d.url, d.bytes)).collect();
    string_out(&lines.join("\n"))
}

#[no_mangle]
pub extern "C" fn arcvoice_phonon_downloads() -> *mut c_char {
    downloads_out(arcvoice_core::models::phonon_downloads())
}

#[no_mangle]
pub extern "C" fn arcvoice_kokoro_downloads() -> *mut c_char {
    downloads_out(arcvoice_core::models::kokoro_downloads())
}

/// # Safety
/// `engine` is a live engine and `text` a valid string.
#[no_mangle]
pub unsafe extern "C" fn arcvoice_engine_phonemize(
    engine: *mut ArcVoiceEngine,
    text: *const c_char,
    british: bool,
) -> *mut c_char {
    let (Some(engine), Some(text)) = (unsafe { engine.as_mut() }, unsafe { borrowed(text) }) else {
        return std::ptr::null_mut();
    };
    match engine.narrator.phonemize(text, british) {
        Ok(phonemes) => string_out(&phonemes),
        Err(error) => string_out(&format!("error: {error:#}")),
    }
}

/// # Safety
/// `samples` and `count` came from one `arcvoice_engine_render` call.
#[no_mangle]
pub unsafe extern "C" fn arcvoice_samples_free(samples: *mut f32, count: usize) {
    if !samples.is_null() {
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(samples, count)) });
    }
}

/// # Safety
/// `string` came from this library.
#[no_mangle]
pub unsafe extern "C" fn arcvoice_string_free(string: *mut c_char) {
    if !string.is_null() {
        drop(unsafe { CString::from_raw(string) });
    }
}
