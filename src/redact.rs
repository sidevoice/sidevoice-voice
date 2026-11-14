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
