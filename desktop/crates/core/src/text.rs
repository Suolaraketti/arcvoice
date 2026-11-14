//! Turns raw per-page PDF text into a clean list of sentences to read aloud.
//!
//! ARC PDFs are noisy: every page carries a running header, a page number and
//! often a watermark line ("Advance Reader Copy", "Uncorrected Proof"). Read
//! aloud, that noise interrupts the story every thirty seconds, so most of the
//! work here is finding lines that repeat across pages and dropping them.
//!
//! The rules mirror `TextCleaner.swift` in the Apple app. Keep the two in step.

use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// Sentences in reading order, with the ones that end a paragraph marked so
/// the reader can pause longer there, the way a narrator does.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Prose {
    pub sentences: Vec<String>,
    /// Indices into `sentences`, ascending.
    pub paragraph_ends: Vec<usize>,
}

impl Prose {
    fn push_block(&mut self, sentences: impl IntoIterator<Item = String>) {
        let before = self.sentences.len();
        self.sentences.extend(sentences);
        if self.sentences.len() > before {
            self.paragraph_ends.push(self.sentences.len() - 1);
        }
    }
}

/// Raw text of each page, in reading order, to sentences in reading order.
pub fn sentences_from_pages(pages: &[String]) -> Vec<String> {
    prose_from_pages(pages).sentences
}

/// Like `sentences_from_pages`, keeping paragraph ends.
///
/// A PDF does not say where paragraphs end, but typeset prose shows it: the
/// last line of a paragraph ends a sentence and stops well short of the
/// right margin. Lines are compared with the book's typical full line.
pub fn prose_from_pages(pages: &[String]) -> Prose {
    let page_lines: Vec<Vec<String>> = pages.iter().map(|page| lines(page)).collect();
    let stripped = remove_repeated_lines(&page_lines);
    let full_line = typical_line_length(&stripped);
    let mut prose = Prose::default();
    for block in join_lines(&stripped, full_line) {
        prose.push_block(split_sentences(&block));
    }
    prose
}

/// The length of a full line of body text: the median of the longer lines.
fn typical_line_length(pages: &[Vec<String>]) -> Option<usize> {
    let mut lengths: Vec<usize> = pages
        .iter()
        .flatten()
        .map(|line| line.chars().count())
        .filter(|length| *length >= 25)
        .collect();
    if lengths.len() < 10 {
        return None;
    }
    lengths.sort_unstable();
    let median = lengths[lengths.len() / 2];
    (median >= 40).then_some(median)
}

/// One paragraph of an EPUB chapter, as the book's markup laid it out.
#[derive(Debug, Clone, PartialEq)]
pub struct Paragraph {
    pub text: String,
    /// True when the markup itself says this is a heading (`<h1>` to `<h6>`).
    pub is_heading: bool,
}

/// Paragraphs of each EPUB chapter, in reading order, to sentences.
///
/// An EPUB has no pages, so there are no running headers or page numbers to
/// find. What it can have is a watermark paragraph stamped into every chapter
/// ("This copy is licensed to reader@example.com"). Stamps go at the start or
/// end of a chapter, so the rule only looks there: a short paragraph among the
/// first or last three of a chapter, whose fingerprint sits in that zone in
/// half or more of the chapters (and at least four), is dropped. Looking only
/// at the edges keeps short lines of dialogue like "No." that recur all
/// through a book. Headings are compared by their exact text instead, so
/// "Chapter 1" and "Chapter 2" survive while a repeated "ADVANCE READER COPY"
/// banner does not.
pub fn sentences_from_chapters(chapters: &[Vec<Paragraph>]) -> Vec<String> {
    prose_from_chapters(chapters).sentences
}

