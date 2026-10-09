//! AEC3 on recorded speech: LibriSpeech is the reply the speaker plays, and its echo reaches the microphone through a
//! room with reflections at 60, 75 and 95 ms, −6, −14 and −20 dB (the proof in sidevoice-core#89); FLEURS is the
//! person, speaking over it.

use super::EchoCanceller;
use crate::audio::FRAME;
use crate::test_support::{clip, FLEURS_ES, QUILTER};

/// The echo of `reply`: its reflections at 16 kHz.
fn room(reply: &[f32]) -> Vec<f32> {
    let mut echo = vec![0.0; reply.len()];
    for (delay_ms, gain_db) in [(60, -6.0), (75, -14.0), (95, -20.0)] {
        let delay = delay_ms * 16;
        let gain = 10f32.powf(gain_db / 20.0);
        for (i, sample) in reply.iter().enumerate() {
            if let Some(slot) = echo.get_mut(i + delay) {
                *slot += sample * gain;
            }
        }
    }
    echo
}

/// What the canceller leaves of `mic` while `reply` plays.
fn cancel(canceller: &EchoCanceller, reply: &[f32], mic: &[f32]) -> Vec<f32> {
    let mut out = Vec::with_capacity(mic.len());
    for (render, capture) in reply
        .as_chunks::<FRAME>()
        .0
        .iter()
        .zip(mic.as_chunks::<FRAME>().0)
    {
        canceller.render(render);
        let mut frame = *capture;
        canceller.capture(&mut frame);
        out.extend_from_slice(&frame);
    }
    out
}

fn energy(samples: &[f32]) -> f64 {
    samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum()
}

fn db(ratio: f64) -> f64 {
    10.0 * ratio.log10()
}

/// The reply, twice in a row: the canceller has converged by the second time.
fn reply() -> Vec<f32> {
    let once = clip(QUILTER, 0.7);
    [once.clone(), once].concat()
}

#[test]
fn the_echo_of_the_reply_is_removed() {
    let reply = reply();
    let echo = room(&reply);
    let left = cancel(&EchoCanceller::with(false).unwrap(), &reply, &echo);
    let tail = left.len() - 4 * 16_000;
    let erle = db(energy(&echo[tail..left.len()]) / energy(&left[tail..]).max(1e-12));
    assert!(erle > 15.0, "{erle:.1} dB of echo removed");
}

#[test]
fn the_person_is_kept_while_the_reply_plays() {
    let reply = reply();
    let echo = room(&reply);
    let person = clip(FLEURS_ES, 0.5);
    let start = reply.len().saturating_sub(person.len()) / FRAME * FRAME;
    let mut mic = echo.clone();
    for (slot, sample) in mic[start..].iter_mut().zip(&person) {
        *slot += sample;
    }
    let both = cancel(&EchoCanceller::with(false).unwrap(), &reply, &mic);
    let alone = cancel(&EchoCanceller::with(false).unwrap(), &reply, &echo);
    let over = db(energy(&both[start..]) / energy(&alone[start..]).max(1e-12));
    assert!(over > 10.0, "the person {over:.1} dB above the echo left");
}

#[test]
fn the_call_s_processing_leaves_finite_audio() {
    let reply = reply();
    let left = cancel(&EchoCanceller::new().unwrap(), &reply, &room(&reply));
    assert_eq!(left.len(), reply.len() / FRAME * FRAME);
    assert!(left.iter().all(|s| s.is_finite()));
}
