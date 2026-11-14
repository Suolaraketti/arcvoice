//! A small letter-to-sound fallback for words the pronunciation dictionary
//! has never seen.
//!
//! Novels are full of invented names. Without a fallback the phonemizer has
//! nothing to say for "Kaelith" and the voice skips the word entirely, which
//! is far worse than a slightly odd pronunciation. The usual fallback,
//! espeak-ng, is GPL licensed, so this is a deliberately simple rule table
//! instead. It is rough, and it only has to be better than silence.

use misaki_rs::fallback::{Fallback, FallbackError};

pub struct RuleFallback;

impl Fallback for RuleFallback {
    fn phonemize(&self, word: &str) -> Result<String, FallbackError> {
        let phonemes = letter_to_sound(word);
        if phonemes.is_empty() {
            Err(FallbackError::NoPhonemes { word: word.to_string() })
        } else {
            Ok(phonemes)
        }
    }
}

/// Letter groups and the Kokoro phonemes they usually make. Longest first.
/// Kokoro writes diphthongs as single capitals: A "ay", I "eye", O "oh",
/// W "ow", Y "oy".
const GROUPS: [(&str, &str); 44] = [
    ("ough", "O"),
    ("tch", "ʧ"),
    ("sch", "sk"),
    ("igh", "I"),
    ("eau", "O"),
    ("ch", "ʧ"),
    ("sh", "ʃ"),
    ("th", "θ"),
    ("ph", "f"),
    ("wh", "w"),
    ("ck", "k"),
    ("ng", "ŋ"),
    ("qu", "kw"),
    ("gh", "ɡ"),
    ("dg", "ʤ"),
    ("wr", "ɹ"),
    ("ee", "i"),
    ("ea", "i"),
    ("oo", "u"),
    ("ou", "W"),
    ("ow", "O"),
    ("oi", "Y"),
    ("oy", "Y"),
    ("ai", "A"),
    ("ay", "A"),
    ("ei", "A"),
    ("ey", "A"),
    ("ae", "A"),
    ("ie", "i"),
    ("au", "ɔ"),
    ("aw", "ɔ"),
    ("oa", "O"),
    ("ue", "u"),
    ("ar", "ɑɹ"),
    ("er", "ɜɹ"),
    ("ir", "ɜɹ"),
    ("ur", "ɜɹ"),
    ("or", "ɔɹ"),
    ("yr", "ɪɹ"),
    ("ya", "jɑ"),
    ("zh", "ʒ"),
    ("kh", "k"),
    ("rh", "ɹ"),
    ("ll", "l"),
];

const VOWEL_SOUNDS: &str = "AIOWYiuæɑɔɛɜɪʊʌə";

fn is_vowel_letter(letter: char) -> bool {
    matches!(letter, 'a' | 'e' | 'i' | 'o' | 'u' | 'y')
}

fn single(letters: &[char], index: usize) -> &'static str {
    let next = letters.get(index + 1).copied();
    let last = index + 1 == letters.len();
    match letters[index] {
        'a' => "æ",
        'e' => "ɛ",
        'i' => "ɪ",
        'o' => "ɑ",
        'u' => "ʌ",
        'y' if index == 0 => "j",
        'y' if last => "i",
        'y' => "ɪ",
        'b' => "b",
        'c' if matches!(next, Some('e' | 'i' | 'y')) => "s",
        'c' => "k",
        'd' => "d",
        'f' => "f",
        'g' => "ɡ",
        'h' => "h",
        'j' => "ʤ",
        'k' => "k",
        'l' => "l",
        'm' => "m",
        'n' => "n",
        'p' => "p",
        'q' => "k",
        'r' => "ɹ",
        's' => "s",
        't' => "t",
        'v' => "v",
        'w' => "w",
        'x' => "ks",
        'z' => "z",
        _ => "",
    }
}

/// The "magic e" long vowels: "ate" says A, "ite" says I, and so on.
fn long_vowel(letter: char) -> Option<&'static str> {
    match letter {
        'a' => Some("A"),
        'e' => Some("i"),
        'i' => Some("I"),
        'o' => Some("O"),
        'u' => Some("u"),
        _ => None,
    }
}

pub fn letter_to_sound(word: &str) -> String {
    let letters: Vec<char> = word
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ñ' => 'n',
            'ç' => 's',
            other => other,
        })
        .filter(|c| c.is_ascii_lowercase())
        .collect();
    if letters.is_empty() {
        return String::new();
    }

    let count = letters.len();
    let mut out = String::new();
    let mut index = 0;
    while index < count {
        let letter = letters[index];

        // Vowel, one consonant, final "e": the vowel is long and the e is silent.
        if index + 3 == count
            && letters[count - 1] == 'e'
            && !is_vowel_letter(letters[index + 1])
            && (index == 0 || !is_vowel_letter(letters[index - 1]))
        {
            if let Some(sound) = long_vowel(letter) {
                out.push_str(sound);
                index += 1;
                out.push_str(single(&letters, index));
                break;
            }
        }

        // A final "e" after a consonant is silent, except in tiny words like "he".
        if letter == 'e' && index + 1 == count && count > 2 && !is_vowel_letter(letters[index - 1]) {
            break;
        }

        // "kn" at the start of a word: the k is silent.
        if index == 0 && letter == 'k' && letters.get(1) == Some(&'n') {
            index += 1;
            continue;
        }

        let rest: String = letters[index..].iter().collect();
        if let Some((group, sound)) = GROUPS.iter().find(|(group, _)| rest.starts_with(group)) {
            out.push_str(sound);
            index += group.len();
            continue;
        }

        // Doubled consonants sound once.
        if !is_vowel_letter(letter) && letters.get(index + 1) == Some(&letter) {
            index += 1;
            continue;
        }

        out.push_str(single(&letters, index));
        index += 1;
    }

    // Stress the first vowel, which is right more often than not for names.
    match out.char_indices().find(|(_, c)| VOWEL_SOUNDS.contains(*c)) {
        Some((position, _)) => {
            let mut stressed = String::with_capacity(out.len() + 2);
            stressed.push_str(&out[..position]);
            stressed.push('ˈ');
            stressed.push_str(&out[position..]);
            stressed
        }
        None => out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_plausible_sounds_for_invented_names() {
        assert_eq!(letter_to_sound("Kaelith"), "kˈAlɪθ");
        assert_eq!(letter_to_sound("Zorane"), "zˈɔɹAn");
        assert_eq!(letter_to_sound("Thessaly"), "θˈɛsæli");
        assert_eq!(letter_to_sound("Knox"), "nˈɑks");
    }

    #[test]
    fn empty_for_non_words() {
        assert_eq!(letter_to_sound("123"), "");
        assert!(RuleFallback.phonemize("***").is_err());
    }
}
