//! Rewrites a sentence into words the phonemizer can say, just before it is
//! spoken. The text shown on screen is never changed.
//!
//! misaki reads ordinary numbers, money, ordinals and abbreviations well, but
//! some things common in novels defeat it: clock times and percentages make it
//! give up on the whole sentence, "1990s" loses its number, Roman numerals are
//! spelled out letter by letter ("Chapter eye vee"), web addresses run
//! together, and a word glued to an em-dash after punctuation ("no!—and")
//! becomes one long nonsense word. Each rule here fixes one of those, and
//! leaves everything else for misaki.

use regex::{Captures, Regex};
use std::sync::OnceLock;

/// The sentence as it should be spoken.
pub fn prepare(text: &str) -> String {
    let mut text = text.to_string();
    for rule in rules() {
        text = (rule.apply)(&rule.regex, &text);
    }
    text
}

struct Rule {
    regex: Regex,
    apply: fn(&Regex, &str) -> String,
}

fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let rule = |pattern: &str, apply: fn(&Regex, &str) -> String| Rule {
            regex: Regex::new(pattern).expect("speech pattern is valid"),
            apply,
        };
        vec![
            // Marks with no sound, and brackets that confuse the tokenizer.
            rule(r"[\u{2122}\u{00AE}\u{00A9}\[\]{}<>*_~^|]", |regex, text| regex.replace_all(text, "").into_owned()),
            // A dash right after punctuation: "incredible!—and".
            rule(r"([!?.,;:\u{2026}])([\u{2014}\u{2013}])", |regex, text| {
                regex.replace_all(text, "$1 $2 ").into_owned()
            }),
            // Email addresses, before web addresses claim their domain.
            rule(r"\b([A-Za-z0-9._%+-]+)@([A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+)\b", |regex, text| {
                regex
                    .replace_all(text, |caps: &Captures| {
                        format!("{} at {}", spoken_address(&caps[1]), spoken_address(&caps[2]))
                    })
                    .into_owned()
            }),
            // Web addresses: "standardebooks.org" -> "standardebooks dot org".
            rule(
                r"(?i)\b(?:https?://)?(?:www\.)?((?:[a-z0-9-]+\.)+(?:com|org|net|io|co|uk|edu|gov|us|ca|info|me|app|dev|ai|au|de))\b(/[^\s,;]*)?",
                |regex, text| {
                    regex.replace_all(text, |caps: &Captures| spoken_address(&caps[1])).into_owned()
                },
            ),
            // Clock times: "3:45 p.m.", "10:30am", "12:00".
            rule(r"\b(\d{1,2}):(\d{2})(?:\s?([AaPp])\.?\s?[Mm]\b\.?)?", |regex, text| {
                let mut out = String::with_capacity(text.len());
                let mut last = 0;
                for caps in regex.captures_iter(text) {
                    let whole = caps.get(0).expect("match");
                    out.push_str(&text[last..whole.start()]);
                    let hour: u64 = caps[1].parse().unwrap_or(99);
                    let minute: u64 = caps[2].parse().unwrap_or(99);
                    if hour > 24 || minute > 59 {
                        out.push_str(whole.as_str());
                    } else {
                        let meridiem = caps.get(3).map(|m| m.as_str().to_ascii_uppercase());
                        out.push_str(&spoken_time(hour, minute, meridiem.as_deref()));
                        // "at 3:45 p.m." ends the sentence: keep its full stop.
                        if meridiem.is_some() && whole.as_str().ends_with('.') && whole.end() == text.trim_end().len() {
                            out.push('.');
                        }
                    }
                    last = whole.end();
                }
                out.push_str(&text[last..]);
                out
            }),
            // Dates written 2/14/2024.
            rule(r"\b(\d{1,2})/(\d{1,2})/(\d{4}|\d{2})\b", |regex, text| {
                regex
                    .replace_all(text, |caps: &Captures| {
                        let month: usize = caps[1].parse().unwrap_or(0);
                        let day: u64 = caps[2].parse().unwrap_or(0);
                        let year: u64 = caps[3].parse().unwrap_or(0);
                        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
                            return caps[0].to_string();
                        }
                        let year = if caps[3].len() == 2 { 2000 + year } else { year };
                        format!("{} {}, {}", MONTHS[month - 1], ordinal(day), spoken_year(year))
                    })
                    .into_owned()
            }),
            // Percentages: "50%", "12.5 %".
            rule(r"(\d+(?:\.\d+)?)\s?%", |regex, text| regex.replace_all(text, "$1 percent").into_owned()),
            // Decades: "1990s", "'90s", "the 60s".
            rule(r"(['\u{2019}])?\b(\d{2}|\d{4})s\b", |regex, text| {
                regex
                    .replace_all(text, |caps: &Captures| {
                        let number: u64 = caps[2].parse().unwrap_or(0);
                        if caps[2].len() == 2 && number % 10 != 0 {
                            return caps[0].to_string();
                        }
                        spoken_decade(number, caps[2].len())
                    })
                    .into_owned()
            }),
            // "#1" -> "number 1".
            rule(r"#(\d+)", |regex, text| regex.replace_all(text, "number $1").into_owned()),
            // "St. Mary" is a saint; "Main St." is a street.
            rule(r"\bSt\. ([A-Z])", |regex, text| regex.replace_all(text, "Saint $1").into_owned()),
            rule(r"\b([A-Z][a-z]+) St\.(\s|,|$)", |regex, text| regex.replace_all(text, "$1 Street$2").into_owned()),
            // Roman numerals after a word that numbers things: "Chapter IV".
            rule(
                r"\b(Chapter|Part|Book|Volume|Act|Scene|Section|Phase|War|Episode|Season|Level|Round|Stage|Canto|Psalm|Article|Appendix)\s+([IVXLCDM]+|[ivxlcdm]+)\b",
                |regex, text| {
                    regex
                        .replace_all(text, |caps: &Captures| match roman_value(&caps[2]) {
                            Some(value) => format!("{} {}", &caps[1], cardinal(value)),
                            None => caps[0].to_string(),
                        })
                        .into_owned()
                },
            ),
            // Monarchs and popes: "Henry VIII" -> "Henry the Eighth".
            rule(r"\b([A-Z][a-z]+) ([IVX]{2,})\b", |regex, text| {
                regex
                    .replace_all(text, |caps: &Captures| match roman_value(&caps[2]) {
                        Some(value) if value <= 30 && !NOT_NAMES.contains(&&caps[1]) => {
                            format!("{} the {}", &caps[1], capitalize(&ordinal(value)))
                        }
                        _ => caps[0].to_string(),
                    })
                    .into_owned()
            }),
            // A heading that is only a numeral: "IV" or "III." -> "Four".
            rule(r"^\s*([IVXLCivxlc]+)\.?\s*$", |regex, text| {
                regex
                    .replace_all(text, |caps: &Captures| match roman_value(&caps[1]) {
                        // A lone "I" is far more likely the pronoun.
                        Some(value) if &caps[1] != "I" => capitalize(&cardinal(value)),
                        _ => caps[0].to_string(),
                    })
                    .into_owned()
            }),
        ]
    })
}

