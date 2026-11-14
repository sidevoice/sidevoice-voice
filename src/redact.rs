//! What a `Debug` shows in place of what a person said or heard: its size. Types that carry a transcript, the text
//! of a reply or audio implement `Debug` by hand with these, so that no `{:?}` of one (a log field, a failed
//! assertion) prints content.

#[cfg(test)]
mod tests;

use std::fmt;

/// A size standing for content: `<12 chars>`, `<480 samples>`.
pub(crate) struct Size(usize, &'static str);

impl fmt::Debug for Size {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{} {}>", self.0, self.1)
    }
}

/// A text, as its length in characters.
pub(crate) fn chars(text: &str) -> Size {
    Size(text.chars().count(), "chars")
}

/// Samples, as their count.
pub(crate) fn samples(samples: &[f32]) -> Size {
    Size(samples.len(), "samples")
}

/// `Debug` for state that holds words or audio and has no size worth showing: the type's name and nothing else.
macro_rules! opaque_debug {
    ($($type:ty),+ $(,)?) => {$(
        impl ::std::fmt::Debug for $type {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.debug_struct(stringify!($type)).finish_non_exhaustive()
            }
        }
    )+};
}
pub(crate) use opaque_debug;

/// A failure's stable code as a log field: the code itself when it is lowercase words (`[a-z0-9_.-]`, 1 to 64 long),
/// else `invalid`. Models are the app's, and a rejection from one can hold whatever its code field was given.
pub(crate) fn code(code: &str) -> &str {
    let plain =
        |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | '-');
    if (1..=64).contains(&code.len()) && code.chars().all(plain) {
        code
    } else {
        "invalid"
    }
}
