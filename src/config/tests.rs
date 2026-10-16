use serde_json::json;

use super::{BargeInMinSpeech, EndOfTurn, Patience, VoiceConfig};

#[cfg(web)]
use wasm_bindgen_test::wasm_bindgen_test as test;

#[test]
fn a_configuration_reads_with_its_defaults_and_names_no_model() {
    let config: VoiceConfig = serde_json::from_value(json!({
        "language": "es",
        "voice": "ef_dora",
        "patience": "calm",
    }))
    .unwrap();
    assert_eq!(config.end_of_turn, EndOfTurn::Silence);
    assert_eq!(config.patience, Patience::Calm);
    assert_eq!(config.audio_grace_ms, 1_000);
    assert_eq!(config.idle_unload_minutes, 10);
    assert_eq!(config.speed, 1.0);
    assert_eq!(config.barge_in_min_speech.seconds(), 1.0);
    assert_eq!(
        (config.listening_bar.quiet, config.listening_bar.playing),
        (0.5, 0.7)
    );
    assert_eq!(
        (config.language.as_deref(), config.voice.as_deref()),
        (Some("es"), Some("ef_dora"))
    );
    assert_eq!(
        serde_json::from_value::<VoiceConfig>(json!({})).unwrap(),
        VoiceConfig::default()
    );
}

#[test]
fn barge_in_duration_must_be_finite_and_positive() {
    for value in [json!(0), json!(-1), json!(61), json!("NaN")] {
        assert!(
            serde_json::from_value::<VoiceConfig>(json!({"barge_in_min_speech": value})).is_err()
        );
    }
    let custom: VoiceConfig = serde_json::from_value(json!({"barge_in_min_speech": 1.25})).unwrap();
    assert_eq!(custom.barge_in_min_speech.seconds(), 1.25);
    assert!(BargeInMinSpeech::new(f32::NAN).is_err());
    assert!(BargeInMinSpeech::new(f32::INFINITY).is_err());
}

#[test]
fn an_unknown_key_is_an_error() {
    assert!(
        serde_json::from_value::<VoiceConfig>(json!({"stt": {"model": "whisper-base"}})).is_err()
    );
    assert!(
        serde_json::from_value::<VoiceConfig>(json!({"listening_bar": {"quiet": 0.5}})).is_err()
    );
    let smart: EndOfTurn = serde_json::from_value(json!("smart-turn")).unwrap();
    assert_eq!(smart, EndOfTurn::SmartTurn);
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
