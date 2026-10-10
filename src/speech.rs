//! A text in sentence chunks, each spoken on its own so the first sound comes early and the heard position
//! moves chunk by chunk. A chunk ends after `.`, `!`, `?`, `…` (or their full-width forms) followed by a space, and
//! at a line break; a chunk shorter than [`MIN_CHUNK`] characters joins the next one, and one longer than
//! [`MAX_CHUNK`] is cut after a `,`, `;` or `:`, or else at a space.

#[cfg(test)]
mod tests;

use std::ops::Range;

/// The shortest chunk spoken alone, in characters.
const MIN_CHUNK: usize = 24;
/// The longest chunk before it is cut, in characters.
const MAX_CHUNK: usize = 240;

/// One chunk of a text: its words, trimmed, and where it sits in the text, in characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Chunk {
    pub(crate) text: String,
    pub(crate) chars: Range<usize>,
}

/// The chunks of `text`, in order. Whitespace between them belongs to none; a text with nothing to say has none.
pub(crate) fn chunks(text: &str) -> Vec<Chunk> {
    let chars: Vec<char> = text.chars().collect();
    let mut chunks = Vec::new();
    let mut start = 0;
    for end in boundaries(&chars) {
        if let Some(range) = trimmed(&chars, start..end) {
            if range.len() >= MIN_CHUNK || end == chars.len() {
                for piece in cut(&chars, range) {
                    chunks.push(piece);
                }
                start = end;
            }
        } else {
            start = end;
        }
    }
    chunks
        .into_iter()
        .map(|range: Range<usize>| Chunk {
            text: chars[range.clone()].iter().collect(),
            chars: range,
        })
        .collect()
}

/// Where sentences end: after a terminal mark followed by whitespace, after a line break, and at the end.
fn boundaries(chars: &[char]) -> Vec<usize> {
    let mut ends = Vec::new();
    for (index, &letter) in chars.iter().enumerate() {
        let next = chars.get(index + 1).copied();
        let terminal = matches!(letter, '.' | '!' | '?' | '…' | '。' | '！' | '？')
            && next.is_none_or(char::is_whitespace);
        if terminal || letter == '\n' {
            ends.push(index + 1);
        }
    }
    if ends.last() != Some(&chars.len()) {
        ends.push(chars.len());
    }
    ends
}

/// `range` without its leading and trailing whitespace, or `None` when nothing is left.
fn trimmed(chars: &[char], range: Range<usize>) -> Option<Range<usize>> {
    let start = range.start
        + chars[range.clone()]
            .iter()
            .position(|c| !c.is_whitespace())?;
    let end = range.start + chars[range].iter().rposition(|c| !c.is_whitespace())? + 1;
    Some(start..end)
}

/// `range` in pieces of at most [`MAX_CHUNK`] characters, each cut after the last `,`, `;` or `:` that fits, or else
/// at the last space, or else hard.
fn cut(chars: &[char], range: Range<usize>) -> Vec<Range<usize>> {
    let mut pieces = Vec::new();
    let mut start = range.start;
    while range.end - start > MAX_CHUNK {
        let window = &chars[start..start + MAX_CHUNK];
        let at = window
            .iter()
            .rposition(|c| matches!(c, ',' | ';' | ':'))
            .map(|i| i + 1)
            .or_else(|| window.iter().rposition(|c| c.is_whitespace()))
            .filter(|&i| i > 0)
            .unwrap_or(MAX_CHUNK);
        if let Some(piece) = trimmed(chars, start..start + at) {
            pieces.push(piece);
        }
        start += at;
    }
    if let Some(piece) = trimmed(chars, start..range.end) {
        pieces.push(piece);
    }
    pieces
}
