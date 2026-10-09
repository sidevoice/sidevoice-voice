use super::{Segment, Segmentation, Segmenter, AUDIO_IDLE_MS, MAX_TURN, PRE_ROLL};

#[cfg(web)]
use wasm_bindgen_test::wasm_bindgen_test as test;

const NUMBERS: Segmentation = Segmentation {
    end_of_turn_silence_ms: 2_500,
    quiet_bar: 0.5,
    playing_bar: 0.8,
};

/// A 32 ms window at −9 dBFS (a level of 0.85 once smoothed), or silent.
fn window(loud: bool) -> Vec<f32> {
    let amplitude = if loud { 0.25 * 2f32.sqrt() } else { 0.0 };
    (0..512)
        .map(|i| if i % 2 == 0 { amplitude } else { -amplitude })
        .collect()
}

#[test]
fn a_turn_opens_with_its_pre_roll_and_ends_after_the_silence() {
    let mut segmenter = Segmenter::new(NUMBERS);
    for _ in 0..40 {
        assert_eq!(segmenter.window(0, &window(true), false, false), None);
    }
    assert_eq!(
        segmenter.window(0, &window(true), true, false),
        Some(Segment::Started)
    );
    assert!(segmenter.speaking());
    let silent = 2_500 * 16 / 512 + 1;
    for _ in 0..silent - 1 {
        assert_eq!(segmenter.window(0, &window(false), false, false), None);
    }
    let Some(Segment::Ended(ended)) = segmenter.window(0, &window(false), false, false) else {
        panic!("ended")
    };
    assert_eq!(ended.pcm.len(), PRE_ROLL + 512 + silent * 512);
    assert!(ended.silence_ms >= 2_500);
}

#[test]
fn speech_under_the_bar_opens_nothing_and_the_bar_rises_while_playing() {
    let mut segmenter = Segmenter::new(NUMBERS);
    let quiet: Vec<f32> = window(true).iter().map(|s| s * 0.05).collect();
    for _ in 0..50 {
        assert_eq!(segmenter.window(0, &quiet, true, false), None);
    }
    let mut segmenter = Segmenter::new(Segmentation {
        playing_bar: 0.9,
        ..NUMBERS
    });
    for _ in 0..50 {
        assert_eq!(segmenter.window(0, &window(true), true, true), None);
    }
    assert_eq!(
        segmenter.window(0, &window(true), true, false),
        Some(Segment::Started)
    );
}

#[test]
fn a_turn_keeps_a_minute_and_ends_when_its_audio_stops() {
    let mut segmenter = Segmenter::new(NUMBERS);
    while segmenter.window(10, &window(true), true, false).is_none() {}
    for _ in 0..(MAX_TURN / 512 + 10) {
        segmenter.window(10, &window(true), true, false);
    }
    assert_eq!(segmenter.deadline(), Some(10 + AUDIO_IDLE_MS));
    assert_eq!(segmenter.poll(10 + AUDIO_IDLE_MS - 1), None);
    let Some(Segment::Ended(ended)) = segmenter.poll(10 + AUDIO_IDLE_MS) else {
        panic!("ended")
    };
    assert_eq!(ended.pcm.len(), MAX_TURN);
    assert_eq!(segmenter.deadline(), None);
}
