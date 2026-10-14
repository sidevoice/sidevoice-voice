//! Segmentation on synthetic windows: a 1 kHz tone at a chosen amplitude (whose loudness BS.1770 knows: 20·log10(a)
//! − 3 LUFS), silence, and the detector's confidence for each, 32 ms apart.

use super::{
    instant, loudness, Segment, Segmentation, Segmenter, AUDIO_IDLE_MS, MAX_TURN, PRE_ROLL,
    PRE_SPEECH,
};

#[cfg(web)]
use wasm_bindgen_test::wasm_bindgen_test as test;

/// Samples in one detector window.
const WINDOW: usize = 512;

/// `smart-turn` at normal patience: speech stops after 0.9 s, a pause ends the turn 3 s after that.
const SMART: Segmentation = Segmentation {
    stop_ms: 900,
    end_ms: 3_000,
    ask: true,
    quiet_bar: 0.5,
    playing_bar: 0.8,
    barge_in_min_ms: 1_000,
};

/// `silence` at normal patience: speech stops after 0.2 s, a pause ends the turn 2.5 s after that.
const SILENCE: Segmentation = Segmentation {
    stop_ms: 200,
    end_ms: 2_500,
    ask: false,
    ..SMART
};

/// Speech at a normal level: −23 LUFS, a volume of 0.87.
const LOUD: f32 = 0.1;
/// Soft speech: −43 LUFS (a volume of 0.67), its RMS at −43 dBFS.
const SOFT: f32 = 0.01;
/// A murmur under the bar: −69 LUFS, a volume of 0.41.
const MURMUR: f32 = 0.000_5;

/// One window of a 1 kHz tone at `amplitude` (whole periods, so windows join seamlessly).
fn tone(amplitude: f32) -> Vec<f32> {
    (0..WINDOW)
        .map(|i| {
            let phase = 2.0 * std::f64::consts::PI * 1_000.0 * i as f64 / 16_000.0;
            (f64::from(amplitude) * phase.sin()) as f32
        })
        .collect()
}

/// A window, and the detector's confidence that it is speech.
#[derive(Clone)]
struct Heard(Vec<f32>, f32);

fn speech(amplitude: f32) -> Heard {
    Heard(tone(amplitude), 0.9)
}

fn silence() -> Heard {
    Heard(vec![0.0; WINDOW], 0.02)
}

/// `count` windows of `heard`.
fn times(heard: Heard, count: usize) -> Vec<Heard> {
    vec![heard; count]
}

/// What segmentation did, window by window: the index of the window in `windows` and what it did.
fn hear(segmenter: &mut Segmenter, windows: &[Heard], playing: bool) -> Vec<(usize, Segment)> {
    windows
        .iter()
        .enumerate()
        .filter_map(|(at, Heard(pcm, confidence))| {
            segmenter
                .window(0, pcm, *confidence, playing)
                .map(|segment| (at, segment))
        })
        .collect()
}

/// A segmenter that has heard a second of room tone at a normal level (its volume settled) and has a turn open.
fn speaking(numbers: Segmentation) -> Segmenter {
    let mut segmenter = Segmenter::new(numbers);
    let mut warm = times(Heard(tone(LOUD), 0.02), 32);
    warm.extend(times(speech(LOUD), 12));
    let segments = hear(&mut segmenter, &warm, false);
    assert!(
        matches!(segments[..], [(43, Segment::Started)]),
        "{segments:?}"
    );
    segmenter
}

#[test]
fn the_volume_is_bs_1770_loudness_as_pipecat_measures_it() {
    // The figures pipecat-ai 1.10.0's `loudness` (0.2.0) gives the same 400 ms of tone.
    for (amplitude, lufs) in [
        (0.1, -22.95),
        (0.014_1, -39.95),
        (0.01, -42.95),
        (0.003, -53.45),
    ] {
        let block: Vec<f32> = (0..16).flat_map(|_| tone(amplitude)).take(6_400).collect();
        assert!(
            (loudness(&block) - lufs).abs() < 1e-9,
            "{amplitude}: {}",
            loudness(&block)
        );
    }
    assert_eq!(loudness(&[0.0; 6_400]), f64::NEG_INFINITY);
}

