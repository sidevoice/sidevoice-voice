//! The session's steps against this OS: the three succeed in order, and again, as a process that declares and
//! activates its audio, stops, and does so anew would.

use super::{AudioSession, Session, SessionConfig, SessionMode};

const CONFIG: SessionConfig = SessionConfig {
    mode: SessionMode::VoiceChat,
    speaker: true,
    bluetooth: true,
};

#[test]
fn a_session_is_declared_activated_and_deactivated_and_the_declaration_can_change() {
    assert_eq!(Session::configure(&CONFIG), Ok(()));
    assert_eq!(Session::activate(), Ok(()));
    assert_eq!(Session::deactivate(), Ok(()));
    let again = SessionConfig {
        mode: SessionMode::Default,
        speaker: false,
        bluetooth: false,
    };
    assert_eq!(Session::configure(&again), Ok(()));
}
