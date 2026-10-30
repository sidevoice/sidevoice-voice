//! Which devices are chosen, without the system's: what a vanished choice becomes, and when the audio thread switches.

use super::{lock, settle, AudioDevice, Choice, Kind, NativeDevices};
use crate::platform::audio_session::{AudioSession, Session};

fn device(id: &str) -> AudioDevice {
    AudioDevice {
        id: id.into(),
        label: id.to_uppercase(),
    }
}

fn chosen(input: Option<&str>, output: Option<&str>) -> NativeDevices {
    let devices = NativeDevices::new();
    *lock(&devices.shared.choice) = Choice {
        input: input.map(Into::into),
        output: output.map(Into::into),
    };
    devices
}

#[test]
fn a_choice_the_list_lacks_is_gone_and_the_default_never_is() {
    let (inputs, outputs) = ([device("alsa:mic")], [device("alsa:speaker")]);
    let choice = |input: Option<&str>, output: Option<&str>| Choice {
        input: input.map(Into::into),
        output: output.map(Into::into),
    };
    assert!(settle(&choice(None, None), &inputs, &outputs).is_empty());
    assert!(settle(
        &choice(Some("alsa:mic"), Some("alsa:speaker")),
        &inputs,
        &outputs
    )
    .is_empty());
    assert_eq!(
        settle(
            &choice(Some("alsa:headset"), Some("alsa:speaker")),
            &inputs,
            &outputs
        ),
        [Kind::Input]
    );
    assert_eq!(
        settle(
            &choice(Some("alsa:headset"), Some("alsa:mic")),
            &inputs,
            &outputs
        ),
        [Kind::Input, Kind::Output]
    );
}

#[test]
fn a_lost_choice_falls_back_to_the_default_and_is_said_once() {
    let devices = chosen(Some("alsa:headset"), Some("alsa:speaker"));
    assert!(devices.lose(Kind::Input));
    assert!(devices.take_switch(Kind::Input));
    assert!(!devices.take_switch(Kind::Input));
    assert!(!devices.take_switch(Kind::Output));
    assert_eq!(devices.chosen().input, None);
    assert_eq!(devices.chosen().output.as_deref(), Some("alsa:speaker"));
    // The default one is not lost again.
    assert!(!devices.lose(Kind::Input));

    let listed = devices.report(vec![device("alsa:mic")], vec![device("alsa:speaker")]);
    assert_eq!(listed.input, None);
    assert_eq!(listed.output.as_deref(), Some("alsa:speaker"));
    let lost = listed.lost.expect("the lost microphone");
    assert_eq!(lost.input.as_deref(), Some("alsa:headset"));
    assert_eq!(lost.output, None);
    let again = devices.report(vec![device("alsa:mic")], vec![device("alsa:speaker")]);
    assert_eq!(again.lost, None);
}

#[test]
fn a_list_without_the_chosen_speaker_loses_it_and_asks_for_a_switch() {
    let devices = chosen(None, Some("alsa:headset"));
    let listed = devices.report(vec![device("alsa:mic")], vec![device("alsa:speaker")]);
    assert_eq!(listed.output, None);
    assert_eq!(
        listed.lost.and_then(|lost| lost.output).as_deref(),
        Some("alsa:headset")
    );
    assert!(devices.take_switch(Kind::Output));
    assert!(!devices.take_switch(Kind::Input));
}

#[test]
fn the_list_serialises_with_lost_only_when_something_was() {
    let devices = chosen(Some("alsa:mic"), None);
    let listed = devices.report(vec![device("alsa:mic")], vec![]);
    assert_eq!(
        serde_json::to_value(&listed).unwrap(),
        serde_json::json!({
            "inputs": [{"id": "alsa:mic", "label": "ALSA:MIC"}],
            "outputs": [],
            "input": "alsa:mic",
            "output": null,
        })
    );
}

#[test]
fn the_list_says_whether_the_speaker_can_be_chosen_as_the_system_does_under_its_seam_name() {
    let devices = NativeDevices::new().report(vec![device("mic")], vec![device("speaker")]);
    assert_eq!(devices.output_selectable, Session::OUTPUT_SELECTABLE);
    let json = serde_json::to_value(&devices).expect("a list serializes");
    assert_eq!(json["outputSelectable"], Session::OUTPUT_SELECTABLE);
    assert!(json.get("output_selectable").is_none());
}
