use std::time::{Duration, Instant};

use super::{Cause, Reopening, Watch, RECOVERY, RETRY, STALL, STALLED_RETRY};

#[test]
fn a_device_chosen_anew_that_cannot_open_is_given_up_at_once() {
    let now = Instant::now();
    let mut reopening = Reopening::new(Cause::Switched, now);
    assert!(reopening.due(now, false));
    assert!(!reopening.failed(now));
}

#[test]
fn a_stream_the_system_took_away_is_tried_again_while_the_route_settles() {
    let now = Instant::now();
    let mut reopening = Reopening::new(Cause::Rerouted, now);
    assert!(reopening.due(now, false));
    assert!(reopening.failed(now), "given up at the first try");
    assert!(
        !reopening.due(now + RETRY / 2, false),
        "tried again at once"
    );
    assert!(reopening.due(now + RETRY, false));
    assert!(reopening.failed(now + RECOVERY - Duration::from_millis(1)));
    assert!(
        !reopening.failed(now + RECOVERY),
        "tried for longer than the recovery"
    );
    assert_eq!(reopening.failures, 3);
}

#[test]
fn a_stalled_stream_is_tried_again_for_as_long_as_the_call_runs() {
    let now = Instant::now();
    let mut reopening = Reopening::new(Cause::Stalled, now);
    let later = now + RECOVERY * 60;
    assert!(
        reopening.failed(later),
        "a paused call's stream was given up"
    );
    assert!(!reopening.due(later + STALLED_RETRY / 2, false));
    assert!(reopening.due(later + STALLED_RETRY, false));
}

#[test]
fn a_stream_is_stalled_only_after_a_whole_stall_without_a_callback() {
    let opened = Instant::now();
    let mut watch = Watch::new(opened);
    assert!(!watch.stalled(0, opened + STALL / 2));
    assert!(
        watch.stalled(0, opened + STALL),
        "a stream that never called back"
    );
    // It calls back again: the stall starts over from then.
    let back = opened + STALL * 3;
    assert!(!watch.stalled(5, back));
    assert!(!watch.stalled(5, back + STALL / 2));
    assert!(watch.stalled(5, back + STALL));
    assert!(!watch.stalled(6, back + STALL));
}

#[test]
fn while_the_system_holds_the_audio_only_a_device_chosen_anew_is_tried() {
    let now = Instant::now();
    let mut switched = Reopening::new(Cause::Switched, now);
    assert!(switched.held(now));
    assert!(switched.due(now, true));
    // A stalled stream is waited for: nothing is left to try.
    let mut stalled = Reopening::new(Cause::Stalled, now);
    assert!(!stalled.held(now));
    // One taken away is not tried while held, and has its whole recovery once let go.
    let mut rerouted = Reopening::new(Cause::Rerouted, now);
    assert!(rerouted.failed(now));
    let later = now + RECOVERY * 3;
    assert!(rerouted.held(later));
    assert!(!rerouted.due(later, true));
    assert!(rerouted.due(later, false));
    assert!(rerouted.failed(later + RECOVERY - Duration::from_millis(1)));
}
