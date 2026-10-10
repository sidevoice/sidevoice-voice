use serde_json::json;

use super::{EndOfTurn, Patience, VoiceConfig};

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
    assert_eq!(
        (config.listening_bar.quiet, config.listening_bar.playing),
        (0.5, 0.8)
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
            let (pause, longest) = patience.smart_turn_ms();
            (
                patience.end_of_turn_silence_ms(),
                pause,
                longest,
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