/// Like `sentences_from_chapters`, keeping paragraph ends.
pub fn prose_from_chapters(chapters: &[Vec<Paragraph>]) -> Prose {
    let chapters: Vec<Vec<(String, bool)>> = chapters
        .iter()
        .map(|paragraphs| {
            paragraphs
                .iter()
                .map(|paragraph| {
                    let text = lines(&paragraph.text).join(" ");
                    let heading = paragraph.is_heading || is_heading(&text);
                    (text, heading)
                })
                .filter(|(text, _)| has_letters_or_digits(text) && !is_contents_entry(text))
                .collect()
        })
        .collect();

    let repeat_key = |text: &str, heading: bool| {
        if heading {
            format!("h:{}", text.to_lowercase())
        } else {
            format!("p:{}", fingerprint(text))
        }
    };
    let chapter_count = chapters.len();
    let mut counts: HashMap<String, usize> = HashMap::new();
    let in_stamp_zone = |index: usize, count: usize| index < 3 || index + 3 >= count;
    if chapter_count >= 4 {
        for paragraphs in &chapters {
            let seen: HashSet<String> = paragraphs
                .iter()
                .enumerate()
                .filter(|(index, (text, _))| in_stamp_zone(*index, paragraphs.len()) && text.chars().count() <= 100)
                .map(|(_, (text, heading))| repeat_key(text, *heading))
                .collect();
            for key in seen {
                *counts.entry(key).or_insert(0) += 1;
            }
        }
    }
    let minimum = ((chapter_count as f64 * 0.5).ceil() as usize).max(4);

    let mut prose = Prose::default();
    for paragraphs in &chapters {
        for (index, (text, heading)) in paragraphs.iter().enumerate() {
            if chapter_count >= 4
                && in_stamp_zone(index, paragraphs.len())
                && text.chars().count() <= 100
                && counts.get(&repeat_key(text, *heading)).copied().unwrap_or(0) >= minimum
            {
                continue;
            }
            if *heading {
                prose.push_block([if is_all_caps(text) { title_case(text) } else { text.clone() }]);
            } else {
                // Each paragraph is its own block, so a paragraph that ends
                // without punctuation never runs into the next one.
                prose.push_block(split_sentences(text));
            }
        }
    }
    prose
}

// ---------------------------------------------------------------------------
// Lines
// ---------------------------------------------------------------------------

/// Normalizes odd characters and splits a page into trimmed, non-empty lines.
pub fn lines(page: &str) -> Vec<String> {
    let mut text = String::with_capacity(page.len());
    for character in page.chars() {
        match character {
            '\r' | '\u{2028}' | '\u{2029}' => text.push('\n'),
            // Soft hyphen, zero-width space and word joiners carry no sound.
            '\u{00AD}' | '\u{200B}' | '\u{2060}' | '\u{FEFF}' => {}
            '\u{00A0}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\t' => text.push(' '),
            '\u{FB00}' => text.push_str("ff"),
            '\u{FB01}' => text.push_str("fi"),
            '\u{FB02}' => text.push_str("fl"),
            '\u{FB03}' => text.push_str("ffi"),
            '\u{FB04}' => text.push_str("ffl"),
            other => text.push(other),
        }
    }
    text.split('\n')
        .map(collapse_spaces)
        .filter(|line| !line.is_empty())
        .collect()
}

pub fn collapse_spaces(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------
// Repeated lines: headers, footers, watermarks
// ---------------------------------------------------------------------------

/// A line reduced to its shape: lowercased, with every run of digits replaced
/// by `#`. "Chapter 12" and "Chapter 13" share a fingerprint, and so do
/// "The Long Road | 41" and "The Long Road | 42".
pub fn fingerprint(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut last_was_digit = false;
    for character in line.to_lowercase().chars() {
        if character.is_numeric() {
            if !last_was_digit {
                out.push('#');
            }
            last_was_digit = true;
        } else {
            out.push(character);
            last_was_digit = false;
        }
    }
    out
}

fn has_letters_or_digits(line: &str) -> bool {
    line.chars().any(|c| c.is_alphanumeric())
}

/// True when a line ends the way prose ends. Running headers almost never do.
pub fn ends_like_sentence(line: &str) -> bool {
    matches!(
        line.chars().last(),
        Some('.' | '!' | '?' | '"' | '\u{201D}' | '\u{2019}' | '\'')
    )
}

fn page_number_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        // "12", "- 12 -", "Page 12", or a lowercase roman numeral up to xxxix.
        Regex::new(r"^[\W_]*(?:(?i:page)\s+)?(?:\d{1,4}|x{0,3}(?:ix|iv|v?i{0,3}))[\W_]*$")
            .expect("page number pattern is valid")
    })
}

pub fn is_page_number(line: &str) -> bool {
    // The roman branch can match nothing at all, so insist on a real digit or
    // roman letter being present.
    line.chars().count() <= 14
        && line.chars().any(|c| c.is_ascii_digit() || matches!(c, 'i' | 'v' | 'x'))
        && page_number_regex().is_match(line)
}

