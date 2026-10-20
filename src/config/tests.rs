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
    assert_eq!(input.barge_in_min_speech.seconds(), 0.6);
    assert_eq!(
        (input.listening_bar.quiet, input.listening_bar.playing),
        (0.5, 0.65)
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
        serde_json::from_value::<InputConfig>(json!({"listening_bar": {"loud": 0.5}})).is_err()
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

#[test]
fn every_turn_number_is_a_setting_with_its_documented_default() {
    let input: InputConfig = serde_json::from_value(json!({})).unwrap();
    assert_eq!(
        (
            input.barge_in_window,
            input.vad_confidence,
            input.speech_start_ms,
            input.pre_roll_ms
        ),
        (2.5, 0.6, 400, 1_000)
    );
    let custom: InputConfig = serde_json::from_value(json!({
        "listening_bar": {"quiet": 0.4, "playing": 0.9},
        "barge_in_min_speech": 0.3,
        "barge_in_window": 4,
        "vad_confidence": 0.75,
        "speech_start_ms": 250,
        "pre_roll_ms": 0,
    }))
    .unwrap();
    assert_eq!(
        (custom.listening_bar.quiet, custom.listening_bar.playing),
        (0.4, 0.9)
    );
    assert_eq!(
        (
            custom.barge_in_window,
            custom.vad_confidence,
            custom.speech_start_ms,
            custom.pre_roll_ms
        ),
        (4.0, 0.75, 250, 0)
    );
    let call: VoiceConfig = serde_json::from_value(json!({"audio_grace_ms": 300})).unwrap();
    assert_eq!(call.audio_grace_ms, 300);
}

#[test]
fn turn_numbers_out_of_range_are_refused() {
    for value in [
        json!({"barge_in_window": 0.5}),
        json!({"barge_in_window": 11}),
        json!({"vad_confidence": 0}),
        json!({"vad_confidence": 1.5}),
        json!({"speech_start_ms": 5001}),
        json!({"speech_start_ms": -1}),
        json!({"speech_start_ms": 1.5}),
        json!({"pre_roll_ms": 5001}),
        json!({"pre_roll_ms": "1000"}),
        json!({"listening_bar": {"quiet": -0.1, "playing": 0.8}}),
        json!({"listening_bar": {"quiet": 0.5, "playing": 1.2}}),
    ] {
        assert!(
            serde_json::from_value::<InputConfig>(value.clone()).is_err(),
            "{value}"
        );
    }
    assert!(serde_json::from_value::<VoiceConfig>(json!({"audio_grace_ms": 60_001})).is_err());
}

#[test]
fn an_empty_config_is_the_documented_defaults() {
    assert_eq!(
        serde_json::from_value::<InputConfig>(json!({})).unwrap(),
        InputConfig::default()
    );
    assert_eq!(
        serde_json::from_value::<OutputConfig>(json!({})).unwrap(),
        OutputConfig::default()
    );
    assert_eq!(
        serde_json::from_value::<VoiceConfig>(json!({})).unwrap(),
        VoiceConfig::default()
    );
    let input = InputConfig::default();
    assert_eq!(
        (
            input.barge_in_window,
            input.vad_confidence,
            input.speech_start_ms,
            input.pre_roll_ms
        ),
        (2.5, 0.6, 400, 1_000)
    );
}

#[test]
fn each_input_field_omitted_falls_back_to_its_default() {
    let full = json!({
        "language": "es",
        "end_of_turn": {"kind": "smart"},
        "patience": "calm",
        "listening_bar": {"quiet": 0.1, "playing": 0.2},
        "barge_in_min_speech": 2.0,
        "barge_in_window": 4.0,
        "vad_confidence": 0.9,
        "speech_start_ms": 100,
        "pre_roll_ms": 200,
    });
    let given: InputConfig = serde_json::from_value(full.clone()).unwrap();
    let default = InputConfig::default();
    for key in full.as_object().unwrap().keys() {
        let mut partial = full.clone();
        partial.as_object_mut().unwrap().remove(key);
        let read: InputConfig = serde_json::from_value(partial).unwrap();
        let mut expected = given.clone();
        match key.as_str() {
            "language" => expected.language = default.language.clone(),
            "end_of_turn" => expected.end_of_turn = default.end_of_turn,
            "patience" => expected.patience = default.patience,
            "listening_bar" => expected.listening_bar = default.listening_bar,
            "barge_in_min_speech" => expected.barge_in_min_speech = default.barge_in_min_speech,
            "barge_in_window" => expected.barge_in_window = default.barge_in_window,
            "vad_confidence" => expected.vad_confidence = default.vad_confidence,
            "speech_start_ms" => expected.speech_start_ms = default.speech_start_ms,
            "pre_roll_ms" => expected.pre_roll_ms = default.pre_roll_ms,
            other => panic!("{other} is not covered"),
        }
        assert_eq!(read, expected, "without {key}");
    }
}

#[test]
fn each_output_and_call_field_omitted_falls_back_to_its_default() {
    let output: OutputConfig = serde_json::from_value(json!({"speed": 1.5})).unwrap();
    assert_eq!(
        (output.speed, output.voice, output.language),
        (1.5, None, None)
    );
    let output: OutputConfig = serde_json::from_value(json!({"voice": "v"})).unwrap();
    assert_eq!(output.speed, 1.0);
    let call: VoiceConfig = serde_json::from_value(json!({"audio_grace_ms": 5})).unwrap();
    assert_eq!((call.audio_grace_ms, call.idle_unload_minutes), (5, 10));
    let call: VoiceConfig = serde_json::from_value(json!({"idle_unload_minutes": 3})).unwrap();
    assert_eq!((call.audio_grace_ms, call.idle_unload_minutes), (1_000, 3));
}

#[test]
fn a_partial_listening_bar_fills_the_other_volume() {
    let read = |value| serde_json::from_value::<InputConfig>(json!({"listening_bar": value}));
    let bar = read(json!({"quiet": 0.3})).unwrap().listening_bar;
    assert_eq!((bar.quiet, bar.playing), (0.3, 0.65));
    let bar = read(json!({"playing": 0.8})).unwrap().listening_bar;
    assert_eq!((bar.quiet, bar.playing), (0.5, 0.8));
    let bar = read(json!({})).unwrap().listening_bar;
    assert_eq!((bar.quiet, bar.playing), (0.5, 0.65));
    assert!(
        read(json!({"quiet": 1.5})).is_err(),
        "a given volume is still checked"
    );
}
