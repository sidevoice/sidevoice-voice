use super::{instant, Level};

#[cfg(web)]
use wasm_bindgen_test::wasm_bindgen_test as test;

#[test]
fn meter_vectors_and_explicit_reset_are_identical_for_input_and_output() {
    assert_eq!(instant(&[]), 0.0);
    assert_eq!(instant(&[0.0; 512]), 0.0);
    assert_eq!(instant(&[1.0; 512]), 1.0);
    assert!((instant(&[0.01; 512]) - 1.0 / 3.0).abs() < 0.0001);
    let mut microphone = Level::default();
    let mut speaker = Level::default();
    for window in [&[1.0; 512][..], &[0.01; 512], &[0.0; 512]] {
        microphone.take(window);
        speaker.take(window);
        assert_eq!(microphone.value(), speaker.value());
    }
    assert!(speaker.value() > 0.0);
    speaker.reset();
    assert_eq!(speaker.value(), 0.0);
}
