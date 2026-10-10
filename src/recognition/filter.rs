//! The acceptance filter: a transcript is dropped when it is empty, when it is written in no Latin letter while the
//! language spoken is written in Latin script (a recogniser that heard noise answers in any script), or when the
//! recogniser reports it too unlikely.

/// Languages written in another script than the Latin one, by their primary BCP 47 subtag: for these the script is
/// not checked.
const NON_LATIN: &[&str] = &[
    "am", "ar", "be", "bg", "bn", "el", "fa", "gu", "he", "hi", "hy", "ja", "ka", "kk", "km", "kn",
    "ko", "ky", "lo", "mk", "ml", "mn", "mr", "my", "ne", "or", "pa", "ps", "ru", "si", "sr", "ta",
    "te", "th", "uk", "ur", "yi", "zh",
];

/// The transcript to report, or `None` to drop it. `language` is the one the stage was told (`None`: detected, and
/// the script is not checked); `logprob`, the recogniser's mean log-probability where it reports one.
pub(crate) fn accepted(text: &str, language: Option<&str>, logprob: Option<f64>) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let latin_language = language.is_some_and(|language| {
        let primary = language.split(['-', '_']).next().unwrap_or_default();
        !NON_LATIN.contains(&primary.to_ascii_lowercase().as_str())
    });
    if latin_language && text.chars().any(char::is_alphabetic) && !text.chars().any(is_latin_letter)
    {
        return None;
    }
    let floor = if text.split_whitespace().count() <= 2 {
        -3.0
    } else {
        -2.0
    };
    if logprob.is_some_and(|value| value < floor) {
        return None;
    }
    Some(text.to_owned())
}

/// Whether `letter` is a letter of the Latin script: Basic Latin, the Latin-1 supplement and the Latin extensions.
fn is_latin_letter(letter: char) -> bool {
    letter.is_alphabetic()
        && matches!(letter as u32,
            0x41..=0x5A | 0x61..=0x7A | 0xC0..=0x24F | 0x1E00..=0x1EFF | 0x2C60..=0x2C7F | 0xA720..=0xA7FF
            | 0xAB30..=0xAB6F | 0xFF21..=0xFF3A | 0xFF41..=0xFF5A)
}