#[test]
fn speech_starts_after_400_ms_of_it_and_a_click_or_a_murmur_opens_nothing() {
    let mut segmenter = Segmenter::new(SMART);
    let mut windows = times(Heard(tone(LOUD), 0.02), 32);
    windows.extend(times(speech(LOUD), 11));
    windows.push(Heard(tone(LOUD), 0.3));
    windows.extend(times(speech(LOUD), 11));
    windows.extend(times(silence(), 40));
    windows.extend(times(speech(MURMUR), 40));
    assert!(hear(&mut segmenter, &windows, false).is_empty());
    assert!(!segmenter.speaking());
    let segments = hear(&mut segmenter, &times(speech(LOUD), 40), false);
    assert!(
        matches!(segments[..], [(_, Segment::Started)]),
        "{segments:?}"
    );
}

#[test]
fn soft_speech_under_minus_30_dbfs_mid_sentence_does_not_pause_the_turn() {
    let mut segmenter = speaking(SMART);
    assert!(instant(&tone(SOFT)) < 0.5, "under −30 dBFS");
    let mut windows = times(speech(LOUD), 30);
    windows.extend(times(speech(SOFT), 120));
    windows.extend(times(speech(LOUD), 30));
    assert!(hear(&mut segmenter, &windows, false).is_empty());
    assert!(segmenter.speaking());
    let segments = hear(&mut segmenter, &times(silence(), 28), false);
    assert!(
        matches!(segments[..], [(27, Segment::Paused { pause: 1, .. })]),
        "{segments:?}"
    );
}

#[test]
fn the_model_is_asked_at_each_stop_and_a_pause_it_left_open_goes_on_with_speech() {
    let mut segmenter = speaking(SMART);
    // A pause of 1.2 s: speech stops at its 28th window (0.9 s), and the model is asked then.
    let mut windows = times(speech(LOUD), 60);
    windows.extend(times(silence(), 38));
    let segments = hear(&mut segmenter, &windows, false);
    assert!(
        matches!(segments[..], [(87, Segment::Paused { pause: 1, .. })]),
        "{segments:?}"
    );
    // It said the turn goes on (it ends nothing); the person goes on, and the answer about that pause is moot.
    assert!(hear(&mut segmenter, &times(speech(LOUD), 40), false).is_empty());
    assert_eq!(segmenter.end_paused(1), None);
    // The next stop asks again, about the turn so far, and the model ends it there.
    let segments = hear(&mut segmenter, &times(silence(), 28), false);
    assert!(
        matches!(segments[..], [(27, Segment::Paused { pause: 2, .. })]),
        "{segments:?}"
    );
    let Some(Segment::Ended(ended)) = segmenter.end_paused(2) else {
        panic!("ended by the model")
    };
    assert_eq!(ended.silence_ms, 896);
    assert!(!segmenter.speaking());
}

#[test]
fn a_pause_ends_the_turn_3_s_after_speech_stopped_whatever_the_model_says() {
    let mut segmenter = speaking(SMART);
    hear(&mut segmenter, &times(speech(LOUD), 60), false);
    // A click in the pause (shorter than speech takes to start) neither ends it nor starts it over.
    let mut windows = times(silence(), 50);
    windows.extend(times(speech(LOUD), 5));
    windows.extend(times(silence(), 100));
    let segments = hear(&mut segmenter, &windows, false);
    let [(27, Segment::Paused { pause: 1, .. }), (121, Segment::Ended(ended))] = &segments[..]
    else {
        panic!("{segments:?}")
    };
    // 0.9 s for speech to stop, and 3 s after it (in whole windows).
    assert_eq!(ended.silence_ms, 122 * 32);
    assert_eq!(segmenter.end_paused(1), None);
}

#[test]
fn with_silence_a_pause_ends_the_turn_2_5_s_after_speech_stopped_and_asks_nothing() {
    let mut segmenter = speaking(SILENCE);
    hear(&mut segmenter, &times(speech(LOUD), 60), false);
    // A breath of 300 ms: speech stops (200 ms) and starts again; the turn goes on.
    let mut windows = times(silence(), 9);
    windows.extend(times(speech(LOUD), 30));
    assert!(hear(&mut segmenter, &windows, false).is_empty());
    let segments = hear(&mut segmenter, &times(silence(), 100), false);
    let [(84, Segment::Ended(ended))] = &segments[..] else {
        panic!("{segments:?}")
    };
    assert_eq!(ended.silence_ms, 85 * 32);
}

