use std::collections::HashSet;

use super::Failure;

#[test]
fn the_codes_are_unique_and_follow_the_pattern() {
    let mut seen = HashSet::new();
    for failure in Failure::ALL {
        let code = failure.code();
        assert!(seen.insert(code), "{code} is used twice");
        let name = code
            .strip_prefix("voice.")
            .expect("the prefix is the crate");
        assert!(
            !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "{code} is not kebab case"
        );
    }
}

#[test]
fn the_codes_are_stable() {
    assert_eq!(Failure::Transcription.code(), "voice.transcription-failed");
    assert_eq!(Failure::ModelLoad.code(), "voice.model-load-failed");
}
