use super::{chunks, MAX_CHUNK};

#[cfg(web)]
use wasm_bindgen_test::wasm_bindgen_test as test;

fn texts(text: &str) -> Vec<String> {
    chunks(text).into_iter().map(|chunk| chunk.text).collect()
}

#[test]
fn sentences_are_chunks_and_their_ranges_point_into_the_text() {
    let text = "  The tests pass on Linux. ¿Y en macOS? Todavía no lo sé…\nNext line here, and then the end";
    let chunks = chunks(text);
    let chars: Vec<char> = text.chars().collect();
    for chunk in &chunks {
        assert_eq!(
            chars[chunk.chars.clone()].iter().collect::<String>(),
            chunk.text
        );
    }
    assert_eq!(
        texts(text),
        [
            "The tests pass on Linux.",
            "¿Y en macOS? Todavía no lo sé…",
            "Next line here, and then the end",
        ]
    );
}

#[test]
fn a_short_sentence_joins_the_next_and_a_decimal_point_is_no_end() {
    assert_eq!(
        texts("Done. The version is 0.2.1 now. Ok."),
        ["Done. The version is 0.2.1 now.", "Ok."]
    );
}

#[test]
fn a_long_sentence_is_cut_after_a_comma_or_at_a_space() {
    let clause = "a clause that goes on and on, ";
    let text = clause.repeat(20);
    let chunks = chunks(&text);
    assert!(chunks.len() > 1);
    for chunk in &chunks {
        assert!(chunk.text.chars().count() <= MAX_CHUNK);
        assert!(chunk.text.ends_with(',') || chunk == chunks.last().unwrap());
    }
    let words = "word ".repeat(100);
    assert!(chunks_end_at_spaces(&words));
}

fn chunks_end_at_spaces(text: &str) -> bool {
    chunks(text)
        .iter()
        .all(|chunk| chunk.text.ends_with("word"))
}

#[test]
fn nothing_to_say_is_no_chunk() {
    assert!(chunks(" \n\t ").is_empty());
    assert!(chunks("").is_empty());
}
