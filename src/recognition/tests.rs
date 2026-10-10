use super::{accepted, Job, Outcome, Recognition, MAX_QUEUE};

#[cfg(web)]
use wasm_bindgen_test::wasm_bindgen_test as test;

fn job(turn: usize) -> Job {
    Job {
        turn,
        pcm: Vec::new(),
    }
}

#[test]
fn one_job_runs_at_a_time_in_order_and_the_queue_is_bounded() {
    let mut recognition = Recognition::default();
    assert_eq!(recognition.push(job(0)).unwrap(), Some(job(0)));
    for turn in 1..MAX_QUEUE {
        assert_eq!(recognition.push(job(turn)).unwrap(), None);
    }
    assert_eq!(recognition.push(job(MAX_QUEUE)), Err(job(MAX_QUEUE)));
    assert!(recognition.is_active(0));
    assert_eq!(recognition.done(), Some(job(1)));
    assert_eq!(recognition.clear(), (1..MAX_QUEUE).collect::<Vec<_>>());
    assert_eq!(recognition.len(), 0);
}

#[test]
fn a_held_transcript_is_due_only_when_nothing_can_join_it() {
    let mut recognition = Recognition::default();
    assert_eq!(recognition.hold(0, "one".into(), 100), Outcome::Held);
    assert_eq!(recognition.deadline(true), None, "a turn is open");
    assert_eq!(recognition.due(100, true), None);
    assert_eq!(
        recognition.hold(1, "two".into(), 200),
        Outcome::Joined(vec![0])
    );
    assert_eq!(recognition.due(199, false), None);
    let due = recognition.due(200, false).expect("due");
    assert_eq!((due.turns, due.text.as_str()), (vec![0, 1], "one two"));
}

#[test]
fn the_filter_drops_empty_foreign_script_and_unlikely_transcripts() {
    assert_eq!(
        accepted("  hola  ", Some("es"), None).as_deref(),
        Some("hola")
    );
    assert_eq!(accepted(" ", Some("es"), None), None);
    assert_eq!(accepted("Привет", Some("es"), None), None);
    assert_eq!(
        accepted("Привет", Some("ru"), None).as_deref(),
        Some("Привет")
    );
    assert_eq!(
        accepted("你好", None, None).as_deref(),
        Some("你好"),
        "a detected language is not checked"
    );
    assert_eq!(
        accepted("Ñandú, ça va", Some("es-ES"), None).as_deref(),
        Some("Ñandú, ça va")
    );
    assert_eq!(
        accepted("1, 2, 3", Some("en"), None).as_deref(),
        Some("1, 2, 3")
    );
    assert_eq!(
        accepted("yes", Some("en"), Some(-2.5)).as_deref(),
        Some("yes")
    );
    assert_eq!(accepted("yes", Some("en"), Some(-3.5)), None);
    assert_eq!(
        accepted("a longer sentence here", Some("en"), Some(-2.5)),
        None
    );
}
