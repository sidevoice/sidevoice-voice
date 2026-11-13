//! Joining transcripts: with a space between them, unless the script on either side of the join is written without
//! spaces between words (Chinese, Japanese, Thai, Lao, Khmer, Myanmar), as Unicode's Script property says of the last
//! letter before it and the first after it (punctuation and digits, of no script of their own, are skipped).

use icu_properties::props::Script;
use icu_properties::CodePointMapData;

/// `first` and then `second`, joined as their scripts write it.
pub(crate) fn join(first: &str, second: &str) -> String {
    let end = first.chars().rev().find_map(script);
    let start = second.chars().find_map(script);
    if end.is_some_and(unspaced) || start.is_some_and(unspaced) {
        format!("{first}{second}")
    } else {
        format!("{first} {second}")
    }
}

/// The script `character` is written in, or `None` for one shared by many (punctuation, digits, marks).
fn script(character: char) -> Option<Script> {
    let script = CodePointMapData::<Script>::new().get(character);
    (script != Script::Common && script != Script::Inherited).then_some(script)
}

/// Whether `script` is written without spaces between words.
fn unspaced(script: Script) -> bool {
    matches!(
        script,
        Script::Han
            | Script::Hiragana
            | Script::Katakana
            | Script::Thai
            | Script::Lao
            | Script::Khmer
            | Script::Myanmar
    )
}
