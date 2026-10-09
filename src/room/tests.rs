use serde_json::json;

use super::{
    Playback, PlaybackReason, PlaybackStatus, RoomEvent, RoomMessage, TurnPhase, TurnTimings,
    UserTurn,
};

#[cfg(web)]
use wasm_bindgen_test::wasm_bindgen_test as test;

#[test]
fn a_finished_turn_is_written_as_the_room_reads_it() {
    let message = RoomMessage::UserTurn(UserTurn {
        client_msg_id: "c-2".into(),
        turn_id: "c-turn-0".into(),
        phase: TurnPhase::Finished,
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
            "client_msg_id": "c-2", "turn_id": "c-turn-0", "phase": "finished", "text": "hola",
            "language": "es", "offline": false, "started_at": 10, "ended_at": 20, "merged": false,
            "timings_ms": {"audio_ms": 1000, "endpoint_silence_ms": 2700, "recognition_ms": 300}}})
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

#[test]
fn a_playback_reason_is_one_the_room_takes() {
    let message = RoomMessage::Playback(Playback {
        client_msg_id: "c-4".into(),
        utterance_id: "u".into(),
        status: PlaybackStatus::Unplayed,
        heard_chars: 0,
        reason: Some(PlaybackReason::NewerTurn),
        at: 40,
    });
    assert_eq!(message.to_json()["data"]["reason"], "newer_turn");
    // The room's vocabulary (sidevoice-core `control/room/playback.rs`, `REASONS`).
    for (reason, word) in [
        (PlaybackReason::UserInterrupted, "user_interrupted"),
        (PlaybackReason::NewerTurn, "newer_turn"),
        (PlaybackReason::CallEnded, "call_ended"),
    ] {
        assert_eq!(serde_json::to_value(reason).unwrap(), word);
    }
}

#[test]
fn the_rooms_answer_to_a_started_turn_names_it_and_gives_its_revision() {
    let started = json!({"type": "voice-user-turn", "data": {
        "session_id": "s", "phase": "started", "turn_id": "c-turn-0", "revision": 12, "thread_id": "t"}});
    assert_eq!(
        RoomEvent::from_json(&started).unwrap(),
        RoomEvent::TurnStarted {
            turn_id: "c-turn-0".into(),
            revision: 12
        }
    );
    let unnamed = json!({"type": "voice-user-turn", "data": {"phase": "started", "revision": 12}});
    assert!(
        RoomEvent::from_json(&unnamed).is_err(),
        "a started answer names its turn"
    );
    let cancelled =
        json!({"type": "voice-user-turn", "data": {"phase": "cancelled", "revision": 12}});
    assert_eq!(RoomEvent::from_json(&cancelled).unwrap(), RoomEvent::Other);
}
