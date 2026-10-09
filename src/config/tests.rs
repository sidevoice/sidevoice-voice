use serde_json::json;

use super::{EndOfTurn, Patience, VoiceConfig};

#[cfg(web)]
use wasm_bindgen_test::wasm_bindgen_test as test;

#[test]
fn a_configuration_reads_with_its_defaults() {
    let config: VoiceConfig = serde_json::from_value(json!({
        "vad": {"model": "silero-vad"},
        "stt": {"model": "whisper-base", "language": "es"},
        "tts": {"model": "kokoro-82m-v1.0", "build": "kokoro-82m-v1.0/sherpa-onnx-int8", "voice": "ef_dora"},
        "patience": "calm",
    }))
    .unwrap();
    assert_eq!(config.end_of_turn, EndOfTurn::Silence);
    assert_eq!(config.patience, Patience::Calm);
    assert_eq!(config.audio_grace_ms, 1_000);
    assert_eq!(config.tts.speed, 1.0);
    assert_eq!(
        (config.listening_bar.quiet, config.listening_bar.playing),
        (0.5, 0.8)
    );
    assert_eq!(config.stt.language.as_deref(), Some("es"));
}

#[test]
fn an_unknown_or_missing_key_is_an_error() {
    let base = json!({"vad": {"model": "v"}, "stt": {"model": "s"}, "tts": {"model": "t"}});
    assert!(serde_json::from_value::<VoiceConfig>(base.clone()).is_ok());
    let mut unknown = base.clone();
    unknown["stt"]["prompt"] = json!("x");
    assert!(serde_json::from_value::<VoiceConfig>(unknown).is_err());
    let mut missing = base;
    missing.as_object_mut().unwrap().remove("tts");
    assert!(serde_json::from_value::<VoiceConfig>(missing).is_err());
    let smart: EndOfTurn = serde_json::from_value(json!("smart-turn")).unwrap();
    assert_eq!(smart, EndOfTurn::SmartTurn);
}

#[test]
fn patience_sets_the_silence_and_the_merge_window() {
    let numbers: Vec<_> = [Patience::Fast, Patience::Normal, Patience::Calm]
        .iter()
        .map(|patience| {
            (
                patience.end_of_turn_silence_ms(),
                patience.merge_window_ms(),
            )
        })
        .collect();
    assert_eq!(numbers, [(2_000, 0), (2_500, 500), (3_500, 1_500)]);
}
