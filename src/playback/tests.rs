use super::{Action, Playback};
use crate::event::PlaybackState;
use crate::say::{SayEvent, SayOutcome, StopReason};

#[cfg(web)]
use wasm_bindgen_test::wasm_bindgen_test as test;

/// Sentences long enough to be a chunk each.
const LONG: [&str; 5] = [
    "The first sentence is long enough alone.",
    "The second sentence is long enough alone.",
    "The third sentence is long enough alone.",
    "The fourth sentence is long enough alone.",
    "The fifth sentence is long enough alone.",
];

fn push(playback: &mut Playback, id: &str, text: &str, actions: &mut Vec<Action>) {
    playback.push(id.into(), text, None, actions);
}

/// The outcomes among `actions`, as (what was said, how it ended).
fn outcomes(actions: &[Action]) -> Vec<(String, SayOutcome)> {
    actions
        .iter()
        .filter_map(|action| match action {
            Action::Say {
                utterance,
                event: SayEvent::Done { outcome },
            } => Some((utterance.clone(), outcome.clone())),
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
fn chunks_are_synthesized_one_ahead_and_heard_at_their_ends() {
    let mut playback = Playback::default();
    let mut actions = Vec::new();
    push(&mut playback, "u", &LONG[..2].join(" "), &mut actions);
    playback.start(&mut actions);
    assert_eq!(playback.state(), PlaybackState::Synthesizing);
    assert_eq!(synthesized(&actions), [0]);
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
    assert_eq!(
        actions,
        [
            Action::Say {
                utterance: "u".into(),
                event: SayEvent::Playing
            },
            Action::Say {
                utterance: "u".into(),
                event: SayEvent::Progress {
                    sounding: Some((0, 40)),
                    heard_chars: 0
                }
            },
        ]
    );
    playback.chunk_played("u", 0, &mut actions);
    playback.synthesized("u", 1, Ok((vec![0.0], 16_000)), &mut actions);
    playback.chunk_started("u", 1, &mut actions);
    playback.chunk_played("u", 1, &mut actions);
    assert_eq!(outcomes(&actions), [("u".into(), SayOutcome::Heard)]);
    assert!(!playback.busy());
}

#[test]
fn something_with_nothing_to_say_is_heard_at_once() {
    let mut playback = Playback::default();
    let mut actions = Vec::new();
    push(&mut playback, "u", "  ", &mut actions);
    assert_eq!(outcomes(&actions), [("u".into(), SayOutcome::Heard)]);
    assert!(!playback.waiting());
}

#[test]
fn a_barge_in_cuts_what_sounds_where_it_was_heard_and_drops_the_queue() {
    let mut playback = Playback::default();
    let mut actions = Vec::new();
    push(&mut playback, "u1", &LONG[..2].join(" "), &mut actions);
    push(&mut playback, "u2", LONG[2], &mut actions);
    playback.start(&mut actions);
    playback.synthesized("u1", 0, Ok((vec![0.0], 16_000)), &mut actions);
    playback.chunk_started("u1", 0, &mut actions);
    playback.chunk_played("u1", 0, &mut actions);
    playback.synthesized("u1", 1, Ok((vec![0.0], 16_000)), &mut actions);
    playback.chunk_started("u1", 1, &mut actions);
    actions.clear();
    playback.interrupt(StopReason::BargeIn, &mut actions);
    assert!(matches!(actions[0], Action::Stop));
    assert_eq!(
        outcomes(&actions),
        [
            (
                "u1".into(),
                SayOutcome::HeardUpTo {
                    heard_chars: 40,
                    reason: StopReason::BargeIn
                }
            ),
            (
                "u2".into(),
                SayOutcome::NotPlayed {
                    reason: StopReason::BargeIn
                }
            ),
        ]
    );
    assert!(!playback.busy() && !playback.waiting());
}

#[test]
fn a_cancel_stops_what_is_said_or_drops_it_from_the_queue() {
    let mut playback = Playback::default();
    let mut actions = Vec::new();
    push(&mut playback, "u1", LONG[0], &mut actions);
    push(&mut playback, "u2", LONG[1], &mut actions);
    push(&mut playback, "u3", LONG[2], &mut actions);
    playback.start(&mut actions);
    actions.clear();
    assert!(playback.cancel("u2", &mut actions));
    assert_eq!(
        outcomes(&actions),
        [(
            "u2".into(),
            SayOutcome::NotPlayed {
                reason: StopReason::Cancelled
            }
        )]
    );
    actions.clear();
    assert!(playback.cancel("u1", &mut actions));
    assert_eq!(
        outcomes(&actions),
        [(
            "u1".into(),
            SayOutcome::NotPlayed {
                reason: StopReason::Cancelled
            }
        )]
    );
    assert!(!playback.cancel("u1", &mut actions), "already gone");
    playback.start(&mut actions);
    assert!(actions
        .iter()
        .any(|action| matches!(action, Action::Synthesize { utterance, .. } if utterance == "u3")));
}

#[test]
fn a_fast_speaker_never_runs_more_than_one_chunk_ahead_of_the_output() {
    let mut playback = Playback::default();
    let mut actions = Vec::new();
    push(&mut playback, "u", &LONG.join(" "), &mut actions);
    playback.start(&mut actions);
    for chunk in 0..5 {
        playback.synthesized("u", chunk, Ok((vec![0.0], 16_000)), &mut actions);
    }
    assert_eq!(synthesized(&actions), [0, 1]);
    actions.clear();
    playback.chunk_started("u", 0, &mut actions);
    assert!(synthesized(&actions).is_empty());
    playback.chunk_played("u", 0, &mut actions);
    assert_eq!(synthesized(&actions), [2]);
}

#[test]
fn a_failed_synthesis_flushes_audio_queued_before_it_sounded_and_says_why() {
    let mut playback = Playback::default();
    let mut actions = Vec::new();
    push(&mut playback, "u", &LONG[..2].join(" "), &mut actions);
    playback.start(&mut actions);
    playback.synthesized("u", 0, Ok((vec![0.0], 16_000)), &mut actions);
    actions.clear();
    playback.synthesized("u", 1, Err("speech-failed".into()), &mut actions);
    assert!(actions.contains(&Action::Stop));
    assert!(actions.contains(&Action::Error("speech-failed".into())));
    assert_eq!(
        outcomes(&actions),
        [(
            "u".into(),
            SayOutcome::NotPlayed {
                reason: StopReason::Failed {
                    code: "speech-failed".into()
                }
            }
        )]
    );
    assert!(!playback.busy());
}
