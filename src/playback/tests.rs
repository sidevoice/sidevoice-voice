use super::{Action, Playback};
use crate::event::PlaybackState;
use crate::room::{PlaybackReason, PlaybackStatus, Reply};

#[cfg(web)]
use wasm_bindgen_test::wasm_bindgen_test as test;

fn reply(id: &str, text: &str) -> Reply {
    Reply {
        utterance_id: id.into(),
        revision: 1,
        reply_revision: 2,
        thread_id: "t".into(),
        history_id: "h".into(),
        text: text.into(),
        language: None,
        replay: false,
    }
}

fn statuses(actions: &[Action]) -> Vec<(PlaybackStatus, usize)> {
    actions
        .iter()
        .filter_map(|action| match action {
            Action::Status {
                status,
                heard_chars,
                ..
            } => Some((*status, *heard_chars)),
            _ => None,
        })
        .collect()
}

#[test]
fn chunks_are_synthesized_one_ahead_and_heard_at_their_ends() {
    let mut playback = Playback::default();
    let mut actions = Vec::new();
    playback.push(
        reply(
            "u",
            "The first sentence of it. And the second sentence of it.",
        ),
        &mut actions,
    );
    playback.start(&mut actions);
    assert_eq!(playback.state(), PlaybackState::Synthesizing);
    assert!(matches!(
        &actions[..],
        [Action::Synthesize { chunk: 0, .. }]
    ));
    actions.clear();
    // A result for a chunk not asked for is ignored.
    playback.synthesized("u", 1, Ok((vec![0.0], 16_000)), &mut actions);
    assert!(actions.is_empty());
    playback.synthesized("u", 0, Ok((vec![0.0], 16_000)), &mut actions);
    assert!(matches!(
        &actions[..],
        [
            Action::Play { chunk: 0, .. },
            Action::Synthesize { chunk: 1, .. }
        ]
    ));
    actions.clear();
    playback.chunk_started("u", 0, &mut actions);
    assert_eq!(playback.state(), PlaybackState::Playing);
    assert_eq!(statuses(&actions), [(PlaybackStatus::Playing, 0)]);
    playback.chunk_played("u", 0, &mut actions);
    playback.synthesized("u", 1, Ok((vec![0.0], 16_000)), &mut actions);
    playback.chunk_started("u", 1, &mut actions);
    actions.clear();
    playback.interrupt(false, &mut actions);
    assert_eq!(statuses(&actions), [(PlaybackStatus::Interrupted, 25)]);
    assert!(!playback.busy());
}

#[test]
fn a_reply_with_nothing_to_say_is_heard_at_once() {
    let mut playback = Playback::default();
    let mut actions = Vec::new();
    playback.push(reply("u", "  "), &mut actions);
    assert_eq!(statuses(&actions), [(PlaybackStatus::Heard, 0)]);
    assert!(!playback.waiting());
}

#[test]
fn a_reply_not_yet_sounding_is_unplayed_when_interrupted() {
    let mut playback = Playback::default();
    let mut actions = Vec::new();
    playback.push(reply("u", "Something."), &mut actions);
    playback.start(&mut actions);
    actions.clear();
    playback.interrupt(false, &mut actions);
    assert_eq!(statuses(&actions), [(PlaybackStatus::Unplayed, 0)]);
    assert!(matches!(actions[0], Action::Stop));
}

fn reasons(actions: &[Action]) -> Vec<(PlaybackStatus, Option<PlaybackReason>)> {
    actions
        .iter()
        .filter_map(|action| match action {
            Action::Status { status, reason, .. } => Some((*status, *reason)),
            _ => None,
        })
        .collect()
}

fn synthesized(actions: &[Action]) -> Vec<usize> {
    actions
        .iter()
        .filter_map(|action| match action {
            Action::Synthesize { chunk, .. } => Some(*chunk),
            _ => None,
        })
        .collect()
}

