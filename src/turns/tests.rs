//! Segmentation on synthetic windows: a 1 kHz tone at a chosen amplitude (whose loudness BS.1770 knows: 20·log10(a)
//! − 3 LUFS), silence, and the detector's confidence for each, 32 ms apart.

use crate::test_support::{clip, EnergyVad, FLEURS_ES, QUILTER};

use super::{instant, loudness, Segment, Segmentation, Segmenter, AUDIO_IDLE_MS, MAX_TURN};

#[cfg(web)]
use wasm_bindgen_test::wasm_bindgen_test as test;

/// Samples in one detector window.
const WINDOW: usize = 512;
/// The default pre-roll: a second of audio.
const PRE_ROLL_SAMPLES: usize = 16_000;
/// The audio an end-of-turn model is given from before speech started: 500 ms and the 400 ms it took to start.
const PRE_SPEECH_SAMPLES: usize = 14_400;

/// `smart-turn` at normal patience: speech stops after 0.9 s, a pause ends the turn 3 s after that.
const SMART: Segmentation = Segmentation {
    stop_ms: 900,
    end_ms: 3_000,
    ask: true,
    quiet_bar: 0.5,
    playing_bar: 0.65,
    barge_in_min_ms: 600,
    barge_in_window: 2.5,
    confidence: 0.6,
    start_ms: 400,
    pre_roll_ms: 1_000,
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
/// Faint speech: −49 LUFS (a volume of 0.61), under the bar while a reply plays but over the one in silence.
const FAINT: f32 = 0.005;
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
    let mut windows = times(Heard(tone(FAINT), 0.02), 32);
    windows.extend(times(speech(FAINT), 40));
    assert!(
        hear(&mut segmenter, &windows, true).is_empty(),
        "soft speech over a reply"
    );
    let segments = hear(&mut segmenter, &times(speech(FAINT), 12), false);
    assert!(
        matches!(segments[..], [(11, Segment::Started)]),
        "{segments:?}"
    );

    let mut segmenter = Segmenter::new(SMART);
    let mut windows = times(Heard(tone(LOUD), 0.02), 32);
    windows.extend(times(speech(LOUD), 32));
    let segments = hear(&mut segmenter, &windows, true);
    assert!(
        matches!(segments[..], [(50, Segment::Started)]),
        "{segments:?}"
    );
}

/// The windows of a person who speaks in bursts over a reply: `speech` windows of speech at `amplitude`, then
/// `gap` of silence, `count` times.
fn bursts(amplitude: f32, speech_windows: usize, gap: usize, count: usize) -> Vec<Heard> {
    let mut windows = Vec::new();
    for _ in 0..count {
        windows.extend(times(speech(amplitude), speech_windows));
        windows.extend(times(silence(), gap));
    }
    windows
}

/// A segmenter that has heard a second of room tone at a normal level, with a reply sounding.
fn playing(numbers: Segmentation) -> Segmenter {
    let mut segmenter = Segmenter::new(numbers);
    assert!(hear(&mut segmenter, &times(Heard(tone(LOUD), 0.02), 32), true).is_empty());
    segmenter
}

#[test]
fn speech_over_a_reply_interrupts_after_the_minimum_of_it() {
    // 0.6 s is 18.75 windows of 32 ms.
    let mut segmenter = playing(SMART);
    assert!(hear(&mut segmenter, &times(speech(LOUD), 18), true).is_empty());
    let started = hear(&mut segmenter, &times(speech(LOUD), 1), true);
    assert!(
        matches!(started[..], [(0, Segment::Started)]),
        "{started:?}"
    );
    assert_eq!(segmenter.start_run(), 19 * WINDOW);
}

#[test]
fn bursts_with_pauses_over_a_reply_add_up_to_an_interruption() {
    // "vale, para… espera… para": 450 ms of speech, 250 ms of pause. Each burst is short of the minimum.
    let mut segmenter = playing(SMART);
    let started = hear(&mut segmenter, &bursts(LOUD, 14, 8, 3), true);
    let [(at, Segment::Started)] = started[..] else {
        panic!("{started:?}")
    };
    assert_eq!(at, 14 + 8 + 4, "the second burst completes the 19 windows");
    // The turn keeps the first burst and the pause: it opens at the first speech heard.
    assert_eq!(segmenter.start_run(), (14 + 8 + 4 + 1) * WINDOW);
}

#[test]
fn short_bursts_with_longer_pauses_do_not_add_up() {
    // Each burst falls out of the 1.5 s window before the next one is heard.
    let mut segmenter = playing(SMART);
    assert!(hear(&mut segmenter, &bursts(LOUD, 8, 50, 6), true).is_empty());
}

