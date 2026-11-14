//! Pulls the text out of an EPUB, chapter by chapter, and hands it to the
//! cleaner.
//!
//! An EPUB is a zip of XHTML files. `META-INF/container.xml` names the
//! package file, the package's spine lists the chapters in reading order, and
//! each chapter's markup already says where paragraphs and headings are. So
//! unlike a PDF there is nothing to guess about layout: the work is reading
//! the markup leniently, because real books are full of HTML entities and
//! sloppy XML that a strict parser would reject.
//!
//! The rules mirror `EPUBTextExtractor.swift` in the Apple app. Keep the two
//! in step.

use anyhow::{anyhow, bail, Result};
use regex::Regex;
use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::sync::OnceLock;

use crate::text::{self, Paragraph};

pub struct ExtractedEpub {
    pub title: Option<String>,
    /// EPUBs have no pages. This is an estimate at 275 words a page, so the
    /// library can show a length that means something.
    pub page_count: usize,
    pub sentences: Vec<String>,
    /// Indices of the sentences that end a paragraph.
    pub paragraph_ends: Vec<usize>,
}

/// True when the bytes look like a zip archive, which is what an EPUB is.
pub fn looks_like_epub(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
}

pub fn extract(bytes: &[u8]) -> Result<ExtractedEpub> {
    let mut archive = Archive::open(bytes)?;
    if archive.is_drm_locked() {
        bail!("That EPUB is DRM locked, so its text can't be read. Download the DRM-free EPUB or PDF instead if you have one.");
    }

    let container = archive
        .text("META-INF/container.xml")
        .ok_or_else(|| anyhow!("That file is not an EPUB (it has no META-INF/container.xml)."))?;
    let package_path = tags(&container)
        .find(|tag| !tag.closing && tag.name == "rootfile")
        .and_then(|tag| tag.attribute("full-path"))
        .ok_or_else(|| anyhow!("That EPUB does not say where its package file is."))?;
    let package = archive
        .text(&package_path)
        .ok_or_else(|| anyhow!("That EPUB's package file ({package_path}) is missing."))?;

    let base = match package_path.rfind('/') {
        Some(slash) => &package_path[..=slash],
        None => "",
    };
    let chapters: Vec<Vec<Paragraph>> = spine_documents(&package)
        .iter()
        .filter_map(|href| archive.text(&resolve(base, href)))
        .map(|markup| paragraphs(&markup))
        .collect();

    let text::Prose { sentences, paragraph_ends } = text::prose_from_chapters(&chapters);
    let characters: usize = sentences.iter().map(|sentence| sentence.chars().count()).sum();
    if sentences.len() < 3 || characters < 200 {
        bail!("That EPUB has no readable text. It may be all images, or locked in a way this version can't read.");
    }
    let words: usize = sentences.iter().map(|sentence| sentence.split_whitespace().count()).sum();
    Ok(ExtractedEpub {
        title: title(&package),
        page_count: words.div_ceil(275).max(1),
        sentences,
        paragraph_ends,
    })
}

// ---------------------------------------------------------------------------
// The zip
// ---------------------------------------------------------------------------

struct Archive<'a> {
    zip: zip::ZipArchive<Cursor<&'a [u8]>>,
}

impl<'a> Archive<'a> {
    fn open(bytes: &'a [u8]) -> Result<Self> {
        let zip = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|error| anyhow!("That file could not be opened as an EPUB ({error})."))?;
        Ok(Self { zip })
    }

    fn bytes(&mut self, path: &str) -> Option<Vec<u8>> {
        let mut file = self.zip.by_name(path).ok()?;
        let mut bytes = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut bytes).ok()?;
        Some(bytes)
    }

    fn text(&mut self, path: &str) -> Option<String> {
        self.bytes(path).map(|bytes| decode(&bytes))
    }

    /// Store-bought EPUBs carry Adobe or Apple DRM: a `rights.xml`, or an
    /// `encryption.xml` that covers the chapters. Many DRM-free books have an
    /// `encryption.xml` too, but only for obfuscated fonts, which is fine.
    fn is_drm_locked(&mut self) -> bool {
        if self.zip.by_name("META-INF/rights.xml").is_ok() || self.zip.by_name("META-INF/sinf.xml").is_ok() {
            return true;
        }
        let Some(encryption) = self.text("META-INF/encryption.xml") else {
            return false;
        };
        let covers_chapters = tags(&encryption)
            .filter(|tag| tag.name == "cipherreference")
            .filter_map(|tag| tag.attribute("uri"))
            .any(|uri| {
                let lowered = uri.to_lowercase();
                [".xhtml", ".html", ".htm", ".xml"].iter().any(|extension| lowered.ends_with(extension))
            });
        covers_chapters
    }
}

