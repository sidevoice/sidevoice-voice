use super::{accepted, join, Job, Outcome, Recognition, MAX_QUEUE};

#[cfg(web)]
use wasm_bindgen_test::wasm_bindgen_test as test;

fn job(turn: usize) -> Job {
    Job {
        turn,
        pcm: Vec::new(),
        language: None,
        last: true,
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
    assert_eq!(recognition.hold(0, "one".into(), None, 100), Outcome::Held);
    assert_eq!(recognition.deadline(true), None, "a turn is open");
    assert_eq!(recognition.due(100, true), None);
    assert_eq!(
        recognition.hold(1, "two".into(), None, 200),
        Outcome::Joined(vec![0])
    );
    assert_eq!(recognition.due(199, false), None);
    let due = recognition.due(200, false).expect("due");
    assert_eq!((due.turns, due.text.as_str()), (vec![0, 1], "one two"));
}

#[test]
fn the_filter_drops_empty_and_foreign_script_transcripts() {
    assert_eq!(accepted("  hola  ", Some("es")).as_deref(), Some("hola"));
    assert_eq!(accepted(" ", Some("es")), None);
    assert_eq!(accepted("Привет", Some("es")), None);
    assert_eq!(accepted("Привет", Some("ru")).as_deref(), Some("Привет"));
    assert_eq!(
        accepted("你好", None).as_deref(),
        Some("你好"),
        "a detected language is not checked"
    );
    assert_eq!(
        accepted("Ñandú, ça va", Some("es-ES")).as_deref(),
        Some("Ñandú, ça va")
    );
    assert_eq!(accepted("1, 2, 3", Some("en")).as_deref(), Some("1, 2, 3"));
}

#[test]
fn the_script_a_language_is_written_in_comes_from_cldr() {
    // Latin-written languages, by their tag or a regional one: a transcript in no Latin letter is dropped.
    for language in [
        "es", "en", "ca", "eu", "gl", "cy", "ga", "mt", "sq", "tr", "vi", "id", "ms", "sw", "uz",
        "az", "fil", "pt-BR", "es_MX",
    ] {
        assert_eq!(accepted("Привет", Some(language)), None, "{language}");
    }
    // Written in another script: its own letters pass.
    for (language, text) in [
        ("ru", "Привет"),
        ("sr", "Здраво"),
        ("ar", "مرحبا"),
        ("fa", "سلام"),
        ("hi", "नमस्ते"),
        ("ja", "こんにちは"),
        ("zh", "你好"),
        ("ko", "안녕"),
        ("el", "Γεια"),
        ("he", "שלום"),
        ("th", "สวัสดี"),
        ("ka", "გამარჯობა"),
        ("hy", "Բարեւ"),
        ("am", "ሰላም"),
    ] {
        assert_eq!(
            accepted(text, Some(language)).as_deref(),
            Some(text),
            "{language}"
        );
    }
    // A tag that names its script wins over the likely one; one that does not parse checks nothing.
    assert_eq!(accepted("Здраво", Some("sr-Latn")), None);
    assert_eq!(
        accepted("Здраво", Some("not a tag")).as_deref(),
        Some("Здраво")
    );
}

#[test]
fn transcripts_in_different_languages_are_never_joined() {
    let mut recognition = Recognition::default();
    let english = Some("en".to_string());
    let spanish = Some("es".to_string());
    assert_eq!(
        recognition.hold(0, "hello".into(), english, 100),
        Outcome::Held
    );
    let Outcome::Split(earlier) = recognition.hold(1, "hola".into(), spanish, 200) else {
        panic!("another language is not joined");
    };
    assert_eq!((earlier.turns, earlier.text.as_str()), (vec![0], "hello"));
    let due = recognition.due(200, false).expect("due");
    assert_eq!((due.turns, due.text.as_str()), (vec![1], "hola"));
}

#[test]
fn a_turn_s_pieces_count_once_against_the_bound_and_the_last_one_is_told() {
    let piece = |turn| Job {
        last: false,
        ..job(turn)
    };
    let mut recognition = Recognition::default();
    assert_eq!(recognition.push(piece(0)).unwrap(), Some(piece(0)));
    assert!(!recognition.is_last());
    for turn in 1..MAX_QUEUE - 1 {
        assert_eq!(recognition.push(job(turn)).unwrap(), None);
    }
    // The open turn's pieces keep coming with the queue full of turns that ended before it.
    let open = MAX_QUEUE - 1;
    for _ in 0..3 {
        assert_eq!(recognition.push(piece(open)).unwrap(), None);
    }
    assert_eq!(recognition.len(), MAX_QUEUE);
    assert_eq!(recognition.push(job(open)).unwrap(), None);
    assert_eq!(recognition.push(job(MAX_QUEUE)), Err(job(MAX_QUEUE)));
    assert_eq!(recognition.clear(), (0..MAX_QUEUE).collect::<Vec<_>>());
}

#[test]
fn transcripts_join_with_a_space_unless_a_script_written_without_spaces_meets_it() {
    assert_eq!(join("hola,", "¿qué tal?"), "hola, ¿qué tal?");
    assert_eq!(join("你好", "世界"), "你好世界");
    assert_eq!(
        join("今日は。", "元気です"),
        "今日は。元気です",
        "punctuation is skipped"
    );
    assert_eq!(join("สวัสดี", "ครับ"), "สวัสดีครับ");
    assert_eq!(join("OK", "好的"), "OK好的");
    assert_eq!(
        join("안녕하세요", "반가워요"),
        "안녕하세요 반가워요",
        "Korean is spaced"
    );
    assert_eq!(join("1, 2,", "3"), "1, 2, 3");

    let mut recognition = Recognition::default();
    recognition.hold(0, "我们".into(), Some("zh".into()), 100);
    recognition.hold(1, "走吧".into(), Some("zh".into()), 200);
    assert_eq!(recognition.due(200, false).expect("due").text, "我们走吧");
}

#[test]
fn a_held_transcript_shows_the_length_of_its_words_and_not_the_words() {
    let held = super::Pending {
        turns: vec![0],
        text: "canary words 7f3a".into(),
        language: None,
        deadline: 0,
    };
    let debug = format!("{held:?}");
    assert!(
        !debug.contains("canary") && debug.contains("<17 chars>"),
        "{debug}"
    );
}
