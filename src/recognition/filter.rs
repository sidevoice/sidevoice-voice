//! The acceptance filter: a transcript is dropped when it is empty, when it is written in no Latin letter while the
//! language spoken is written in Latin script (a recogniser that heard noise answers in any script), or when the
//! recogniser reports it too unlikely.

use icu_locale::subtags::script;
use icu_locale::{LanguageIdentifier, LocaleExpander};

/// The mean log-probability below which a transcript is too unlikely to keep, and the floor for one of one or two words
/// (short transcripts score lower). PROVISIONAL: carried over from core, not measured. They are to be chosen from the
/// distributions a recogniser gives real speech against noise, coughs and silence, with the run recorded here. No
/// transcriber the call is given reports a log-probability yet (`Transcriber::transcribe` answers text only), so these
/// never apply until one does.
const PROVISIONAL_FLOOR: f64 = -2.0;
const PROVISIONAL_SHORT_FLOOR: f64 = -3.0;

/// Whether `language` (a BCP 47 tag) is written in Latin script: the script its tag names, or else the one CLDR says
/// it is likely written in (CLDR's likely subtags, from ICU4X's compiled data for languages with Basic coverage or
/// more; built on each call, it only points at that data). A tag that does not parse, or a language CLDR does not know,
/// is not.
fn written_in_latin(language: &str) -> bool {
    // `es_MX`, as POSIX locales write it, is `es-MX`.
    let Ok(mut id) = language.replace('_', "-").parse::<LanguageIdentifier>() else {
        return false;
    };
    LocaleExpander::new_common().maximize(&mut id);
    id.script == Some(script!("Latn"))
}

/// The transcript to report, or `None` to drop it. `language` is the one the stage was told (`None`: detected, and
/// the script is not checked); `logprob`, the recogniser's mean log-probability where it reports one.
pub(crate) fn accepted(text: &str, language: Option<&str>, logprob: Option<f64>) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let latin_language = language.is_some_and(written_in_latin);
    if latin_language && text.chars().any(char::is_alphabetic) && !text.chars().any(is_latin_letter)
    {
        return None;
    }
    let floor = if text.split_whitespace().count() <= 2 {
        PROVISIONAL_SHORT_FLOOR
    } else {
        PROVISIONAL_FLOOR
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