/// Capitalized words that come before a numeral without being a ruler's name.
const NOT_NAMES: [&str; 12] = ["The", "A", "An", "In", "Of", "And", "But", "Or", "On", "At", "To", "Is"];

const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November",
    "December",
];

/// "reader.name" -> "reader dot name". Hyphens are kept for the phonemizer.
fn spoken_address(address: &str) -> String {
    address.split('.').filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" dot ")
}

fn spoken_time(hour: u64, minute: u64, meridiem: Option<&str>) -> String {
    let mut out = cardinal(hour);
    match minute {
        0 if meridiem.is_none() && hour <= 12 => out.push_str(" o'clock"),
        0 => {}
        1..=9 => {
            out.push_str(" oh ");
            out.push_str(&cardinal(minute));
        }
        _ => {
            out.push(' ');
            out.push_str(&cardinal(minute));
        }
    }
    if let Some(meridiem) = meridiem {
        out.push(' ');
        out.push_str(meridiem);
        out.push('M');
    }
    out
}

const ONES: [&str; 20] = [
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve",
    "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen",
];
const TENS: [&str; 10] = ["", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];

/// 342 -> "three hundred forty-two".
pub fn cardinal(number: u64) -> String {
    match number {
        0..=19 => ONES[number as usize].to_string(),
        20..=99 => {
            let tens = TENS[(number / 10) as usize];
            match number % 10 {
                0 => tens.to_string(),
                ones => format!("{tens}-{}", ONES[ones as usize]),
            }
        }
        100..=999 => join(cardinal(number / 100) + " hundred", number % 100),
        1_000..=999_999 => join(cardinal(number / 1_000) + " thousand", number % 1_000),
        1_000_000..=999_999_999 => join(cardinal(number / 1_000_000) + " million", number % 1_000_000),
        _ => join(cardinal(number / 1_000_000_000) + " billion", number % 1_000_000_000),
    }
}

fn join(head: String, rest: u64) -> String {
    if rest == 0 {
        head
    } else {
        format!("{head} {}", cardinal(rest))
    }
}

/// 21 -> "twenty-first".
pub fn ordinal(number: u64) -> String {
    let words = cardinal(number);
    let split = words.rfind([' ', '-']).map_or(0, |index| index + 1);
    let (head, last) = words.split_at(split);
    let last = match last {
        "one" => "first".to_string(),
        "two" => "second".to_string(),
        "three" => "third".to_string(),
        "five" => "fifth".to_string(),
        "eight" => "eighth".to_string(),
        "nine" => "ninth".to_string(),
        "twelve" => "twelfth".to_string(),
        other if other.ends_with('y') => format!("{}ieth", &other[..other.len() - 1]),
        other => format!("{other}th"),
    };
    format!("{head}{last}")
}

/// 1998 -> "nineteen ninety-eight", 2004 -> "two thousand four".
pub fn spoken_year(year: u64) -> String {
    match year {
        1000..=1999 | 2010..=2099 => {
            let (century, rest) = (year / 100, year % 100);
            match rest {
                0 => format!("{} hundred", cardinal(century)),
                1..=9 => format!("{} oh {}", cardinal(century), cardinal(rest)),
                _ => format!("{} {}", cardinal(century), cardinal(rest)),
            }
        }
        _ => cardinal(year),
    }
}

