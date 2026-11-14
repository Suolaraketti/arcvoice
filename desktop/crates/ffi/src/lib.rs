//! C interface to the ArcVoice engine for the iPhone and Mac apps. See
//! `include/arcvoice.h` for the contract.

use arcvoice_core::pacing::{self, Break};
use arcvoice_core::{Kokoro, LoadOptions, SAMPLE_RATE};
use std::ffi::{c_char, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

pub struct ArcVoiceEngine {
    kokoro: Kokoro,
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
    model_path: *const c_char,
    voices_path: *const c_char,
    threads: u32,
    error: *mut *mut c_char,
) -> *mut ArcVoiceEngine {
    guarded(error, std::ptr::null_mut(), || {
        let model = unsafe { borrowed(model_path) }.ok_or("the model path is missing")?;
        let voices = unsafe { borrowed(voices_path) }.ok_or("the voices path is missing")?;
        let options = LoadOptions { threads: (threads > 0).then_some(threads as usize), ..LoadOptions::default() };
        let kokoro = Kokoro::load_with(Path::new(model), Path::new(voices), &options).map_err(|e| format!("{e:#}"))?;
        Ok(Box::into_raw(Box::new(ArcVoiceEngine { kokoro })))
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
    string_out(&engine.kokoro.english_voices().join("\n"))
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
    ends_paragraph: bool,
    samples: *mut *mut f32,
    count: *mut usize,
    error: *mut *mut c_char,
) -> bool {
    guarded(error, false, || {
        let engine = unsafe { engine.as_mut() }.ok_or("the voice is not loaded")?;
        let text = unsafe { borrowed(text) }.ok_or("the sentence is missing")?;
        let voice = unsafe { borrowed(voice) }.unwrap_or("af_heart");
        if samples.is_null() || count.is_null() {
            return Err("no place to put the audio".to_string());
        }
        let speech = engine.kokoro.synthesize(text, voice, speed).map_err(|e| format!("{e:#}"))?;
        let after = if ends_paragraph { Break::Paragraph } else { Break::Sentence };
        let finished = pacing::finish_sentence(&speech, SAMPLE_RATE, text, after, speed).into_boxed_slice();
        unsafe {
            *count = finished.len();
            *samples = Box::into_raw(finished).cast::<f32>();
        }
        Ok(true)
    })
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
    match engine.kokoro.phonemize(text, british) {
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
