//! Pulls the text out of a PDF, page by page, and hands it to the cleaner.

use anyhow::{anyhow, bail, Result};
use pdf_extract::{Document, Object};
use std::panic;

use crate::text;

pub struct ExtractedPdf {
    pub title: Option<String>,
    pub page_count: usize,
    pub sentences: Vec<String>,
}

/// Raw text of every page, in order.
pub fn page_texts(bytes: &[u8]) -> Result<Vec<String>> {
    // The PDF parser can panic on badly formed files. A broken PDF should be
    // an error message, not a crashed app.
    let outcome = panic::catch_unwind(|| pdf_extract::extract_text_from_mem_by_pages(bytes));
    match outcome {
        Ok(Ok(pages)) => Ok(pages),
        Ok(Err(error)) => {
            let message = error.to_string();
            if message.to_lowercase().contains("encrypt") || message.to_lowercase().contains("password") {
                bail!("That PDF is password protected or DRM locked, so its text can't be read.")
            }
            Err(anyhow!("That file could not be read as a PDF ({message})."))
        }
        Err(_) => bail!("That PDF has a layout this version can't read."),
    }
}

pub fn extract(bytes: &[u8]) -> Result<ExtractedPdf> {
    let pages = page_texts(bytes)?;
    let sentences = text::sentences_from_pages(&pages);
    let characters: usize = sentences.iter().map(|sentence| sentence.chars().count()).sum();
    if sentences.len() < 3 || characters < 200 {
        bail!("That PDF has no readable text. It is probably scanned page images, which this version can't read yet.");
    }
    Ok(ExtractedPdf { title: title(bytes), page_count: pages.len(), sentences })
}

/// The title from the PDF's metadata, when it has a real one.
fn title(bytes: &[u8]) -> Option<String> {
    let document = panic::catch_unwind(|| Document::load_mem(bytes)).ok()?.ok()?;
    let info = match document.trailer.get(b"Info").ok()? {
        Object::Reference(id) => document.get_object(*id).ok()?,
        other => other,
    };
    let raw = match info.as_dict().ok()?.get(b"Title").ok()? {
        Object::String(raw, _) => raw,
        _ => return None,
    };
    clean_title(&decode_pdf_string(raw))
}

fn decode_pdf_string(raw: &[u8]) -> String {
    if raw.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = raw[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else if let Ok(utf8) = std::str::from_utf8(raw) {
        utf8.to_string()
    } else {
        raw.iter().map(|&byte| byte as char).collect()
    }
}

/// PDF metadata titles are often junk left behind by the layout tool.
fn clean_title(raw: &str) -> Option<String> {
    let title = raw.trim().trim_matches('\0').trim();
    if title.chars().count() < 2 {
        return None;
    }
    let lowered = title.to_lowercase();
    let junk = ["untitled", "microsoft word", ".indd", ".doc", ".pdf", ".qxd"];
    if junk.iter().any(|marker| lowered.contains(marker)) {
        return None;
    }
    Some(title.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_junk_titles() {
        assert_eq!(clean_title("  The Long Road Home "), Some("The Long Road Home".to_string()));
        assert_eq!(clean_title("Microsoft Word - draft7.docx"), None);
        assert_eq!(clean_title("LongRoad_interior.indd"), None);
        assert_eq!(clean_title(""), None);
    }

    #[test]
    fn decodes_utf16_titles() {
        let raw = [0xFE, 0xFF, 0x00, b'H', 0x00, b'i'];
        assert_eq!(decode_pdf_string(&raw), "Hi");
    }
}