#[test]
fn a_fast_speaker_never_runs_more_than_one_chunk_ahead_of_the_output() {
    let mut playback = Playback::default();
    let mut actions = Vec::new();
    playback.push(reply("u", &LONG.join(" ")), &mut actions);
    playback.start(&mut actions);
    for chunk in 0..5 {
        playback.synthesized("u", chunk, Ok((vec![0.0], 16_000)), &mut actions);
    }
    // The output reports nothing: two chunks are at it, and no third is asked for.
    assert_eq!(synthesized(&actions), [0, 1]);
    actions.clear();
    playback.chunk_started("u", 0, &mut actions);
    assert!(synthesized(&actions).is_empty());
    playback.chunk_played("u", 0, &mut actions);
    assert_eq!(synthesized(&actions), [2]);
}

#[test]
fn a_failed_synthesis_flushes_audio_queued_before_it_sounded() {
    let mut playback = Playback::default();
    let mut actions = Vec::new();
    playback.push(reply("u", &LONG[..2].join(" ")), &mut actions);
    playback.start(&mut actions);
    playback.synthesized("u", 0, Ok((vec![0.0], 16_000)), &mut actions);
    actions.clear();
    // Chunk 0 is at the output and has not started yet when chunk 1 fails.
    playback.synthesized("u", 1, Err("speech-failed".into()), &mut actions);
    assert!(matches!(actions[0], Action::Stop));
    assert!(actions.contains(&Action::Error("speech-failed".into())));
    assert_eq!(reasons(&actions), [(PlaybackStatus::Failed, None)]);
    assert!(!playback.busy());
}

#[test]
fn reasons_are_the_rooms_words() {
    let mut playback = Playback::default();
    let mut actions = Vec::new();
    playback.push(reply("u1", "Playing."), &mut actions);
    playback.push(reply("u2", "Queued."), &mut actions);
    playback.start(&mut actions);
    playback.synthesized("u1", 0, Ok((vec![0.0], 16_000)), &mut actions);
    playback.chunk_started("u1", 0, &mut actions);
    actions.clear();
    playback.interrupt(false, &mut actions);
    assert_eq!(
        reasons(&actions),
        [
            (
                PlaybackStatus::Interrupted,
                Some(PlaybackReason::UserInterrupted)
            ),
            (PlaybackStatus::Unplayed, Some(PlaybackReason::NewerTurn)),
        ]
    );
    playback.push(reply("u3", "Playing."), &mut actions);
    playback.push(reply("u4", "Queued."), &mut actions);
    playback.start(&mut actions);
    actions.clear();
    playback.interrupt(true, &mut actions);
    assert_eq!(
        reasons(&actions),
        [
            (PlaybackStatus::Unplayed, Some(PlaybackReason::CallEnded)),
            (PlaybackStatus::Unplayed, Some(PlaybackReason::CallEnded)),
        ]
    );
}

/// Sentences long enough to be a chunk each.
const LONG: [&str; 5] = [
    "The first sentence is long enough alone.",
    "The second sentence is long enough alone.",
    "The third sentence is long enough alone.",
    "The fourth sentence is long enough alone.",
    "The fifth sentence is long enough alone.",
];

#[test]
fn a_boundary_retires_the_stale_reply_being_spoken_and_those_queued() {
    let mut playback = Playback::default();
    let mut actions = Vec::new();
    let mut old = reply("old", &LONG[..2].join(" "));
    old.revision = 3;
    let mut fresh = reply("fresh", "Written after the turn.");
    fresh.revision = 9;
    let mut queued = reply("queued", "Also old.");
    queued.revision = 4;
    playback.push(old, &mut actions);
    playback.push(fresh, &mut actions);
    playback.push(queued, &mut actions);
    playback.start(&mut actions);
    playback.synthesized("old", 0, Ok((vec![0.0], 16_000)), &mut actions);
    playback.chunk_started("old", 0, &mut actions);
    actions.clear();
    playback.retire_before(8, &mut actions);
    assert!(matches!(actions[0], Action::Stop));
    assert_eq!(
        reasons(&actions),
        [
            (PlaybackStatus::Interrupted, Some(PlaybackReason::NewerTurn)),
            (PlaybackStatus::Unplayed, Some(PlaybackReason::NewerTurn)),
        ]
    );
    assert!(!playback.busy());
    assert!(playback.waiting(), "the reply written after the turn stays");
}
