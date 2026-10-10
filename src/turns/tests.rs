use super::{Segment, Segmentation, Segmenter, AUDIO_IDLE_MS, MAX_TURN, PRE_ROLL};

#[cfg(web)]
use wasm_bindgen_test::wasm_bindgen_test as test;

const NUMBERS: Segmentation = Segmentation {
    end_of_turn_silence_ms: 2_500,
    pause_ms: None,
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

#[test]
fn with_smart_turn_each_pause_is_offered_once_and_ends_the_turn_if_it_is_still_on() {
    let mut segmenter = Segmenter::new(Segmentation {
        pause_ms: Some(900),
        end_of_turn_silence_ms: 3_000,
        ..NUMBERS
    });
    while segmenter.window(0, &window(true), true, false).is_none() {}
    let mut offered = Vec::new();
    for _ in 0..40 {
        if let Some(Segment::Paused { pcm, pause }) =
            segmenter.window(0, &window(false), false, false)
        {
            offered.push((pause, pcm.len()));
        }
    }
    assert_eq!(offered.len(), 1, "once per pause");
    assert_eq!(offered[0].0, 1);
    // Speech again (loud long enough for the smoothed level): the first pause is over, so an answer about it ends
    // nothing.
    for _ in 0..10 {
        segmenter.window(0, &window(true), true, false);
    }
    assert_eq!(segmenter.end_paused(1), None);
    for _ in 0..30 {
        if let Some(Segment::Paused { pause, .. }) =
            segmenter.window(0, &window(false), false, false)
        {
            assert_eq!(pause, 2);
        }
    }
    let Some(Segment::Ended(ended)) = segmenter.end_paused(2) else {
        panic!("ended by the model")
    };
    assert!(
        ended.silence_ms >= 900 && ended.silence_ms < 3_000,
        "{}",
        ended.silence_ms
    );
}