/// 1990 -> "nineteen nineties", 90 -> "nineties", 1900 -> "nineteen hundreds".
fn spoken_decade(number: u64, digits: usize) -> String {
    let plural = |words: String| {
        if let Some(stem) = words.strip_suffix('y') {
            format!("{stem}ies")
        } else {
            format!("{words}s")
        }
    };
    if digits == 2 {
        return plural(cardinal(number));
    }
    match number % 100 {
        0 if number % 1000 == 0 => plural(cardinal(number)),
        0 => format!("{} hundreds", cardinal(number / 100)),
        rest => format!("{} {}", cardinal(number / 100), plural(cardinal(rest))),
    }
}

/// The value of a valid Roman numeral, any case.
fn roman_value(numeral: &str) -> Option<u64> {
    let digit = |c: char| match c.to_ascii_uppercase() {
        'I' => Some(1),
        'V' => Some(5),
        'X' => Some(10),
        'L' => Some(50),
        'C' => Some(100),
        'D' => Some(500),
        'M' => Some(1000),
        _ => None,
    };
    let values: Vec<u64> = numeral.chars().map(digit).collect::<Option<_>>()?;
    let mut total = 0;
    for (index, value) in values.iter().enumerate() {
        if values.get(index + 1).is_some_and(|next| next > value) {
            total -= *value as i64;
        } else {
            total += *value as i64;
        }
    }
    let total = u64::try_from(total).ok().filter(|total| *total > 0)?;
    // Only canonical spellings: "IIII" or "VX" are not numerals.
    (to_roman(total).eq_ignore_ascii_case(numeral)).then_some(total)
}

fn to_roman(mut number: u64) -> String {
    const TABLE: [(u64, &str); 13] = [
        (1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"), (50, "L"), (40, "XL"),
        (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I"),
    ];
    let mut out = String::new();
    for (value, letters) in TABLE {
        while number >= value {
            out.push_str(letters);
            number -= value;
        }
    }
    out
}

fn capitalize(words: &str) -> String {
    let mut characters = words.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_in_words() {
        assert_eq!(cardinal(0), "zero");
        assert_eq!(cardinal(42), "forty-two");
        assert_eq!(cardinal(1_305), "one thousand three hundred five");
        assert_eq!(ordinal(21), "twenty-first");
        assert_eq!(ordinal(12), "twelfth");
        assert_eq!(ordinal(40), "fortieth");
        assert_eq!(spoken_year(1998), "nineteen ninety-eight");
        assert_eq!(spoken_year(2004), "two thousand four");
        assert_eq!(spoken_year(1905), "nineteen oh five");
        assert_eq!(spoken_year(2024), "twenty twenty-four");
    }

    #[test]
    fn times_dates_and_percentages() {
        assert_eq!(prepare("She left at 10:30am and was back by 12:00."), "She left at ten thirty AM and was back by twelve o'clock.");
        assert_eq!(prepare("Meet me at 3:05 p.m."), "Meet me at three oh five PM.");
        assert_eq!(prepare("It was 3:45 p.m. on Friday."), "It was three forty-five PM on Friday.");
        assert_eq!(prepare("Due 2/14/2024."), "Due February fourteenth, twenty twenty-four.");
        assert_eq!(prepare("He won 50% of it."), "He won 50 percent of it.");
        assert_eq!(prepare("Room 12:75 is odd."), "Room 12:75 is odd.");
    }

    #[test]
    fn decades_and_numerals() {
        assert_eq!(prepare("The 1990s and the '90s and the 60s."), "The nineteen nineties and the nineties and the sixties.");
        assert_eq!(prepare("In the 1900s."), "In the nineteen hundreds.");
        assert_eq!(prepare("Chapter IV"), "Chapter four");
        assert_eq!(prepare("World War II ended; Henry VIII had six wives."), "World War two ended; Henry the Eighth had six wives.");
        assert_eq!(prepare("III"), "Three");
        assert_eq!(prepare("Iii."), "Three");
        assert_eq!(prepare("I"), "I");
        assert_eq!(prepare("Malcolm X spoke."), "Malcolm X spoke.");
        assert_eq!(prepare("I said MIX it."), "I said MIX it.");
        assert_eq!(prepare("The XX chromosome."), "The XX chromosome.");
    }

    #[test]
    fn addresses_dashes_and_symbols() {
        assert_eq!(
            prepare("Visit standardebooks.org or email reader@example.com today."),
            "Visit standardebooks dot org or email reader at example dot com today."
        );
        assert_eq!(prepare("It was incredible!\u{2014}and then"), "It was incredible! \u{2014} and then");
        assert_eq!(prepare("The Gutenberg\u{2122} License [Copyright]"), "The Gutenberg License Copyright");
        assert_eq!(prepare("St. Mary's on Main St. at noon"), "Saint Mary's on Main Street at noon");
        assert_eq!(prepare("Ranked #1."), "Ranked number 1.");
    }
}
