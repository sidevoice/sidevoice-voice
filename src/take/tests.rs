use super::{Recording, TakePause, TakeTurn, MAX_TAKE};

#[cfg(web)]
use wasm_bindgen_test::wasm_bindgen_test as test;

fn turn(id: &str, speech_ms: u64, pauses: &[(u64, u64)], end_ms: Option<u64>) -> TakeTurn {
    TakeTurn {
        turn_id: id.into(),
        speech_ms,
        pauses: pauses
            .iter()
            .map(|&(from_ms, to_ms)| TakePause { from_ms, to_ms })
            .collect(),
        end_ms,
    }
}

#[test]
fn a_take_keeps_what_was_heard_with_silence_while_muted_and_its_times_from_its_start() {
    let mut recording = Recording::new(1_000);
    recording.extend(&[0.5; 160], false);
    recording.extend(&[0.5; 160], true);
    recording.extend(&[0.25; 160], false);
    let take = recording.finish(vec![turn("t-0", 0, &[(5, 10)], Some(20))]);
    assert_eq!(take.sample_rate, 16_000);
    assert_eq!(take.samples.len(), 480);
    assert!(take.samples[..160].iter().all(|&s| s == 0.5));
    assert!(take.samples[160..320].iter().all(|&s| s == 0.0), "muted");
    assert!(take.samples[320..].iter().all(|&s| s == 0.25));
    assert_eq!((take.started_at, take.ended_at), (1_000, 1_030));
    assert_eq!(take.turns, [turn("t-0", 0, &[(5, 10)], Some(20))]);
}

#[test]
fn a_take_keeps_two_minutes_and_the_marks_within_them() {
    let mut recording = Recording::new(0);
    for _ in 0..(MAX_TAKE / 16_000 + 2) {
        recording.extend(&[0.1; 16_000], false);
    }
    let take = recording.finish(vec![
        turn("t-0", 1_000, &[(2_000, 3_000), (119_000, 121_000)], None),
        turn("t-1", 119_500, &[(120_500, 121_000)], Some(125_000)),
        turn("t-2", 121_000, &[], Some(122_000)),
    ]);
    assert_eq!(take.samples.len(), MAX_TAKE);
    assert_eq!(take.ended_at, 120_000);
    assert_eq!(
        take.turns,
        [
            turn("t-0", 1_000, &[(2_000, 3_000), (119_000, 120_000)], None),
            turn("t-1", 119_500, &[], None),
        ]
    );
}
