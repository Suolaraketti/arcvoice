//! Pronunciations for the words misaki's dictionaries do not have.
//!
//! In fiction these are mostly character and place names, and they are said
//! hundreds of times a book, so getting them right matters more than anything
//! else after the dictionary itself. A word goes through these steps until
//! one answers:
//!
//! 1. A short list of book-world words ("NetGalley").
//! 2. The CMU Pronouncing Dictionary: 135,000 words, real names among them.
//! 3. Possessives and plurals of a word that resolves ("Bingley's").
//! 4. Pieces: hyphenated words, CamelCase ("BookFunnel"), "e" + a word
//!    ("ebook"), letters mixed with digits ("CC0").
//! 5. The neural model in `g2p_nn`, for invented names.
//! 6. The old letter rules, which nothing should reach.
//!
//! The answer is in misaki's phoneme alphabet, which Kokoro was trained on.

use misaki_rs::fallback::{Fallback, FallbackError};
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::g2p_nn::NeuralG2p;
use crate::{fallback, speech_text};

/// Plugs into misaki as the handler for words it does not know.
pub struct WordResolver;

impl Fallback for WordResolver {
    fn phonemize(&self, word: &str) -> Result<String, FallbackError> {
        // Never fail: an error makes misaki drop the whole sentence.
        Ok(resolve(word))
    }
}

/// Loads the CMU dictionary and the neural model now, so the first unusual
/// name in a book does not pause the reading while they load.
pub fn warm_up() {
    let _ = cmu().len();
    if let Some(model) = NeuralG2p::shared() {
        let _ = model.predict("kaelith");
    }
}

/// Phonemes for one unknown word.
pub fn resolve(word: &str) -> String {
    let word = word
        .replace('\u{2019}', "'")
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
        .to_string();
    if word.is_empty() {
        return String::new();
    }
    resolve_at(&word, 0)
}

fn resolve_at(word: &str, depth: usize) -> String {
    let lower = word.to_lowercase();
    if let Some(phonemes) = known(&lower) {
        return phonemes;
    }

    // Possessives: "Darcy's", "the Bennets'".
    if let Some(stem) = word.strip_suffix("'s").or_else(|| word.strip_suffix("'S")).filter(|stem| !stem.is_empty()) {
        return append_s(&resolve_at(stem, depth));
    }
    if let Some(stem) = word.strip_suffix('\'').filter(|stem| stem.ends_with(['s', 'S'])) {
        return resolve_at(stem, depth);
    }

    if depth < 3 {
        // Hyphens and slashes: "Upper-worlders", "and/or".
        let pieces: Vec<&str> = word.split(['-', '/', '\u{2010}', '\u{2011}']).filter(|p| !p.is_empty()).collect();
        if pieces.len() > 1 {
            return pieces.iter().map(|piece| resolve_at(piece, depth + 1)).collect::<Vec<_>>().join(" ");
        }

        // Letters next to digits: "CC0", "R2D2", "4th".
        let runs = character_runs(word);
        if runs.len() > 1 {
            return runs.iter().map(|run| resolve_run(run, depth)).collect::<Vec<_>>().join(" ");
        }
        if word.chars().all(|c| c.is_ascii_digit()) {
            return resolve_run(word, depth);
        }

        // CamelCase, when every piece is a word: "BookFunnel", "NetGalley".
        let humps = camel_pieces(word);
        if humps.len() > 1 {
            let resolved: Option<Vec<String>> = humps.iter().map(|hump| known(&hump.to_lowercase())).collect();
            if let Some(resolved) = resolved {
                return join_compound(&resolved);
            }
        }

        // "ebook", "ereader": an e in front of a known word.
        if lower.len() >= 5 && lower.starts_with('e') {
            if let Some(rest) = known(&lower[1..]) {
                return format!("ˈi{}", rest.replace('ˈ', "ˌ"));
            }
        }

        // A prefix on a known word: "Uncopyright", "overcrowded".
        for (prefix, sound) in PREFIXES {
            if let Some(rest) = lower.strip_prefix(prefix).filter(|rest| rest.len() >= 4) {
                if let Some(phonemes) = known(rest) {
                    return format!("{sound}{phonemes}");
                }
            }
        }

        // Two words run together: "standardebooks", "Netherfield", "Hunsford".
        if let Some(phonemes) = split_compound(&lower) {
            return phonemes;
        }

        // Plurals of known words: "Lucases", "Morlocks" (if "morlock" were known).
        if let Some(stem) = lower.strip_suffix('s').filter(|stem| stem.len() > 2 && !stem.ends_with('s')) {
            if let Some(phonemes) = known(stem) {
                return append_s(&phonemes);
            }
        }
    }

    // Short all-capital words that are not in any dictionary are initials.
    if (2..=4).contains(&word.chars().count()) && word.chars().all(|c| c.is_ascii_uppercase()) {
        return spell(word);
    }

    let letters = fold_to_ascii_letters(&lower);
    if letters.len() >= 2 {
        if let Some(model) = NeuralG2p::shared() {
            let arpabet = model.predict(&letters);
            if !arpabet.is_empty() {
                return arpabet_to_misaki(&arpabet);
            }
        }
    }
    if letters.len() == 1 {
        return spell(&letters);
    }
    fallback::letter_to_sound(word)
}