#[test]
fn a_backchannel_over_a_reply_does_not_interrupt() {
    let mut segmenter = playing(SMART);
    assert!(hear(&mut segmenter, &times(speech(LOUD), 12), true).is_empty());
    assert!(hear(&mut segmenter, &times(silence(), 60), true).is_empty());
    assert!(hear(&mut segmenter, &bursts(LOUD, 5, 25, 6), true).is_empty());
}

#[test]
fn the_audio_before_a_failed_overlap_never_reaches_a_turn() {
    let mut segmenter = playing(SMART);
    assert!(hear(&mut segmenter, &times(speech(LOUD), 10), true).is_empty());
    assert!(hear(&mut segmenter, &times(silence(), 50), true).is_empty());
    assert!(
        segmenter.recent.is_empty(),
        "audio older than the window is forgotten"
    );
    // The volume climbs again from the pause, so speech qualifies a few windows after it starts.
    let started = hear(&mut segmenter, &times(speech(LOUD), 40), true);
    let [(at, Segment::Started)] = started[..] else {
        panic!("{started:?}")
    };
    assert!(at >= 18, "{started:?}");
    // From the first window the detector held for speech, the ones the volume had not yet raised included; the old
    // overlap, forgotten in the silence, is left out.
    assert_eq!(segmenter.start_run(), (at + 1) * WINDOW);
    let Some(Segment::Ended(ended)) = segmenter.poll(AUDIO_IDLE_MS) else {
        panic!("turn")
    };
    assert_eq!(ended.pcm.len(), 40 * WINDOW, "the earlier overlap leaked");
}

/// Plays `first` windows of a soft opening word, held for speech under the playing bar, then loud speech, over a reply
/// whose room tone is as soft as the word; the windows heard, and where the turn opened.
fn interrupted_softly(numbers: Segmentation, first: usize) -> (Segmenter, usize) {
    let mut segmenter = Segmenter::new(numbers);
    hear(&mut segmenter, &times(Heard(tone(FAINT), 0.02), 80), true);
    let mut windows = times(speech(FAINT), first);
    windows.extend(times(speech(LOUD), 60));
    let started = hear(&mut segmenter, &windows, true);
    let [(at, Segment::Started)] = started[..] else {
        panic!("{started:?}")
    };
    (segmenter, at)
}

#[test]
fn the_first_word_under_the_playing_bar_is_kept_when_the_next_words_clear_it() {
    // "pero" is held for speech but under the bar: alone it interrupts nothing, and the words that follow do.
    let mut segmenter = Segmenter::new(SMART);
    hear(&mut segmenter, &times(Heard(tone(FAINT), 0.02), 80), true);
    assert!(hear(&mut segmenter, &times(speech(FAINT), 40), true).is_empty());
    assert_eq!(segmenter.overlap.speech(), 0);

    let (mut segmenter, at) = interrupted_softly(SMART, 6);
    // The soft word and the loud windows up to the one that opened the turn: every window heard since the word began.
    assert_eq!(segmenter.start_run(), (at + 1) * WINDOW);
    let Some(Segment::Ended(ended)) = segmenter.poll(AUDIO_IDLE_MS) else {
        panic!("turn")
    };
    assert_eq!(ended.pcm.len(), (6 + 60) * WINDOW);
    assert_eq!(ended.pcm[..WINDOW], tone(FAINT)[..], "the first word");
}

#[test]
fn playback_the_detector_never_held_for_speech_stays_out_of_the_turn() {
    // A second of reply audio (not held) before the soft word: the turn opens from the word, not from the reply.
    let mut segmenter = Segmenter::new(SMART);
    hear(&mut segmenter, &times(Heard(tone(FAINT), 0.02), 80), true);
    assert!(hear(&mut segmenter, &times(Heard(tone(FAINT), 0.1), 30), true).is_empty());
    let mut windows = times(speech(FAINT), 6);
    windows.extend(times(speech(LOUD), 60));
    let started = hear(&mut segmenter, &windows, true);
    assert!(
        matches!(started[..], [(_, Segment::Started)]),
        "{started:?}"
    );
    let Some(Segment::Ended(ended)) = segmenter.poll(AUDIO_IDLE_MS) else {
        panic!("turn")
    };
    assert_eq!(ended.pcm.len(), (6 + 60) * WINDOW, "the reply leaked");
}

#[test]
fn the_opening_before_the_first_qualified_window_is_bounded_by_the_pre_roll() {
    // 20 windows (640 ms) of soft speech, and the loud ones up to the bar: a 200 ms pre-roll keeps only the last of it
    // before the 19 windows that cleared the bar.
    let numbers = Segmentation {
        pre_roll_ms: 200,
        ..SMART
    };
    let (segmenter, at) = interrupted_softly(numbers, 20);
    assert!(
        at + 1 - 19 > 3_200 / WINDOW,
        "more is held than the pre-roll"
    );
    assert_eq!(segmenter.start_run(), 19 * WINDOW + 3_200);
}

