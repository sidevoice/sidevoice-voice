use serde_json::json;

use super::{BargeInMinSpeech, EndOfTurn, InputConfig, OutputConfig, Patience, VoiceConfig};

#[cfg(web)]
use wasm_bindgen_test::wasm_bindgen_test as test;

#[test]
fn each_half_reads_with_its_defaults_and_names_no_model() {
    let input: InputConfig = serde_json::from_value(json!({
        "language": "es",
        "patience": "calm",
    }))
    .unwrap();
    assert_eq!(input.end_of_turn, EndOfTurn::Silence { ms: None });
    assert_eq!(input.patience, Patience::Calm);
    assert_eq!(input.barge_in_min_speech.seconds(), 1.0);
    assert_eq!(
        (input.listening_bar.quiet, input.listening_bar.playing),
        (0.5, 0.7)
    );
    assert_eq!(input.language.as_deref(), Some("es"));
    let output: OutputConfig = serde_json::from_value(json!({"voice": "ef_dora"})).unwrap();
    assert_eq!(output.voice.as_deref(), Some("ef_dora"));
    assert_eq!((output.speed, output.language), (1.0, None));
    let call: VoiceConfig = serde_json::from_value(json!({})).unwrap();
    assert_eq!(call, VoiceConfig::default());
    assert_eq!((call.audio_grace_ms, call.idle_unload_minutes), (1_000, 10));
}

#[test]
fn the_end_of_turn_is_silence_with_its_own_pause_or_smart() {
    let read = |value| serde_json::from_value::<EndOfTurn>(value);
    assert_eq!(
        read(json!({"kind": "silence", "ms": 1500})).unwrap(),
        EndOfTurn::Silence { ms: Some(1_500) }
    );
    assert_eq!(
        read(json!({"kind": "silence"})).unwrap(),
        EndOfTurn::Silence { ms: None }
    );
    assert_eq!(read(json!({"kind": "smart"})).unwrap(), EndOfTurn::Smart {});
    assert!(read(json!("smart-turn")).is_err());
    assert!(read(json!({"kind": "smart", "model": "smart-turn-v3"})).is_err());
}

#[test]
fn barge_in_duration_must_be_finite_and_positive() {
    for value in [json!(0), json!(-1), json!(61), json!("NaN")] {
        assert!(
            serde_json::from_value::<InputConfig>(json!({"barge_in_min_speech": value})).is_err()
        );
    }
    let custom: InputConfig = serde_json::from_value(json!({"barge_in_min_speech": 1.25})).unwrap();
    assert_eq!(custom.barge_in_min_speech.seconds(), 1.25);
    assert!(BargeInMinSpeech::new(f32::NAN).is_err());
    assert!(BargeInMinSpeech::new(f32::INFINITY).is_err());
}

#[test]
fn an_unknown_key_is_an_error() {
    assert!(
        serde_json::from_value::<InputConfig>(json!({"stt": {"model": "whisper-base"}})).is_err()
    );
    assert!(
        serde_json::from_value::<InputConfig>(json!({"listening_bar": {"quiet": 0.5}})).is_err()
    );
    assert!(serde_json::from_value::<OutputConfig>(json!({"tts": "kokoro"})).is_err());
    assert!(serde_json::from_value::<VoiceConfig>(json!({"language": "es"})).is_err());
}

#[test]
fn patience_sets_the_silence_the_smart_turn_pauses_and_the_merge_window() {
    let numbers: Vec<_> = [Patience::Fast, Patience::Normal, Patience::Calm]
        .iter()
        .map(|patience| {
            (
                patience.silence_ms(),
                patience.smart_turn_stop_ms(),
                patience.smart_turn_longest_ms(),
                patience.merge_window_ms(),
            )
        })
        .collect();
    assert_eq!(
        numbers,
        [
            (2_000, 600, 2_500, 0),
            (2_500, 900, 3_000, 500),
            (3_500, 1_300, 4_000, 1_500)
        ]
    );
}