/// A word made of two dictionary words, split where both halves are
/// longest. Each half must be at least four letters, so a name is not cut
/// into fragments that happen to be words: "Pemberley" is not "pember" +
/// "ley" (said "lay"), and the model handles it better.
fn split_compound(lower: &str) -> Option<String> {
    if lower.len() < 8 || !lower.bytes().all(|byte| byte.is_ascii_lowercase()) {
        return None;
    }
    let half = |part: &str| {
        known(part).or_else(|| {
            // The second half may itself be "e" + word or a plural.
            let rest = part.strip_prefix('e').filter(|rest| rest.len() >= 4).and_then(known);
            let rest = rest.map(|phonemes| format!("ˈi{}", phonemes.replace('ˈ', "ˌ")));
            rest.or_else(|| part.strip_suffix('s').filter(|stem| stem.len() >= 3).and_then(known).map(|p| append_s(&p)))
        })
    };
    (4..=lower.len() - 4)
        .filter_map(|split| {
            let (left, right) = lower.split_at(split);
            Some((left.len().min(right.len()), known(left)?, half(right)?))
        })
        .max_by_key(|(shorter, _, _)| *shorter)
        .map(|(_, left, right)| join_compound(&[left, right]))
}

/// A run of digits read as a number, or of letters resolved as a word.
fn resolve_run(run: &str, depth: usize) -> String {
    if run.chars().all(|c| c.is_ascii_digit()) {
        let number = run.parse::<u64>().ok().filter(|_| run.len() <= 12);
        return match number {
            Some(number) => speech_text::cardinal(number)
                .split([' ', '-'])
                .map(|word| known(word).unwrap_or_default())
                .collect::<Vec<_>>()
                .join(" "),
            None => run.chars().map(|digit| resolve_run(&digit.to_string(), depth)).collect::<Vec<_>>().join(" "),
        };
    }
    resolve_at(run, depth + 1)
}

/// Letter names: "CC" -> "si si".
fn spell(letters: &str) -> String {
    letters
        .chars()
        .filter_map(|letter| known(&letter.to_lowercase().to_string()))
        .collect::<Vec<_>>()
        .join("")
}

/// Joins the pronunciations of a compound's pieces, keeping the first
/// piece's main stress and making the others secondary, as English does.
fn join_compound(pieces: &[String]) -> String {
    pieces
        .iter()
        .enumerate()
        .map(|(index, piece)| if index == 0 { piece.clone() } else { piece.replace('ˈ', "ˌ") })
        .collect()
}