#[test]
fn overlap_is_forgotten_when_output_stops() {
    // A candidate heard while output sounded cannot finish under the ordinary 400 ms bar.
    let mut segmenter = playing(SMART);
    assert!(hear(&mut segmenter, &times(speech(LOUD), 18), true).is_empty());
    assert!(hear(&mut segmenter, &times(speech(LOUD), 11), false).is_empty());
    assert!(matches!(
        hear(&mut segmenter, &times(speech(LOUD), 1), false)[..],
        [(0, Segment::Started)]
    ));
}

#[test]
fn speech_under_the_raised_bar_adds_nothing() {
    let mut segmenter = Segmenter::new(SMART);
    hear(&mut segmenter, &times(Heard(tone(MURMUR), 0.02), 32), true);
    // Confidence is high: the volume alone falls below the raised bar.
    assert!(hear(&mut segmenter, &times(speech(MURMUR), 100), true).is_empty());
    assert_eq!(segmenter.overlap.speech(), 0);
    // Held for speech, so kept as a possible opening word, but no more than the sliding window of it: 1.5 s.
    assert_eq!(segmenter.recent.len(), 24_000 / WINDOW * WINDOW);
}

#[test]
fn the_raised_bar_rejects_speech_just_below_point_six_five_and_accepts_above_it() {
    // A volume of 0.65 is −45 LUFS: a tone of amplitude 0.0079.
    for (amplitude, accepted) in [(0.007_5, false), (0.008_4, true)] {
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
            "volume {amplitude} at the 0.65 bar: {events:?}"
        );
    }
}

#[test]
fn the_minimum_and_its_window_follow_the_configuration() {
    let mut segmenter = playing(Segmentation {
        barge_in_min_ms: 1_250,
        ..SMART
    });
    // 40 windows of speech are needed within 3.125 s (97 windows): 20 and a pause of 20 fall short of them.
    assert!(hear(&mut segmenter, &bursts(LOUD, 20, 20, 1), true).is_empty());
    let started = hear(&mut segmenter, &times(speech(LOUD), 30), true);
    let [(at, Segment::Started)] = started[..] else {
        panic!("{started:?}")
    };
    assert!((19..30).contains(&at), "{started:?}");
    assert_eq!(segmenter.start_run(), (40 + at + 1) * WINDOW);
}

/// Speech from the recorded clips over a reply, after a second of room tone: the milliseconds from the first sample of
/// `audio` to the interruption, if it comes.
fn interrupted_at(
    numbers: Segmentation,
    audio: &[f32],
    confidence: impl Fn(&[f32]) -> f32,
) -> Option<usize> {
    let mut segmenter = playing(numbers);
    audio
        .as_chunks::<WINDOW>()
        .0
        .iter()
        .enumerate()
        .find_map(|(at, pcm)| {
            matches!(
                segmenter.window(0, pcm, confidence(pcm), true),
                Some(Segment::Started)
            )
            .then_some((at + 1) * WINDOW / 16)
        })
}

/// The recorded Spanish clip as a person speaking it in bursts of `burst_ms` with `gap_ms` between them.
fn spoken_in_bursts(peak: f32, burst_ms: usize, gap_ms: usize) -> Vec<f32> {
    let voice = clip(FLEURS_ES, peak);
    let mut audio = Vec::new();
    // The clip starts with a second of near silence.
    let mut from = 16_000;
    for _ in 0..6 {
        audio.extend_from_slice(&voice[from..from + burst_ms * 16]);
        audio.extend(silence_ms(gap_ms));
        from += burst_ms * 16 + 300 * 16;
    }
    audio
}

fn silence_ms(ms: usize) -> Vec<f32> {
    vec![0.0; ms * 16]
}

#[test]
fn recorded_speech_in_bursts_interrupts_a_reply() {
    // Real speech dips below the bar between words, so even unbroken talking is a series of bursts.
    for (peak, burst, gap) in [(0.3, 450, 250), (0.2, 350, 300), (0.15, 450, 250)] {
        let audio = spoken_in_bursts(peak, burst, gap);
        let at = interrupted_at(SMART, &audio, EnergyVad::probability);
        assert!(
            at.is_some_and(|ms| ms <= 2_000),
            "peak {peak}, bursts {burst}/{gap}: {at:?}"
        );
    }
    let talking = clip(FLEURS_ES, 0.3);
    assert!(interrupted_at(SMART, &talking, EnergyVad::probability).is_some_and(|ms| ms <= 2_000));
}