/// Book files are UTF-8 almost always; UTF-16 shows up now and then.
fn decode(bytes: &[u8]) -> String {
    let utf16 = |pairs: &[u8], big_endian: bool| {
        let units: Vec<u16> = pairs
            .chunks_exact(2)
            .map(|pair| if big_endian { u16::from_be_bytes([pair[0], pair[1]]) } else { u16::from_le_bytes([pair[0], pair[1]]) })
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, false),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, true),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Joins a path from the package file to the folder the package sits in,
/// undoing `%20` escapes and `..` steps.
fn resolve(base: &str, href: &str) -> String {
    let href = href.split('#').next().unwrap_or_default();
    let mut parts: Vec<String> = Vec::new();
    for part in format!("{base}{}", percent_decode(href)).split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other.to_string()),
        }
    }
    parts.join("/")
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 3 <= bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
            if let Ok(value) = u8::from_str_radix(hex, 16) {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------
// The package: title and reading order
// ---------------------------------------------------------------------------

/// Hrefs of the chapters to read, in order. Skips the table of contents page
/// (it is a list of chapter names, not story), anything marked as outside the
/// main reading order, and spine entries that are images rather than text.
fn spine_documents(package: &str) -> Vec<String> {
    let mut manifest: HashMap<String, (String, String, String)> = HashMap::new();
    let mut spine: Vec<(String, bool)> = Vec::new();
    for tag in tags(package).filter(|tag| !tag.closing) {
        match tag.name.as_str() {
            "item" => {
                if let (Some(id), Some(href)) = (tag.attribute("id"), tag.attribute("href")) {
                    let media_type = tag.attribute("media-type").unwrap_or_default();
                    let properties = tag.attribute("properties").unwrap_or_default();
                    manifest.insert(id, (href, media_type, properties));
                }
            }
            "itemref" => {
                if let Some(idref) = tag.attribute("idref") {
                    let linear = tag.attribute("linear").is_none_or(|value| value != "no");
                    spine.push((idref, linear));
                }
            }
            _ => {}
        }
    }
    spine
        .into_iter()
        .filter(|(_, linear)| *linear)
        .filter_map(|(idref, _)| manifest.get(&idref))
        .filter(|(_, media_type, properties)| {
            let is_text = media_type.is_empty() || media_type.contains("html");
            let is_nav = properties.split_whitespace().any(|property| property == "nav");
            is_text && !is_nav
        })
        .map(|(href, _, _)| href.clone())
        .collect()
}

fn title(package: &str) -> Option<String> {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    let regex = REGEX.get_or_init(|| {
        Regex::new(r"(?is)<(?:[a-z0-9]+:)?title\b[^>]*>(.*?)</(?:[a-z0-9]+:)?title\s*>").expect("title pattern is valid")
    });
    let raw = regex.captures(package)?.get(1)?.as_str();
    let title = text::collapse_spaces(&decode_entities(raw));
    (title.chars().count() >= 2).then_some(title)
}

// ---------------------------------------------------------------------------
// Chapter markup to paragraphs
// ---------------------------------------------------------------------------

/// Elements whose contents are never read aloud.
const SKIPPED: [&str; 10] = ["head", "script", "style", "title", "svg", "math", "rt", "rp", "template", "object"];

/// Elements that start a new paragraph when they open or close.
const BLOCKS: [&str; 36] = [
    "p", "div", "h1", "h2", "h3", "h4", "h5", "h6", "li", "ul", "ol", "dl", "dt", "dd", "blockquote", "section",
    "article", "header", "footer", "aside", "nav", "main", "figure", "figcaption", "table", "tr", "td", "th",
    "caption", "pre", "hr", "br", "body", "hgroup", "address", "center",
];

/// Footnote markers, footnotes and page break markers are part of the book's
/// apparatus, not its story. Read aloud they are a stray "12" mid-sentence.
fn is_apparatus(tag: &Tag) -> bool {
    let kinds = format!(
        "{} {}",
        tag.attribute("epub:type").unwrap_or_default(),
        tag.attribute("role").unwrap_or_default()
    )
    .to_lowercase();
    ["noteref", "footnote", "endnote", "rearnote", "pagebreak", "doc-noteref", "doc-footnote", "doc-endnote", "doc-pagebreak"]
        .iter()
        .any(|kind| kinds.split_whitespace().any(|word| word == *kind))
}

/// Splits one chapter's XHTML into paragraphs, marking real headings.
pub fn paragraphs(markup: &str) -> Vec<Paragraph> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut current_is_heading = false;
    let mut heading_depth = 0usize;
    // While inside an element that is not read, its name and nesting depth.
    let mut skipping: Option<(String, usize)> = None;

    let flush = |current: &mut String, current_is_heading: &mut bool, out: &mut Vec<Paragraph>| {
        let text = text::collapse_spaces(current);
        if !text.is_empty() {
            out.push(Paragraph { text, is_heading: *current_is_heading });
        }
        current.clear();
        *current_is_heading = false;
    };

    for piece in pieces(markup) {
        match piece {
            Piece::Text(raw) => {
                if skipping.is_none() && !raw.is_empty() {
                    if heading_depth > 0 && !raw.trim().is_empty() {
                        current_is_heading = true;
                    }
                    current.push_str(&decode_entities(raw));
                }
            }
            Piece::Tag(tag) => {
                if let Some((name, depth)) = &mut skipping {
                    if tag.name == *name {
                        if tag.closing {
                            *depth -= 1;
                            if *depth == 0 {
                                skipping = None;
                            }
                        } else if !tag.self_closing {
                            *depth += 1;
                        }
                    }
                    continue;
                }
                if !tag.closing && !tag.self_closing && (SKIPPED.contains(&tag.name.as_str()) || is_apparatus(&tag)) {
                    skipping = Some((tag.name.clone(), 1));
                    continue;
                }
                if BLOCKS.contains(&tag.name.as_str()) {
                    flush(&mut current, &mut current_is_heading, &mut out);
                }
                if matches!(tag.name.as_str(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6") && !tag.self_closing {
                    if tag.closing {
                        heading_depth = heading_depth.saturating_sub(1);
                    } else {
                        heading_depth += 1;
                    }
                }
            }
        }
    }
    flush(&mut current, &mut current_is_heading, &mut out);
    out
}

// ---------------------------------------------------------------------------
// A lenient markup scanner
// ---------------------------------------------------------------------------

struct Tag {
    /// Lowercased local name: `<xhtml:p>` and `<P>` are both `p`.
    name: String,
    attributes: String,
    closing: bool,
    self_closing: bool,
}

impl Tag {
    /// The value of an attribute, matched on its full name, entities decoded.
    fn attribute(&self, wanted: &str) -> Option<String> {
        static REGEX: OnceLock<Regex> = OnceLock::new();
        let regex = REGEX.get_or_init(|| {
            Regex::new(r#"([^\s=/]+)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))"#).expect("attribute pattern is valid")
        });
        regex.captures_iter(&self.attributes).find_map(|captures| {
            let name = captures.get(1)?.as_str();
            if !name.eq_ignore_ascii_case(wanted) {
                return None;
            }
            let value = captures.get(2).or(captures.get(3)).or(captures.get(4))?.as_str();
            Some(decode_entities(value))
        })
    }
}

enum Piece<'a> {
    Text(&'a str),
    Tag(Tag),
}

/// Just the tags of a piece of markup.
fn tags(markup: &str) -> impl Iterator<Item = Tag> + '_ {
    pieces(markup).filter_map(|piece| match piece {
        Piece::Tag(tag) => Some(tag),
        Piece::Text(_) => None,
    })
}

/// Splits markup into text runs and tags. Comments, processing instructions
/// and doctypes are dropped; CDATA is passed through as text.
fn pieces(markup: &str) -> impl Iterator<Item = Piece<'_>> {
    let mut rest = markup;
    std::iter::from_fn(move || loop {
        if rest.is_empty() {
            return None;
        }
        let Some(open) = rest.find('<') else {
            let text = rest;
            rest = "";
            return Some(Piece::Text(text));
        };
        if open > 0 {
            let text = &rest[..open];
            rest = &rest[open..];
            return Some(Piece::Text(text));
        }
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.find("-->").map_or("", |end| &after[end + 3..]);
            continue;
        }
        if let Some(after) = rest.strip_prefix("<![CDATA[") {
            let end = after.find("]]>").unwrap_or(after.len());
            let text = &after[..end];
            rest = after.get(end + 3..).unwrap_or("");
            return Some(Piece::Text(text));
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            rest = rest.find('>').map_or("", |end| &rest[end + 1..]);
            continue;
        }
        // Find the closing '>' that is not inside a quoted attribute value.
        let mut quote: Option<char> = None;
        let mut end = None;
        for (index, character) in rest.char_indices().skip(1) {
            match (quote, character) {
                (Some(open_quote), c) if c == open_quote => quote = None,
                (None, '"' | '\'') => quote = Some(character),
                (None, '>') => {
                    end = Some(index);
                    break;
                }
                _ => {}
            }
        }
        let Some(end) = end else {
            // A stray '<' with no tag after it is text.
            let text = &rest[..1];
            rest = &rest[1..];
            return Some(Piece::Text(text));
        };
        let inner = rest[1..end].trim();
        rest = &rest[end + 1..];

        let closing = inner.starts_with('/');
        let inner = inner.trim_start_matches('/');
        let self_closing = inner.ends_with('/');
        let inner = inner.trim_end_matches('/');
        let name_end = inner.find(|c: char| c.is_whitespace()).unwrap_or(inner.len());
        let qualified = &inner[..name_end];
        let local = qualified.rsplit(':').next().unwrap_or(qualified);
        if local.is_empty() || !local.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
            // "a < b" in sloppy markup: not a tag.
            continue;
        }
        return Some(Piece::Tag(Tag {
            name: local.to_ascii_lowercase(),
            attributes: inner[name_end..].to_string(),
            closing,
            self_closing,
        }));
    })
}