/// misaki's rule for adding -s: "s" after a voiceless sound, "ᵻz" after a
/// hissing one, "z" otherwise.
fn append_s(phonemes: &str) -> String {
    match phonemes.chars().last() {
        None => String::new(),
        Some('s' | 'z' | 'ʃ' | 'ʒ' | 'ʧ' | 'ʤ') => format!("{phonemes}ᵻz"),
        Some('p' | 't' | 'k' | 'f' | 'θ') => format!("{phonemes}s"),
        Some(_) => format!("{phonemes}z"),
    }
}

fn character_runs(word: &str) -> Vec<String> {
    let mut runs: Vec<String> = Vec::new();
    let mut last_kind = None;
    for character in word.chars().filter(|c| c.is_alphanumeric()) {
        let kind = character.is_ascii_digit();
        if last_kind == Some(kind) {
            runs.last_mut().expect("a run is open").push(character);
        } else {
            runs.push(character.to_string());
        }
        last_kind = Some(kind);
    }
    runs
}

/// "BookFunnel" -> ["Book", "Funnel"]; "iPhone" -> ["i", "Phone"].
fn camel_pieces(word: &str) -> Vec<String> {
    let mut pieces: Vec<String> = vec![String::new()];
    let mut previous_lower = false;
    for character in word.chars() {
        if character.is_uppercase() && previous_lower {
            pieces.push(String::new());
        }
        pieces.last_mut().expect("a piece is open").push(character);
        previous_lower = character.is_lowercase();
    }
    pieces
}