#[test]
fn recorded_echo_of_the_reply_does_not_interrupt_it() {
    // The reply as the canceller leaves it: far below the person's voice. The detector is fooled into calling it
    // speech, so only the raised bar stands in the way.
    for peak in [0.02, 0.03] {
        let echo = clip(QUILTER, peak);
        assert_eq!(
            interrupted_at(SMART, &echo, EnergyVad::probability),
            None,
            "echo at a peak of {peak}"
        );
    }
    // A detector that does not take it for speech stops it however loud it is.
    let loud = clip(QUILTER, 0.3);
    assert_eq!(interrupted_at(SMART, &loud, |_| 0.05), None);
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
fn a_turn_opens_with_its_pre_roll_and_the_model_hears_it_from_before_its_speech() {
    let mut segmenter = speaking(SMART);
    let mut windows = times(speech(LOUD), 60);
    windows.extend(times(silence(), 28));
    let segments = hear(&mut segmenter, &windows, false);
    let [(_, Segment::Paused { pcm, .. })] = &segments[..] else {
        panic!("{segments:?}")
    };
    // 0.9 s before the window speech started with, and all after it.
    assert_eq!(pcm.len(), PRE_SPEECH_SAMPLES + 88 * WINDOW);
    let Some(Segment::Ended(ended)) = segmenter.end_paused(1) else {
        panic!("ended")
    };
    // The second before that window, the window, and all after it.
    assert_eq!(ended.pcm.len(), PRE_ROLL_SAMPLES + WINDOW + 88 * WINDOW);

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

#[test]
fn the_sliding_window_is_a_multiple_of_the_minimum_that_follows_the_configuration() {
    // A window as long as the minimum leaves no room for a pause between bursts.
    let mut segmenter = playing(Segmentation {
        barge_in_window: 1.0,
        ..SMART
    });
    assert!(hear(&mut segmenter, &bursts(LOUD, 14, 8, 3), true).is_empty());
    // Bursts too far apart for 2.5 times the minimum add up in ten times as long a window.
    let mut segmenter = playing(Segmentation {
        barge_in_window: 10.0,
        ..SMART
    });
    let started = hear(&mut segmenter, &bursts(LOUD, 14, 50, 3), true);
    assert!(
        started
            .iter()
            .any(|(_, segment)| segment == &Segment::Started),
        "{started:?}"
    );
}

#[test]
fn the_detector_confidence_is_the_configured_one() {
    let mut windows = times(Heard(tone(LOUD), 0.02), 32);
    windows.extend(times(Heard(tone(LOUD), 0.7), 20));
    let mut segmenter = Segmenter::new(SMART);
    assert!(!hear(&mut segmenter, &windows, false).is_empty());
    let mut segmenter = Segmenter::new(Segmentation {
        confidence: 0.8,
        ..SMART
    });
    assert!(hear(&mut segmenter, &windows, false).is_empty());
    let mut segmenter = Segmenter::new(Segmentation {
        confidence: 0.3,
        ..SMART
    });
    let mut windows = times(Heard(tone(LOUD), 0.02), 32);
    windows.extend(times(Heard(tone(LOUD), 0.4), 20));
    assert!(!hear(&mut segmenter, &windows, false).is_empty());
}

#[test]
fn speech_starts_after_the_configured_time_of_it() {
    let started_at = |start_ms| {
        let mut segmenter = Segmenter::new(Segmentation { start_ms, ..SMART });
        let mut windows = times(Heard(tone(LOUD), 0.02), 32);
        windows.extend(times(speech(LOUD), 40));
        let segments = hear(&mut segmenter, &windows, false);
        let [(at, Segment::Started)] = segments[..] else {
            panic!("{segments:?}")
        };
        at
    };
    assert_eq!(started_at(400), 43);
    assert_eq!(started_at(100), 34);
    assert!(started_at(1_000) > 60);
}

#[test]
fn a_turn_opens_with_the_configured_pre_roll() {
    let ended_with = |pre_roll_ms| {
        let mut segmenter = Segmenter::new(Segmentation {
            pre_roll_ms,
            ..SMART
        });
        let mut windows = times(Heard(tone(LOUD), 0.02), 100);
        windows.extend(times(speech(LOUD), 60));
        windows.extend(times(silence(), 28));
        hear(&mut segmenter, &windows, false);
        let Some(Segment::Ended(ended)) = segmenter.end_paused(1) else {
            panic!("ended")
        };
        ended.pcm.len()
    };
    // The speech that started the turn (400 ms is 12 windows, rounded half to even) is kept whatever the pre-roll, so a
    // short one cannot cut into it; a longer one adds what it has beyond that.
    let kept = ended_with(0);
    assert_eq!(ended_with(2_000), kept + 32_000 - 12 * WINDOW);
}