fn contents_entry_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        // A table of contents row: dot leaders followed by a page number.
        Regex::new(r"(?:\.{4,}|(?:\s\.){3,})\s*\d+\s*$").expect("contents pattern is valid")
    })
}

pub fn is_contents_entry(line: &str) -> bool {
    contents_entry_regex().is_match(line)
}

fn is_edge(index: usize, count: usize) -> bool {
    index < 2 || index + 2 >= count
}

/// Drops lines that are page furniture rather than story.
///
/// Three rules, from most to least certain:
/// 1. A short line whose fingerprint shows up on 40% or more of the pages is a
///    watermark or a global header, wherever it sits on the page.
/// 2. A short line in the first or last two lines of a page, that does not end
///    like a sentence, and whose fingerprint sits in that same edge zone on
///    three or more pages, is a running header or footer. This is what catches
///    chapter titles repeated at the top of each page.
/// 3. A bare page number in the edge zone.
pub fn remove_repeated_lines(page_lines: &[Vec<String>]) -> Vec<Vec<String>> {
    let page_count = page_lines.len();
    let mut anywhere: HashMap<String, usize> = HashMap::new();
    let mut edge: HashMap<String, usize> = HashMap::new();

    if page_count >= 4 {
        for lines in page_lines {
            let mut seen = HashSet::new();
            let mut seen_edge = HashSet::new();
            for (index, line) in lines.iter().enumerate() {
                if line.chars().count() > 100 {
                    continue;
                }
                let key = fingerprint(line);
                if is_edge(index, lines.len()) {
                    seen_edge.insert(key.clone());
                }
                seen.insert(key);
            }
            for key in seen {
                *anywhere.entry(key).or_insert(0) += 1;
            }
            for key in seen_edge {
                *edge.entry(key).or_insert(0) += 1;
            }
        }
    }

    let anywhere_minimum = ((page_count as f64 * 0.4).ceil() as usize).max(4);
    let edge_minimum = 3;

    page_lines
        .iter()
        .map(|lines| {
            let count = lines.len();
            let mut kept = Vec::with_capacity(count);
            for (index, line) in lines.iter().enumerate() {
                // Ornaments like "* * *" have nothing to say out loud.
                if !has_letters_or_digits(line) || is_contents_entry(line) {
                    continue;
                }
                let length = line.chars().count();
                let at_edge = is_edge(index, count);
                if page_count >= 4 && length <= 100 {
                    let key = fingerprint(line);
                    if anywhere.get(&key).copied().unwrap_or(0) >= anywhere_minimum {
                        continue;
                    }
                    if at_edge
                        && length <= 70
                        && !ends_like_sentence(line)
                        && edge.get(&key).copied().unwrap_or(0) >= edge_minimum
                    {
                        continue;
                    }
                }
                if at_edge && is_page_number(line) {
                    continue;
                }
                kept.push(line.clone());
            }
            kept
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Joining lines back into running text
// ---------------------------------------------------------------------------

const HEADING_WORDS: [&str; 14] = [
    "chapter",
    "part",
    "book",
    "prologue",
    "epilogue",
    "interlude",
    "acknowledgments",
    "acknowledgements",
    "contents",
    "dedication",
    "introduction",
    "foreword",
    "preface",
    "afterword",
];

fn is_all_caps(line: &str) -> bool {
    let mut letters = 0;
    for character in line.chars().filter(|c| c.is_alphabetic()) {
        if !character.is_uppercase() {
            return false;
        }
        letters += 1;
    }
    letters >= 3
}

/// Chapter headings have no closing punctuation, so left alone they run
/// straight into the first sentence of the chapter. Spotting them lets the
/// reader give them their own beat.
pub fn is_heading(line: &str) -> bool {
    if line.chars().count() > 60 || ends_like_sentence(line) {
        return false;
    }
    let words: Vec<&str> = line.split(' ').collect();
    if words.is_empty() || words.len() > 8 {
        return false;
    }
    let first = words[0]
        .trim_matches(|c: char| c.is_ascii_punctuation())
        .to_lowercase();
    HEADING_WORDS.contains(&first.as_str()) || is_all_caps(line)
}

/// "CHAPTER ONE" reads more naturally as "Chapter One". Roman numerals stay
/// in capitals ("PART III" is "Part III", not "Part Iii") so they can still be
/// read as numbers.
fn title_case(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut start_of_word = true;
    let mut words_left = line.split(|c: char| !c.is_alphabetic()).filter(|word| !word.is_empty());
    let mut keep_word = false;
    for character in line.chars() {
        if character.is_alphabetic() && start_of_word {
            keep_word = words_left.next().is_some_and(is_roman_numeral);
        }
        if keep_word && character.is_alphabetic() {
            out.push(character);
            start_of_word = false;
            continue;
        }
        if character.is_alphabetic() {
            if start_of_word {
                out.extend(character.to_uppercase());
            } else {
                out.extend(character.to_lowercase());
            }
            start_of_word = false;
        } else {
            out.push(character);
            start_of_word = character != '\'' && character != '\u{2019}';
        }
    }
    out
}

fn is_roman_numeral(word: &str) -> bool {
    word.len() <= 8
        && word.chars().all(|c| matches!(c, 'I' | 'V' | 'X' | 'L' | 'C'))
        && !matches!(word, "I" | "LIV" | "CIVIC" | "CIVIL" | "VIC" | "LIC")
}

/// Merges the lines of every page into blocks of running text. A heading
/// becomes a block of its own; everything between headings is one block,
/// carried across page breaks so a sentence split by a page turn is whole.
pub fn join_into_blocks(pages: &[Vec<String>]) -> Vec<String> {
    join_lines(pages, None)
}

/// `join_into_blocks`, also ending a block at each paragraph end when the
/// length of a full line is known.
fn join_lines(pages: &[Vec<String>], full_line: Option<usize>) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current = String::new();
    let ends_paragraph = |line: &str| {
        full_line.is_some_and(|full| ends_like_sentence(line) && line.chars().count() * 100 < full * 78)
    };

    for lines in pages {
        for line in lines {
            if is_heading(line) {
                if !current.trim().is_empty() {
                    blocks.push(current.trim().to_string());
                }
                current.clear();
                blocks.push(if is_all_caps(line) { title_case(line) } else { line.clone() });
                continue;
            }
            if current.is_empty() {
                current.push_str(line);
            } else {
                // A word broken across lines: "some-" + "thing" -> "something".
                let ends_with_hyphen = current.ends_with('-') || current.ends_with('\u{2010}');
                let letter_before_hyphen = {
                    let mut reversed = current.chars().rev();
                    reversed.next();
                    reversed.next().is_some_and(|c| c.is_alphabetic())
                };
                let continues_lowercase = line.chars().next().is_some_and(|c| c.is_lowercase());
                if ends_with_hyphen && letter_before_hyphen && continues_lowercase {
                    current.pop();
                    current.push_str(line);
                } else {
                    current.push(' ');
                    current.push_str(line);
                }
            }
            if ends_paragraph(line) {
                blocks.push(current.trim().to_string());
                current.clear();
            }
        }
    }
    if !current.trim().is_empty() {
        blocks.push(current.trim().to_string());
    }
    blocks
}

// ---------------------------------------------------------------------------
// Sentences
// ---------------------------------------------------------------------------

/// Words that end in a period without ending the sentence.
const ABBREVIATIONS: [&str; 26] = [
    "mr", "mrs", "ms", "dr", "prof", "sr", "jr", "st", "mt", "vs", "gen", "col", "lt", "sgt",
    "capt", "rev", "hon", "no", "vol", "fig", "inc", "ltd", "co", "mx", "messrs", "mme",
];

/// Verbs that mark "she said" style tags after a line of dialogue.
pub const SPEECH_VERBS: [&str; 30] = [
    "said", "asked", "whispered", "called", "shouted", "replied", "cried", "murmured", "muttered",
    "yelled", "answered", "snapped", "laughed", "sighed", "breathed", "hissed", "growled",
    "demanded", "added", "continued", "began", "exclaimed", "screamed", "insisted", "wondered",
    "says", "asks", "whispers", "calls", "replies",
];

/// True when the text starts with a dialogue tag such as "Mara whispered" or
/// "said the ferryman". After `"Wait!"` that is the same sentence carrying on,
/// even though the name is capitalized.
pub fn starts_with_speech_tag(rest: &str) -> bool {
    rest.split_whitespace().take(2).any(|word| {
        let word = word.trim_matches(|c: char| !c.is_alphabetic()).to_lowercase();
        SPEECH_VERBS.contains(&word.as_str())
    })
}

fn is_closer(character: char) -> bool {
    matches!(character, '"' | '\'' | '\u{201D}' | '\u{2019}' | ')' | ']')
}

fn is_sentence_opener(character: char) -> bool {
    character.is_uppercase()
        || character.is_numeric()
        || matches!(character, '"' | '\'' | '\u{201C}' | '\u{2018}' | '(' | '[')
}

/// Splits a block of running text into sentences.
///
/// A sentence ends at `.`, `!`, `?` or `…`, plus any closing quotes, when
/// what follows after the space starts like a new sentence. That last test is
/// what keeps `"Stop!" she said.` in one piece.
pub fn split_sentences(block: &str) -> Vec<String> {
    let characters: Vec<char> = block.chars().collect();
    let mut sentences = Vec::new();
    let mut start = 0;
    let mut index = 0;

    while index < characters.len() {
        let character = characters[index];
        if !matches!(character, '.' | '!' | '?' | '\u{2026}') {
            index += 1;
            continue;
        }

        // Swallow runs like "?!" or "..." and any closing quotes after them.
        let mut end = index + 1;
        while end < characters.len()
            && (matches!(characters[end], '.' | '!' | '?' | '\u{2026}') || is_closer(characters[end]))
        {
            end += 1;
        }

        let at_end = end >= characters.len();
        let followed_by_space = !at_end && characters[end].is_whitespace();
        let mut next = end;
        while next < characters.len() && characters[next].is_whitespace() {
            next += 1;
        }
        let opens_sentence = next < characters.len() && is_sentence_opener(characters[next]);

        let mut boundary = at_end || (followed_by_space && opens_sentence);
        if boundary && !at_end && character == '.' && end == index + 1 {
            // "Dr. Reyes" and "J. R. R. Tolkien" are not sentence ends.
            let mut word_start = index;
            while word_start > start && characters[word_start - 1].is_alphabetic() {
                word_start -= 1;
            }
            let word: String = characters[word_start..index].iter().collect();
            let is_initial = word.chars().count() == 1 && word.chars().all(|c| c.is_uppercase());
            if is_initial || ABBREVIATIONS.contains(&word.to_lowercase().as_str()) {
                boundary = false;
            }
        }

        if boundary && !at_end && matches!(character, '!' | '?') && end > index + 1 {
            // A quoted exclamation or question followed by its "she said" tag.
            let rest: String = characters[next..].iter().take(40).collect();
            if starts_with_speech_tag(&rest) {
                boundary = false;
            }
        }

        if boundary {
            push_sentence(&mut sentences, &characters[start..end]);
            start = next;
            index = next;
        } else {
            index = end;
        }
    }
    if start < characters.len() {
        push_sentence(&mut sentences, &characters[start..]);
    }
    sentences
}

fn push_sentence(sentences: &mut Vec<String>, characters: &[char]) {
    let text: String = characters.iter().collect();
    let sentence = collapse_spaces(&text);
    if has_letters_or_digits(&sentence) {
        sentences.push(sentence);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn simulated_arc() -> Vec<String> {
        let words = [
            "amber", "birch", "cedar", "delta", "ember", "fjord", "grove", "heath", "inlet",
            "jetty", "knoll", "larch",
        ];
        (1..=12)
            .map(|page| {
                let mut lines = vec![if page % 2 == 1 {
                    "THE LONG ROAD HOME".to_string()
                } else {
                    "Jane Q. Author".to_string()
                }];
                if page == 2 {
                    lines.push("Chapter One ........ 3".to_string());
                    lines.push("Chapter Two . . . . 17".to_string());
                }
                if page == 5 {
                    lines.push("CHAPTER TWO".to_string());
                }
                for line in 1..=5 {
                    lines.push(format!(
                        "This is {} line {} {} and it carries on with the story for a while.",
                        words[page - 1],
                        words[(page * line) % 12],
                        words[(page + line) % 12]
                    ));
                    if line == 2 {
                        lines.push("Advance Reader Copy. Not for sale.".to_string());
                    }
                }
                if [3, 6, 9, 11].contains(&page) {
                    lines.push("\u{201C}No,\u{201D} she said.".to_string());
                }
                lines.push(if page > 3 {
                    format!("- {page} -")
                } else {
                    ["i", "ii", "iii"][page - 1].to_string()
                });
                lines.join("\n")
            })
            .collect()
    }

    #[test]
    fn strips_furniture_and_keeps_story() {
        let sentences = sentences_from_pages(&simulated_arc());
        let all = sentences.join("\n");
        assert!(!all.to_lowercase().contains("long road"));
        assert!(!all.contains("Author"));
        assert!(!all.contains("Advance"));
        assert!(!all.contains("...."));
        assert!(!all.contains("- 7 -"));
        assert_eq!(sentences.iter().filter(|s| s.starts_with("This is ")).count(), 60);
        assert_eq!(sentences.iter().filter(|s| s.contains("she said")).count(), 4);
        assert!(sentences.contains(&"Chapter Two".to_string()));
    }

    #[test]
    fn recognizes_page_numbers() {
        for yes in ["12", "Page 4", "xiv", "1984", "\u{2014} 7 \u{2014}", "- 12 -"] {
            assert!(is_page_number(yes), "{yes} should be a page number");
        }
        for no in ["vivid.", "mix", "I", "No.", "...", "Chapter 12 begins"] {
            assert!(!is_page_number(no), "{no} should not be a page number");
        }
    }

    #[test]
    fn rejoins_hyphenated_words_across_lines() {
        let pages = vec![vec!["She found some-".to_string(), "thing under the well-".to_string(), "Known bridge.".to_string()]];
        assert_eq!(
            join_into_blocks(&pages),
            vec!["She found something under the well- Known bridge."]
        );
    }

    #[test]
    fn splits_sentences_sensibly() {
        let text = "\u{201C}Stop!\u{201D} she said. Dr. Reyes looked up. J. R. Hale waited... Then nothing. \u{201C}Why?\u{201D} \u{201C}Because.\u{201D}";
        assert_eq!(
            split_sentences(text),
            vec![
                "\u{201C}Stop!\u{201D} she said.",
                "Dr. Reyes looked up.",
                "J. R. Hale waited...",
                "Then nothing.",
                "\u{201C}Why?\u{201D}",
                "\u{201C}Because.\u{201D}",
            ]
        );
        assert_eq!(
            split_sentences("\u{201C}Wait!\u{201D} Mara whispered. \u{201C}Why?\u{201D} The door closed."),
            vec!["\u{201C}Wait!\u{201D} Mara whispered.", "\u{201C}Why?\u{201D}", "The door closed."]
        );
    }

    #[test]
    fn finds_paragraph_ends_from_short_last_lines() {
        let full = "The keeper climbed the stairs again and looked out across the grey";
        let pages = vec![[
            full,
            "water, where the boat had been. It was gone now.",
            "\u{201C}No,\u{201D} she said.",
            full,
            "water and the gulls and the long line of the shore beyond the point",
            "where the light could not reach at all, and he went back down.",
            full,
            full,
            full,
            full,
            full,
            "where nothing moved.",
        ]
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")];
        let prose = prose_from_pages(&pages);
        let ends: Vec<&str> = prose.paragraph_ends.iter().map(|&index| prose.sentences[index].as_str()).collect();
        assert_eq!(ends.len(), 3, "{ends:?}");
        assert_eq!(ends[0], "It was gone now.");
        assert_eq!(ends[1], "\u{201C}No,\u{201D} she said.");
        assert!(ends[2].ends_with("where nothing moved."));
    }

    #[test]
    fn epub_paragraphs_and_headings_end_blocks() {
        let paragraph = |text: &str, is_heading: bool| Paragraph { text: text.to_string(), is_heading };
        let prose = prose_from_chapters(&[vec![
            paragraph("CHAPTER III", true),
            paragraph("One. Two.", false),
            paragraph("Three.", false),
        ]]);
        assert_eq!(prose.sentences, vec!["Chapter III", "One.", "Two.", "Three."]);
        assert_eq!(prose.paragraph_ends, vec![0, 2, 3]);
    }

    #[test]
    fn headings_stand_alone() {
        assert!(is_heading("CHAPTER ONE"));
        assert!(is_heading("Chapter 12"));
        assert!(is_heading("Part Two: The Sea"));
        assert!(!is_heading("Chapter one was over."));
        assert!(!is_heading("She walked to the river and sat down beside the old stone wall there"));
    }
}