/// Lowercase a to z only, with accents removed: "Cézanne" -> "cezanne".
fn fold_to_ascii_letters(lower: &str) -> String {
    lower
        .chars()
        .filter_map(|character| {
            let plain = match character {
                'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' => 'a',
                'æ' => return Some("ae".to_string()),
                'ç' | 'č' | 'ć' => 'c',
                'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ę' => 'e',
                'ì' | 'í' | 'î' | 'ï' | 'ī' => 'i',
                'ñ' | 'ń' => 'n',
                'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => 'o',
                'œ' => return Some("oe".to_string()),
                'ß' => return Some("ss".to_string()),
                'ś' | 'š' => 's',
                'ù' | 'ú' | 'û' | 'ü' | 'ū' => 'u',
                'ý' | 'ÿ' => 'y',
                'ž' | 'ź' | 'ż' => 'z',
                'ł' => 'l',
                c if c.is_ascii_lowercase() => c,
                _ => return None,
            };
            Some(plain.to_string())
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Dictionaries
// ---------------------------------------------------------------------------

/// Prefixes that keep their own sound in front of a word, with secondary
/// stress so the word's own stress stays the main one.
const PREFIXES: [(&str, &str); 11] = [
    ("un", "ˌʌn"),
    ("non", "ˌnɑn"),
    ("over", "ˌOvəɹ"),
    ("under", "ˌʌndəɹ"),
    ("out", "ˌWt"),
    ("mis", "ˌmɪs"),
    ("dis", "ˌdɪs"),
    ("pre", "ˌpɹi"),
    ("re", "ˌɹi"),
    ("anti", "ˌænti"),
    ("super", "ˌsupəɹ"),
];

/// Book-world words that CMU and the model get wrong.
const BOOK_WORDS: [(&str, &str); 15] = [
    // The particle in names like "de Bourgh"; CMU has the letter name.
    ("de", "də"),
    ("netgalley", "nˈɛtɡˌæli"),
    ("bookfunnel", "bˈʊkfˌʌnᵊl"),
    ("goodreads", "ɡˈʊdɹˌidz"),
    ("booktok", "bˈʊktˌɑk"),
    ("bookstagram", "bˈʊkstəɡɹˌæm"),
    ("edelweiss", "ˈAdᵊlvˌIs"),
    ("audiobook", "ˈɔdiOˌbʊk"),
    ("audiobooks", "ˈɔdiOˌbʊks"),
    // Text-speak that turns up in dialogue and messages.
    ("omg", "ˌOˌɛmʤˈi"),
    ("idk", "ˌIˌdiˈA"),
    ("tbh", "ˌtiˌbiˈAʧ"),
    ("btw", "ˌbiˌtiˈdʌbᵊljˌu"),
    ("smh", "ˌɛsˌɛmˈAʧ"),
    ("ngl", "ˌɛnˌʤiˈɛl"),
];

/// A word's phonemes from the book-world list or CMU, if either has it.
fn known(lower: &str) -> Option<String> {
    if let Some((_, phonemes)) = BOOK_WORDS.iter().find(|(word, _)| *word == lower) {
        return Some(phonemes.to_string());
    }
    cmu().get(lower).cloned()
}

fn cmu() -> &'static HashMap<String, String> {
    static DICTIONARY: OnceLock<HashMap<String, String>> = OnceLock::new();
    DICTIONARY.get_or_init(|| {
        let mut words = HashMap::with_capacity(130_000);
        for line in include_str!("../assets/cmudict.dict").lines() {
            let line = line.split('#').next().unwrap_or_default().trim();
            let Some((word, arpabet)) = line.split_once(' ') else { continue };
            // "(2)" marks an alternative; the first listed is the usual one.
            if word.ends_with(')') {
                continue;
            }
            let phones: Vec<&str> = arpabet.split_whitespace().collect();
            words.insert(word.to_string(), arpabet_to_misaki(&phones));
        }
        words
    })
}

/// Converts ARPAbet ("D AA1 R S IY0") to misaki's alphabet ("dˈɑɹsi").
///
/// Checked against the 32,769 words both dictionaries have: the sounds agree
/// 95% of the time, the rest being mostly ə against ɪ in unstressed syllables.
pub fn arpabet_to_misaki(phones: &[&str]) -> String {
    // (stress mark, symbol, stressed?) per phone; stressed is None for consonants.
    let mut parts: Vec<(&str, &str, Option<bool>)> = Vec::with_capacity(phones.len());
    for phone in phones {
        let (base, stress) = match phone.as_bytes().last() {
            Some(digit @ b'0'..=b'2') => (&phone[..phone.len() - 1], Some(digit - b'0')),
            _ => (*phone, None),
        };
        let vowel = match base {
            "AA" => Some("ɑ"),
            "AE" => Some("æ"),
            "AH" => Some(if stress == Some(0) { "ə" } else { "ʌ" }),
            "AO" => Some("ɔ"),
            "AW" => Some("W"),
            "AY" => Some("I"),
            "EH" => Some("ɛ"),
            "ER" => Some(if stress == Some(0) { "əɹ" } else { "ɜɹ" }),
            "EY" => Some("A"),
            "IH" => Some("ɪ"),
            "IY" => Some("i"),
            "OW" => Some("O"),
            "OY" => Some("Y"),
            "UH" => Some("ʊ"),
            "UW" => Some("u"),
            _ => None,
        };
        if let Some(symbol) = vowel {
            let mark = match stress {
                Some(1) => "ˈ",
                Some(2) => "ˌ",
                _ => "",
            };
            parts.push((mark, symbol, Some(matches!(stress, Some(1 | 2)))));
            continue;
        }
        let consonant = match base {
            "B" => "b",
            "CH" => "ʧ",
            "D" => "d",
            "DH" => "ð",
            "F" => "f",
            "G" => "ɡ",
            "HH" => "h",
            "JH" => "ʤ",
            "K" => "k",
            "L" => "l",
            "M" => "m",
            "N" => "n",
            "NG" => "ŋ",
            "P" => "p",
            "R" => "ɹ",
            "S" => "s",
            "SH" => "ʃ",
            "T" => "t",
            "TH" => "θ",
            "V" => "v",
            "W" => "w",
            "Y" => "j",
            "Z" => "z",
            "ZH" => "ʒ",
            _ => continue,
        };
        parts.push(("", consonant, None));
    }

    let mut out = String::new();
    for index in 0..parts.len() {
        let (mark, mut symbol, _) = parts[index];
        let previous = index.checked_sub(1).map(|i| parts[i]);
        let next = parts.get(index + 1).copied();
        // American flap: the t in "butter", written T by misaki.
        if symbol == "t" {
            if let (Some(previous), Some(next)) = (previous, next) {
                if (previous.2.is_some() || previous.1 == "ɹ") && next.2 == Some(false) {
                    symbol = "T";
                }
            }
        }
        // A schwa that is really a syllabic l or n: "useful", "absorbent".
        if symbol == "ə" {
            if let (Some(previous), Some(next)) = (previous, next) {
                let after = parts.get(index + 2).copied();
                let closes = after.is_none_or(|after| after.2.is_none());
                let syllabic_l = next.1 == "l" && previous.2.is_none() && !["l", "n", "ɹ", "j", "w"].contains(&previous.1);
                let syllabic_n = next.1 == "n" && ["t", "d", "b", "p", "T"].contains(&previous.1);
                if closes && (syllabic_l || syllabic_n) {
                    symbol = "ᵊ";
                }
            }
        }
        out.push_str(mark);
        out.push_str(symbol);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_arpabet() {
        assert_eq!(arpabet_to_misaki(&["D", "AA1", "R", "S", "IY0"]), "dˈɑɹsi");
        assert_eq!(arpabet_to_misaki(&["B", "AH1", "T", "ER0"]), "bˈʌTəɹ");
        assert_eq!(arpabet_to_misaki(&["Y", "UW1", "S", "F", "AH0", "L"]), "jˈusfᵊl");
    }

    #[test]
    fn names_come_from_cmu() {
        assert_eq!(resolve("Wickham"), "wˈɪkəm");
        assert_eq!(resolve("Bingley"), "bˈɪŋli");
        assert_eq!(resolve("Halloran"), "hˈæləɹən");
    }

    #[test]
    fn possessives_and_plurals() {
        assert_eq!(resolve("Bingley\u{2019}s"), "bˈɪŋliz");
        assert_eq!(resolve("Darcy's"), "dˈɑɹsiz");
        // Not in CMU, so built from the stem: a hissing end takes "ᵻz".
        assert_eq!(resolve("Halloran's"), "hˈæləɹənz");
        assert_eq!(append_s("lˈukəs"), "lˈukəsᵻz");
    }

    #[test]
    fn compounds_and_prefixes() {
        assert_eq!(resolve("ebook"), "ˈibˌʊk");
        assert_eq!(resolve("BookFunnel"), "bˈʊkfˌʌnᵊl");
        assert_eq!(resolve("Upper-worlders"), format!("{} {}", resolve("upper"), resolve("worlders")));
        assert_eq!(resolve("CC0"), format!("{} {}", known("cc").unwrap(), known("zero").unwrap()));
    }

    #[test]
    fn prefixes_on_known_words() {
        assert_eq!(resolve("Uncopyright"), format!("ˌʌn{}", known("copyright").unwrap()));
    }

    #[test]
    fn splits_run_together_words() {
        assert_eq!(resolve("standardebooks"), format!("{}{}", known("standard").unwrap(), "ˈibˌʊks".replace('ˈ', "ˌ")));
        assert_eq!(resolve("Netherfield"), join_compound(&[known("nether").unwrap(), known("field").unwrap()]));
        assert_eq!(split_compound("pemberley"), None);
        // No split leaves both halves four letters or longer and known.
        assert_eq!(split_compound("kaelith"), None);
    }

    #[test]
    fn invented_names_use_the_model() {
        assert_eq!(resolve("Pemberley"), "pˈɛmbəɹli");
        assert_eq!(resolve("Kaelith"), "kˈɛlɪθ");
        assert!(!resolve("Vyrnwood").is_empty());
        assert!(!resolve("Cézanne").is_empty());
    }

    #[test]
    fn never_empty_for_letters() {
        for word in ["Zx", "Qwrtzp", "Aa", "x"] {
            assert!(!resolve(word).is_empty(), "{word}");
        }
    }
}