#[test]
fn the_bar_rises_while_a_reply_plays() {
    let mut segmenter = Segmenter::new(SMART);
    let mut windows = times(Heard(tone(SOFT), 0.02), 32);
    windows.extend(times(speech(SOFT), 40));
    assert!(
        hear(&mut segmenter, &windows, true).is_empty(),
        "soft speech over a reply"
    );
    let segments = hear(&mut segmenter, &times(speech(SOFT), 12), false);
    assert!(
        matches!(segments[..], [(11, Segment::Started)]),
        "{segments:?}"
    );

    let mut segmenter = Segmenter::new(SMART);
    let mut windows = times(Heard(tone(LOUD), 0.02), 32);
    windows.extend(times(speech(LOUD), 32));
    let segments = hear(&mut segmenter, &windows, true);
    assert!(
        matches!(segments[..], [(63, Segment::Started)]),
        "{segments:?}"
    );
}

#[test]
fn overlap_needs_the_first_full_threshold_window_and_discards_short_preroll() {
    let mut segmenter = Segmenter::new(SMART);
    hear(&mut segmenter, &times(Heard(tone(LOUD), 0.02), 32), true);
    assert!(hear(&mut segmenter, &times(speech(LOUD), 31), true).is_empty());
    assert_eq!(segmenter.start_run(), 0);
    assert!(hear(&mut segmenter, &times(silence(), 1), true).is_empty());
    assert!(hear(&mut segmenter, &times(speech(LOUD), 31), true).is_empty());
    let started = hear(&mut segmenter, &times(speech(LOUD), 1), true);
    assert!(
        matches!(started[..], [(0, Segment::Started)]),
        "{started:?}"
    );
    assert_eq!(segmenter.start_run(), 32 * WINDOW);
    let Some(Segment::Ended(ended)) = segmenter.poll(AUDIO_IDLE_MS) else {
        panic!("turn")
    };
    assert_eq!(
        ended.pcm.len(),
        32 * WINDOW,
        "short overlap leaked into pre-roll"
    );
}

#[test]
fn overlap_resets_on_low_volume_and_when_output_stops() {
    let mut segmenter = Segmenter::new(SMART);
    hear(&mut segmenter, &times(Heard(tone(LOUD), 0.02), 32), true);
    assert!(hear(&mut segmenter, &times(speech(LOUD), 20), true).is_empty());
    assert!(hear(&mut segmenter, &times(Heard(tone(LOUD), 0.02), 1), true).is_empty());
    assert!(hear(&mut segmenter, &times(speech(LOUD), 31), true).is_empty());
    // A candidate heard while output sounded cannot finish under the ordinary 400 ms bar.
    let mut segmenter = Segmenter::new(SMART);
    hear(&mut segmenter, &times(Heard(tone(LOUD), 0.02), 32), true);
    assert!(hear(&mut segmenter, &times(speech(LOUD), 20), true).is_empty());
    assert!(hear(&mut segmenter, &times(speech(LOUD), 11), false).is_empty());
    assert!(matches!(
        hear(&mut segmenter, &times(speech(LOUD), 1), false)[..],
        [(0, Segment::Started)]
    ));
}

#[test]
fn a_below_volume_window_resets_a_nearly_qualified_overlap() {
    let mut segmenter = Segmenter::new(SMART);
    hear(&mut segmenter, &times(Heard(tone(LOUD), 0.02), 32), true);
    assert!(hear(&mut segmenter, &times(speech(LOUD), 31), true).is_empty());
    // Confidence remains high: the level alone falls below the raised bar.
    assert!(hear(&mut segmenter, &times(speech(MURMUR), 13), true).is_empty());
    assert!(hear(&mut segmenter, &times(speech(LOUD), 1), true).is_empty());
    assert!(hear(&mut segmenter, &times(speech(LOUD), 64), true)
        .iter()
        .any(|(_, event)| event == &Segment::Started));
}

#[test]
fn raised_bar_rejects_speech_just_below_point_eight_and_accepts_above_it() {
    for (amplitude, accepted) in [(0.044, false), (0.046, true)] {
        let mut segmenter = Segmenter::new(SMART);
        hear(
            &mut segmenter,
            &times(Heard(tone(amplitude), 0.02), 80),
            true,
        );
        let events = hear(&mut segmenter, &times(speech(amplitude), 32), true);
        assert_eq!(
            events.iter().any(|(_, event)| event == &Segment::Started),
            accepted,
            "volume {} at the 0.8 bar: {events:?}",
            amplitude
        );
    }
}

