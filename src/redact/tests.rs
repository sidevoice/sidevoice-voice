#[cfg(web)]
use wasm_bindgen_test::wasm_bindgen_test as test;

use crate::call::{Effect, Input};
use crate::event::{TurnEvent, TurnTimings, VoiceEvent};
use crate::playback::Action;
use crate::speech::chunks;
use crate::take::Take;

const WORDS: &str = "canary words 7f3a";

/// Every canary spelling a derived `Debug` could print: the text, and the bytes or samples it became.
fn assert_hidden(debug: &str) {
    assert!(!debug.contains("canary"), "{debug}");
    assert!(!debug.contains("7f3a"), "{debug}");
    assert!(!debug.contains("99, 97, 110"), "{debug}");
    assert!(!debug.contains("0.7"), "{debug}");
}

#[test]
fn a_turn_event_shows_the_length_of_its_transcript_and_nothing_of_it() {
    let event = TurnEvent::Finished {
        turn_id: "t1".into(),
        text: WORDS.into(),
        language: Some("en".into()),
        started_at: 1,
        ended_at: 2,
        merged: false,
        timings: TurnTimings {
            audio_ms: 3,
            endpoint_silence_ms: 4,
            recognition_ms: 5,
        },
    };
    let debug = format!("{:?}", VoiceEvent::Turn(event));
    assert_hidden(&debug);
    assert!(
        debug.contains("<17 chars>") && debug.contains("t1"),
        "{debug}"
    );
}

#[test]
fn a_take_shows_the_count_of_its_samples_and_nothing_of_them() {
    let take = Take {
        sample_rate: 16_000,
        samples: vec![0.7; 5],
        started_at: 1,
        ended_at: 2,
        turns: Vec::new(),
    };
    let debug = format!("{:?}", VoiceEvent::Take(take));
    assert_hidden(&debug);
    assert!(debug.contains("<5 samples>"), "{debug}");
}

#[test]
fn the_calls_inputs_and_effects_show_sizes_in_place_of_words_and_audio() {
    let say = Input::Say {
        id: "s1".into(),
        text: WORDS.into(),
        language: None,
    };
    let heard = Input::Transcribed {
        turn: 0,
        result: Ok(WORDS.into()),
    };
    let spoken = Input::Synthesized {
        utterance: "u".into(),
        chunk: 0,
        result: Ok((vec![0.7; 5], 24_000)),
    };
    let window = Input::Window {
        pcm: vec![0.7; 5],
        speech: true,
        probability: None,
    };
    let synthesize = Effect::Synthesize {
        utterance: "u".into(),
        chunk: 0,
        text: WORDS.into(),
        language: None,
    };
    let play = Effect::Play {
        utterance: "u".into(),
        chunk: 0,
        samples: vec![0.7; 5],
        sample_rate: 24_000,
    };
    let transcribe = Effect::Transcribe {
        turn: 0,
        pcm: vec![0.7; 5],
        language: None,
    };
    let action = Action::Synthesize {
        utterance: "u".into(),
        chunk: 0,
        text: WORDS.into(),
        language: None,
    };
    let played = Action::Play {
        utterance: "u".into(),
        chunk: 0,
        samples: vec![0.7; 5],
        sample_rate: 24_000,
    };
    for debug in [
        format!("{say:?}"),
        format!("{heard:?}"),
        format!("{spoken:?}"),
        format!("{window:?}"),
        format!("{synthesize:?}"),
        format!("{play:?}"),
        format!("{transcribe:?}"),
        format!("{action:?}"),
        format!("{played:?}"),
    ] {
        assert_hidden(&debug);
    }
    assert!(format!("{say:?}").contains("<17 chars>"));
    assert!(format!("{play:?}").contains("<5 samples>"));
}

#[test]
fn a_chunk_shows_the_length_of_its_words_and_not_the_words() {
    let chunk = chunks(WORDS).remove(0);
    let debug = format!("{chunk:?}");
    assert_hidden(&debug);
    assert!(debug.contains("<17 chars>"), "{debug}");
}

#[cfg(native)]
#[test]
fn an_audio_device_shows_neither_its_label_nor_its_id() {
    let device = crate::AudioDevice {
        id: "canary-id-7f3a".into(),
        label: "canary words 7f3a".into(),
    };
    assert_hidden(&format!("{device:?}"));
}
