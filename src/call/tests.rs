//! The call on recorded speech: the clips go through a detector on energy, window by window, 32 ms apart, and the
//! tests play the models, the speaker and the room by hand.

use super::{Call, Effect, Input, Transcript};
use crate::config::{Patience, VoiceConfig};
use crate::event::{Karaoke, Listening, VoiceEvent};
use crate::room::{PlaybackStatus, Reply, RoomEvent, RoomMessage, TurnPhase, UserTurn};
use crate::test_support::{clip, config, silence, EnergyVad, FLEURS_ES, QUILTER, WINDOW};

#[cfg(web)]
use wasm_bindgen_test::wasm_bindgen_test as test;

/// Milliseconds per detector window.
const WINDOW_MS: u64 = 32;
/// The Unix time of the call's millisecond 0.
const EPOCH: u64 = 1_800_000_000_000;

struct Run {
    call: Call,
    vad: EnergyVad,
    now: u64,
    effects: Vec<Effect>,
}

impl Run {
    fn new(config: VoiceConfig) -> Self {
        let mut run = Self {
            call: Call::new(config, "c".into(), EPOCH),
            vad: EnergyVad::new(),
            now: 0,
            effects: Vec::new(),
        };
        run.input(Input::Start);
        run
    }

    fn input(&mut self, input: Input) {
        let effects = self.call.handle(self.now, input);
        self.effects.extend(effects);
    }

    /// Feeds audio window by window, running what falls due between windows.
    fn hear(&mut self, pcm: &[f32]) {
        for window in pcm.chunks(WINDOW) {
            self.now += WINDOW_MS;
            self.due();
            let speech = self.vad.window(window);
            self.input(Input::Window {
                pcm: window.to_vec(),
                speech,
            });
        }
    }

    /// Lets time pass with no audio (a muted or lost microphone), running what falls due.
    fn wait(&mut self, ms: u64) {
        let until = self.now + ms;
        while let Some(deadline) = self.call.deadline().filter(|&deadline| deadline <= until) {
            self.now = self.now.max(deadline);
            let effects = self.call.poll(self.now);
            self.effects.extend(effects);
        }
        self.now = until;
    }

    fn due(&mut self) {
        while let Some(deadline) = self
            .call
            .deadline()
            .filter(|&deadline| deadline <= self.now)
        {
            let _ = deadline;
            let effects = self.call.poll(self.now);
            let empty = effects.is_empty();
            self.effects.extend(effects);
            if empty {
                break;
            }
        }
    }

    /// Takes the effects so far.
    fn take(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    /// The turn messages among `effects`.
    fn turns(effects: &[Effect]) -> Vec<UserTurn> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Event(VoiceEvent::RoomMessage(RoomMessage::UserTurn(turn))) => {
                    Some(turn.clone())
                }
                _ => None,
            })
            .collect()
    }

    /// The playback reports among `effects`, as (utterance, status, heard characters).
    fn playbacks(effects: &[Effect]) -> Vec<(String, PlaybackStatus, usize)> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Event(VoiceEvent::RoomMessage(RoomMessage::Playback(report))) => Some((
                    report.utterance_id.clone(),
                    report.status,
                    report.heard_chars,
                )),
                _ => None,
            })
            .collect()
    }

    /// The transcription asked for among `effects`: the turn and its audio's length.
    fn transcribe(effects: &[Effect]) -> Option<(usize, usize)> {
        effects.iter().find_map(|effect| match effect {
            Effect::Transcribe { turn, pcm, .. } => Some((*turn, pcm.len())),
            _ => None,
        })
    }

    /// The chunks to synthesize among `effects`.
    fn synthesize(effects: &[Effect]) -> Vec<(String, usize, String)> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Synthesize {
                    utterance,
                    chunk,
                    text,
                    ..
                } => Some((utterance.clone(), *chunk, text.clone())),
                _ => None,
            })
            .collect()
    }

    fn transcribed(&mut self, turn: usize, text: &str) {
        self.input(Input::Transcribed {
            turn,
            result: Ok(Transcript {
                text: text.into(),
                logprob: None,
            }),
        });
    }

    fn reply(&mut self, id: &str, text: &str) {
        self.input(Input::Room(RoomEvent::Reply(reply(id, text))));
    }

    /// Plays chunk `chunk` of `id`: synthesized, queued, started.
    fn sound(&mut self, id: &str, chunk: usize) {
        self.input(Input::Synthesized {
            utterance: id.into(),
            chunk,
            result: Ok((vec![0.0; 2_400], 24_000)),
        });
        self.input(Input::ChunkStarted {
            utterance: id.into(),
            chunk,
        });
    }

    fn played(&mut self, id: &str, chunk: usize) {
        self.input(Input::ChunkPlayed {
            utterance: id.into(),
            chunk,
        });
    }
}