#[test]
fn a_lone_short_word_in_silence_uses_the_normal_start_detector() {
    let mut segmenter = Segmenter::new(SMART);
    hear(&mut segmenter, &times(Heard(tone(LOUD), 0.02), 32), false);
    assert!(matches!(
        hear(&mut segmenter, &times(speech(LOUD), 12), false)[..],
        [(11, Segment::Started)]
    ));
}

#[test]
fn configured_overlap_duration_counts_capture_samples() {
    let mut segmenter = Segmenter::new(Segmentation {
        barge_in_min_ms: 1_250,
        ..SMART
    });
    hear(&mut segmenter, &times(Heard(tone(LOUD), 0.02), 32), true);
    assert!(hear(&mut segmenter, &times(speech(LOUD), 39), true).is_empty());
    assert!(matches!(
        hear(&mut segmenter, &times(speech(LOUD), 1), true)[..],
        [(0, Segment::Started)]
    ));
    assert_eq!(segmenter.start_run(), 40 * WINDOW);
}

#[test]
fn a_turn_opens_with_its_pre_roll_and_the_model_hears_it_from_before_its_speech() {
    let mut segmenter = speaking(SMART);
    let mut windows = times(speech(LOUD), 60);
    windows.extend(times(silence(), 28));
    let segments = hear(&mut segmenter, &windows, false);
    let [(_, Segment::Paused { pcm, .. })] = &segments[..] else {
        panic!("{segments:?}")
    };
    // 0.9 s before the window speech started with, and all after it.
    assert_eq!(pcm.len(), PRE_SPEECH + 88 * WINDOW);
    let Some(Segment::Ended(ended)) = segmenter.end_paused(1) else {
        panic!("ended")
    };
    // The second before that window, the window, and all after it.
    assert_eq!(ended.pcm.len(), PRE_ROLL + WINDOW + 88 * WINDOW);

    // A long turn: the model hears its last 8 s.
    let mut segmenter = speaking(SMART);
    let mut windows = times(speech(LOUD), 400);
    windows.extend(times(silence(), 28));
    let segments = hear(&mut segmenter, &windows, false);
    let [(_, Segment::Paused { pcm, .. })] = &segments[..] else {
        panic!("{segments:?}")
    };
    assert_eq!(pcm.len(), 8 * 16_000);
}

#[test]
fn a_turn_keeps_a_minute_and_ends_when_its_audio_stops() {
    let mut segmenter = speaking(SMART);
    for _ in 0..(MAX_TURN / WINDOW + 10) {
        segmenter.window(10, &tone(LOUD), 0.9, false);
    }
    assert_eq!(segmenter.deadline(), Some(10 + AUDIO_IDLE_MS));
    assert_eq!(segmenter.poll(10 + AUDIO_IDLE_MS - 1), None);
    let Some(Segment::Ended(ended)) = segmenter.poll(10 + AUDIO_IDLE_MS) else {
        panic!("ended")
    };
    assert_eq!(ended.pcm.len(), MAX_TURN);
    assert_eq!(segmenter.deadline(), None);
    // Speech that went on through it has to start again to open a turn.
    let segments = hear(&mut segmenter, &times(speech(LOUD), 12), false);
    assert!(
        matches!(segments[..], [(11, Segment::Started)]),
        "{segments:?}"
    );
}

#[test]
fn speech_that_stops_in_a_turn_is_a_pause_until_it_starts_again() {
    assert!(!Segmenter::new(SILENCE).paused(), "no turn, no pause");
    let mut segmenter = speaking(SILENCE);
    assert!(!segmenter.paused());
    // 160 ms without speech: not stopped yet, and speech goes on.
    hear(&mut segmenter, &times(silence(), 5), false);
    assert!(!segmenter.paused());
    hear(&mut segmenter, &times(speech(LOUD), 2), false);
    // 0.2 s without it: speech stopped, a pause; the turn goes on.
    hear(&mut segmenter, &times(silence(), 6), false);
    assert!(segmenter.paused() && segmenter.speaking());
    // Speech that starts again ends the pause.
    hear(&mut segmenter, &times(speech(LOUD), 12), false);
    assert!(!segmenter.paused() && segmenter.speaking());
}