/// Decodes `&amp;`, `&#8217;`, `&#x2014;` and the named entities books use.
/// An unknown entity is dropped rather than read out as "ampersand foo".
pub fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let end = after.find(';').filter(|end| *end <= 10);
        let Some(end) = end else {
            out.push('&');
            rest = after;
            continue;
        };
        let entity = &after[..end];
        let decoded = if let Some(number) = entity.strip_prefix("#x").or_else(|| entity.strip_prefix("#X")) {
            u32::from_str_radix(number, 16).ok().and_then(char::from_u32).map(String::from)
        } else if let Some(number) = entity.strip_prefix('#') {
            number.parse::<u32>().ok().and_then(char::from_u32).map(String::from)
        } else {
            named_entity(entity).map(String::from)
        };
        match decoded {
            Some(value) => out.push_str(&value),
            None if entity.chars().all(|c| c.is_ascii_alphanumeric()) => {}
            None => {
                // Not an entity at all, just an ampersand in the text.
                out.push('&');
                rest = after;
                continue;
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn named_entity(name: &str) -> Option<&'static str> {
    Some(match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => "\u{00A0}",
        "ensp" | "emsp" | "thinsp" => " ",
        "shy" | "zwsp" | "zwnj" | "zwj" => "",
        "mdash" => "\u{2014}",
        "ndash" => "\u{2013}",
        "hellip" => "\u{2026}",
        "lsquo" => "\u{2018}",
        "rsquo" => "\u{2019}",
        "ldquo" => "\u{201C}",
        "rdquo" => "\u{201D}",
        "sbquo" => "\u{201A}",
        "bdquo" => "\u{201E}",
        "laquo" => "\u{00AB}",
        "raquo" => "\u{00BB}",
        "copy" => "\u{00A9}",
        "reg" => "\u{00AE}",
        "trade" => "\u{2122}",
        "deg" => "\u{00B0}",
        "middot" => "\u{00B7}",
        "bull" => "\u{2022}",
        "times" => "\u{00D7}",
        "eacute" => "é",
        "egrave" => "è",
        "ecirc" => "ê",
        "euml" => "ë",
        "aacute" => "á",
        "agrave" => "à",
        "acirc" => "â",
        "auml" => "ä",
        "aring" => "å",
        "aelig" => "æ",
        "ccedil" => "ç",
        "iacute" => "í",
        "icirc" => "î",
        "iuml" => "ï",
        "ntilde" => "ñ",
        "oacute" => "ó",
        "ocirc" => "ô",
        "ouml" => "ö",
        "oslash" => "ø",
        "uacute" => "ú",
        "ucirc" => "û",
        "uuml" => "ü",
        "szlig" => "ß",
        "Eacute" => "É",
        "Aacute" => "Á",
        "Ccedil" => "Ç",
        "Ntilde" => "Ñ",
        "Ouml" => "Ö",
        "Uuml" => "Ü",
        "Auml" => "Ä",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn text_of(paragraphs: &[Paragraph]) -> Vec<(&str, bool)> {
        paragraphs.iter().map(|p| (p.text.as_str(), p.is_heading)).collect()
    }

    #[test]
    fn reads_paragraphs_and_headings() {
        let markup = r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head><title>Chapter 1</title><style>p { margin: 0 }</style></head>
<body>
  <section epub:type="chapter">
    <h1 class="ch"><span class="num">Chapter 1</span><br/>The Arrival</h1>
    <p><span class="dropcap">T</span>he keeper&rsquo;s log was &#8220;careful&#x201D;&nbsp;&mdash; always.<a epub:type="noteref" href="notes.xhtml#n1">1</a></p>
    <p>She said <i>nothing</i>
       at all.</p>
    <!-- a comment with <p>markup</p> -->
    <span epub:type="pagebreak" id="page7" title="7"/>
    <p>Fish &amp; chips, &unknown; and a lone & ampersand.</p>
    <aside epub:type="footnote" id="n1"><p>A note.</p></aside>
  </section>
</body></html>"#;
        assert_eq!(
            text_of(&paragraphs(markup)),
            vec![
                ("Chapter 1", true),
                ("The Arrival", true),
                ("The keeper\u{2019}s log was \u{201C}careful\u{201D} \u{2014} always.", false),
                ("She said nothing at all.", false),
                ("Fish & chips, and a lone & ampersand.", false),
            ]
        );
    }

    #[test]
    fn resolves_paths_from_the_package_folder() {
        assert_eq!(resolve("OEBPS/", "Text/ch%201.xhtml#start"), "OEBPS/Text/ch 1.xhtml");
        assert_eq!(resolve("OEBPS/Text/", "../Text/ch2.xhtml"), "OEBPS/Text/ch2.xhtml");
        assert_eq!(resolve("", "ch3.xhtml"), "ch3.xhtml");
    }

    #[test]
    fn follows_the_spine_and_skips_the_contents_page() {
        let package = r#"<package><metadata><dc:title>The Keeper&apos;s Daughter</dc:title></metadata>
          <manifest>
            <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
            <item id="c2" href="c2.xhtml" media-type="application/xhtml+xml"/>
            <item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/>
            <item id="cover" href="cover.xhtml" media-type="application/xhtml+xml"/>
            <item id="img" href="map.svg" media-type="image/svg+xml"/>
          </manifest>
          <spine><itemref idref="cover" linear="no"/><itemref idref="nav"/><itemref idref="c1"/>
            <itemref idref="img"/><itemref idref="c2"/></spine></package>"#;
        assert_eq!(spine_documents(package), vec!["c1.xhtml", "c2.xhtml"]);
        assert_eq!(title(package).as_deref(), Some("The Keeper's Daughter"));
    }

    /// Builds a small EPUB in memory, with a watermark line stamped into every
    /// chapter the way ARC delivery services do it.
    pub(crate) fn sample_epub(encrypted_chapters: bool) -> Vec<u8> {
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buffer);
            let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            let deflated = zip::write::SimpleFileOptions::default();
            zip.start_file("mimetype", stored).unwrap();
            zip.write_all(b"application/epub+zip").unwrap();
            zip.start_file("META-INF/container.xml", deflated).unwrap();
            zip.write_all(br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#).unwrap();
            if encrypted_chapters {
                zip.start_file("META-INF/encryption.xml", deflated).unwrap();
                zip.write_all(br#"<encryption><EncryptedData><CipherData><CipherReference URI="OEBPS/c1.xhtml"/></CipherData></EncryptedData></encryption>"#).unwrap();
            }
            let mut manifest = String::new();
            let mut spine = String::new();
            let story = [
                ("tide", "Gulls argued over the rail."),
                ("fog", "The lamp turned all night."),
                ("storm", "Shutters banged on the east side."),
                ("calm", "A seal watched them from the rocks."),
                ("frost", "The oil had thickened in the cold."),
            ];
            for (index, (weather, detail)) in story.iter().enumerate() {
                let number = index + 1;
                manifest.push_str(&format!(r#"<item id="c{number}" href="c{number}.xhtml" media-type="application/xhtml+xml"/>"#));
                spine.push_str(&format!(r#"<itemref idref="c{number}"/>"#));
                zip.start_file(format!("OEBPS/c{number}.xhtml"), deflated).unwrap();
                zip.write_all(format!(
                    r#"<html><body><h2>Chapter {number}</h2>
                    <p>The {weather} came in slowly that morning. {detail}</p>
                    <p>&ldquo;Again?&rdquo; she asked about the {weather}. He only nodded and kept writing.</p>
                    <p>&ldquo;No.&rdquo;</p>
                    <p>She went back down the stairs without another word about the {weather}.</p>
                    <p>The log for the {weather} stayed open on the table until dark.</p>
                    <p class="wm">This advance copy is licensed to reader@example.com.</p></body></html>"#
                ).as_bytes()).unwrap();
            }
            zip.start_file("OEBPS/content.opf", deflated).unwrap();
            zip.write_all(format!(
                r#"<package><metadata><dc:title>The Keeper's Daughter</dc:title></metadata><manifest>{manifest}</manifest><spine>{spine}</spine></package>"#
            ).as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        buffer.into_inner()
    }

    #[test]
    fn extracts_a_whole_book_without_its_watermark() {
        let bytes = sample_epub(false);
        assert!(looks_like_epub(&bytes));
        let book = extract(&bytes).unwrap();
        assert_eq!(book.title.as_deref(), Some("The Keeper's Daughter"));
        assert_eq!(book.sentences[0], "Chapter 1");
        assert_eq!(book.sentences[1], "The tide came in slowly that morning.");
        assert_eq!(book.sentences[3], "\u{201C}Again?\u{201D} she asked about the tide.");
        assert!(book.sentences.contains(&"Chapter 5".to_string()));
        assert!(!book.sentences.iter().any(|sentence| sentence.contains("licensed")));
        // The short "No." mid-chapter repeats in every chapter but is story.
        assert_eq!(book.sentences.iter().filter(|sentence| *sentence == "\u{201C}No.\u{201D}").count(), 5);
        assert_eq!(book.sentences.len(), 5 * 8);
    }

    #[test]
    fn refuses_drm_locked_books() {
        let error = extract(&sample_epub(true)).err().unwrap().to_string();
        assert!(error.contains("DRM"), "{error}");
    }
}