fn reply(id: &str, text: &str) -> Reply {
    Reply {
        utterance_id: id.into(),
        revision: 7,
        reply_revision: 8,
        thread_id: "thread".into(),
        history_id: "row".into(),
        text: text.into(),
        language: Some("en".into()),
        replay: false,
    }
}

fn speech() -> Vec<f32> {
    clip(QUILTER, 0.9)
}

#[test]
fn a_spoken_sentence_is_one_turn_reported_after_the_merge_window() {
    let mut run = Run::new(config());
    run.hear(&silence(500));
    run.hear(&speech());
    let started = Run::turns(&run.take());
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].phase, TurnPhase::Started);
    assert_eq!(started[0].turn_id, "c-turn-0");
    assert!(started[0].started_at > EPOCH && started[0].ended_at.is_none());

    run.hear(&silence(3_000));
    let effects = run.take();
    let (turn, samples) = Run::transcribe(&effects).expect("the turn is transcribed");
    // The pre-roll, the clip from where speech was confirmed, and the silence that ended it.
    let clip_ms = speech().len() / 16;
    assert!(
        (clip_ms + 2_000..clip_ms + 4_000).contains(&(samples / 16)),
        "{} ms",
        samples / 16
    );
    assert!(Run::turns(&effects).is_empty());

    run.transcribed(
        turn,
        " Mister Quilter is the apostle of the middle classes. ",
    );
    assert!(
        Run::turns(&run.take()).is_empty(),
        "held for the merge window"
    );
    run.wait(500);
    let finished = Run::turns(&run.take());
    assert_eq!(finished.len(), 1);
    let finished = &finished[0];
    assert_eq!(finished.phase, TurnPhase::Finished);
    assert_eq!(finished.turn_id, "c-turn-0");
    assert_eq!(
        finished.text.as_deref(),
        Some("Mister Quilter is the apostle of the middle classes.")
    );
    assert_eq!(finished.language.as_deref(), Some("en"));
    assert!(!finished.merged && !finished.offline);
    let timings = finished.timings.expect("timings");
    assert!(timings.endpoint_silence_ms >= 2_500);
    assert!(timings.recognition_ms.is_some_and(|ms| ms < 1_000));
    assert_ne!(finished.client_msg_id, started[0].client_msg_id);
}

#[test]
fn a_short_pause_stays_inside_the_turn() {
    let mut run = Run::new(config());
    run.hear(&speech());
    run.hear(&silence(400));
    run.hear(&clip(FLEURS_ES, 0.9));
    run.hear(&silence(3_000));
    let effects = run.take();
    let turns = Run::turns(&effects);
    assert_eq!(turns.len(), 1, "{turns:?}");
    assert_eq!(turns[0].phase, TurnPhase::Started);
    let (_, samples) = Run::transcribe(&effects).expect("one transcription");
    assert!(samples / 16 > (speech().len() + clip(FLEURS_ES, 0.9).len()) / 16);
}

