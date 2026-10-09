use serde_json::json;

use super::{Playback, PlaybackStatus, RoomEvent, RoomMessage, TurnPhase, TurnTimings, UserTurn};

#[cfg(web)]
use wasm_bindgen_test::wasm_bindgen_test as test;

#[test]
fn a_finished_turn_is_written_as_the_room_reads_it() {
    let message = RoomMessage::UserTurn(UserTurn {
        client_msg_id: "c-2".into(),
        turn_id: "c-turn-0".into(),
        phase: TurnPhase::Finished,
        revision: 4,
        text: Some("hola".into()),
        language: Some("es".into()),
        offline: false,
        started_at: 10,
        ended_at: Some(20),
        merged: false,
        timings: Some(TurnTimings {
            audio_ms: 1_000,
            endpoint_silence_ms: 2_700,
            recognition_ms: Some(300),
        }),
    });
    assert_eq!(message.client_msg_id(), "c-2");
    assert_eq!(
        message.to_json(),
        json!({"type": "voice-user-turn", "data": {
            "client_msg_id": "c-2", "turn_id": "c-turn-0", "phase": "finished", "revision": 4, "text": "hola",
            "language": "es", "offline": false, "started_at": 10, "ended_at": 20, "merged": false,
            "timings": {"audio_ms": 1000, "endpoint_silence_ms": 2700, "recognition_ms": 300}}})
    );
}

#[test]
fn a_playback_report_carries_the_heard_characters() {
    let message = RoomMessage::Playback(Playback {
        client_msg_id: "c-3".into(),
        utterance_id: "u".into(),
        status: PlaybackStatus::Interrupted,
        heard_chars: 24,
        reason: None,
        at: 30,
    });
    assert_eq!(
        message.to_json(),
        json!({"type": "voice-playback", "data": {
            "client_msg_id": "c-3", "utterance_id": "u", "status": "interrupted", "heard_chars": 24, "at": 30}})
    );
}

#[test]
fn a_reply_is_read_and_other_room_messages_are_let_through() {
    let reply = RoomEvent::from_json(&json!({"type": "voice-reply", "data": {
        "utterance_id": "u", "revision": 3, "reply_revision": 4, "thread_id": "t", "history_id": "h",
        "text": "Hecho.", "language": "es", "replay": true, "seq": 9}}))
    .unwrap();
    let RoomEvent::Reply(reply) = reply else {
        panic!("a reply")
    };
    assert_eq!(
        (reply.text.as_str(), reply.replay, reply.language.as_deref()),
        ("Hecho.", true, Some("es"))
    );
    assert_eq!(
        RoomEvent::from_json(&json!({"type": "voice-ping", "data": {}})).unwrap(),
        RoomEvent::Other
    );
    assert!(
        RoomEvent::from_json(&json!({"type": "voice-reply", "data": {"utterance_id": "u"}}))
            .is_err()
    );
    assert!(RoomEvent::from_json(&json!("voice-reply")).is_err());
}