#[test]
fn a_turn_within_the_merge_window_joins_the_one_before() {
    let mut run = Run::new(VoiceConfig {
        patience: Patience::Calm,
        ..config()
    });
    run.hear(&speech());
    run.hear(&silence(3_800));
    let (first, _) = Run::transcribe(&run.take()).expect("the first turn");
    run.transcribed(first, "Mister Quilter is the apostle");
    // The person goes on before the 1.5 s merge window closes.
    run.hear(&speech());
    run.hear(&silence(3_800));
    let effects = run.take();
    let (second, _) = Run::transcribe(&effects).expect("the second turn");
    run.transcribed(second, "of the middle classes");
    run.wait(1_500);
    let mut turns = Run::turns(&effects);
    turns.extend(Run::turns(&run.take()));
    let phases: Vec<_> = turns
        .iter()
        .map(|turn| {
            (
                turn.turn_id.as_str(),
                turn.phase,
                turn.merged,
                turn.text.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        phases,
        [
            ("c-turn-1", TurnPhase::Started, false, None),
            ("c-turn-0", TurnPhase::Cancelled, true, None),
            (
                "c-turn-1",
                TurnPhase::Finished,
                true,
                Some("Mister Quilter is the apostle of the middle classes")
            ),
        ]
    );
}

#[test]
fn speech_over_a_reply_interrupts_it_and_drops_the_queue() {
    let mut run = Run::new(config());
    run.reply(
        "u1",
        "The tests pass on Linux. The macOS job is still running.",
    );
    run.reply("u2", "And one more thing to say after that.");
    let effects = run.take();
    assert_eq!(
        Run::synthesize(&effects),
        [("u1".into(), 0, "The tests pass on Linux.".into())]
    );
    run.sound("u1", 0);
    run.played("u1", 0);
    run.sound("u1", 1);
    run.take();

    run.hear(
        &clip(QUILTER, 1.0)
            .iter()
            .map(|s| s * 1.6)
            .collect::<Vec<_>>(),
    );
    let effects = run.take();
    assert!(effects.contains(&Effect::StopPlayback));
    let turns = Run::turns(&effects);
    assert_eq!(turns[0].phase, TurnPhase::Started);
    assert_eq!(turns[0].revision, 7);
    assert_eq!(
        Run::playbacks(&effects),
        [
            ("u1".into(), PlaybackStatus::Interrupted, 24),
            ("u2".into(), PlaybackStatus::Unplayed, 0),
        ]
    );
    // A chunk the output reports after the stop changes nothing.
    run.played("u1", 1);
    assert!(Run::playbacks(&run.take()).is_empty());
}

#[test]
fn the_listening_bar_rises_while_a_reply_plays() {
    let mut run = Run::new(config());
    run.reply(
        "u1",
        "A reply long enough to be playing while the person talks quietly.",
    );
    run.sound("u1", 0);
    run.take();
    // Speech at a normal level, as the echo canceller leaves the room's voice: not loud enough to barge in.
    run.hear(&clip(QUILTER, 0.25));
    let effects = run.take();
    assert!(Run::turns(&effects).is_empty());
    assert!(!effects.contains(&Effect::StopPlayback));

    // The same speech with nothing playing opens a turn.
    run.played("u1", 0);
    run.take();
    run.hear(&silence(1_000));
    run.hear(&clip(QUILTER, 0.25));
    assert_eq!(Run::turns(&run.take())[0].phase, TurnPhase::Started);
}

#[test]
fn a_reply_waits_for_the_turn_and_the_grace_after_it() {
    let mut run = Run::new(config());
    run.hear(&speech());
    run.reply("u1", "Here is my answer to what you said.");
    assert!(
        Run::synthesize(&run.take()).is_empty(),
        "the person is speaking"
    );
    run.hear(&silence(3_000));
    let (turn, _) = Run::transcribe(&run.take()).expect("transcribed");
    assert!(
        Run::synthesize(&run.take()).is_empty(),
        "the turn is on its way"
    );
    run.transcribed(turn, "a question");
    run.wait(500);
    let effects = run.take();
    assert_eq!(Run::turns(&effects)[0].phase, TurnPhase::Finished);
    assert!(Run::synthesize(&effects).is_empty(), "the grace");
    run.wait(999);
    assert!(Run::synthesize(&run.take()).is_empty());
    run.wait(1);
    assert_eq!(Run::synthesize(&run.take()).len(), 1);
}

#[test]
fn a_reply_played_to_its_end_is_heard_and_moves_the_karaoke() {
    let mut run = Run::new(config());
    let text = "The build is green on every platform. I will open the next pull request now.";
    run.reply("u1", text);
    run.sound("u1", 0);
    let effects = run.take();
    assert_eq!(
        Run::playbacks(&effects),
        [("u1".into(), PlaybackStatus::Playing, 0)]
    );
    assert!(
        effects.contains(&Effect::Event(VoiceEvent::Karaoke(Karaoke {
            utterance_id: "u1".into(),
            sounding: Some((0, 37)),
            heard_chars: 0,
        })))
    );
    assert!(effects
        .iter()
        .any(|effect| matches!(effect, Effect::Synthesize { chunk: 1, .. })));
    run.played("u1", 0);
    run.sound("u1", 1);
    run.played("u1", 1);
    let effects = run.take();
    assert_eq!(
        Run::playbacks(&effects),
        [("u1".into(), PlaybackStatus::Heard, text.chars().count())]
    );
    // The call is listening again and nothing plays.
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Event(VoiceEvent::State(state)) if state.playback == crate::PlaybackState::Idle
    )));
}

#[test]
fn a_reply_that_cannot_be_spoken_fails_and_the_next_one_starts() {
    let mut run = Run::new(config());
    run.reply("u1", "This one fails.");
    run.reply("u2", "This one is spoken.");
    run.take();
    run.input(Input::Synthesized {
        utterance: "u1".into(),
        chunk: 0,
        result: Err("synthesis-failed".into()),
    });
    let effects = run.take();
    assert_eq!(
        Run::playbacks(&effects),
        [("u1".into(), PlaybackStatus::Failed, 0)]
    );
    assert!(
        effects.contains(&Effect::Event(VoiceEvent::Error(crate::VoiceError {
            code: "synthesis-failed".into()
        })))
    );
    assert_eq!(
        Run::synthesize(&effects),
        [("u2".into(), 0, "This one is spoken.".into())]
    );
}

#[test]
fn a_reply_sent_again_is_ignored_unless_it_is_a_replay() {
    let mut run = Run::new(config());
    run.reply("u1", "Once.");
    run.sound("u1", 0);
    run.played("u1", 0);
    run.take();
    run.reply("u1", "Once.");
    assert!(Run::synthesize(&run.take()).is_empty());
    let mut again = reply("u1", "Once.");
    again.replay = true;
    run.input(Input::Room(RoomEvent::Reply(again)));
    assert_eq!(Run::synthesize(&run.take()).len(), 1);
}

#[test]
fn transcripts_that_say_nothing_cancel_their_turn() {
    for (text, result) in [
        ("   ", Ok(())),
        ("Привет, как дела", Ok(())),
        ("", Err("transcription-failed")),
    ] {
        let mut run = Run::new(config());
        run.hear(&speech());
        run.hear(&silence(3_000));
        let (turn, _) = Run::transcribe(&run.take()).expect("transcribed");
        run.input(Input::Transcribed {
            turn,
            result: result
                .map(|()| Transcript {
                    text: text.into(),
                    logprob: None,
                })
                .map_err(str::to_owned),
        });
        let effects = run.take();
        let turns = Run::turns(&effects);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].phase, TurnPhase::Cancelled);
        assert_eq!(turns[0].text, None);
        let failed = effects
            .iter()
            .any(|effect| matches!(effect, Effect::Event(VoiceEvent::Error(_))));
        assert_eq!(failed, result.is_err());
    }
}

#[test]
fn turns_wait_in_order_and_the_ninth_is_dropped() {
    let mut run = Run::new(VoiceConfig {
        patience: Patience::Fast,
        ..config()
    });
    for _ in 0..9 {
        run.hear(&speech());
        run.hear(&silence(2_600));
    }
    let effects = run.take();
    let asked = effects
        .iter()
        .filter(|effect| matches!(effect, Effect::Transcribe { .. }));
    assert_eq!(asked.count(), 1, "one at a time");
    let cancelled: Vec<_> = Run::turns(&effects)
        .into_iter()
        .filter(|turn| turn.phase == TurnPhase::Cancelled)
        .map(|turn| turn.turn_id)
        .collect();
    assert_eq!(cancelled, ["c-turn-8"]);
    assert!(
        effects.contains(&Effect::Event(VoiceEvent::Error(crate::VoiceError {
            code: "transcription-queue-full".into()
        })))
    );
    // Each transcript starts the next turn's transcription, in order.
    run.transcribed(0, "zero");
    assert_eq!(Run::transcribe(&run.take()).map(|(turn, _)| turn), Some(1));
}

#[test]
fn a_turn_ends_when_its_audio_stops_arriving() {
    let mut run = Run::new(config());
    run.hear(&speech()[..16_000 * 2]);
    run.take();
    run.wait(4_999);
    assert!(Run::transcribe(&run.take()).is_none());
    run.wait(1);
    let (turn, _) = Run::transcribe(&run.take()).expect("closed for lack of audio");
    assert_eq!(turn, 0);
}

#[test]
fn muting_ends_the_turn_and_cancelling_drops_it() {
    let mut run = Run::new(config());
    run.hear(&speech()[..16_000 * 2]);
    run.input(Input::Mute(true));
    let effects = run.take();
    assert!(Run::transcribe(&effects).is_some());
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Event(VoiceEvent::State(state)) if state.listening == Listening::Muted
    )));
    run.hear(&speech());
    assert!(Run::turns(&run.take()).is_empty(), "muted");

    run.input(Input::Cancel);
    let turns = Run::turns(&run.take());
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].phase, TurnPhase::Cancelled);
    run.transcribed(0, "too late");
    assert!(Run::turns(&run.take()).is_empty());
}

#[test]
fn turns_spoken_offline_say_so() {
    let mut run = Run::new(config());
    run.input(Input::Online(false));
    run.hear(&speech());
    let turns = Run::turns(&run.take());
    assert!(turns[0].offline);
}

#[test]
fn stopping_cancels_the_turn_and_interrupts_the_reply() {
    let mut run = Run::new(config());
    run.reply("u1", "Something to say while the call stops.");
    run.sound("u1", 0);
    run.take();
    run.input(Input::Stop);
    let effects = run.take();
    assert!(effects.contains(&Effect::StopPlayback));
    assert_eq!(
        Run::playbacks(&effects),
        [("u1".into(), PlaybackStatus::Interrupted, 0)]
    );
    run.hear(&speech());
    assert!(Run::turns(&run.take()).is_empty(), "stopped");
}
